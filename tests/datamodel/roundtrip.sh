#!/usr/bin/env bash
# Acceptance test round trip for the own data model (docs/DATAMODEL-V2.md §3.3), per restructuring stage.
#
#   Y  paperless-ngx v3.4.2 (upstream manage.py, unchanged) exports the reference data
#      (copy of paperless_snapshot)                                  = yardstick
#   1  PaperRust `manage migrate` on a copy (all stages) → `datamodel check` → export X
#      → X equals Y (manifest, files; compare.py tree)
#   2  Switch with the assistant: `paperrust-migrate-db switch` ngx copy → empty DB (loads
#      v3.4.2, then `migrate` with all stages) → export S → S equals Y
#   3  Way back: paperless-ngx 3.4.2 imports X into a fresh Django DB → DB content equals
#      the upstream import of Y (compare.py db)
#   4  PaperRust imports X into a fresh, fully migrated DB (import via the
#      compatibility views) → export Z → Z equals X
#   5  Rollback: `datamodel rollback --to 0` on the copy from 1 → export R equals Y,
#      `migrate` again → `datamodel check`
#
# Databases pl_w7_dm_* (own copies, deleted at the end), folder $DM_WORKROOT/dm_* (default %LOCALAPPDATA%/Temp/paperrust-dm).
# Invocation: bash tests/datamodel/roundtrip.sh
#   PAPERRUST_BIN       paperrust program (default target/debug/paperrust.exe)
#   MIGRATE_DB_BIN      paperrust-migrate-db (default target/debug/paperrust-migrate-db.exe)
#   KEEP=1              do not delete copies
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Reference environment (Python venv, PostgreSQL programs, snapshot) is only in the main checkout; in
# a worktree take it from there (REFERENCE_DIR overrides).
REFD="${REFERENCE_DIR:-$HERE/../../reference}"
if [[ ! -x "$REFD/venv/Scripts/python.exe" ]]; then
  REFD="$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir)/../reference"
fi
source "$REFD/env.sh"
HERE_PLCLI="$(cd "$HERE/../parity/pl-cli" && pwd)"
# short path: export file names (128-character titles) otherwise exceed MAX_PATH on Windows
WORKROOT="${DM_WORKROOT:-$(cygpath -u "${LOCALAPPDATA:-$HOME}")/Temp/paperrust-dm}"
CR=$(printf '
')
psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q "$@"; }
psql_db() { local db="$1"; shift; "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$db" -At "$@" | tr -d "$CR"; }
# copy_env <name> [empty]: DB pl_w7_<name> from paperless_snapshot (or empty) + folder .work/<name>
copy_env() {
  local name="$1" mode="${2:-snapshot}" db="pl_w7_$1" w="$WORKROOT/$1"
  psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"
  if [[ "$mode" == empty ]]; then
    psql_su -c "CREATE DATABASE $db OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
  else
    psql_su -c "CREATE DATABASE $db OWNER paperless TEMPLATE paperless_snapshot"
  fi
  rm -rf "$w"; mkdir -p "$w/consume" "$w/logs" "$w/export"
  [[ "$mode" == empty ]] || tar -xf "$REF_DIR/snapshot/data.tar" -C "$w"
  mkdir -p "$w/data" "$w/media"
}
use_env() {
  local w="$WORKROOT/$1"
  export PAPERLESS_DBNAME="pl_w7_$1"
  export PAPERLESS_DATA_DIR="$(cygpath -w "$w/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$w/media")"
  export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$w/consume")" PAPERLESS_LOGGING_DIR="$(cygpath -w "$w/logs")"
  export PAPERLESS_REDIS_PREFIX="pl_w7_$1"
}
manage_py() { "$PY" "$REPO_DIR/upstream/src/manage.py" "$@"; }
T="${CARGO_TARGET_DIR:-$HERE/../../target}/debug"
PR="${PAPERRUST_BIN:-$T/paperrust.exe}"
MDB="${MIGRATE_DB_BIN:-$T/paperrust-migrate-db.exe}"
export PAPERRUST_DATAMODEL_BACKUP=off
fail=0
ok() { echo "OK   $1"; }
bad() { echo "FAIL $1"; fail=1; }
IGN="documents_document.modified django_migrations.applied django_migrations.id"
rs() { "$PR" manage "$@"; }
norm_anon() { psql_db "pl_w7_$1" -c "UPDATE auth_user SET password='!', date_joined='2000-01-01' WHERE username='AnonymousUser'" >/dev/null; }
tree_eq() { "$PY" -I "$HERE_PLCLI/compare.py" tree "$1" "$2" | tail -3; return "${PIPESTATUS[0]}"; }
# Files equal (except manifest.json) and manifest equal in content (without order; further
# arguments go to manifest_eq.py, e.g. --ignore documents.document.modified)
content_eq() { local a="$1" b="$2"; shift 2
  local other; other=$("$PY" -I "$HERE_PLCLI/compare.py" tree "$a" "$b" | grep -E '^(ONLY|DIFF)' | grep -v ' manifest.json')
  [[ -z "$other" ]] || { echo "$other" | head -5; return 1; }
  "$PY" -I "$HERE/manifest_eq.py" "$a/manifest.json" "$b/manifest.json" "$@" | head -8; return "${PIPESTATUS[0]}"; }
