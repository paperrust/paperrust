#!/usr/bin/env bash
# start [a|b|c] | stop | reset – own Django instance for the social/OIDC parity (see w12env.sh).
set -euo pipefail
export W12_VARIANT="${2:-${W12_VARIANT:-a}}"
source "$(dirname "${BASH_SOURCE[0]}")/w12env.sh"
pidf="$W12_WORK/django.pid"; mkdir -p "$W12_WORK"/{data/index,media,consume,logs}
psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -q -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -c "$1"; }
stop_django() { [[ -f "$pidf" ]] && taskkill //PID "$(cat "$pidf")" //T //F >/dev/null 2>&1 || true; rm -f "$pidf"; }
case "${1:-start}" in
  reset)
    psql_su "DROP DATABASE IF EXISTS pl_w12_ref WITH (FORCE)"
    psql_su "CREATE DATABASE pl_w12_ref TEMPLATE paperless_snapshot OWNER paperless"
    echo "pl_w12_ref neu angelegt";;
  start)
    stop_django
    PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname='pl_w12_ref'" | grep -q 1 || bash "$0" reset
    if ! curl -s -o /dev/null "http://127.0.0.1:8141/jwks"; then
      detach "$W12_WORK/sinks.pid" "$REF_DIR/venv/Scripts/python.exe" "$W12_WORK/sinks.out.log" "$W12_WORK/sinks.err.log" -I "$(cygpath -w "$W12_DIR/w12_sinks.py")" "$(cygpath -w "$W12_WORK")"
    fi
    [[ -d "$W12_WORK/locale" ]] || "$REF_DIR/venv/Scripts/python.exe" -I "$(cygpath -w "$W12_DIR/w3_msgfmt.py")" "$(cygpath -w "$REPO_DIR/upstream/src/locale")" "$(cygpath -w "$W12_WORK/locale")"
    export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$DJANGO_PORT" GRANIAN_WORKERS=1
    detach "$pidf" "$REF_DIR/venv/Scripts/granian.exe" "$W12_WORK/django.out.log" "$W12_WORK/django.err.log" \
      --interface asginl --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
    for _ in $(seq 1 90); do code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DJANGO_PORT/accounts/login/" || true); [[ "$code" == 200 ]] && break; sleep 1; done
    [[ "$code" == 200 ]] || { echo "Start fehlgeschlagen ($code)"; tail -20 "$W12_WORK/django.err.log"; exit 1; }
    echo "$W12_VARIANT" > "$W12_WORK/variant"
    echo "Django W12 ($W12_VARIANT): PID $(cat "$pidf"), http://127.0.0.1:$DJANGO_PORT";;
  stop)
    stop_django
    [[ -f "$W12_WORK/sinks.pid" ]] && taskkill //PID "$(cat "$W12_WORK/sinks.pid")" //T //F >/dev/null 2>&1 || true; rm -f "$W12_WORK/sinks.pid"; echo gestoppt;;
esac
