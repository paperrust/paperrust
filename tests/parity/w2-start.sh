#!/usr/bin/env bash
# Starts the W2 copy: Django :8100 and PaperRust :8101 (PIDs in tests/parity/.w2/run/).
#   w2-start.sh [dj|rs|all]   (default all). Does not reset beforehand (w2-reset.sh).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w2-env.sh"
what="${1:-all}"
mkdir -p "$W2_DIR/run" "$W2_DIR/logs"
[[ -d "$W2_DIR/data/data" ]] || bash "$W2_REPO/tests/parity/w2-reset.sh"
alive() { [[ -f "$1" ]] && powershell.exe -NoProfile -NonInteractive -Command "if (Get-Process -Id $(cat "$1") -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }" </dev/null >/dev/null 2>&1; }
wait_http() {
  local code=""
  for _ in $(seq 1 "${2:-120}"); do
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$1/api/" || true)
    [[ "$code" =~ ^(200|302|401|403)$ ]] && return 0
    sleep 1
  done
  echo "Start auf :$1 fehlgeschlagen (HTTP $code)"; return 1
}
if [[ $what == all || $what == dj ]]; then
  if alive "$W2_DIR/run/dj.pid"; then echo "Django W2 läuft (PID $(cat "$W2_DIR/run/dj.pid"))"; else
    export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$W2_DJ_PORT" GRANIAN_WORKERS=1
    detach "$W2_DIR/run/dj.pid" "$REF_DIR/venv/Scripts/granian.exe" "$W2_DIR/logs/dj.out.log" "$W2_DIR/logs/dj.err.log" \
      --interface asginl --ws --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
    wait_http "$W2_DJ_PORT"; echo "Django W2: PID $(cat "$W2_DIR/run/dj.pid") :$W2_DJ_PORT"
  fi
fi
if [[ $what == all || $what == rs ]]; then
  if alive "$W2_DIR/run/rs.pid"; then echo "PaperRust W2 läuft (PID $(cat "$W2_DIR/run/rs.pid"))"; else
    detach "$W2_DIR/run/rs.pid" "$W2_DIR/bin/paperrust-w2.exe" "$W2_DIR/logs/rs.log" "$W2_DIR/logs/rs.err.log"
    wait_http "$W2_RS_PORT" 20 || { tail -5 "$W2_DIR/logs/rs.err.log" "$W2_DIR/logs/rs.log"; exit 1; }
    echo "PaperRust W2: PID $(cat "$W2_DIR/run/rs.pid") :$W2_RS_PORT"
  fi
fi
