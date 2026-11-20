#!/usr/bin/env bash
# Resets the W13 copy: DB pl_w13_ref = paperless_snapshot, work/data = snapshot/data.tar.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w13env.sh"
[[ "$PAPERLESS_DBNAME" == pl_w13_ref ]] || { echo "refusing $PAPERLESS_DBNAME" >&2; exit 1; }
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres)
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS pl_w13_ref WITH (FORCE);" \
              -c "CREATE DATABASE pl_w13_ref OWNER paperless TEMPLATE paperless_snapshot STRATEGY WAL_LOG;"
rm -rf "$W13_WORK/data/data" "$W13_WORK/data/media" "$W13_WORK/data/scratch"
tar -xf "$REF_DIR/snapshot/data.tar" -C "$W13_WORK/data"
mkdir -p "$W13_WORK/data/consume" "$W13_WORK/data/scratch"
: > "$W13_WORK/run/tasks.jsonl"