fresh_up() { copy_env "$1" empty >/dev/null 2>&1; use_env "$1"; mkdir -p "$WORKROOT/$1/data/index"; manage_py migrate --no-input >/dev/null 2>&1; }
[[ -x "$PR" ]] || { echo "$PR fehlt"; exit 2; }

# Y: yardstick from upstream
copy_env dm_ngx >/dev/null 2>&1; use_env dm_ngx
Y="$WORKROOT/dm_ngx/export"
manage_py document_exporter --no-progress-bar "$Y" >"$WORKROOT/dm_ngx/exp.log" 2>&1 || { echo "upstream export failed"; tail -5 "$WORKROOT/dm_ngx/exp.log"; exit 1; }

# 1: rebuild in place, export
copy_env dm_pr >/dev/null 2>&1; use_env dm_pr
X="$WORKROOT/dm_pr/export"
rs migrate --no-input >"$WORKROOT/dm_pr/migrate.log" 2>&1 || { bad "migrate"; tail -5 "$WORKROOT/dm_pr/migrate.log"; }
rs datamodel status
rs datamodel check && ok "Kompatibilitäts-Sichten = v3.4.2-Spalten" || bad "datamodel check"
rs document_exporter "$X" >"$WORKROOT/dm_pr/exp.log" 2>&1 || { bad "export X"; tail -5 "$WORKROOT/dm_pr/exp.log"; }
# strict (bytes); from stage 4 only the guardian records are ordered differently (upstream: plan order
# of a join, PaperRust: by id) – otherwise order and content equal
GU="guardian.userobjectpermission,guardian.groupobjectpermission"
tree_eq "$Y" "$X" && ok "1 Export nach Umbau = ngx-Export (byte-gleich)"   || { content_eq "$Y" "$X" --ordered --any-order "$GU" && ok "1 Export nach Umbau = ngx-Export (Reihenfolge außer guardian gleich)" || bad "1 Export nach Umbau"; }

# 2: switch with the assistant
if [[ -x "$MDB" ]]; then
  copy_env dm_sw empty >/dev/null 2>&1
  S="$WORKROOT/dm_sw/export"
  ( unset PAPERLESS_MEDIA_ROOT PAPERLESS_DATA_DIR
    "$MDB" switch --quiet --no-backup --paperrust "$(cygpath -w "$PR")" \
      --from "postgres://paperless:paperless@127.0.0.1:$PGPORT/pl_w7_dm_ngx" \
      --media "$(cygpath -w "$WORKROOT/dm_ngx/media")" \
      --target "postgres://paperless:paperless@127.0.0.1:$PGPORT/pl_w7_dm_sw" \
      --target-media "$(cygpath -w "$WORKROOT/dm_sw/media")" \
      --report-dir "$(cygpath -w "$WORKROOT/dm_sw/report")" >"$WORKROOT/dm_sw/switch.log" 2>&1 )
  rc=$?; [[ $rc -eq 0 || $rc -eq 3 ]] || { bad "switch (Exit $rc)"; tail -15 "$WORKROOT/dm_sw/switch.log"; }
  use_env dm_sw
  # The database move only loads the v3.4.2 schema; the restructuring happens on first start (container:
  # paperrust-entrypoint → manage migrate), here by hand
  rs migrate --no-input >"$WORKROOT/dm_sw/migrate.log" 2>&1 || bad "2 migrate nach switch"
  rs datamodel check >/dev/null && ok "2 Assistent: Sichten = v3.4.2" || bad "2 datamodel check nach switch"
  rs document_exporter "$S" >"$WORKROOT/dm_sw/exp.log" 2>&1 || bad "export S"
  # The order of models without Meta.ordering follows the COPY order after the move
  content_eq "$Y" "$S" && ok "2 Export nach Umstieg = ngx-Export (inhaltlich)" || bad "2 Export nach Umstieg"
