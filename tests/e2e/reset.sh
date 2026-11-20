#!/usr/bin/env bash
# Before every run: DB pl_e2e anew from TEMPLATE paperless_snapshot, own data folder from
# reference/snapshot/data.tar, empty own Garnet DB. Reference DB `paperless` stays untouched.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -q)
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS $E2E_DB WITH (FORCE)" \
              -c "CREATE DATABASE $E2E_DB TEMPLATE paperless_snapshot OWNER paperless"
rm -rf "$WORK/data"; mkdir -p "$WORK/data/consume" "$WORK/data/scratch"
tar xf "$REPO/reference/snapshot/data.tar" -C "$WORK/data"
"$PY" -I -c "import socket,time; s=socket.create_connection(('127.0.0.1',$REDIS_PORT),3); s.settimeout(3); s.sendall(b'*2\r\n\$6\r\nSELECT\r\n\$1\r\n$E2E_REDIS_DB\r\n*1\r\n\$7\r\nFLUSHDB\r\n'); time.sleep(0.3); s.recv(64)" || echo "[reset] Garnet FLUSHDB fehlgeschlagen" >&2
echo "[reset] $E2E_DB + $WORK/data zurückgesetzt"
