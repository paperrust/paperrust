#!/usr/bin/env bash
# Restores ONLY the W8 copy: DB pl_w8_ref, .w8/data, Redis keys w8*.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w8-env.sh"
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres)
have_tpl=$("${SUPER[@]}" -Atc "select count(*) from pg_database where datname='$W8_TPL'")
if [[ $have_tpl != 1 ]]; then
  "${SUPER[@]}" -c "CREATE DATABASE $W8_TPL OWNER paperless TEMPLATE paperless_snapshot STRATEGY FILE_COPY;" \
                -c "ALTER DATABASE $W8_TPL WITH ALLOW_CONNECTIONS false;"
fi
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS $W8_DB WITH (FORCE);"
"${SUPER[@]}" -c "CREATE DATABASE $W8_DB OWNER paperless TEMPLATE $W8_TPL STRATEGY WAL_LOG;"
rm -rf "$W8_DIR/data/data" "$W8_DIR/data/media"
mkdir -p "$W8_DIR/data/consume" "$W8_DIR/logs"
tar -xf "$REF_DIR/snapshot/data.tar" -C "$W8_DIR/data"
"$PY" -I "$W8_REPO/tests/parity/pl-ai/w8_redis.py" "$REDIS_PORT" flush
echo "[w8-reset] $W8_DB + .w8/data wiederhergestellt"
