#!/usr/bin/env bash
# Starts mock LLM (:8181) and Django (:8180) of the W8 copy. PIDs in .w8/run/.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w8-env.sh"
mkdir -p "$W8_DIR/run" "$W8_DIR/logs"
[[ -d "$W8_DIR/data/data" ]] || bash "$W8_REPO/tests/parity/pl-ai/w8-reset.sh"
alive() { [[ -f "$1" ]] && tasklist //FI "PID eq $(cat "$1")" 2>/dev/null | grep -q "$(cat "$1")"; }
if ! alive "$W8_DIR/run/mock.pid"; then
  detach "$W8_DIR/run/mock.pid" "$PY" "$W8_DIR/logs/mock.out.log" "$W8_DIR/logs/mock.err.log" \
    -I "$(cygpath -w "$W8_REPO/tests/parity/pl-ai/mock_llm.py")" "$W8_MOCK_PORT" "$MOCK_LLM_LOG"
fi
if ! alive "$W8_DIR/run/dj.pid"; then
  export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$W8_DJ_PORT" GRANIAN_WORKERS=1
  detach "$W8_DIR/run/dj.pid" "$REF_DIR/venv/Scripts/granian.exe" "$W8_DIR/logs/dj.out.log" "$W8_DIR/logs/dj.err.log" \
    --interface asginl --ws --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
fi
for _ in $(seq 1 120); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$W8_DJ_PORT/api/" || true)
  [[ "$code" =~ ^(200|302|401|403)$ ]] && break
  sleep 1
done
echo "W8: Django $(cat "$W8_DIR/run/dj.pid") :$W8_DJ_PORT (HTTP $code), Mock $(cat "$W8_DIR/run/mock.pid") :$W8_MOCK_PORT"
