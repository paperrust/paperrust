#!/usr/bin/env bash
# Phase 6 – fixes: interleaved timing (round r: Rust before → Rust after → Django),
# Rust one after the other on 127.0.0.1:8090 (start/stop by PID), Django own granian on :8091
# (runs across all rounds, start/stop via dj-start.sh/dj-stop.sh). DB: pl_pgvalidate.
#   tests/pgvalidate/fix_bench.sh <before.exe> <after.exe> <out-dir> [rounds] [users] [only]
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
BEFORE="$1"; AFTER="$2"; OUT="$3"; ROUNDS="${4:-3}"; USERS="${5:-admin,alice,bob}"; ONLY="${6:-}"
mkdir -p "$OUT"
# Ports/PID file overridable (parallel agents): PGV_RS_PORT, PGV_DJ_PORT, PGV_RS_PIDFILE
RS_PORT="${PGV_RS_PORT:-8090}"; DJ_PORT="${PGV_DJ_PORT:-8091}"
RS_PIDF="${PGV_RS_PIDFILE:-results/pgvalidate/bin/rs.pid}"
PY="python -I"
ONLY_ARG=(); [[ -n "$ONLY" ]] && ONLY_ARG=(--only "$ONLY")

rs_run() { # <exe> <label> <round>
  ( export PAPERRUST_LOG=warn; source tests/pgvalidate/rs-env.sh
    detach "$RS_PIDF" "$1" "$OUT/$2.log" "$OUT/$2.err.log" )
  for _ in $(seq 1 100); do curl -s -o /dev/null http://127.0.0.1:$RS_PORT/api/ && break; sleep 0.3; done
  $PY tests/pgvalidate/fix_bench.py --label "$2" --round "$3" --base http://127.0.0.1:$RS_PORT \
      --out "$OUT/runs.jsonl" --bodies "$OUT/bodies" --users "$USERS" "${ONLY_ARG[@]}"
  bash tests/pgvalidate/rs-stop.sh >/dev/null
  sleep 1
}

for r in $(seq 1 "$ROUNDS"); do
  [[ "$BEFORE" != "-" ]] && rs_run "$BEFORE" rs-before "$r"
  rs_run "$AFTER" rs-after "$r"
  # PGV_DJ_PER_ROUND=1: start Django only for its run and then end it by PID
  # (never simultaneously with a Rust server).
  # PGV_NO_DJ=1: Rust before/after only (e.g. single measure without a renewed Django measurement)
  [[ "${PGV_NO_DJ:-0}" == 1 ]] && continue
  [[ "${PGV_DJ_PER_ROUND:-0}" == 1 ]] && bash tests/pgvalidate/dj-start.sh
  $PY tests/pgvalidate/fix_bench.py --label django --round "$r" --base http://127.0.0.1:$DJ_PORT \
      --out "$OUT/runs.jsonl" --bodies "$OUT/bodies" --users "$USERS" "${ONLY_ARG[@]}"
  [[ "${PGV_DJ_PER_ROUND:-0}" == 1 ]] && { bash tests/pgvalidate/dj-stop.sh >/dev/null; sleep 2; }
done
echo done
