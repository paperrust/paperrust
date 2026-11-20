#!/usr/bin/env bash
# L1 measurement: the same binary, PAPERRUST_QUERY_ENGINE=legacy vs ast, interleaved (round r: legacy → ast),
# servers one after the other on 127.0.0.1:$PORT against $DB (copy), start/stop by PID.
#   l1_bench.sh <exe> <db> <out-dir> [rounds] [users] [only]
set -uo pipefail
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$REPO"
EXE="$1"; DB="$2"; OUT="$3"; ROUNDS="${4:-3}"; USERS="${5:-admin,alice,bob}"; ONLY="${6:-}"
PORT="${L1_PORT:-8094}"
mkdir -p "$OUT"
PIDF="$OUT/rs.pid"
PY="python -I"
ONLY_ARG=(); [[ -n "$ONLY" ]] && ONLY_ARG=(--only "$ONLY")
source reference/env.sh

rs_run() { # <engine> <round>
  (
    export PAPERLESS_DBNAME="$DB"
    export PAPERRUST_BIND=127.0.0.1:$PORT
    export PAPERRUST_ROUTES="$(cygpath -w "$REPO/results/pgvalidate/routes-all-rust.toml")"
    export PAPERRUST_LOG=warn
    export PAPERLESS_URL=http://127.0.0.1:$PORT
    export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$PORT"
    export PAPERRUST_QUERY_ENGINE="$1"
    export NO_COLOR=1
    detach "$PIDF" "$EXE" "$OUT/$1.log" "$OUT/$1.err.log"
  )
  for _ in $(seq 1 200); do curl -s -o /dev/null http://127.0.0.1:$PORT/api/ && break; sleep 0.3; done
  $PY tests/pgvalidate/fix_bench.py --label "$1" --round "$2" --base http://127.0.0.1:$PORT \
      --out "$OUT/runs.jsonl" --bodies "$OUT/bodies" --users "$USERS" "${ONLY_ARG[@]}"
  local pid; pid=$(cat "$PIDF")
  powershell.exe -NoProfile -NonInteractive -Command "Stop-Process -Id $pid -Force" </dev/null >/dev/null 2>&1
  sleep 2
}

for r in $(seq 1 "$ROUNDS"); do
  rs_run legacy "$r"
  rs_run ast "$r"
done
echo done
