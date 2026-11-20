#!/usr/bin/env bash
# End-to-end test "switch from an older paperless-ngx version" (3.1.3 → PaperRust), test data only:
#   D1  way (a), new database: switch --from postgres://pl_mig_v313 → pl_mig_copy
#       (catch-up while copying; source stays unchanged; target schema = fresh v3.4.2)
#   D2  way (a), same database: switch --from X --target X on a copy (pl_mig_inplace)
#       (automatic backup with pg_dump, catch-up in place; schema = fresh v3.4.2;
#        backup can be restored and yields 3.1.3 again)
#   D3  dry run and detect on the 3.1.3 source
#   E   way (b), export: paperrust manage document_exporter from the 3.1.3 copy after the catch-up,
#       switch --from <export> → pl_mig_export (needs a built paperrust; otherwise skipped)
# Beforehand: bash tests/dbmigrate/make_v313_fixture.sh ; cargo build -p pl-dbmigrate
# Clean up: bash tests/dbmigrate/run_catchup.sh --cleanup   (deletes pl_mig_* and .work/catchup)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN="$(cd "$(git -C "$HERE" rev-parse --git-common-dir)/.." && pwd)"
source "$MAIN/reference/env.sh" >/dev/null
unset PAPERLESS_MEDIA_ROOT PAPERLESS_DATA_DIR PAPERRUST_MIGRATE_TARGET PAPERRUST_MIGRATE_SOURCE
unset PAPERLESS_DBHOST PAPERLESS_DBPORT PAPERLESS_DBNAME PAPERLESS_DBUSER PAPERLESS_DBPASS
WORK="$HERE/.work/catchup"
T="${CARGO_TARGET_DIR:-$MAIN/target}/debug"
BIN="$T/paperrust-migrate-db.exe"
PR="${PAPERRUST_EXE:-$T/paperrust.exe}"
export PGPASSWORD="$PG_SUPERPASS"
fail() { echo "FEHLER: $*"; exit 1; }
w() { cygpath -w "$1"; }
su() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d "${DB:-postgres}" "$@"; }
q() { DB=$1 su -tA -c "$2"; }
newdb() { su -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE ${2:-template0}"; }
url() { echo "postgres://paperless:paperless@127.0.0.1:$PGPORT/$1"; }
schema() { "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" --schema-only --no-owner --no-privileges "$1" \
  | grep -v '^--\|^SET\|^SELECT pg_catalog\|^\\' | grep -v 'paperrust_' ; }
expect() { local want=$1; shift; set +e; "$@" >"$WORK/last.out" 2>"$WORK/last.err"; local rc=$?; set -e
  [[ " $want " == *" $rc "* ]] || { tail -40 "$WORK/last.out"; tail -20 "$WORK/last.err"; fail "Exit $rc, erwartet $want: $*"; }; }
jq_py() { "$PY" -I -c "import json,sys; r=json.load(open(sys.argv[1],encoding='utf-8')); print(eval(sys.argv[2]))" "$1/umstieg-bericht.json" "$2"; }

if [[ "${1:-}" == "--cleanup" ]]; then
  for d in $(q postgres "SELECT datname FROM pg_database WHERE datname LIKE 'pl\_mig\_%'"); do
    su -c "DROP DATABASE IF EXISTS $d WITH (FORCE)"; done
  rm -rf "$WORK"; echo "aufgeräumt"; exit 0
fi
[[ -x "$BIN" ]] || fail "$BIN fehlt (cargo build -p pl-dbmigrate)"
[[ -n "$(q postgres "SELECT 1 FROM pg_database WHERE datname='pl_mig_v313'")" ]] || fail "pl_mig_v313 fehlt (make_v313_fixture.sh)"
rm -rf "$WORK"; mkdir -p "$WORK"
mkdir -p "$WORK/snap"; tar -xf "$REPO_DIR/reference/snapshot/data.tar" -C "$WORK/snap" media
MEDIA="$WORK/snap/media"
newdb pl_mig_ref342 paperless_snapshot; schema pl_mig_ref342 > "$WORK/schema-342.sql"   # fresh v3.4.2 (template accepts no connections)
MIG_BEFORE=$(q pl_mig_v313 "SELECT count(*) FROM django_migrations")
[[ "$MIG_BEFORE" == 242 ]] || fail "Fixture: $MIG_BEFORE statt 242 Migrationen"

# ---------------------------------------------------------------------------------------- D3
echo "[D3] Probelauf/detect auf paperless-ngx 3.1.3"
expect 0 "$BIN" detect --from "$(url pl_mig_v313)"
grep -q '"schema": "3.1.x"' "$WORK/last.out" || { cat "$WORK/last.out"; fail "detect erkennt 3.1.x nicht"; }
newdb pl_mig_copy
expect "0 3" "$BIN" switch --dry-run --quiet --from "$(url pl_mig_v313)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_copy)" --target-media "$(w "$WORK/D1/media")" --report-dir "$(w "$WORK/D3")"
echo "    Probelauf: $(jq_py "$WORK/D3" "r['verdict']")"

# ---------------------------------------------------------------------------------------- D1
echo "[D1] Weg (a) in eine neue Datenbank (Quelle 3.1.3 bleibt unverändert)"
expect "0 3" "$BIN" switch --from "$(url pl_mig_v313)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_copy)" --target-media "$(w "$WORK/D1/media")" --report-dir "$(w "$WORK/D1")"
cp "$WORK/last.out" "$WORK/D1/ausgabe.txt"; cp "$WORK/last.err" "$WORK/D1/fortschritt.txt"
echo "    Ergebnis: $(jq_py "$WORK/D1" "r['verdict']"), Zählung: $(jq_py "$WORK/D1" "sorted(set(c['level'] for c in r['counts']))")"
[[ "$(jq_py "$WORK/D1" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung D1"
[[ "$(jq_py "$WORK/D1" "r['files']['target']['problems']")" == 0 ]] || fail "Dateien D1"
grep -q "Kurzfassung" "$WORK/D1/ausgabe.txt" || fail "Kurzfassung fehlt"
[[ "$(q pl_mig_v313 "SELECT count(*) FROM django_migrations")" == "$MIG_BEFORE" ]] || fail "Quelle verändert"
[[ -z "$(q pl_mig_v313 "SELECT to_regclass('documents_documentbarcode')")" ]] || fail "Quelle verändert (Tabelle)"
schema pl_mig_copy > "$WORK/schema-copy.sql"
diff -u "$WORK/schema-342.sql" "$WORK/schema-copy.sql" > "$WORK/D1/schema.diff" || fail "Schema D1 ≠ v3.4.2 (siehe $WORK/D1/schema.diff)"
[[ "$(q pl_mig_copy "SELECT count(*) FROM django_migrations")" == 247 ]] || fail "django_migrations D1"
[[ "$(q pl_mig_copy "SELECT count(*) FROM documents_documentbarcode")" == 0 ]] || fail "neue Tabelle documentbarcode"
[[ "$(q pl_mig_copy "SELECT count(*) FROM paperless_applicationconfiguration WHERE barcode_store_values IS NULL AND llm_embedding_api_key IS NULL")" == "$(q pl_mig_v313 "SELECT count(*) FROM paperless_applicationconfiguration")" ]] || fail "neue Spalten nicht NULL"
echo "    Schema = frisches v3.4.2, 247 Migrationen, Quelle unverändert"
sed -n '/Kurzfassung/,$p' "$WORK/D1/ausgabe.txt" | sed 's/^/    /'

# ---------------------------------------------------------------------------------------- D2
echo "[D2] Weg (a) gleiche Datenbank (Docker: nur Image tauschen) – Sicherung + Nachholen an Ort und Stelle"
newdb pl_mig_inplace pl_mig_v313
expect "0 3" env PAPERRUST_PG_BIN="$(w "$PG_BIN")" "$BIN" switch --from "$(url pl_mig_inplace)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_inplace)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/D2")"
cp "$WORK/last.out" "$WORK/D2/ausgabe.txt"
echo "    Ergebnis: $(jq_py "$WORK/D2" "r['verdict']")"
dump=$(ls "$WORK/D2"/paperless-vor-umstieg-*.pgdump 2>/dev/null | head -1)
[[ -s "$dump" ]] || fail "keine Sicherung"
schema pl_mig_inplace > "$WORK/schema-inplace.sql"
diff -u "$WORK/schema-342.sql" "$WORK/schema-inplace.sql" > "$WORK/D2/schema.diff" || fail "Schema D2 ≠ v3.4.2 (siehe $WORK/D2/schema.diff)"
[[ "$(q pl_mig_inplace "SELECT count(*) FROM django_migrations")" == 247 ]] || fail "django_migrations D2"
# restore backup → 3.1.3 again
newdb pl_mig_restore
"$PG_BIN/pg_restore" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" --no-owner -d pl_mig_restore "$dump"
[[ "$(q pl_mig_restore "SELECT count(*) FROM django_migrations")" == 242 ]] || fail "Sicherung nicht 3.1.3"
echo "    Sicherung $(basename "$dump") ok (zurückgespielt: 242 Migrationen = 3.1.3), Schema danach = v3.4.2"
# without pg_dump (Docker image): backup as a copy of the database on the server
newdb pl_mig_inplace2 pl_mig_v313
expect "0 3" env PAPERRUST_PG_BIN= PAPERRUST_DESKTOP_PG_BIN= PATH=/nonexistent "$BIN" switch --quiet --from "$(url pl_mig_inplace2)" \
  --media "$(w "$MEDIA")" --target "$(url pl_mig_inplace2)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/D2c")"
copydb=$(q postgres "SELECT datname FROM pg_database WHERE datname LIKE 'pl_mig_inplace2_vor_umstieg_%'")
[[ -n "$copydb" ]] || fail "keine Server-Kopie als Sicherung"
[[ "$(q "$copydb" "SELECT count(*) FROM django_migrations")" == 242 ]] || fail "Server-Kopie nicht 3.1.3"
[[ "$(q pl_mig_inplace2 "SELECT count(*) FROM django_migrations")" == 247 ]] || fail "D2c nicht nachgeholt"
echo "    ohne pg_dump: Sicherung als Server-Kopie $copydb (242 Migrationen), nachgeholt auf 247"
# second run: nothing left to catch up
expect "0 3" "$BIN" switch --quiet --from "$(url pl_mig_inplace)" --media "$(w "$MEDIA")" \
  --target "$(url pl_mig_inplace)" --target-media "$(w "$MEDIA")" --report-dir "$(w "$WORK/D2b")"
[[ -z "$(ls "$WORK/D2b"/*.pgdump 2>/dev/null)" ]] || fail "unnötige Sicherung im zweiten Lauf"

# ---------------------------------------------------------------------------------------- E
if [[ -x "$PR" ]]; then
  echo "[E] Weg (b) Export → Import (Export aus der nachgeholten Kopie)"
  mkdir -p "$WORK/E/export"
  ( export PAPERLESS_DBENGINE=postgresql PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=$PGPORT PAPERLESS_DBNAME=pl_mig_copy \
      PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless PAPERLESS_MEDIA_ROOT="$(w "$WORK/D1/media")" \
      PAPERLESS_DATA_DIR="$(w "$WORK/E/data")"; mkdir -p "$WORK/E/data"
    "$PR" manage document_exporter "$(w "$WORK/E/export")" >"$WORK/E/export.log" 2>&1 ) || { tail -20 "$WORK/E/export.log"; fail "Export"; }
  newdb pl_mig_export
  expect "0 3" "$BIN" switch --from "$(w "$WORK/E/export")" --paperrust "$(w "$PR")" \
    --target "$(url pl_mig_export)" --target-media "$(w "$WORK/E/media")" --report-dir "$(w "$WORK/E/r")"
  cp "$WORK/last.out" "$WORK/E/ausgabe.txt"
  echo "    Ergebnis: $(jq_py "$WORK/E/r" "r['verdict']"), Zählung: $(jq_py "$WORK/E/r" "sorted(set(c['level'] for c in r['counts']))")"
  [[ "$(jq_py "$WORK/E/r" "all(c['level'] in ('OK','INFO') for c in r['counts'])")" == True ]] || fail "Zählung E"
  [[ "$(jq_py "$WORK/E/r" "r['files']['target']['problems']")" == 0 ]] || fail "Dateien E"
  sed -n '/Kurzfassung/,$p' "$WORK/E/ausgabe.txt" | sed 's/^/    /'
else
  echo "[E] übersprungen: $PR fehlt (paperrust bauen)"
fi
echo "OK – alle Nachhol-Tests bestanden. Aufräumen: bash tests/dbmigrate/run_catchup.sh --cleanup"
