#!/usr/bin/env bash
# Embedded schema of a freshly migrated paperless-ngx 2.20.15 DB (invoked from make_v220_fixture.sh
# with GEN_SCHEMA=1): crates/pl-dbmigrate/schema/v2.20-{postgres-pre.sql,postgres-post.sql,columns.tsv,migrations.tsv}.
# django_celery_results (placeholder in the fixture, deleted by 3.0) stays out.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/../../crates/pl-dbmigrate/schema"
DB=$1
export PGPASSWORD=paperless
P=("$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" -At -F $'\t')
clean() { sed -E -e '/^SET /d' -e '/^SELECT pg_catalog\.set_config/d' -e '/^\\(un)?restrict /d' -e '/^--/d' | cat -s; }
dump() { "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" --schema-only --no-owner --no-privileges \
  --no-comments --section="$1" --exclude-table='django_celery_results_*' | clean; }
{ echo "-- paperless-ngx v2.20.15, Django-Schema auf PostgreSQL (pre-data). Erzeugt von tests/dbmigrate/gen_schema_v220.sh – nicht von Hand ändern."
  dump pre-data; } >"$OUT/v2.20-postgres-pre.sql"
{ echo "-- paperless-ngx v2.20.15, Django-Schema auf PostgreSQL (post-data). Erzeugt von tests/dbmigrate/gen_schema_v220.sh – nicht von Hand ändern."
  dump post-data; } >"$OUT/v2.20-postgres-post.sql"
{ printf '# table\tcolumn\tordinal\tudt\tmaxlen\tnullable\tgenerated\tpk\n'
  "${P[@]}" -c "
SELECT c.table_name, c.column_name, c.ordinal_position, c.udt_name, coalesce(c.character_maximum_length::text, ''),
       (c.is_nullable = 'YES')::int, (c.is_generated = 'ALWAYS')::int,
       (EXISTS (SELECT 1 FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
                WHERE i.indisprimary AND i.indrelid = ('public.' || quote_ident(c.table_name))::regclass
                  AND a.attname = c.column_name))::int
FROM information_schema.columns c JOIN information_schema.tables t
  ON t.table_schema = c.table_schema AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE'
WHERE c.table_schema = 'public' AND c.table_name NOT LIKE 'django_celery_results_%' ORDER BY 1, 3"; } >"$OUT/v2.20-columns.tsv"
{ printf '# app\tname  (django_migrations einer frisch migrierten v2.20.15-DB, ohne django_celery_results)\n'
  "${P[@]}" -c "SELECT app, name FROM django_migrations WHERE app <> 'django_celery_results' ORDER BY app, name"; } >"$OUT/v2.20-migrations.tsv"
for f in "$OUT"/v2.20-*; do sed -i 's/\r$//' "$f"; done
wc -l "$OUT"/v2.20-*
