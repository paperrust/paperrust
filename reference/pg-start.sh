#!/usr/bin/env bash
# Starts PostgreSQL of the reference (127.0.0.1:54329). PID: reference/run/postgres.pid
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
mkdir -p "$REF_DIR/run" "$REF_DIR/logs"
if [[ -f "$PGDATA/postmaster.pid" ]] && "$PG_BIN/pg_ctl" -D "$PGDATA" status >/dev/null 2>&1; then
  echo "PostgreSQL läuft bereits (PID $(head -1 "$PGDATA/postmaster.pid" | tr -d '\r'))"
else
  # start pg_ctl detached (see detach in env.sh), then wait for readiness.
  detach "$REF_DIR/run/pg_ctl.pid" "$PG_BIN/pg_ctl.exe" "$REF_DIR/logs/pg_ctl.out.log" "$REF_DIR/logs/pg_ctl.err.log" \
    start -D "$(cygpath -w "$PGDATA")" -l "$(cygpath -w "$REF_DIR/logs/postgres.log")" 
  for _ in $(seq 1 60); do
    "$PG_BIN/pg_isready" -q -h 127.0.0.1 -p "$PGPORT" && break
    sleep 1
  done
  "$PG_BIN/pg_isready" -q -h 127.0.0.1 -p "$PGPORT" || { echo "Start fehlgeschlagen, siehe logs/postgres.log"; tail -5 "$REF_DIR/logs/postgres.log"; exit 1; }
fi
head -1 "$PGDATA/postmaster.pid" | tr -d '\r' > "$REF_DIR/run/postgres.pid"
echo "PostgreSQL läuft: PID $(cat "$REF_DIR/run/postgres.pid"), 127.0.0.1:$PGPORT"
