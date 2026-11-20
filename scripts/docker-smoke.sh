#!/usr/bin/env bash
# Smoke test of the PaperRust image with docker/docker-compose.postgres.yml (Linux host with Docker).
#
#   scripts/docker-smoke.sh                       # image paperrust/paperrust:local must exist
#   PAPERRUST_IMAGE=paperrust/paperrust:ci scripts/docker-smoke.sh
#   KEEP=1 scripts/docker-smoke.sh                # do not remove the container afterwards
#
# Checks: start with empty DB (migrate, superuser, index), healthcheck "healthy", login page 200,
# API with token 200, upload + ingestion of a text document via the built-in worker,
# management command (document_exporter) in the container, orderly stop. Own project name and
# port, so that a running installation is not touched. Local addresses only.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PAPERRUST_IMAGE="${PAPERRUST_IMAGE:-paperrust/paperrust:local}"
PROJECT="${SMOKE_PROJECT:-paperrust-smoke}"
PORT="${SMOKE_PORT:-18000}"
WORK="$(mktemp -d)"
log() { echo "[smoke] $*"; }
die() {
	echo "[smoke] FEHLER: $*" >&2
	dc logs --no-color --tail 200 || true
	exit 1
}

# own compose file: example + port + test environment (copy, the example stays unchanged)
cp "$REPO/docker/docker-compose.postgres.yml" "$WORK/docker-compose.yml"
sed -i "s/\"8000:8000\"/\"127.0.0.1:${PORT}:8000\"/" "$WORK/docker-compose.yml"
sed -i '/^    build:/,/^      dockerfile:/d' "$WORK/docker-compose.yml"
cat >"$WORK/docker-compose.env" <<EOF
PAPERLESS_SECRET_KEY=smoke-test-only-$(date +%s)-not-secret
PAPERLESS_TIME_ZONE=Europe/Zurich
PAPERLESS_OCR_LANGUAGE=eng
PAPERLESS_ADMIN_USER=admin
PAPERLESS_ADMIN_PASSWORD=smoke-admin
PAPERLESS_URL=http://127.0.0.1:${PORT}
EOF
mkdir -p "$WORK/consume" "$WORK/export"
chmod 0777 "$WORK/consume" "$WORK/export" # Container UID 1000 must be allowed to write

dc() { docker compose -p "$PROJECT" -f "$WORK/docker-compose.yml" "$@"; }
cleanup() {
	if [[ "${KEEP:-0}" != 1 ]]; then
		dc down -v --remove-orphans >/dev/null 2>&1 || true
		# Delete files of the container UID via a container if necessary (different host user)
		rm -rf "$WORK" 2>/dev/null || docker run --rm -v "$WORK:/w" --entrypoint rm "$PAPERRUST_IMAGE" -rf /w/export /w/consume || true
		rm -rf "$WORK" 2>/dev/null || true
	else
		log "behalten: $WORK (docker compose -p $PROJECT -f $WORK/docker-compose.yml down -v)"
	fi
}
trap cleanup EXIT

log "Image $PAPERRUST_IMAGE"
docker image inspect "$PAPERRUST_IMAGE" >/dev/null || die "Image fehlt (docker build -f docker/Dockerfile -t $PAPERRUST_IMAGE .)"
ids="$(docker run --rm --entrypoint id "$PAPERRUST_IMAGE")"
[[ "$ids" == uid=1000* ]] || die "Image läuft nicht als UID 1000 ($ids)"
langs="$(docker run --rm --entrypoint tesseract "$PAPERRUST_IMAGE" --list-langs)"
[[ $'\n'"$langs"$'\n' == *$'\n'deu$'\n'* ]] || die "tesseract deu fehlt ($langs)"

dc up -d
log "warte auf healthy"
cid="$(dc ps -q paperrust)"
for _ in $(seq 1 90); do
	st="$(docker inspect -f '{{.State.Health.Status}}' "$cid" 2>/dev/null || echo none)"
	[[ "$st" == healthy ]] && break
	[[ "$(docker inspect -f '{{.State.Running}}' "$cid")" == true ]] || die "Container beendet"
	sleep 2
done
[[ "$st" == healthy ]] || die "nicht healthy ($st)"

base="http://127.0.0.1:${PORT}"
code="$(curl -s -o /dev/null -w '%{http_code}' "$base/accounts/login/")"
[[ "$code" == 200 ]] || die "Login-Seite: HTTP $code"
code="$(curl -s -o /dev/null -w '%{http_code}' "$base/__rs/health")"
[[ "$code" != 200 ]] || die "/__rs/health ist von außen erreichbar (sollte nur lokal antworten)"

token="$(curl -sf -X POST -d 'username=admin&password=smoke-admin' "$base/api/token/" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')"
[[ -n "$token" ]] || die "kein API-Token für admin"
auth=(-H "Authorization: Token $token")
code="$(curl -s -o /dev/null -w '%{http_code}' "${auth[@]}" "$base/api/documents/")"
[[ "$code" == 200 ]] || die "/api/documents/: HTTP $code"

log "Hochladen + Einlesen (Worker im selben Prozess)"
printf 'PaperRust smoke test %s\nRechnung 2026\n' "$(date +%s)" >"$WORK/smoke.txt"
task="$(curl -sf "${auth[@]}" -F "document=@$WORK/smoke.txt" -F title=smoke "$base/api/documents/post_document/" | tr -d '"')"
[[ -n "$task" ]] || die "post_document ohne Task-ID"
for _ in $(seq 1 60); do
	n="$(curl -sf "${auth[@]}" "$base/api/documents/?title__icontains=smoke" | sed -n 's/^{"count":\([0-9]*\).*/\1/p')"
	[[ "${n:-0}" -ge 1 ]] && break
	sleep 2
done
[[ "${n:-0}" -ge 1 ]] || die "Dokument nach 120 s nicht eingelesen (Task $task)"

log "Verwaltungsbefehl im Container"
dc exec -T paperrust document_exporter ../export >/dev/null || die "document_exporter fehlgeschlagen"
[[ -f "$WORK/export/manifest.json" ]] || die "Export ohne manifest.json"

log "geordneter Stopp"
t0=$SECONDS
dc stop -t 45 paperrust
code="$(docker inspect -f '{{.State.ExitCode}}' "$cid")"
log "Stopp nach $((SECONDS - t0)) s, Exit-Code $code"
[[ "$code" == 0 || "$code" == 143 ]] || die "Exit-Code $code beim Stoppen"
log "OK"
