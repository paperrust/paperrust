#!/usr/bin/env bash
# W23-B3 (social_account_providers, storage_paths/test, documents email): PaperRust B on :8195
# against the reference DB `paperless` READ ONLY (the cases write nothing; email sending fails
# at localhost:25 on both sides), without worker (the then Django fallback :8000 was removed on 2026-11-14).
#   source tests/parity/pl-mail/w23b3env.sh
#   start_b3 / stop_b3 ; python tests/parity/harness/run_parity.py --crate pl-objects --case 'w23.*' ...
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
export W23_WORK="$REPO_DIR/target/w23b3"
export PAPERRUST_BIND="127.0.0.1:8195"
export PAPERRUST_ROUTES="$(cygpath -w "$REPO_DIR/results/parity/w23/routes-w23b3.toml")"
export PAPERRUST_BRANDING=off
export PAPERRUST_LOG="${PAPERRUST_LOG:-info}"
export PARITY_A_URL="http://127.0.0.1:8000" PARITY_B_URL="http://127.0.0.1:8195"
W23_BIN="$REPO_DIR/results/bin/paperrust-w23b3.exe"
start_b3() { mkdir -p "$W23_WORK"; detach "$W23_WORK/b3.pid" "$W23_BIN" "$W23_WORK/b3.out.log" "$W23_WORK/b3.err.log" serve --no-worker
  for _ in $(seq 1 60); do c=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8195/api/ || true); [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "B3 PID $(cat "$W23_WORK/b3.pid")"; return 0; }; sleep 1; done; echo "B3 start failed"; return 1; }
stop_b3() { [[ -f "$W23_WORK/b3.pid" ]] && taskkill //PID "$(cat "$W23_WORK/b3.pid")" //F >/dev/null 2>&1; rm -f "$W23_WORK/b3.pid"; }
