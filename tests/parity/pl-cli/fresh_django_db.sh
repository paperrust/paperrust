#!/usr/bin/env bash
# Creates an empty DB and runs upstream `manage.py migrate` (paperless-ngx v3.4.2, unchanged).
# Invocation: bash tests/parity/pl-cli/fresh_django_db.sh <dbname> [<workdir>]
# Only DB names pl_w7_* (never `paperless`).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../../reference/env.sh"
DB="$1"; WORK="${2:-$HERE/.work/$DB}"
[[ "$DB" == pl_w7_* ]] || { echo "nur pl_w7_* erlaubt" >&2; exit 2; }
mkdir -p "$WORK/data" "$WORK/media" "$WORK/consume" "$WORK/logs"
PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q \
  -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
export PAPERLESS_DBNAME=$DB
export PAPERLESS_DATA_DIR="$(cygpath -w "$WORK/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$WORK/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$WORK/consume")" PAPERLESS_LOGGING_DIR="$(cygpath -w "$WORK/logs")"
"$PY" "$REPO_DIR/upstream/src/manage.py" migrate --no-input >"$WORK/migrate.log" 2>&1 || { tail -30 "$WORK/migrate.log"; exit 1; }
echo "ok $DB"