else
  echo "SKIP 2 ($MDB fehlt: cargo build -p pl-dbmigrate)"
fi

# 3: way back to paperless-ngx 3.4.2
fresh_up dm_imp_y; manage_py document_importer --no-progress-bar "$Y" >"$WORKROOT/dm_imp_y.log" 2>&1 || bad "upstream import Y"
fresh_up dm_imp_x; manage_py document_importer --no-progress-bar "$X" >"$WORKROOT/dm_imp_x.log" 2>&1 || { bad "upstream import X"; tail -5 "$WORKROOT/dm_imp_x.log"; }
norm_anon dm_imp_y; norm_anon dm_imp_x
"$PY" -I "$HERE_PLCLI/compare.py" db pl_w7_dm_imp_y pl_w7_dm_imp_x --ignore $IGN | tail -6
[[ ${PIPESTATUS[0]} -eq 0 ]] && ok "3 ngx importiert PaperRust-Export wie ngx-Export" || bad "3 ngx-Import"

# 4: PaperRust imports into the new model and exports again
copy_env dm_ri empty >/dev/null 2>&1; use_env dm_ri
Z="$WORKROOT/dm_ri/export"
rs migrate --no-input >/dev/null 2>&1 || bad "4 migrate leer"
rs document_importer "$X" >"$WORKROOT/dm_ri/imp.log" 2>&1 || { bad "4 import X"; tail -5 "$WORKROOT/dm_ri/imp.log"; }
rs document_exporter "$Z" >"$WORKROOT/dm_ri/exp.log" 2>&1 || bad "4 export Z"
# modified = import time (auto_now, upstream likewise)
content_eq "$X" "$Z" --ignore documents.document.modified && ok "4 Import→Export im neuen Modell verlustfrei" || bad "4 Import/Export"
# Sequences after the import: a new document gets the next free id
n=$(psql_db pl_w7_dm_ri -c "SELECT (SELECT max(id) FROM documents_document) < nextval(pg_get_serial_sequence(CASE WHEN to_regclass('paperrust_document') IS NULL THEN 'documents_document' ELSE 'paperrust_document' END, 'id')::regclass)")
[[ "$n" == t ]] && ok "4 Sequenz nach Import" || bad "4 Sequenz nach Import"

# 5: rollback and restructuring again
use_env dm_pr
R="$WORKROOT/dm_pr/export_rollback"; mkdir -p "$R"
rs datamodel rollback --to 0 >"$WORKROOT/dm_pr/rollback.log" 2>&1 || { bad "5 rollback"; tail -5 "$WORKROOT/dm_pr/rollback.log"; }
rs document_exporter "$R" >/dev/null 2>&1 || bad "5 export R"
tree_eq "$Y" "$R" && ok "5 Export nach Rückbau = ngx-Export (byte-gleich)"   || { content_eq "$Y" "$R" --ordered --any-order "$GU" && ok "5 Export nach Rückbau = ngx-Export (Reihenfolge außer guardian gleich)" || bad "5 Rückbau"; }
rs migrate --no-input >/dev/null 2>&1 && rs datamodel check >/dev/null && ok "5 erneuter Umbau" || bad "5 erneuter Umbau"

if [[ "${KEEP:-0}" != 1 ]]; then
  for n in dm_ngx dm_pr dm_sw dm_imp_y dm_imp_x dm_ri; do psql_su -c "DROP DATABASE IF EXISTS pl_w7_$n WITH (FORCE)" 2>/dev/null; rm -rf "$WORKROOT/$n"; done
fi
exit $fail
