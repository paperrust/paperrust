#!/usr/bin/env bash
# start | stop | reset  – own Django instance for the pl-accounts parity (see w3env.sh).
# reset: DB pl_w3_ref anew from paperless_snapshot (Django must be stopped for that or is reconnected).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w3env.sh"
pidf="$W3_WORK/django.pid"; mkdir -p "$W3_WORK"/{data/index,media,consume,logs}
psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -q -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -c "$1"; }
case "${1:-start}" in
  reset)
    psql_su "DROP DATABASE IF EXISTS pl_w3_ref WITH (FORCE)"
    psql_su "CREATE DATABASE pl_w3_ref TEMPLATE paperless_snapshot OWNER paperless"
    "$REF_DIR/venv/Scripts/python.exe" -I -c "import socket;s=socket.create_connection(('127.0.0.1',$REDIS_PORT));s.sendall(b'*2\r\n\$4\r\nKEYS\r\n\$7\r\nw3ref:*\r\n');print(s.recv(65536)[:200])" >/dev/null
    echo "pl_w3_ref neu angelegt";;
  start)
    if [[ -f "$pidf" ]] && tasklist //FI "PID eq $(cat "$pidf")" 2>/dev/null | grep -qi granian; then echo "läuft (PID $(cat "$pidf"))"; exit 0; fi
    PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname='pl_w3_ref'" | grep -q 1 || "$0" reset
    if ! curl -s -o /dev/null "http://127.0.0.1:8126/jwks"; then
      detach "$W3_WORK/sinks.pid" "$REF_DIR/venv/Scripts/python.exe" "$W3_WORK/sinks.out.log" "$W3_WORK/sinks.err.log" -I "$(cygpath -w "$W3_DIR/w3_sinks.py")" "$(cygpath -w "$W3_WORK")"
    fi
    "$REF_DIR/venv/Scripts/python.exe" -I "$(cygpath -w "$W3_DIR/w3_msgfmt.py")" "$(cygpath -w "$REPO_DIR/upstream/src/locale")" "$(cygpath -w "$W3_WORK/locale")"
    export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$DJANGO_PORT" GRANIAN_WORKERS=1
    detach "$pidf" "$REF_DIR/venv/Scripts/granian.exe" "$W3_WORK/django.out.log" "$W3_WORK/django.err.log" \
      --interface asginl --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
    for _ in $(seq 1 90); do code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DJANGO_PORT/accounts/login/" || true); [[ "$code" == 200 ]] && break; sleep 1; done
    [[ "$code" == 200 ]] || { echo "Start fehlgeschlagen ($code)"; tail -20 "$W3_WORK/django.err.log"; exit 1; }
    echo "Django W3: PID $(cat "$pidf"), http://127.0.0.1:$DJANGO_PORT";;
  stop)
    [[ -f "$pidf" ]] && taskkill //PID "$(cat "$pidf")" //T //F >/dev/null 2>&1 || true; rm -f "$pidf"
    [[ -f "$W3_WORK/sinks.pid" ]] && taskkill //PID "$(cat "$W3_WORK/sinks.pid")" //T //F >/dev/null 2>&1 || true; rm -f "$W3_WORK/sinks.pid"; echo gestoppt;;
esac
