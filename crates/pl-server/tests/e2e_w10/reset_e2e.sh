#!/usr/bin/env bash
# W10 E2E (AI in the worker): own DB pl_w10_test (TEMPLATE paperless_snapshot) + own data directory.
set -euo pipefail
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-w10-e2e}"
PSQL="$REPO/reference/pg/pgsql/bin/psql"
export PGPASSWORD=postgres-local-only
"$PSQL" -h 127.0.0.1 -p 54329 -U postgres -d postgres -v ON_ERROR_STOP=1 -q \
  -c "DROP DATABASE IF EXISTS pl_w10_test WITH (FORCE)" \
  -c "CREATE DATABASE pl_w10_test TEMPLATE paperless_snapshot OWNER paperless"
# Trigger tag of the AI automation (root tag without matching; tree fields minimal, test copy only)
PGPASSWORD=paperless "$PSQL" -h 127.0.0.1 -p 54329 -U paperless -d pl_w10_test -v ON_ERROR_STOP=1 -q -c   "INSERT INTO documents_tag (tn_ancestors_pks, tn_ancestors_count, tn_children_pks, tn_children_count, tn_depth,      tn_descendants_pks, tn_descendants_count, tn_index, tn_level, tn_priority, tn_order, tn_siblings_pks, tn_siblings_count,      name, match, matching_algorithm, is_insensitive, color, is_inbox_tag)    VALUES ('', 0, '', 0, 0, '', 0, 0, 1, 0, 999, '', 0, 'w10-ai-trigger', '', 0, true, '#000000', false)"
rm -rf "$E2E"; mkdir -p "$E2E/consume" "$E2E/scratch" "$E2E/logs"
tar xf "$REPO/reference/snapshot/data.tar" -C "$E2E"
# no LLM index from the snapshot: the test checks the build by the Rust worker
rm -rf "$E2E/data/llm_index"
echo "reset ok: $E2E"
