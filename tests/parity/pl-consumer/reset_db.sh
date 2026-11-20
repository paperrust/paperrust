#!/usr/bin/env bash
# Recreate own DB copies for pl-consumer (never the reference DB `paperless`):
#   pl_consumer_test     – Rust tests (crates/pl-consumer/tests/e2e.rs, parity.rs)
#   pl_consumer_test_up  – upstream run (tests/parity/pl-consumer/run_upstream.py)
# Other names via PL_CONSUMER_DB (e.g. PL_CONSUMER_DB=pl_w9_test → pl_w9_test + pl_w9_test_up).
set -euo pipefail
BASE="${PL_CONSUMER_DB:-pl_consumer_test}"
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
PSQL="$REPO/reference/pg/pgsql/bin/psql"
[ -x "$PSQL" ] || PSQL="$PSQL.exe"
export PGPASSWORD=paperless
for db in "${@:-$BASE ${BASE}_up}"; do
  for d in $db; do
    case "$d" in pl_consumer_test*|pl_w9_test*) ;; *) echo "refusing $d" >&2; exit 1;; esac
    "$PSQL" -h 127.0.0.1 -p 54329 -U paperless -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS $d WITH (FORCE)" -c "CREATE DATABASE $d TEMPLATE paperless_snapshot"
  done
done
