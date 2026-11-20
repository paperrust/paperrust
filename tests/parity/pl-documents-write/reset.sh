#!/usr/bin/env bash
# Resets the W4 copy: DB pl_w4_ref = paperless_snapshot, work/data = snapshot/data.tar.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w4env.sh"
[[ "$PAPERLESS_DBNAME" == pl_w4_ref ]] || { echo "refusing $PAPERLESS_DBNAME" >&2; exit 1; }
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres)
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS pl_w4_ref WITH (FORCE);" \
              -c "CREATE DATABASE pl_w4_ref OWNER paperless TEMPLATE paperless_snapshot STRATEGY WAL_LOG;"
rm -rf "$W4_WORK/data/data" "$W4_WORK/data/media"
tar -xf "$REF_DIR/snapshot/data.tar" -C "$W4_WORK/data"
mkdir -p "$W4_WORK/data/consume"
