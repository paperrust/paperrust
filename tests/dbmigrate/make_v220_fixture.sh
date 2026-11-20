#!/usr/bin/env bash
# Real paperless-ngx 2.20.15 databases for the switch test (test data only, own DBs pl_mig_*):
#   pl_mig_v220        PostgreSQL: upstream v2.20.15 `manage.py migrate` (unchanged, `git archive v2.20.15`)
#                      + seed_v220.py; files in .work/v220/media (MD5 checksums like 2.x)
#   pl_mig_v220_old    copy, reset to 2.20.4 (documents.1075 backwards, like paperless-ngx itself)
#   pl_mig_v220_django copy of pl_mig_v220, restructured with upstream v3.4.2 `manage.py migrate`
#                      (expected value: exactly what paperless-ngx 3.4.2 itself makes of it)
#   pl_mig_v220_old_django  pl_mig_v220_old via the upstream path 2.20.15 → 3.4.2 (3.4.2 alone
#                      refuses 2.20.4 with paperless.E002)
#   pl_mig_dev         paperless_snapshot (v3.4.2) + upstream `dev` (`git archive origin/dev`) migrate:
#                      documents.0028_replace_tag_tree_fields, 0029_… (paperless-ngx newer than 3.4.2)
#   .work/v220/sqlite/db.sqlite3  the same data on SQLite (own media folder .work/v220/sqlite-media)
#   GEN_SCHEMA=1: additionally generate embedded 2.20 schema crates/pl-dbmigrate/schema/v2.20-*
#                 (from a freshly migrated, empty DB pl_mig_v220_fresh)
# The reference venv is for 3.4.2; what 2.20.15 additionally imports at start is replaced by
# stubs/v220_settings.py with empty modules; django-celery-results is a placeholder
# (stubs/django_celery_results: 3.0 deletes these tables anyway).
# Clean up: bash tests/dbmigrate/run_v2_newer.sh --cleanup
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN="$(cd "$(git -C "$HERE" rev-parse --git-common-dir)/.." && pwd)"
source "$MAIN/reference/env.sh" >/dev/null
UP="${UPSTREAM_DIR:-$REPO_DIR/upstream}"
BASE="$HERE/.work/v220"
export PGPASSWORD="$PG_SUPERPASS"
su() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d "${DB:-postgres}" "$@"; }
q() { DB=$1 su -tA -c "$2"; }
newdb() { su -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE ${2:-template0}"; }
quiet() { grep -v "HINT\|can't find\|WARNINGS\|System check\|^\s*$" || true; }

rm -rf "$BASE"; mkdir -p "$BASE/src"
git -C "$UP" archive v2.20.15 src | tar -x -C "$BASE/src"
export V220_SRC="$BASE/src"
SEED="$(cygpath -w "$HERE/seed_v220.py")"
py220() {   # $1 = working folder (data/media), rest = manage.py arguments
  local work=$1; shift
  ( WORK="$work"; source "$HERE/v220_env.sh"; cd "$V220_SRC/src" && "$PY" manage.py "$@" ) 2>&1 | quiet
}
seed() { py220 "$1" shell -c "exec(open(r'$SEED', encoding='utf-8').read())"; }

if [[ "${GEN_SCHEMA:-0}" == 1 ]]; then
  newdb pl_mig_v220_fresh
  PAPERLESS_DBNAME=pl_mig_v220_fresh py220 "$BASE/fresh" migrate --no-input -v 0
  bash "$HERE/gen_schema_v220.sh" pl_mig_v220_fresh
  su -c "DROP DATABASE IF EXISTS pl_mig_v220_fresh WITH (FORCE)"
fi

# PostgreSQL
newdb pl_mig_v220
PAPERLESS_DBNAME=pl_mig_v220 py220 "$BASE" migrate --no-input -v 0
PAPERLESS_DBNAME=pl_mig_v220 seed "$BASE"
echo "pl_mig_v220: $(q pl_mig_v220 "SELECT count(*) FROM django_migrations") Migrationen, $(q pl_mig_v220 "SELECT count(*) FROM documents_document") Dokumente"

# 2.20.4 (without documents.1075)
newdb pl_mig_v220_old pl_mig_v220
PAPERLESS_DBNAME=pl_mig_v220_old py220 "$BASE/old" migrate documents 1074_workflowrun_deleted_at_workflowrun_restored_at_and_more --no-input -v 0
echo "pl_mig_v220_old: $(q pl_mig_v220_old "SELECT count(*) FROM django_migrations") Migrationen (ohne 1075)"

# Expected value: upstream 3.4.2 restructures the 2.20 database itself (reads the files for SHA-256)
django342() {   # $1 = target DB, $2 = template
  newdb "$1" "$2"
  ( cd "$UP/src" && PAPERLESS_DBNAME="$1" DJANGO_SETTINGS_MODULE=paperless.settings PYTHONPATH="$(cygpath -w "$UP/src")" \
      PAPERLESS_MEDIA_ROOT="$(cygpath -w "$BASE/media")" PAPERLESS_DATA_DIR="$(cygpath -w "$BASE/data")" \
      "$PY" manage.py migrate --no-input -v 0 ) 2>&1 | quiet
  echo "$1: $(q "$1" "SELECT count(*) FROM django_migrations") Migrationen (upstream 3.4.2 migrate)"
}
django342 pl_mig_v220_django pl_mig_v220
# upstream 3.4.2 refuses 2.20.4 (paperless.E002 "first to 2.20.15"); reference = the path demanded by
# upstream: first 2.20.15 `migrate` (documents.1075), then 3.4.2 `migrate`
newdb pl_mig_v220_old_215 pl_mig_v220_old
PAPERLESS_DBNAME=pl_mig_v220_old_215 py220 "$BASE/old" migrate --no-input -v 0
django342 pl_mig_v220_old_django pl_mig_v220_old_215
su -c "DROP DATABASE IF EXISTS pl_mig_v220_old_215 WITH (FORCE)"

# paperless-ngx newer than 3.4.2: upstream dev on a copy of the v3.4.2 snapshot
DEV="$HERE/.work/dev"; rm -rf "$DEV"; mkdir -p "$DEV/src" "$DEV/data" "$DEV/media"
git -C "$UP" archive origin/dev src | tar -x -C "$DEV/src"
newdb pl_mig_dev paperless_snapshot
( cd "$DEV/src/src" && PAPERLESS_DBNAME=pl_mig_dev DJANGO_SETTINGS_MODULE=paperless.settings PYTHONPATH="$(cygpath -w "$DEV/src/src")" \
    PAPERLESS_DATA_DIR="$(cygpath -w "$DEV/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$DEV/media")" \
    "$PY" manage.py migrate --no-input -v 0 ) 2>&1 | quiet
echo "pl_mig_dev: $(q pl_mig_dev "SELECT string_agg(name, ', ' ORDER BY name) FROM django_migrations WHERE app = 'documents' AND name >= '0028'") (upstream dev)"

# SQLite (same seed, own media folder)
mkdir -p "$BASE/sqlite" "$BASE/sqlite-media"
( WORK="$BASE/sqlite"; source "$HERE/v220_env.sh"; unset PAPERLESS_DBENGINE PAPERLESS_DBHOST PAPERLESS_DBNAME
  export PAPERLESS_DATA_DIR="$(cygpath -w "$BASE/sqlite")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$BASE/sqlite-media")"
  cd "$V220_SRC/src" && "$PY" manage.py migrate --no-input -v 0 \
  && "$PY" manage.py shell -c "exec(open(r'$SEED', encoding='utf-8').read())" ) 2>&1 | quiet
ls -la "$BASE/sqlite/db.sqlite3"
