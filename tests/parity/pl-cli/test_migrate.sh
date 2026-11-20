#!/usr/bin/env bash
# migrate: Rust fresh installation on an empty DB vs. upstream `manage.py migrate` on an empty DB.
# Compares pg_dump --schema-only, all table contents (without transient columns), sequences;
# afterwards second run (existing DB), --check, paperrust migrations, error cases.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BIN="${PLCLI_BIN:-$REPO_DIR/target/debug/examples/manage.exe}"
OUT="$WORKROOT/migrate"; rm -rf "$OUT"; mkdir -p "$OUT"
copy_env mig_rs empty; use_env mig_rs
"$BIN" migrate --no-input --skip-paperrust >"$OUT/rs_migrate.txt"
[[ -d "$WORKROOT/pl_w7_mig_up" ]] || true
bash "$HERE_PLCLI/fresh_django_db.sh" pl_w7_mig_up "$WORKROOT/mig_up" >/dev/null
dump() { local db="$1" f="$2"
  "$PG_BIN/pg_dump" -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$db" --schema-only --no-owner --no-privileges --no-comments \
    | grep -v -E '^.(un)?restrict ' >"$f.schema.sql"
  psql_db "$db" -F $'\t' -c "SELECT sequencename, last_value FROM pg_sequences WHERE schemaname='public' ORDER BY 1" >"$f.seq.tsv"
  : >"$f.data.tsv"
  for t in $(psql_db "$db" -c "SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE' ORDER BY 1"); do
    case "$t" in
      django_migrations) q="SELECT app, name FROM django_migrations ORDER BY app, name";;  # upstream order of the *_squashed rows fluctuates between runs
      auth_user) q="SELECT id, length(password), left(password,1), last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active FROM auth_user ORDER BY id";;
      *) q="SELECT * FROM \"$t\" ORDER BY 1";;
    esac
    { echo "## $t"; psql_db "$db" -F $'\t' -c "$q"; } >>"$f.data.tsv"
  done
}
dump pl_w7_mig_rs "$OUT/rs"; dump pl_w7_mig_up "$OUT/up"
fail=0
for k in schema.sql seq.tsv data.tsv; do
  if cmp -s "$OUT/rs.$k" "$OUT/up.$k"; then echo "OK   $k identisch"; else echo "DIFF $k"; diff "$OUT/up.$k" "$OUT/rs.$k" | head -20; fail=1; fi
done
grep -c "Applying" "$OUT/rs_migrate.txt" | sed 's/^/Applying-Zeilen Rust: /'
grep -c "Applying" "$WORKROOT/mig_up/migrate.log" | sed 's/^/Applying-Zeilen Django: /'
diff <(grep "Applying\|Operations\|Apply all\|Running" "$WORKROOT/mig_up/migrate.log") <(grep "Applying\|Operations\|Apply all\|Running" "$OUT/rs_migrate.txt") >/dev/null \
  && echo "OK   migrate-Ausgabe gleich" || { echo "DIFF migrate-Ausgabe"; fail=1; }
# upstream recognizes the Rust DB as completely migrated
use_env mig_rs
manage_py migrate --check >/dev/null 2>&1 && echo "OK   upstream migrate --check auf Rust-DB" || { echo "FAIL upstream migrate --check"; fail=1; }
echo "offene Migrationen laut upstream: $(manage_py showmigrations 2>/dev/null | grep -c '\[ \]' || true)"
# Second run (existing DB) + paperrust migrations
"$BIN" migrate --no-input >"$OUT/rs_migrate2.txt"; cat "$OUT/rs_migrate2.txt" | tail -5
"$BIN" migrate --check && echo "OK   Rust migrate --check" || { echo "FAIL Rust --check"; fail=1; }
psql_db pl_w7_mig_rs -c "SELECT name FROM paperrust_migrations ORDER BY 1"
"$BIN" migrate --no-input | grep -q "No migrations to apply" && echo "OK   idempotent"
# upstream DB (Django-created) is recognized by Rust as v3.4.2
use_env mig_up; "$BIN" migrate --no-input --skip-paperrust | grep -q "No migrations to apply" && echo "OK   Rust erkennt Django-DB"
# Error cases: missing / newer migration, foreign DB
psql_db pl_w7_mig_up -c "DELETE FROM django_migrations WHERE app='documents' AND name='0027_documentbarcode'" >/dev/null
"$BIN" migrate --no-input >"$OUT/err1.txt" 2>&1 && { echo "FAIL fehlende Migration nicht erkannt"; fail=1; } || { echo "OK   fehlende Migration:"; head -3 "$OUT/err1.txt"; }
psql_db pl_w7_mig_up -c "INSERT INTO django_migrations (app,name,applied) VALUES ('documents','0027_documentbarcode',now()),('documents','0099_future',now())" >/dev/null
"$BIN" migrate --no-input >"$OUT/err2.txt" 2>&1 && { echo "FAIL neuere Migration nicht erkannt"; fail=1; } || { echo "OK   neuere Migration:"; head -3 "$OUT/err2.txt"; }
copy_env mig_foreign empty; use_env mig_foreign; psql_db pl_w7_mig_foreign -c "CREATE TABLE foo (id int)" >/dev/null
"$BIN" migrate --no-input >"$OUT/err3.txt" 2>&1 && { echo "FAIL fremde DB"; fail=1; } || { echo "OK   fremde DB:"; head -2 "$OUT/err3.txt"; }
for db in pl_w7_mig_rs pl_w7_mig_up pl_w7_mig_foreign; do psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"; done
exit $fail
