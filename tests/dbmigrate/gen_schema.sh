#!/usr/bin/env bash
# Generates the embedded target schema of pl-dbmigrate (crates/pl-dbmigrate/schema/v3.4.2-*):
#   1. own empty DB pl_dbmigrate_schema on the reference PostgreSQL (never touch `paperless`)
#   2. upstream `manage.py migrate` (paperless-ngx v3.4.2, unchanged) against it
#   3. pg_dump --schema-only (pre-data/post-data), cleaned; column list; django_migrations;
#      replacements by squashed migrations (MigrationLoader)
# Invocation: bash tests/dbmigrate/gen_schema.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
OUT="$REPO_DIR/crates/pl-dbmigrate/schema"
WORK="$HERE/.work/schema"
DB=pl_dbmigrate_schema
mkdir -p "$OUT" "$WORK/data" "$WORK/media" "$WORK/consume" "$WORK/logs"

PSQL_SU=("$PG_BIN/psql" -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q)
PGPASSWORD="$PG_SUPERPASS" "${PSQL_SU[@]}" -c "DROP DATABASE IF EXISTS $DB" \
  -c "CREATE DATABASE $DB OWNER paperless ENCODING 'UTF8' TEMPLATE template0"

export PAPERLESS_DBNAME=$DB
export PAPERLESS_DATA_DIR="$WORK/data" PAPERLESS_MEDIA_ROOT="$WORK/media"
export PAPERLESS_CONSUMPTION_DIR="$WORK/consume" PAPERLESS_LOGGING_DIR="$WORK/logs"
export DJANGO_SETTINGS_MODULE=paperless.settings
export PYTHONPATH="$REPO_DIR/upstream/src"
"$PY" "$REPO_DIR/upstream/src/manage.py" migrate --no-input >"$WORK/migrate.log" 2>&1 \
  || { tail -30 "$WORK/migrate.log"; exit 1; }

P=("$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" -At -F $'\t')
clean() {
  # SET/set_config lines (would empty search_path), \restrict (pg_dump 18), comments, runs of blank lines
  sed -E -e '/^SET /d' -e '/^SELECT pg_catalog\.set_config/d' -e '/^\\(un)?restrict /d' -e '/^--/d' \
    | cat -s
}
{ echo "-- paperless-ngx v3.4.2, Django-Schema auf PostgreSQL (pre-data). Erzeugt von tests/dbmigrate/gen_schema.sh – nicht von Hand ändern."
  "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" --schema-only --no-owner --no-privileges \
    --no-comments --section=pre-data | clean; } >"$OUT/v3.4.2-postgres-pre.sql"
{ echo "-- paperless-ngx v3.4.2, Django-Schema auf PostgreSQL (post-data). Erzeugt von tests/dbmigrate/gen_schema.sh – nicht von Hand ändern."
  "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" --schema-only --no-owner --no-privileges \
    --no-comments --section=post-data | clean; } >"$OUT/v3.4.2-postgres-post.sql"

{ echo "# table	column	ordinal	udt	maxlen	nullable	generated	pk"
  "${P[@]}" -c "
SELECT c.table_name, c.column_name, c.ordinal_position, c.udt_name, coalesce(c.character_maximum_length::text, ''),
       (c.is_nullable = 'YES')::int, (c.is_generated = 'ALWAYS')::int,
       (EXISTS (SELECT 1 FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
                WHERE i.indisprimary AND i.indrelid = ('public.' || quote_ident(c.table_name))::regclass
                  AND a.attname = c.column_name))::int
FROM information_schema.columns c JOIN information_schema.tables t
  ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
WHERE c.table_schema = 'public' ORDER BY 1, 3"; } >"$OUT/v3.4.2-columns.tsv"

{ echo "# app	name  (django_migrations einer frisch migrierten v3.4.2-DB)"
  "${P[@]}" -c "SELECT app, name FROM django_migrations ORDER BY app, name"; } >"$OUT/v3.4.2-migrations.tsv"

{ echo "# app	name	squashed_app	squashed_name  (durch gesquashte Migration ersetzt)"
  "$PY" -I -c "
import os, sys
sys.path.insert(0, os.environ['PYTHONPATH'])
import django; django.setup()
from django.db.migrations.loader import MigrationLoader
l = MigrationLoader(None, ignore_no_migrations=True)
rows = sorted((r[0], r[1], k[0], k[1]) for k, m in l.replacements.items() for r in m.replaces)
for r in rows: print('\t'.join(r))
"; } >"$OUT/v3.4.2-replaced.tsv"

# Force Unix line endings (Windows psql)
for f in "$OUT"/v3.4.2-*; do sed -i 's/\r$//' "$f"; done
wc -l "$OUT"/v3.4.2-*
PGPASSWORD="$PG_SUPERPASS" "${PSQL_SU[@]}" -c "DROP DATABASE IF EXISTS $DB"
echo "Schema erzeugt in $OUT"

# All migration names of the paperless apps that ever existed in the upstream history up to v3.4.2
# (older, gradually updated installations still have these rows in django_migrations).
{ echo "# app	name  (historische Migrationen der paperless-Apps bis v3.4.2, aus git log)"
  git -C "$REPO_DIR/upstream" log --format= --name-only v3.4.2 -- 'src/*/migrations/*.py' \
    | sed -nE 's#^src/([a-z_]+)/migrations/([0-9A-Za-z_]+)\.py$#\1\t\2#p' | grep -v $'\t__init__$' | sort -u; } \
  >"$OUT/v3.4.2-historical.tsv"
wc -l "$OUT/v3.4.2-historical.tsv"
