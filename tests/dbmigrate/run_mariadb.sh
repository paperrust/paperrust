#!/usr/bin/env bash
# End-to-end test MariaDB → PostgreSQL for paperrust-migrate-db.
#   - official portable MariaDB (ZIP, archive.mariadb.org) to tests/dbmigrate/.mariadb (git-ignored),
#     SHA-256 checked against the published value; port 33069, 127.0.0.1 only, small buffer
#   - mysqlclient (official wheel from PyPI) isolated to .work/pydeps (reference venv stays unchanged)
#   - upstream v3.4.2 `migrate` (utf8mb4/utf8mb4_unicode_ci like upstream) + reference/seed.py + seed_edge.py
#   - --dry-run, move to pl_dbmigrate_test_mariadb, --verify-only; stop MariaDB afterwards
# Invocation: bash tests/dbmigrate/run_mariadb.sh   (KEEP_MARIADB=1: leave the server running)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
VER=11.4.8
SHA=ed86e93157af46317bb49161451c2ec258498a6fa8e68ca821ef1d780d855e6b
MDIR="$HERE/.mariadb"
MBIN="$MDIR/mariadb-$VER-winx64/bin"
WORK="$HERE/.work/mariadb"
BIN="${CARGO_TARGET_DIR:-$REPO_DIR/target}/debug/paperrust-migrate-db.exe"
TDB=pl_dbmigrate_test_mariadb
fail() { echo "FEHLER: $*"; exit 1; }

mkdir -p "$MDIR/download" "$WORK"/{data,media,consume,logs,run}
if [[ ! -x "$MBIN/mariadbd.exe" ]]; then
  zip="$MDIR/download/mariadb-$VER-winx64.zip"
  [[ -f "$zip" ]] || curl -fsSL --max-time 900 -o "$zip" "https://archive.mariadb.org/mariadb-$VER/winx64-packages/mariadb-$VER-winx64.zip"
  echo "$SHA  $zip" | sha256sum -c - || { rm -f "$zip"; fail "Prüfsumme MariaDB-ZIP falsch"; }
  (cd "$MDIR" && unzip -q "$zip")
fi
if [[ ! -d "$HERE/.work/pydeps/MySQLdb" ]]; then
  "$PY" -m pip install --quiet --no-deps --only-binary :all: --target "$(cygpath -w "$HERE/.work/pydeps")" "mysqlclient==2.3.0"
fi

DATA="$MDIR/data"
if [[ ! -d "$DATA/mysql" ]]; then
  "$MBIN/mariadb-install-db.exe" --datadir="$(cygpath -w "$DATA")" --port=33069 --password=root-local-only >"$WORK/logs/install-db.log" 2>&1 \
    || { tail "$WORK/logs/install-db.log"; fail "mariadb-install-db"; }
