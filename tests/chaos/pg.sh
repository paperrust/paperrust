#!/usr/bin/env bash
# Own throwaway PostgreSQL instance for the outage tests (tests/chaos).
# The shared reference instance (127.0.0.1:54329) is NEVER touched: own data directory under
# %TEMP%/paperrust-chaos/pg, own port (default 54400), own log file.
# Invocation: tests/chaos/pg.sh [init|start|stop|kill|restart|status|createdb <name> [--snapshot]|dropdb <name>|psql …|destroy]
#   init      = initdb (if not present) + start + role paperless
#   stop      = pg_ctl stop -m fast (clean),  kill = pg_ctl stop -m immediate (like crash/power failure)
#   createdb  = create empty DB; with --snapshot from reference/snapshot/paperless.dump (reference test data)
#   destroy   = stop and delete data directory
# Environment: CH_PGPORT (54400), CH_WORK (%TEMP%/paperrust-chaos). Local test values only.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TMPW="$(cygpath -u "${TEMP:-/tmp}")"
CH_WORK="${CH_WORK:-$TMPW/paperrust-chaos}"
PGP="${CH_PGPORT:-54400}"
[[ "$PGP" == "54329" ]] && { echo "[pg] Port 54329 ist die geteilte Referenz-Instanz - tabu"; exit 2; }
B="$REPO/reference/pg/pgsql/bin"
D="$CH_WORK/pg/data"
LOG="$CH_WORK/pg/postgres.log"
export PGHOST=127.0.0.1 PGPORT="$PGP" PGUSER=postgres
unset PGPASSWORD PGDATABASE PGDATA

ctl() { "$B/pg_ctl.exe" -D "$(cygpath -w "$D")" "$@"; }
psql_su() { "$B/psql.exe" -h 127.0.0.1 -p "$PGP" -U postgres -d postgres -v ON_ERROR_STOP=1 -q "$@"; }
running() { "$B/pg_isready.exe" -h 127.0.0.1 -p "$PGP" -q; }

init() {
  mkdir -p "$CH_WORK/pg"
  if [[ ! -f "$D/PG_VERSION" ]]; then
    "$B/initdb.exe" -D "$(cygpath -w "$D")" -U postgres --auth=trust -E UTF8 --locale=C >"$CH_WORK/pg/initdb.log" 2>&1 \
      || { echo "[pg] initdb fehlgeschlagen"; tail -5 "$CH_WORK/pg/initdb.log"; return 1; }
    {
      echo "port = $PGP"
      echo "listen_addresses = '127.0.0.1'"
      echo "max_connections = 120"
      echo "shared_buffers = 64MB"
      echo "fsync = on"
      echo "log_min_messages = warning"
    } >>"$D/postgresql.conf"
  fi
  start || return 1
  psql_su -c "DO \$\$BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='paperless') THEN CREATE ROLE paperless LOGIN SUPERUSER PASSWORD 'paperless'; END IF; END\$\$" \
    && echo "[pg] Instanz bereit auf 127.0.0.1:$PGP ($D)"
}

start() {
  running && return 0
  ctl -l "$(cygpath -w "$LOG")" -w -t 60 start </dev/null >/dev/null 2>&1 || { echo "[pg] Start fehlgeschlagen"; tail -5 "$LOG"; return 1; }
  echo "[pg] gestartet :$PGP"
}

createdb() {
  local db="$1"; shift
  case "$db" in paperless|paperless_snapshot|pl_scale_1m) echo "[pg] DB-Name $db tabu"; return 2 ;; esac
  psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db OWNER paperless" || return 1
  if [[ "${1:-}" == "--snapshot" ]]; then
    "$B/pg_restore.exe" -h 127.0.0.1 -p "$PGP" -U postgres -d "$db" --no-owner --role=paperless \
      "$(cygpath -w "$REPO/reference/snapshot/paperless.dump")" >"$CH_WORK/pg/restore-$db.log" 2>&1 \
      || { echo "[pg] pg_restore meldete Fehler (siehe $CH_WORK/pg/restore-$db.log)"; tail -3 "$CH_WORK/pg/restore-$db.log"; }
    psql_su -d "$db" -c "REASSIGN OWNED BY postgres TO paperless" >/dev/null 2>&1 || true
  fi
  echo "[pg] DB $db angelegt${1:+ ($1)}"
}

case "${1:-status}" in
  init) init ;;
  start) start ;;
  stop) running && ctl -m fast -w stop >/dev/null && echo "[pg] gestoppt (fast)"; true ;;
  kill) running && ctl -m immediate -w stop >/dev/null && echo "[pg] hart beendet (immediate)"; true ;;
  restart) ctl -m fast -w stop >/dev/null 2>&1; start ;;
  status) running && echo "[pg] läuft :$PGP" || { echo "[pg] läuft nicht"; exit 1; } ;;
  createdb) shift; createdb "$@" ;;
  dropdb) psql_su -c "DROP DATABASE IF EXISTS $2 WITH (FORCE)" ;;
  psql) shift; "$B/psql.exe" -h 127.0.0.1 -p "$PGP" -U postgres "$@" ;;
  destroy) running && ctl -m fast -w stop >/dev/null; rm -rf "$CH_WORK/pg"; echo "[pg] Instanz gelöscht" ;;
  *) echo "usage: pg.sh [init|start|stop|kill|restart|status|createdb <db> [--snapshot]|dropdb <db>|psql …|destroy]"; exit 2 ;;
esac
