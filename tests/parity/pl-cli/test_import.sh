#!/usr/bin/env bash
# document_importer: Rust vs upstream. Target in each case a fresh DB (Rust: `migrate`, upstream:
# `manage.py migrate`) with empty media folders. Comparison of DB content (all tables) and media.
# Transient and excluded: documents_document.modified (auto_now → import time, in upstream as in
# Rust), django_migrations.applied/id, AnonymousUser password/date_joined (random/time point).
# Prerequisite: test_export.sh ran (exports under .work/exp/out_*).
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BIN="${PLCLI_BIN:-$REPO_DIR/target/debug/examples/manage.exe}"
E="$WORKROOT/exp"; fail=0
IGN="documents_document.modified django_migrations.applied django_migrations.id"
fresh_rs() { copy_env "$1" empty >/dev/null 2>&1; use_env "$1"; "$BIN" migrate --no-input --skip-paperrust >/dev/null; }
fresh_up() { copy_env "$1" empty >/dev/null 2>&1; use_env "$1"; mkdir -p "$WORKROOT/$1/data/index"; manage_py migrate --no-input >/dev/null 2>&1; }
norm_anon() { psql_db "pl_w7_$1" -c "UPDATE auth_user SET password='!', date_joined='2000-01-01' WHERE username='AnonymousUser'" >/dev/null; }
imp_rs() { local n="$1"; use_env "$n"; shift; "$BIN" document_importer "$@" >"$WORKROOT/log_$n.txt" 2>&1 || { echo "rust import failed"; tail -5 "$WORKROOT/log_$n.txt"; }; }
imp_up() { local n="$1"; use_env "$n"; shift; manage_py document_importer --no-progress-bar "$@" >"$WORKROOT/log_$n.txt" 2>&1 || { echo "upstream import failed"; tail -5 "$WORKROOT/log_$n.txt"; }; }
cmpdb() { norm_anon "$1"; norm_anon "$2"
  local nd; nd=$(psql_db "pl_w7_$2" -c "SELECT count(*) FROM documents_document"); echo "   Dokumente im Ziel: $nd"; [[ "$nd" -gt 0 ]] || return 1
  "$PY" -I "$HERE_PLCLI/compare.py" db "pl_w7_$1" "pl_w7_$2" --ignore $IGN | tail -12
  local r=${PIPESTATUS[0]}
  diff <(psql_db "pl_w7_$1" -c "SELECT sequencename, last_value FROM pg_sequences WHERE schemaname='public' AND sequencename NOT LIKE 'django_migrations%' ORDER BY 1") \
       <(psql_db "pl_w7_$2" -c "SELECT sequencename, last_value FROM pg_sequences WHERE schemaname='public' AND sequencename NOT LIKE 'django_migrations%' ORDER BY 1") >/dev/null \
    || { echo "DIFF sequences"; r=1; }
  "$PY" -I "$HERE_PLCLI/compare.py" tree "$WORKROOT/$1/media/documents" "$WORKROOT/$2/media/documents" | tail -3 || r=1
  return $r; }
check() { if "$@"; then echo "OK   $CASE"; else echo "FAIL $CASE"; fail=1; fi; }

# Reference: upstream imports the upstream export
fresh_up imp_ref; imp_up imp_ref "$E/out_up_default"
# 1) Rust imports the upstream export (Rust-migrated DB)
CASE="Rust-Import upstream-Export"; fresh_rs imp_a; imp_rs imp_a "$E/out_up_default"; check cmpdb imp_ref imp_a
# 2) upstream imports the Rust export (Django-migrated DB)
CASE="upstream-Import Rust-Export"; fresh_up imp_b; imp_up imp_b "$E/out_rs_default"; check cmpdb imp_ref imp_b
# 3) ZIP in both directions
CASE="Rust-Import upstream-ZIP"; fresh_rs imp_c; imp_rs imp_c "$(ls "$E"/out_up_zip/*.zip)"; check cmpdb imp_ref imp_c
CASE="upstream-Import Rust-ZIP"; fresh_up imp_d; imp_up imp_d "$(ls "$E"/out_rs_zip/*.zip)"; check cmpdb imp_ref imp_d
# 4) split manifest
# (reference here: upstream imports the same split export – m2m IDs depend on the file order)
fresh_up imp_e2; imp_up imp_e2 "$E/out_up_split"
CASE="Rust-Import upstream-split"; fresh_rs imp_e; imp_rs imp_e "$E/out_up_split"; check cmpdb imp_e2 imp_e
# 5) Passphrase in both directions
use_env exp; rm -rf "$E/out_rs_pw" "$E/out_up_pw"; mkdir -p "$E/out_rs_pw" "$E/out_up_pw"
"$BIN" document_exporter "$E/out_rs_pw" --passphrase geheim >/dev/null 2>&1
manage_py document_exporter --no-progress-bar "$E/out_up_pw" --passphrase geheim >/dev/null 2>&1
grep -q '"password": "fake-mail-password"' "$E/out_rs_pw/manifest.json" && { echo "FAIL Passwort im Klartext"; fail=1; } || echo "OK   Passwort verschlüsselt"
CASE="upstream-Import Rust-Passphrase-Export"; fresh_up imp_f; imp_up imp_f "$E/out_rs_pw" --passphrase geheim; check cmpdb imp_ref imp_f
CASE="Rust-Import upstream-Passphrase-Export"; fresh_rs imp_g; imp_rs imp_g "$E/out_up_pw" --passphrase geheim; check cmpdb imp_ref imp_g
CASE="Rust-Import ohne Passphrase lehnt ab"; fresh_rs imp_h; use_env imp_h
out=$("$BIN" document_importer "$E/out_up_pw" 2>&1); grep -q "No passphrase was given, but this export contains encrypted fields" <<<"$out" && echo "OK   $CASE" || { echo "FAIL $CASE"; fail=1; }
# 6) --data-only
CASE="Rust-Import --data-only"; fresh_rs imp_i; imp_rs imp_i "$E/out_up_dataonly" --data-only
fresh_up imp_j; imp_up imp_j "$E/out_up_dataonly" --data-only
norm_anon imp_i; norm_anon imp_j; check "$PY" -I "$HERE_PLCLI/compare.py" db pl_w7_imp_i pl_w7_imp_j --ignore $IGN
for n in imp_e2 imp_ref imp_a imp_b imp_c imp_d imp_e imp_f imp_g imp_h imp_i imp_j; do psql_su -c "DROP DATABASE IF EXISTS pl_w7_$n WITH (FORCE)" 2>/dev/null; done
exit $fail
