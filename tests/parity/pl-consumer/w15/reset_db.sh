#!/usr/bin/env bash
# W15 parity: own DB copies from paperless_snapshot (never the reference DB `paperless`) and
# common initial state (setup.sql):
#   pl_w15_ref – upstream (run_upstream.sh)
#   pl_w15_rs  – Rust (cargo test -p pl-consumer --test w15_parity -- --ignored)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
PSQL="$REPO/reference/pg/pgsql/bin/psql"
[ -x "$PSQL" ] || PSQL="$PSQL.exe"
export PGPASSWORD=paperless
for d in pl_w15_ref pl_w15_rs; do
  "$PSQL" -q -h 127.0.0.1 -p 54329 -U paperless -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS $d WITH (FORCE)" -c "CREATE DATABASE $d TEMPLATE paperless_snapshot"
  "$PSQL" -q -h 127.0.0.1 -p 54329 -U paperless -d "$d" -v ON_ERROR_STOP=1 -f "$HERE/setup.sql"
done
# Rust side: PaperRust tag migration (adjacency list, without django-treenode columns) as at start
"$PSQL" -q -h 127.0.0.1 -p 54329 -U paperless -d pl_w15_rs -v ON_ERROR_STOP=1 \
  -f "$REPO/crates/pl-objects/migrations/0001_paperrust_tag_adjacency.sql"
echo "pl_w15_ref, pl_w15_rs bereit"
