#!/usr/bin/env bash
# createsuperuser / manage_superuser / changepassword: Rust vs upstream on one DB copy each;
# password hashes are checked by upstream (django.contrib.auth.authenticate) against the Rust DB.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BIN="${PLCLI_BIN:-$REPO_DIR/target/debug/examples/manage.exe}"
fail=0
row() { psql_db "pl_w7_$1" -F '|' -c "SELECT username, email, is_staff, is_superuser, is_active, first_name, last_name, left(password, 22), (SELECT count(*) FROM auth_user_groups g WHERE g.user_id=u.id) FROM auth_user u WHERE username='$2'"; }
auth() { use_env "$1"; manage_py shell -c "from django.contrib.auth import authenticate; print(authenticate(username='$2', password='$3') is not None)" 2>/dev/null | tail -1; }
for side in rs up; do copy_env "usr_$side" >/dev/null 2>&1; done
# createsuperuser --no-input
use_env usr_rs; DJANGO_SUPERUSER_PASSWORD=Sup3r-Secret-9 "$BIN" createsuperuser --no-input --username root2 --email "Root@Example.COM" >"$WORKROOT/usr_rs.out" 2>&1
use_env usr_up; DJANGO_SUPERUSER_PASSWORD=Sup3r-Secret-9 manage_py createsuperuser --no-input --username root2 --email "Root@Example.COM" >"$WORKROOT/usr_up.out" 2>&1
diff <(grep -v "^?\|HINT\|WARN\|System check\|^$" "$WORKROOT/usr_up.out") "$WORKROOT/usr_rs.out" && echo "OK   createsuperuser Ausgabe" || fail=1
[[ "$(row usr_rs root2)" == "$(row usr_up root2)" ]] && echo "OK   createsuperuser Zeile: $(row usr_rs root2)" || { echo "FAIL Zeile"; row usr_rs root2; row usr_up root2; fail=1; }
[[ "$(auth usr_rs root2 Sup3r-Secret-9)" == True ]] && echo "OK   upstream authenticate mit Rust-Hash" || { echo "FAIL authenticate"; fail=1; }
# duplicate name
use_env usr_rs; "$BIN" createsuperuser --no-input --username root2 2>&1 | tail -1
use_env usr_up; manage_py createsuperuser --no-input --username root2 2>&1 | tail -1
# manage_superuser: superuser already exists (admin) → message; empty DB → create
for side in rs up; do use_env "usr_$side"; done
use_env usr_rs; a=$(PAPERLESS_ADMIN_USER=neu PAPERLESS_ADMIN_PASSWORD=x "$BIN" manage_superuser)
use_env usr_up; b=$(PAPERLESS_ADMIN_USER=neu PAPERLESS_ADMIN_PASSWORD=x manage_py manage_superuser 2>/dev/null | tail -1)
[[ "$a" == "$b" ]] && echo "OK   manage_superuser (vorhanden): $a" || { echo "FAIL manage_superuser: '$a' vs '$b'"; fail=1; }
use_env usr_rs; a=$(PAPERLESS_ADMIN_USER=alice "$BIN" manage_superuser)
use_env usr_up; b=$(PAPERLESS_ADMIN_USER=alice manage_py manage_superuser 2>/dev/null | tail -1)
[[ "$a" == "$b" ]] && echo "OK   manage_superuser (Name existiert): $a" || { echo "FAIL: '$a' vs '$b'"; fail=1; }
for side in rs up; do psql_db "pl_w7_usr_$side" -c "UPDATE auth_user SET is_superuser=false" >/dev/null; done
use_env usr_rs; a=$(PAPERLESS_ADMIN_USER=chef PAPERLESS_ADMIN_MAIL=chef@x.invalid "$BIN" manage_superuser)
use_env usr_up; b=$(PAPERLESS_ADMIN_USER=chef PAPERLESS_ADMIN_MAIL=chef@x.invalid manage_py manage_superuser 2>/dev/null | tail -1)
[[ "$a" == "$b" ]] && echo "OK   manage_superuser (ohne Passwort): $a" || { echo "FAIL: '$a' vs '$b'"; fail=1; }
use_env usr_rs; a=$(PAPERLESS_ADMIN_USER=chef PAPERLESS_ADMIN_MAIL=chef@x.invalid PAPERLESS_ADMIN_PASSWORD=Chef-Pw-77 "$BIN" manage_superuser)
use_env usr_up; b=$(PAPERLESS_ADMIN_USER=chef PAPERLESS_ADMIN_MAIL=chef@x.invalid PAPERLESS_ADMIN_PASSWORD=Chef-Pw-77 manage_py manage_superuser 2>/dev/null | tail -1)
[[ "$a" == "$b" ]] && echo "OK   manage_superuser (anlegen): $a" || { echo "FAIL: '$a' vs '$b'"; fail=1; }
[[ "$(row usr_rs chef)" == "$(row usr_up chef)" ]] && echo "OK   Zeile chef gleich" || { row usr_rs chef; row usr_up chef; fail=1; }
[[ "$(auth usr_rs chef Chef-Pw-77)" == True ]] && echo "OK   authenticate chef" || fail=1
# changepassword (input via stdin); too short password first
use_env usr_rs; printf 'kurz\nkurz\nNeu-Pass-word-42\nNeu-Pass-word-42\n' | "$BIN" changepassword bob >"$WORKROOT/cpw_rs.out" 2>&1
cat "$WORKROOT/cpw_rs.out"
[[ "$(auth usr_rs bob Neu-Pass-word-42)" == True ]] && echo "OK   changepassword wirksam (upstream prüft)" || { echo "FAIL changepassword"; fail=1; }
use_env usr_rs; printf 'a\nb\na\nb\na\nb\n' | "$BIN" changepassword bob 2>&1 | tail -1
for side in rs up; do psql_su -c "DROP DATABASE IF EXISTS pl_w7_usr_$side WITH (FORCE)"; done
exit $fail
