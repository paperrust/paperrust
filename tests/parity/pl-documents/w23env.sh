#!/usr/bin/env bash
# W23 B2: read-only parity metadata/suggestions. B = PaperRust :8194 against the reference DB `paperless`
# and reference/data (read only; the two routes write nothing). At that time all other
# paths went via proxy to Django :8000 (A); since 2026-11-14 they answer 404. No worker. Start/stop by PID (target/w23b2/run/rs.pid).
#   source tests/parity/pl-documents/w23env.sh && w23_start; ...; w23_stop
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
export W23_DIR="$REPO_DIR/target/w23b2"
export W23_PORT=8194
export PAPERRUST_BIND="127.0.0.1:$W23_PORT"
export PAPERRUST_ROUTES="$(cygpath -w "$REPO_DIR/results/parity/w23/routes-w23b2.toml")"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8000,http://127.0.0.1:$W23_PORT"
export PAPERRUST_BRANDING=off
export PAPERRUST_LOG="${PAPERRUST_LOG:-info}"
export PARITY_A_URL=http://127.0.0.1:8000
export PARITY_B_URL="http://127.0.0.1:$W23_PORT"
W23_BIN="$REPO_DIR/results/bin/paperrust-w23b2.exe"
w23_start() {
  mkdir -p "$W23_DIR/run" "$W23_DIR/logs"
  detach "$W23_DIR/run/rs.pid" "$W23_BIN" "$W23_DIR/logs/rs.out.log" "$W23_DIR/logs/rs.err.log" serve --no-worker
  for _ in $(seq 1 60); do
    c=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$W23_PORT/api/" || true)
    [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "PaperRust W23 B2 PID $(cat "$W23_DIR/run/rs.pid") :$W23_PORT"; return 0; }
    sleep 1
  done
  echo "Start fehlgeschlagen"; return 1
}
w23_stop() { [[ -f "$W23_DIR/run/rs.pid" ]] && taskkill //PID "$(cat "$W23_DIR/run/rs.pid")" //T //F >/dev/null 2>&1; rm -f "$W23_DIR/run/rs.pid"; }
