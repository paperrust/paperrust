#!/usr/bin/env bash
# End-to-end test "take over paperless-ngx 2.20 and newer than 3.4.2 directly" (test data only, DBs pl_mig_*):
#   A  2.20.15, same database (Docker: just swap the image): backup + restructuring in place;
#      schema = upstream 3.4.2 after its own `migrate`, data row by row = upstream (compare_db.py)
#   B  2.20.4 (without documents.1075), same database: like A against the upstream result
#   C  2.20.15 → new PostgreSQL database (copy with 2.20 schema, restructuring in the target, one transaction);
#      source unchanged, data = upstream
#   D  2.20.15 on SQLite → new PostgreSQL database; schema = upstream, converted values checked
#   E  Dry run/detect on 2.20; encrypted document → clear abort, nothing changed
#   N1 paperless-ngx dev (documents.0028/0029, newer than 3.4.2): same DB and copy to new DB
#   N2 unknown future migration: clear message with export path, no crash, nothing changed
# Beforehand: bash tests/dbmigrate/make_v220_fixture.sh ; cargo build -p pl-dbmigrate
# Clean up: bash tests/dbmigrate/run_v2_newer.sh --cleanup   (deletes pl_mig_v2*, pl_mig_dev* and .work/v2test)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN="$(cd "$(git -C "$HERE" rev-parse --git-common-dir)/.." && pwd)"
source "$MAIN/reference/env.sh" >/dev/null
unset PAPERLESS_MEDIA_ROOT PAPERLESS_DATA_DIR PAPERRUST_MIGRATE_TARGET PAPERRUST_MIGRATE_SOURCE
unset PAPERLESS_DBHOST PAPERLESS_DBPORT PAPERLESS_DBNAME PAPERLESS_DBUSER PAPERLESS_DBPASS
WORK="$HERE/.work/v2test"
FIX="$HERE/.work/v220"
BIN="${BIN:-${CARGO_TARGET_DIR:-$MAIN/target}/debug/paperrust-migrate-db.exe}"
export PGPASSWORD="$PG_SUPERPASS"
fail() { echo "FEHLER: $*"; exit 1; }
w() { cygpath -w "$1"; }
su() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d "${DB:-postgres}" "$@"; }
q() { DB=$1 su -tA -c "$2"; }
newdb() { su -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE ${2:-template0}" 2>&1 | grep -v NOTICE || true; }
url() { echo "postgres://paperless:paperless@127.0.0.1:$PGPORT/$1"; }
dsn() { echo "postgresql://$PG_SUPERUSER:$PG_SUPERPASS@127.0.0.1:$PGPORT/$1"; }
schema() { "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" --schema-only --no-owner --no-privileges "$1" \
  | grep -v -e '^--' -e '^SET' -e '^SELECT pg_catalog' -e '^[\]' -e 'paperrust_'; }
expect() { local want=$1; shift; set +e; "$@" >"$WORK/last.out" 2>"$WORK/last.err"; local rc=$?; set -e
  [[ " $want " == *" $rc "* ]] || { tail -40 "$WORK/last.out"; tail -20 "$WORK/last.err"; fail "Exit $rc, erwartet $want: $*"; }; }
jq_py() { "$PY" -I -c "import json,sys; r=json.load(open(sys.argv[1],encoding='utf-8')); print(eval(sys.argv[2]))" "$1/umstieg-bericht.json" "$2"; }
# Django creates these ContentTypes/permissions via post_migrate, PaperRust on first start (manage migrate)
POST_MIGRATE="documents.documentbarcode,paperless.applicationconfiguration:view_global_statistics,paperless.applicationconfiguration:view_system_monitoring"
same_as_upstream() { "$PY" -I "$HERE/compare_db.py" "$(dsn "$1")" "$(dsn "$2")" --allow-extra-ct "$POST_MIGRATE" > "$WORK/compare-$2.txt" \
  || { cat "$WORK/compare-$2.txt"; fail "$2 weicht vom upstream-Ergebnis $1 ab"; }; tail -1 "$WORK/compare-$2.txt"; }

if [[ "${1:-}" == "--cleanup" ]]; then
  for d in $(q postgres "SELECT datname FROM pg_database WHERE datname LIKE 'pl\_mig\_v2%' OR datname LIKE 'pl\_mig\_dev%'"); do
    su -c "DROP DATABASE IF EXISTS $d WITH (FORCE)"; done
  rm -rf "$WORK" "$HERE/.work/v220" "$HERE/.work/dev"; echo "aufgeräumt"; exit 0
fi
[[ -x "$BIN" ]] || fail "$BIN fehlt (cargo build -p pl-dbmigrate)"
for d in pl_mig_v220 pl_mig_v220_old pl_mig_v220_django pl_mig_v220_old_django pl_mig_dev; do
  [[ -n "$(q postgres "SELECT 1 FROM pg_database WHERE datname='$d'")" ]] || fail "$d fehlt (make_v220_fixture.sh)"
done
rm -rf "$WORK"; mkdir -p "$WORK"
schema pl_mig_v220_django > "$WORK/schema-upstream.sql"
MEDIA="$FIX/media"
[[ "$(q pl_mig_v220 "SELECT count(*) FROM django_migrations")" == 213 ]] || fail "Fixture pl_mig_v220 nicht 2.20.15"

# --------------------------------------------------------------------------------------- E
echo "[E] Probelauf/detect auf paperless-ngx 2.20.15"
expect 0 "$BIN" detect --from "$(url pl_mig_v220)"
grep -q '"schema": "2.20.x"' "$WORK/last.out" || { cat "$WORK/last.out"; fail "detect erkennt 2.20.x nicht"; }
newdb pl_mig_v2_copy
expect "0 3" "$BIN" switch --dry-run --quiet --from "$(url pl_mig_v220)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_v2_copy)" --target-media "$(w "$WORK/C/media")" --report-dir "$(w "$WORK/E")"
echo "    Probelauf: $(jq_py "$WORK/E" "r['verdict']")"
newdb pl_mig_v2_gpg pl_mig_v220
q pl_mig_v2_gpg "UPDATE documents_document SET storage_type = 'gpg' WHERE id = 2" >/dev/null
expect 1 "$BIN" switch --quiet --from "$(url pl_mig_v2_gpg)" --target "$(url pl_mig_v2_gpg)" --media "$(w "$MEDIA")" \
  --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/E2")"
grep -q "verschlüsselt" "$WORK/E2/umstieg-bericht.txt" || fail "verschlüsselte Dokumente nicht erkannt"
[[ "$(q pl_mig_v2_gpg "SELECT count(*) FROM django_migrations")" == 213 ]] || fail "gpg-Quelle verändert"
echo "    verschlüsseltes Dokument: Abbruch mit Hinweis, Datenbank unverändert"

# --------------------------------------------------------------------------------------- A
echo "[A] 2.20.15 gleiche Datenbank – Sicherung + Umbau an Ort und Stelle"
newdb pl_mig_v2_inplace pl_mig_v220
expect "0 3" env PAPERRUST_PG_BIN="$(w "$PG_BIN")" "$BIN" switch --from "$(url pl_mig_v2_inplace)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_v2_inplace)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/A")"
cp "$WORK/last.out" "$WORK/A/ausgabe.txt"
echo "    Ergebnis: $(jq_py "$WORK/A" "r['verdict']")"
[[ "$(jq_py "$WORK/A" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung A"
schema pl_mig_v2_inplace > "$WORK/schema-A.sql"
diff -u "$WORK/schema-upstream.sql" "$WORK/schema-A.sql" > "$WORK/A/schema.diff" || fail "Schema A ≠ upstream (siehe $WORK/A/schema.diff)"
same_as_upstream pl_mig_v220_django pl_mig_v2_inplace
dump=$(ls "$WORK/A"/paperless-vor-umstieg-*.pgdump 2>/dev/null | head -1)
newdb pl_mig_v2_restore
"$PG_BIN/pg_restore" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" --no-owner -d pl_mig_v2_restore "$dump"
[[ "$(q pl_mig_v2_restore "SELECT count(*) FROM django_migrations")" == 213 ]] || fail "Sicherung nicht 2.20.15"
echo "    Schema = upstream, Daten = upstream, Sicherung $(basename "$dump") ergibt wieder 2.20.15"
expect "0 3" "$BIN" switch --quiet --from "$(url pl_mig_v2_inplace)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_v2_inplace)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/A2")"
[[ -z "$(ls "$WORK/A2"/*.pgdump 2>/dev/null)" ]] || fail "unnötige Sicherung im zweiten Lauf"
grep -q "Migrationsstand entspricht paperless-ngx 3.4.2" "$WORK/A2/umstieg-bericht.txt" || fail "zweiter Lauf: nicht 3.4.2"

# --------------------------------------------------------------------------------------- B
echo "[B] 2.20.4 (ohne documents.1075) gleiche Datenbank"
newdb pl_mig_v2_old pl_mig_v220_old
expect "0 3" env PAPERRUST_PG_BIN="$(w "$PG_BIN")" "$BIN" switch --quiet --from "$(url pl_mig_v2_old)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_v2_old)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/B")"
grep -q "2.19.6/2.20.0–2.20.4" "$WORK/B/umstieg-bericht.txt" || fail "B: Version nicht erkannt"
schema pl_mig_v2_old > "$WORK/schema-B.sql"
diff -u "$WORK/schema-upstream.sql" "$WORK/schema-B.sql" > "$WORK/B.schema.diff" || fail "Schema B ≠ upstream"
same_as_upstream pl_mig_v220_old_django pl_mig_v2_old

