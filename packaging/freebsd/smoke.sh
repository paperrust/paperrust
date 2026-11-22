#!/bin/sh
# Smoke test of the FreeBSD package, INSIDE the FreeBSD build machine (as root), after build.sh:
#
#   sh packaging/freebsd/smoke.sh [out/paperrust-<ver>.pkg]
#
# Follows packaging/freebsd/README.md: PostgreSQL 17 from pkg, empty database, pkg add, config file,
# `service paperrust migrate`, `service paperrust start` (daemon(8) supervisor, user paperrust),
# then: /__rs/health 200 (loopback), login page 200, /api/ 200/302/401/403, statics + web UI 200,
# admin via manage_superuser, API token, upload + ingestion of a text document by the built-in
# worker, `manage check` (Tesseract and PDFium found), orderly stop. Local addresses only.
# Leaves PostgreSQL and the package installed (throw-away CI machine).
set -eu
set -x

REPO="$(pwd)"
PKG="${1:-}"
if [ -z "$PKG" ]; then
	PKG="$(ls "$REPO"/out/paperrust-*.pkg 2>/dev/null | sed -n 1p)"
fi
[ -n "$PKG" ] && [ -f "$PKG" ] || { echo "package not found (out/paperrust-*.pkg)" >&2; exit 1; }
PORT="${SMOKE_PORT:-18000}"
BASE="http://127.0.0.1:$PORT"
CONF=/usr/local/etc/paperrust.conf
LOG=/var/log/paperrust/console.log
TMP="$(mktemp -d /tmp/paperrust-smoke.XXXXXX)"
PG="${SMOKE_PG_MAJOR:-17}"

log() { printf '\n[smoke] %s\n' "$*" >&2; }
dump() {
	set +x
	echo "----- $LOG (tail) -----" >&2
	tail -n 200 "$LOG" 2>/dev/null >&2 || true
	for f in /var/db/paperrust/data/log/paperrust.log; do
		[ -f "$f" ] && { echo "----- $f (tail) -----" >&2; tail -n 100 "$f" >&2; }
	done
	echo "----- processes -----" >&2
	ps auxww | grep -E '[p]aperrust|[d]aemon:' >&2 || true
}
die() {
	printf '[smoke] ERROR: %s\n' "$*" >&2
	dump
	exit 1
}
http() { curl -s -o /dev/null -w '%{http_code}' "$@" || true; }

# --------------------------------------------------------------------------------------- PostgreSQL
log "PostgreSQL $PG"
export ASSUME_ALWAYS_YES=yes
pkg install -y "postgresql$PG-server" "postgresql$PG-client" curl
sysrc postgresql_enable=YES
if [ ! -d "/var/db/postgres/data$PG" ]; then
	service postgresql initdb
fi
service postgresql start || service postgresql status
for _ in $(seq 1 30); do
	pg_isready -h 127.0.0.1 -p 5432 && break
	sleep 1
done
pg_isready -h 127.0.0.1 -p 5432 || die "PostgreSQL does not accept connections"
DBPASS="smoke-db-$(date +%s)"
su -m postgres -c "psql -v ON_ERROR_STOP=1 -d postgres" <<EOF
DROP DATABASE IF EXISTS paperrust;
DROP ROLE IF EXISTS paperrust;
CREATE ROLE paperrust LOGIN PASSWORD '$DBPASS';
CREATE DATABASE paperrust OWNER paperrust ENCODING 'UTF8' TEMPLATE template0;
EOF

# --------------------------------------------------------------------------------------- package
log "pkg add $PKG"
if pkg info -e paperrust; then pkg delete -y paperrust; fi
pkg add "$PKG"
pkg info -l paperrust | sed -n '1,15p'
id paperrust || die "post-install did not create user paperrust"
for f in /usr/local/bin/paperrust /usr/local/bin/paperrust-pdftool /usr/local/bin/paperrust-migrate-db \
	/usr/local/etc/rc.d/paperrust /usr/local/share/paperrust/static/base.css /usr/local/share/paperrust/light/index.html; do
	[ -e "$f" ] || die "$f missing after pkg add"
done
/usr/local/bin/paperrust --version
tesseract --version | sed -n 1p
tesseract --list-langs

