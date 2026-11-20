#!/usr/bin/env bash
# Helper functions for the pl-cli parity tests (source them). Only own DBs pl_w7_* and own folders.
HERE_PLCLI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE_PLCLI/../../../reference/env.sh"
WORKROOT="$HERE_PLCLI/.work"
CR=$(printf '\r')
psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q "$@"; }
psql_db() { local db="$1"; shift; "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$db" -At "$@" | tr -d "$CR"; }
# copy_env <name> [empty]: DB pl_w7_<name> from paperless_snapshot (or empty) + folder .work/<name>/{data,media}
copy_env() {
  local name="$1" mode="${2:-snapshot}" db="pl_w7_$1" w="$WORKROOT/$1"
  psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"
  if [[ "$mode" == empty ]]; then
    psql_su -c "CREATE DATABASE $db OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
  else
    psql_su -c "CREATE DATABASE $db OWNER paperless TEMPLATE paperless_snapshot"
  fi
  rm -rf "$w"; mkdir -p "$w/consume" "$w/logs" "$w/export"
  if [[ "$mode" == empty ]]; then mkdir -p "$w/data" "$w/media"
  else tar -xf "$REF_DIR/snapshot/data.tar" -C "$w"; fi
  mkdir -p "$w/data" "$w/media"
}
# use_env <name>: switch environment (PAPERLESS_*) to the DB/folder of this copy (for upstream and Rust)
use_env() {
  local w="$WORKROOT/$1"
  export PAPERLESS_DBNAME="pl_w7_$1"
  export PAPERLESS_DATA_DIR="$(cygpath -w "$w/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$w/media")"
  export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$w/consume")" PAPERLESS_LOGGING_DIR="$(cygpath -w "$w/logs")"
  export PAPERLESS_REDIS_PREFIX="pl_w7_$1"   # own prefix in the reference Redis (cache/Celery separated)
}
manage_py() { "$PY" "$REPO_DIR/upstream/src/manage.py" "$@"; }
