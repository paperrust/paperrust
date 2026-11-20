#!/usr/bin/env bash
# Restores the seed snapshot (DB + data/data + data/media), empties Redis (cache, Celery
# queue). Idempotent, a few seconds. Django keeps running (connections are re-established).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$REF_DIR"
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -U "$PG_SUPERUSER" -d postgres)
"$PG_BIN/pg_isready" -q -h 127.0.0.1 -p "$PGPORT" || ./pg-start.sh >/dev/null
have_tpl=$("${SUPER[@]}" -Atc "select count(*) from pg_database where datname='paperless_snapshot'")
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS paperless WITH (FORCE);"
if [[ $have_tpl == 1 ]]; then
  "${SUPER[@]}" -c "CREATE DATABASE paperless OWNER paperless TEMPLATE paperless_snapshot STRATEGY WAL_LOG;"
else
  "${SUPER[@]}" -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0;"
  "$PG_BIN/pg_restore" --no-owner -d paperless snapshot/paperless.dump
  "${SUPER[@]}" -c "CREATE DATABASE paperless_snapshot OWNER paperless TEMPLATE paperless STRATEGY FILE_COPY;" \
                -c "ALTER DATABASE paperless_snapshot WITH ALLOW_CONNECTIONS false;"
fi
rm -rf data/data data/media
tar -xf snapshot/data.tar -C data
mkdir -p data/consume
"$PY" -I -c "import socket; s=socket.create_connection(('127.0.0.1',$REDIS_PORT),2); s.sendall(b'FLUSHALL\r\n'); s.recv(16)" 2>/dev/null || true
echo "[reset] Snapshot wiederhergestellt"
