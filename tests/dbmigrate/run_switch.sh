#!/usr/bin/env bash
# End-to-end test "switch with one click" (package Z1): paperrust-migrate-db switch/verify/detect
# for every source kind, with test data only:
#   A  SQLite data folder (copy of tests/dbmigrate/.work/sqlite from run_sqlite.sh)
#      dry run, switch, damaged target original → verify FAIL
#   B  PostgreSQL copy of paperless_snapshot (+ media from reference/snapshot/data.tar)
#      switch, spot check via API v1 of a started paperrust (temporary token),
#      same database (Docker case "just swap the image")
#   C  Export (paperrust manage document_exporter from the copy of B): folder, ZIP and an
#      export with a damaged original → WARN "already in the source"
# Source is checked for being unchanged each time. Databases: pl_switch_* (own).
# Invocation: bash tests/dbmigrate/run_switch.sh   (beforehand: cargo build -p pl-dbmigrate; paperrust built)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
# Nothing from the reference environment may act as target/media folder
unset PAPERLESS_MEDIA_ROOT PAPERLESS_DATA_DIR PAPERRUST_MIGRATE_TARGET PAPERRUST_MIGRATE_SOURCE
# PAPERLESS_DB* of the reference (DB `paperless`) never as target default: set only specifically per call
DBENV=(PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=$PGPORT PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless PAPERLESS_DBENGINE=postgresql)
unset PAPERLESS_DBHOST PAPERLESS_DBPORT PAPERLESS_DBNAME PAPERLESS_DBUSER PAPERLESS_DBPASS
WORK="$HERE/.work/switch"
T="${CARGO_TARGET_DIR:-$REPO_DIR/target}/debug"
BIN="$T/paperrust-migrate-db.exe"
PR="${PAPERRUST_EXE:-$T/paperrust.exe}"
API_PORT=${SWITCH_API_PORT:-18931}
fail() { echo "FEHLER: $*"; exit 1; }
w() { cygpath -w "$1"; }
m() { cygpath -m "$1"; }
PSQL_SU=("$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q)
su() { PGPASSWORD="$PG_SUPERPASS" "${PSQL_SU[@]}" "$@"; }
newdb() { su -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE ${2:-template0}"; }
url() { echo "postgres://paperless:paperless@127.0.0.1:$PGPORT/$1"; }
# Run with expected exit code (0 OK, 3 WARN, 1 FAIL)
expect() { local want=$1; shift; set +e; "$@" >"$WORK/last.out" 2>"$WORK/last.err"; local rc=$?; set -e
  [[ " $want " == *" $rc "* ]] || { tail -30 "$WORK/last.out"; tail -20 "$WORK/last.err"; fail "Exit $rc, erwartet $want: $*"; }; }
verdict() { "$PY" -I -c "import json,sys; print(json.load(open(sys.argv[1],encoding='utf-8'))['verdict'])" "$1/umstieg-bericht.json"; }
jq_py() { "$PY" -I -c "import json,sys; r=json.load(open(sys.argv[1],encoding='utf-8')); print(eval(sys.argv[2]))" "$1/umstieg-bericht.json" "$2"; }
tree_hash() { (cd "$1" && find . -type f ! -name 'db.sqlite3-shm' ! -name 'media.lock' -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -c1-16); }

[[ -x "$BIN" ]] || fail "$BIN fehlt (cargo build -p pl-dbmigrate)"
[[ -x "$PR" ]] || fail "$PR fehlt"
[[ -f "$HERE/.work/sqlite/data/db.sqlite3" ]] || fail "SQLite-Fixture fehlt (bash tests/dbmigrate/run_sqlite.sh)"
rm -rf "$WORK"; mkdir -p "$WORK"

# ---------------------------------------------------------------------------------------- A
echo "[A] SQLite-Datenordner"
mkdir -p "$WORK/A/paperless"
cp -r "$HERE/.work/sqlite/data" "$HERE/.work/sqlite/media" "$WORK/A/paperless/"
rm -rf "$WORK/A/paperless/data/index"
SRC_A="$WORK/A/paperless"
before=$(tree_hash "$SRC_A")
expect 0 "$BIN" detect --from "$(w "$SRC_A")"
grep -q '"kind": "sqlite-folder"' "$WORK/last.out" || fail "detect A"
expect "0 3" "$BIN" switch --dry-run --quiet --from "$(w "$SRC_A")" --report-dir "$(w "$WORK/A/r-dry")"
echo "    Probelauf: $(verdict "$WORK/A/r-dry")"
newdb pl_switch_sqlite
expect "0 3" "$BIN" switch --quiet --from "$(w "$SRC_A")" --target "$(url pl_switch_sqlite)" \
  --target-media "$(w "$WORK/A/target-media")" --report-dir "$(w "$WORK/A/r-switch")"
echo "    Umstieg: $(verdict "$WORK/A/r-switch"), Zählung: $(jq_py "$WORK/A/r-switch" "sorted(set(c['level'] for c in r['counts']))"), Dateien Ziel: $(jq_py "$WORK/A/r-switch" "r['files']['target']['ok']") ok / $(jq_py "$WORK/A/r-switch" "r['files']['target']['problems']") Befunde"
[[ "$(jq_py "$WORK/A/r-switch" "all(c['level']=='OK' for c in r['counts'])")" == True ]] || fail "Zählung A"
[[ "$(tree_hash "$SRC_A")" == "$before" ]] || fail "Quelle A verändert"
# damaged target original → FAIL
victim=$(find "$WORK/A/target-media/documents/originals" -type f -name '*.pdf' | sort | head -1)
printf 'X' | dd of="$victim" bs=1 seek=100 conv=notrunc status=none
expect 1 "$BIN" verify --quiet --from "$(w "$SRC_A")" --target "$(url pl_switch_sqlite)" \
  --target-media "$(w "$WORK/A/target-media")" --report-dir "$(w "$WORK/A/r-corrupt")"
[[ "$(jq_py "$WORK/A/r-corrupt" "[p['issue'] for p in r['files']['target']['problem_list'] if p['issue']=='checksum-mismatch']")" == "['checksum-mismatch']" ]] || fail "Beschädigung nicht erkannt"
echo "    Beschädigung: $(verdict "$WORK/A/r-corrupt") – $(grep -m1 'falsche Prüfsumme' "$WORK/A/r-corrupt/umstieg-bericht.txt" | sed 's/^ *//')"

# ---------------------------------------------------------------------------------------- B
echo "[B] PostgreSQL (Kopie von paperless_snapshot)"
newdb pl_switch_src_pg paperless_snapshot
mkdir -p "$WORK/B/snap"; tar -xf "$REPO_DIR/reference/snapshot/data.tar" -C "$WORK/B/snap" media
MEDIA_B="$WORK/B/snap/media"
before=$(tree_hash "$MEDIA_B")
newdb pl_switch_pg
expect "0 3" "$BIN" switch --quiet --skip-unknown-tables --from "$(url pl_switch_src_pg)" --media "$(w "$MEDIA_B")" \
  --target "$(url pl_switch_pg)" --target-media "$(w "$WORK/B/target-media")" --report-dir "$(w "$WORK/B/r-switch")"
echo "    Umstieg: $(verdict "$WORK/B/r-switch"), Dokumente $(jq_py "$WORK/B/r-switch" "r['counts'][0]['source']")→$(jq_py "$WORK/B/r-switch" "r['counts'][0]['target']")"
[[ "$(tree_hash "$MEDIA_B")" == "$before" ]] || fail "Quelle B (Medien) verändert"
# start paperrust on the target → spot check via API v1 with temporary token
mkdir -p "$WORK/B/data"
(
  export "${DBENV[@]}" PAPERLESS_DBNAME=pl_switch_pg PAPERLESS_MEDIA_ROOT="$(w "$WORK/B/target-media")" PAPERLESS_DATA_DIR="$(w "$WORK/B/data")"
  export PAPERRUST_BIND=127.0.0.1:$API_PORT PAPERRUST_ROUTES="$(w "$REPO_DIR/routes.toml")" NO_COLOR=1
  exec "$PR" serve </dev/null >"$WORK/B/serve.log" 2>&1
) & SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do curl -fs "http://127.0.0.1:$API_PORT/__rs/health" >/dev/null 2>&1 && break; sleep 1; done
expect "0 3" "$BIN" verify --quiet --from "$(url pl_switch_src_pg)" --media "$(w "$MEDIA_B")" \
  --target "$(url pl_switch_pg)" --target-media "$(w "$WORK/B/target-media")" --report-dir "$(w "$WORK/B/r-api")" \
  --api-url "http://127.0.0.1:$API_PORT" --api-temp-token --sample 10
kill $SERVER_PID 2>/dev/null || true; trap - EXIT
[[ "$(jq_py "$WORK/B/r-api" "r['spot_check']['status']")" == done ]] || fail "Stichprobe: $(jq_py "$WORK/B/r-api" "r['spot_check']['message']")"
echo "    Stichprobe API: $(jq_py "$WORK/B/r-api" "sum(d['ok'] for d in r['spot_check']['documents'])")/$(jq_py "$WORK/B/r-api" "len(r['spot_check']['documents'])") ok, Urteil $(verdict "$WORK/B/r-api")"
[[ "$(jq_py "$WORK/B/r-api" "all(d['ok'] for d in r['spot_check']['documents'])")" == True ]] || fail "Stichprobe abweichend"
tok=$(PGPASSWORD=paperless "$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d pl_switch_pg -c "select count(*) from authtoken_token")
src_tok=$(PGPASSWORD=paperless "$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d pl_switch_src_pg -c "select count(*) from authtoken_token")
[[ "$tok" == "$src_tok" ]] || fail "temporärer Token nicht entfernt ($tok vs $src_tok)"
# same database (Docker: just swap the image) → check only
expect "0 3" "$BIN" switch --quiet --from "$(url pl_switch_pg)" --media "$(w "$WORK/B/target-media")" \
  --target "$(url pl_switch_pg)" --report-dir "$(w "$WORK/B/r-same")"
grep -q "dieselbe Datenbank" "$WORK/B/r-same/umstieg-bericht.txt" || fail "gleiche DB nicht erkannt"
echo "    gleiche Datenbank: $(verdict "$WORK/B/r-same")"

# ---------------------------------------------------------------------------------------- C
echo "[C] Export (paperrust manage document_exporter aus pl_switch_src_pg)"
exporter() { env "${DBENV[@]}" PAPERLESS_DBNAME=pl_switch_src_pg PAPERLESS_MEDIA_ROOT="$(w "$MEDIA_B")" PAPERLESS_DATA_DIR="$(w "$WORK/C/data")" \
  "$PR" manage document_exporter "$@" >"$WORK/C/export.log" 2>&1 || { tail -20 "$WORK/C/export.log"; fail "document_exporter"; }; }
mkdir -p "$WORK/C/data" "$WORK/C/export" "$WORK/C/zip"
exporter "$(w "$WORK/C/export")"
exporter "$(w "$WORK/C/zip")" --zip --zip-name export
ZIP="$WORK/C/zip/export.zip"; [[ -f "$ZIP" ]] || fail "ZIP fehlt"
run_export() { # $1 source, $2 DB, $3 name, $4 expected exit codes
  newdb "$2"
  PAPERLESS_DATA_DIR="$(w "$WORK/C/data-$3")" expect "$4" "$BIN" switch --from "$(w "$1")" --target "$(url "$2")" \
    --target-media "$(w "$WORK/C/media-$3")" --paperrust "$(w "$PR")" --report-dir "$(w "$WORK/C/r-$3")"
  echo "    $3: $(verdict "$WORK/C/r-$3"), Dokumente $(jq_py "$WORK/C/r-$3" "r['counts'][0]['source']")→$(jq_py "$WORK/C/r-$3" "r['counts'][0]['target']"), Zählung $(jq_py "$WORK/C/r-$3" "sorted(set(c['level'] for c in r['counts']))")"
}
before=$(tree_hash "$WORK/C/export")
run_export "$WORK/C/export" pl_switch_export dir "0 3"
[[ "$(tree_hash "$WORK/C/export")" == "$before" ]] || fail "Export-Ordner verändert"
run_export "$ZIP" pl_switch_zip zip "0 3"
# damaged original in the export → same in the target → WARN with "already in the source"
cp -r "$WORK/C/export" "$WORK/C/export-bad"
victim=$(find "$WORK/C/export-bad" -type f -name '*.pdf' ! -path '*archive*' ! -path '*thumbnail*' | sort | head -1)
printf 'X' | dd of="$victim" bs=1 seek=100 conv=notrunc status=none
run_export "$WORK/C/export-bad" pl_switch_export_bad bad "3"
grep -q "bereits in der Quelle" "$WORK/C/r-bad/umstieg-bericht.txt" || fail "Vorschaden nicht als solcher gemeldet"

echo "ALLE UMSTIEGS-TESTS BESTANDEN (Berichte unter $WORK/*/r-*)"