# --------------------------------------------------------------------------------------- C
echo "[C] 2.20.15 → neue Datenbank (Quelle bleibt unverändert)"
newdb pl_mig_v2_copy
expect "0 3" "$BIN" switch --from "$(url pl_mig_v220)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_v2_copy)" --target-media "$(w "$WORK/C/media")" --report-dir "$(w "$WORK/C")"
cp "$WORK/last.out" "$WORK/C/ausgabe.txt"
echo "    Ergebnis: $(jq_py "$WORK/C" "r['verdict']")"
[[ "$(jq_py "$WORK/C" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung C"
[[ "$(q pl_mig_v220 "SELECT count(*) FROM django_migrations")" == 213 ]] || fail "Quelle verändert"
[[ "$(q pl_mig_v220 "SELECT count(*) FROM information_schema.columns WHERE table_name='documents_document' AND column_name='storage_type'")" == 1 ]] || fail "Quelle verändert (Spalte)"
schema pl_mig_v2_copy > "$WORK/schema-C.sql"
diff -u "$WORK/schema-upstream.sql" "$WORK/schema-C.sql" > "$WORK/C/schema.diff" || fail "Schema C ≠ upstream (siehe $WORK/C/schema.diff)"
same_as_upstream pl_mig_v220_django pl_mig_v2_copy
sed -n '/Kurzfassung/,$p' "$WORK/C/ausgabe.txt" | head -6 | sed 's/^/    /'

# --------------------------------------------------------------------------------------- D
echo "[D] 2.20.15 auf SQLite → neue PostgreSQL-Datenbank"
newdb pl_mig_v2_sqlite
expect "0 3" "$BIN" switch --quiet --from "sqlite:///$(w "$FIX/sqlite/db.sqlite3" | tr '\\' '/')" --media "$(w "$FIX/sqlite-media")" \
  --target "$(url pl_mig_v2_sqlite)" --target-media "$(w "$WORK/D/media")" --report-dir "$(w "$WORK/D")"
echo "    Ergebnis: $(jq_py "$WORK/D" "r['verdict']")"
[[ "$(jq_py "$WORK/D" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung D"
schema pl_mig_v2_sqlite > "$WORK/schema-D.sql"
diff -u "$WORK/schema-upstream.sql" "$WORK/schema-D.sql" > "$WORK/D.schema.diff" || fail "Schema D ≠ upstream (siehe $WORK/D.schema.diff)"
# converted values like upstream (row IDs and contents match the PostgreSQL fixture)
for sql in "SELECT string_agg(rule_type::text || '=' || coalesce(value, ''), ';' ORDER BY id) FROM documents_savedviewfilterrule" \
           "SELECT string_agg(user_id::text || '=' || (settings->'saved_views')::text, ';' ORDER BY user_id) FROM documents_uisettings" \
           "SELECT string_agg(id::text || '=' || checksum || '/' || coalesce(archive_checksum, ''), ';' ORDER BY id) FROM documents_document" \
           "SELECT mode || '/' || coalesce(archive_file_generation, '-') || '/' || barcode_dpi FROM paperless_applicationconfiguration" \
           "SELECT string_agg(name || '=' || maximum_age || '/' || \"order\", ';' ORDER BY id) FROM paperless_mail_mailrule" \
           "SELECT count(*) FROM documents_paperlesstask"; do
  [[ "$(q pl_mig_v2_sqlite "$sql")" == "$(q pl_mig_v220_django "$sql")" ]] || fail "D: Wert weicht ab: $sql"
done
echo "    Schema = upstream; Regeln, Ansichten, SHA-256, OCR-Modus, Mail-Regeln, Aufgaben wie upstream"

# --------------------------------------------------------------------------------------- N1
echo "[N1] paperless-ngx dev (neuer als 3.4.2: documents.0028/0029)"
expect 0 "$BIN" detect --from "$(url pl_mig_dev)"
grep -q '"newer"' "$WORK/last.out" || { cat "$WORK/last.out"; fail "detect meldet die neueren Migrationen nicht"; }
newdb pl_mig_dev_inplace pl_mig_dev
expect "0 3" "$BIN" switch --quiet --from "$(url pl_mig_dev_inplace)" --target "$(url pl_mig_dev_inplace)" --report-dir "$(w "$WORK/N1a")"
[[ "$(jq_py "$WORK/N1a" "r['verdict']")" != FAIL ]] || fail "N1a FAIL"
grep -q "neuere paperless-ngx-Version" "$WORK/N1a/umstieg-bericht.txt" || fail "N1a: Hinweis fehlt"
newdb pl_mig_dev_copy
expect "0 3" "$BIN" switch --quiet --from "$(url pl_mig_dev)" --target "$(url pl_mig_dev_copy)" --report-dir "$(w "$WORK/N1b")"
echo "    gleiche DB: $(jq_py "$WORK/N1a" "r['verdict']"), Kopie: $(jq_py "$WORK/N1b" "r['verdict']")"
[[ "$(jq_py "$WORK/N1b" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung N1b"
schema pl_mig_dev > "$WORK/schema-dev.sql"; schema pl_mig_dev_copy > "$WORK/schema-devcopy.sql"
diff -u "$WORK/schema-dev.sql" "$WORK/schema-devcopy.sql" > "$WORK/N1b.schema.diff" || fail "Schema der Kopie ≠ dev (siehe $WORK/N1b.schema.diff)"
"$PY" -I "$HERE/compare_db.py" "$(dsn pl_mig_dev)" "$(dsn pl_mig_dev_copy)" > "$WORK/compare-dev.txt" || { cat "$WORK/compare-dev.txt"; fail "Kopie ≠ dev"; }
echo "    Kopie: Schema und Daten = Quelle ($(tail -1 "$WORK/compare-dev.txt"))"

# --------------------------------------------------------------------------------------- N2
echo "[N2] unbekannte künftige Migration"
newdb pl_mig_dev_future pl_mig_dev
q pl_mig_dev_future "INSERT INTO django_migrations (app, name, applied) VALUES ('documents', '0030_future_example', now())" >/dev/null
expect 1 "$BIN" switch --quiet --from "$(url pl_mig_dev_future)" --target "$(url pl_mig_dev_future)" --report-dir "$(w "$WORK/N2")"
grep -q "neueren paperless-ngx-Version" "$WORK/N2/umstieg-bericht.txt" || fail "N2: Meldung fehlt"
grep -q "document_exporter" "$WORK/N2/umstieg-bericht.txt" || fail "N2: Export-Weg fehlt"
[[ "$(jq_py "$WORK/N2" "r['verdict']")" == FAIL ]] || fail "N2: kein FAIL"
newdb pl_mig_dev_future_copy
expect 1 "$BIN" switch --quiet --from "$(url pl_mig_dev_future)" --target "$(url pl_mig_dev_future_copy)" --report-dir "$(w "$WORK/N2b")"
[[ "$(q pl_mig_dev_future_copy "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public'")" == 0 ]] || fail "N2b: Ziel nicht leer"
echo "    FAIL mit Meldung und Export-Weg, nichts verändert"
echo "OK – alle 2.x-/Neuer-Tests bestanden. Aufräumen: bash tests/dbmigrate/run_v2_newer.sh --cleanup"
