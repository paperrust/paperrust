#!/usr/bin/env bash
# Phase 6 – fixes: DB time per request (sum of the sqlx-elapsed of all queries) and buffers
# for measures whose effect is lost in wall-clock noise. Interleaved: round r = before, after.
#   tests/pgvalidate/fix_dbtime.sh <before.exe> <after.exe> <out-dir> <rounds> <users> <only>
# Buffer numbers only with auto_explain on pl_pgvalidate (beforehand as postgres: ALTER DATABASE pl_pgvalidate SET
# session_preload_libraries=auto_explain, auto_explain.log_min_duration=0, log_analyze=on, log_buffers=on,
# log_timing=off, log_level=notice; afterwards ALTER DATABASE pl_pgvalidate RESET ALL). Evaluation: fix_buffers.py.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
BEFORE="$1"; AFTER="$2"; OUT="$3"; ROUNDS="$4"; USERS="$5"; ONLY="$6"
mkdir -p "$OUT"
RS_PORT="${PGV_RS_PORT:-8090}"; RS_PIDF="${PGV_RS_PIDFILE:-results/pgvalidate/bin/rs.pid}"
run() { # <exe> <label> <round>
  local log="$OUT/$2-r$3.log"
  ( export PAPERRUST_LOG="info,sqlx::query=debug"; source tests/pgvalidate/rs-env.sh
    detach "$RS_PIDF" "$1" "$log" "$log.err" )
  for _ in $(seq 1 100); do curl -s -o /dev/null http://127.0.0.1:$RS_PORT/api/ && break; sleep 0.3; done
  python -I tests/pgvalidate/capture.py --kind rs --base http://127.0.0.1:$RS_PORT --log "$log" \
      --out "$OUT/$2-r$3.json" --users "$USERS" --only "$ONLY" --settle 0.6
  bash tests/pgvalidate/rs-stop.sh >/dev/null
  sleep 1
}
for r in $(seq 1 "$ROUNDS"); do
  run "$BEFORE" before "$r"
  run "$AFTER" after "$r"
done
echo done
