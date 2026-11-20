#!/usr/bin/env bash
# W14 E2E: own DB pl_w14_test (TEMPLATE paperless_snapshot) + own data directory.
set -euo pipefail
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-w14-e2e}"
PSQL="$REPO/reference/pg/pgsql/bin/psql"
export PGPASSWORD=postgres-local-only
"$PSQL" -h 127.0.0.1 -p 54329 -U postgres -d postgres -v ON_ERROR_STOP=1 -q \
  -c "DROP DATABASE IF EXISTS pl_w14_test WITH (FORCE)" \
  -c "CREATE DATABASE pl_w14_test TEMPLATE paperless_snapshot OWNER paperless"
rm -rf "$E2E"; mkdir -p "$E2E/consume" "$E2E/scratch" "$E2E/logs"
tar xf "$REPO/reference/snapshot/data.tar" -C "$E2E"
echo "reset ok: $E2E"