cat >"$CONF" <<EOF
PAPERRUST_DBHOST=127.0.0.1
PAPERRUST_DBPORT=5432
PAPERRUST_DBNAME=paperrust
PAPERRUST_DBUSER=paperrust
PAPERRUST_DBPASS='$DBPASS'
PAPERRUST_SECRET_KEY='smoke-test-only-$(date +%s)-not-secret'
PAPERRUST_BIND=127.0.0.1:$PORT
PAPERRUST_URL=$BASE
PAPERRUST_TIME_ZONE=Europe/Zurich
PAPERRUST_OCR_LANGUAGE=eng
PAPERRUST_ADMIN_USER=admin
PAPERRUST_ADMIN_PASSWORD='smoke-admin'
EOF
chown root:paperrust "$CONF"
chmod 0640 "$CONF"

# as the service user with the same environment as the rc script
as_paperrust() {
	(
		set -a
		. "$CONF"
		set +a
		cd /var/db/paperrust
		su -m paperrust -c "$*"
	)
}

# --------------------------------------------------------------------------------------- service
log "service paperrust migrate"
sysrc paperrust_enable=YES
service paperrust migrate || die "manage migrate failed"
as_paperrust /usr/local/bin/paperrust manage manage_superuser || die "manage_superuser failed"
log "manage check"
as_paperrust /usr/local/bin/paperrust manage check >"$TMP/check.txt" 2>&1 || true
cat "$TMP/check.txt"
if grep -q "can't find PDFium" "$TMP/check.txt"; then die "PDFium (pkg pdfium) was not found or not loadable"; fi
if grep -q "can't find tesseract" "$TMP/check.txt"; then die "tesseract was not found"; fi

log "service paperrust start"
service paperrust start
code=000
for _ in $(seq 1 90); do
	code="$(http "$BASE/__rs/health")"
	[ "$code" = 200 ] && break
	service paperrust status >/dev/null 2>&1 || die "service ended during start"
	sleep 2
done
[ "$code" = 200 ] || die "/__rs/health: HTTP $code after 180 s"
service paperrust status

code="$(http "$BASE/accounts/login/")"
[ "$code" = 200 ] || die "login page: HTTP $code"
code="$(http "$BASE/api/")"
case "$code" in 200 | 302 | 401 | 403) ;; # 302: unauthenticated browsers go to the login page (as in paperless-ngx) *) die "/api/: HTTP $code" ;; esac
code="$(http "$BASE/static/base.css")"
[ "$code" = 200 ] || die "/static/base.css: HTTP $code (PAPERRUST_STATICDIR)"
code="$(http -L "$BASE/rs/app/")"
[ "$code" = 200 ] || die "/rs/app/ (web UI): HTTP $code (share/paperrust/light)"

token="$(curl -sf -X POST -d 'username=admin&password=smoke-admin' "$BASE/api/token/" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')"
[ -n "$token" ] || die "no API token for admin"
code="$(http -H "Authorization: Token $token" "$BASE/api/documents/")"
[ "$code" = 200 ] || die "/api/documents/: HTTP $code"

log "upload + ingestion (worker in the same process)"
printf 'PaperRust FreeBSD smoke test %s\nInvoice 2026\n' "$(date +%s)" >"$TMP/smoke.txt"
task="$(curl -sf -H "Authorization: Token $token" -F "document=@$TMP/smoke.txt" -F title=smoke "$BASE/api/documents/post_document/" | tr -d '"')"
[ -n "$task" ] || die "post_document returned no task id"
n=0
for _ in $(seq 1 60); do
	n="$(curl -sf -H "Authorization: Token $token" "$BASE/api/documents/?title__icontains=smoke" | sed -n 's/^{"count":\([0-9]*\).*/\1/p')"
	[ "${n:-0}" -ge 1 ] && break
	sleep 2
done
[ "${n:-0}" -ge 1 ] || die "document not ingested after 120 s (task $task)"

log "orderly stop"
service paperrust stop
sleep 2
if pgrep -f /usr/local/bin/paperrust >/dev/null; then die "paperrust still running after service stop"; fi
code="$(http "$BASE/__rs/health")"
[ "$code" = 000 ] || die "server still answers after stop (HTTP $code)"

rm -rf "$TMP"
log "OK"
