#!/usr/bin/env bash
# Generates crates/pl-cli/baseline/*: data of a v3.4.2 DB freshly created with upstream `manage.py migrate`
# (ContentTypes, permissions, django_migrations in application order, singleton rows)
# as well as the exporter descriptor (gen_descriptor.py, from Django _meta).
# Invocation: bash tests/parity/pl-cli/gen_baseline.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../../reference/env.sh"
OUT="$REPO_DIR/crates/pl-cli/baseline"
DB=pl_w7_baseline
bash "$HERE/fresh_django_db.sh" "$DB"
P=("$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$DB" -At -F $'\t')
{ echo "# id	app_label	model  (django_content_type nach upstream migrate, v3.4.2)"
  "${P[@]}" -c "SELECT id, app_label, model FROM django_content_type ORDER BY id"; } >"$OUT/v3.4.2-content-types.tsv"
{ echo "# id	content_type_id	codename	name  (auth_permission nach upstream migrate, v3.4.2)"
  "${P[@]}" -c "SELECT id, content_type_id, codename, name FROM auth_permission ORDER BY id"; } >"$OUT/v3.4.2-permissions.tsv"
{ echo "# app	name  (django_migrations in Anwendungsreihenfolge = id)"
  "${P[@]}" -c "SELECT app, name FROM django_migrations ORDER BY id"; } >"$OUT/v3.4.2-migrations-applied.tsv"
{ echo "# sequence	last_value  (Sequenzen mit Wert nach upstream migrate)"
  "${P[@]}" -c "SELECT sequencename, last_value FROM pg_sequences WHERE schemaname='public' AND last_value IS NOT NULL ORDER BY 1"; } >"$OUT/v3.4.2-sequences.tsv"
"${P[@]}" -c "SELECT row_to_json(t) FROM paperless_applicationconfiguration t ORDER BY id" >"$OUT/v3.4.2-applicationconfiguration.json"
{ echo "# Ausgabe von upstream migrate (Applying-Zeilen in Planreihenfolge)"
  sed -nE 's/^  Applying ([A-Za-z_]+\.[0-9A-Za-z_]+)\.\.\. OK.*$/\1/p' "$HERE/.work/$DB/migrate.log"; } >"$OUT/v3.4.2-applying.txt"
# Check: only these tables may have rows
"${P[@]}" -c "SELECT format('SELECT %L, count(*) FROM %I;', table_name, table_name) FROM information_schema.tables
  WHERE table_schema='public' AND table_type='BASE TABLE' ORDER BY 1" | "${P[@]}" | awk -F'\t' '$2>0' >"$OUT/v3.4.2-nonempty.tsv"
export PAPERLESS_DBNAME=$DB
"$PY" -X utf8 "$HERE/gen_descriptor.py" >"$OUT/export-descriptor.json"
for f in "$OUT"/*; do sed -i 's/\r$//' "$f"; done
PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)"
wc -l "$OUT"/*