fi
start_mariadb() {
  "$MBIN/mariadb-admin.exe" -h 127.0.0.1 -P 33069 -uroot -proot-local-only ping >/dev/null 2>&1 && return 0
  detach "$WORK/run/mariadb.pid" "$MBIN/mariadbd.exe" "$WORK/logs/mariadb.out.log" "$WORK/logs/mariadb.err.log" \
    "--defaults-file=$(cygpath -w "$DATA/my.ini")" --port=33069 --bind-address=127.0.0.1 \
    --innodb-buffer-pool-size=64M --max-connections=20 --performance-schema=OFF --skip-name-resolve
  for _ in $(seq 1 60); do "$MBIN/mariadb-admin.exe" -h 127.0.0.1 -P 33069 -uroot -proot-local-only ping >/dev/null 2>&1 && return 0; sleep 1; done
  tail "$WORK/logs/mariadb.err.log"; fail "MariaDB startet nicht"
}
stop_mariadb() { [[ "${KEEP_MARIADB:-0}" == 1 ]] && return 0
  "$MBIN/mariadb-admin.exe" -h 127.0.0.1 -P 33069 -uroot -proot-local-only shutdown >/dev/null 2>&1 || true
  pid=$(cat "$WORK/run/mariadb.pid" 2>/dev/null || true); [[ -n "$pid" ]] && taskkill //F //PID "$pid" >/dev/null 2>&1 || true; }
trap stop_mariadb EXIT
start_mariadb
M=("$MBIN/mariadb.exe" -h 127.0.0.1 -P 33069 -uroot -proot-local-only)

if [[ "${REUSE_SOURCE:-0}" != 1 ]]; then
  "${M[@]}" -e "DROP DATABASE IF EXISTS paperless_src; CREATE DATABASE paperless_src CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
    CREATE USER IF NOT EXISTS 'paperless'@'127.0.0.1' IDENTIFIED BY 'paperless'; GRANT ALL ON paperless_src.* TO 'paperless'@'127.0.0.1';
    CREATE USER IF NOT EXISTS 'paperless_ro'@'127.0.0.1' IDENTIFIED BY 'paperless_ro'; GRANT SELECT ON paperless_src.* TO 'paperless_ro'@'127.0.0.1';"
  rm -rf "$WORK"/{data,media,consume}; mkdir -p "$WORK"/{data,media,consume}
  (
    source "$HERE/mariadb_env.sh"
    export PAPERLESS_DATA_DIR="$WORK/data" PAPERLESS_MEDIA_ROOT="$WORK/media"
    export PAPERLESS_CONSUMPTION_DIR="$WORK/consume" PAPERLESS_LOGGING_DIR="$WORK/logs"
    export DJANGO_SETTINGS_MODULE=paperless.settings
    echo "[src] migrate (MariaDB $VER)"; "$PY" "$REPO_DIR/upstream/src/manage.py" migrate --no-input >"$WORK/logs/migrate.log" 2>&1 \
      || { tail -20 "$WORK/logs/migrate.log"; exit 1; }
    echo "[src] reference/seed.py"; "$PY" -I "$REPO_DIR/reference/seed.py" >"$WORK/logs/seed.log" 2>&1 || { tail -20 "$WORK/logs/seed.log"; exit 1; }
    echo "[src] seed_edge.py"; "$PY" -I "$HERE/seed_edge.py"
  )
fi
# source with pure read user (SELECT permission) – proves that the tool cannot write anything
SRC="mariadb://paperless_ro:paperless_ro@127.0.0.1:33069/paperless_src"
TGT="postgres://paperless:paperless@127.0.0.1:$PGPORT/$TDB"
cks() { "${M[@]}" -N -e "CHECKSUM TABLE $("${M[@]}" -N -e "SELECT GROUP_CONCAT(CONCAT('paperless_src.', table_name)) FROM information_schema.tables WHERE table_schema='paperless_src'")" | md5sum; }
before=$(cks)

echo "[1] --dry-run"
"$BIN" --dry-run --quiet --source "$SRC" --report "$WORK/dry-run.json" | tail -3 || fail "dry-run"
PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres \
  -c "DROP DATABASE IF EXISTS $TDB WITH (FORCE)" -c "CREATE DATABASE $TDB OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
echo "[2] Umzug"
SECONDS=0
"$BIN" --source "$SRC" --target "$TGT" --report "$WORK/migrate.json" >"$WORK/migrate.out" 2>"$WORK/migrate.err" \
  || { grep -v " ok " "$WORK/migrate.out" | tail -30; fail "Umzug"; }
tail -3 "$WORK/migrate.out"; echo "    Dauer ${SECONDS} s"
echo "[3] --verify-only"
"$BIN" --verify-only --quiet --source "$SRC" --target "$TGT" --report "$WORK/verify.json" | tail -2 || fail "verify-only"
[[ "$before" == "$(cks)" ]] || fail "Quelle verändert!"
echo "[4] Quelle unverändert (CHECKSUM TABLE gleich)"
echo "OK: MariaDB-Test bestanden"
