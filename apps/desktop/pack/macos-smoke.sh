#!/usr/bin/env bash
# PaperRust Desktop (macOS): headless smoke test of a built package – the console supervisor
# `paperrust-desktop run --no-browser` with a fresh data folder: first start (initdb, migrations, admin
# from PAPERRUST_DESKTOP_ADMIN_*), login page, API token, upload of an image with OCR (bundled Tesseract +
# PDFium), orderly `paperrust-desktop stop`, no processes left. The window (paperrust-app) is not
# started (no display session on CI).
#
#   apps/desktop/pack/macos-smoke.sh <dir with paperrust-desktop>
#       e.g. results/desktop/PaperRust.app/Contents/Resources or results/desktop/PaperRust-<v>-macos-aarch64
#
# Uses only 127.0.0.1, a temporary data folder and a random throw-away password; bash 3.2 compatible.
set -euo pipefail
[[ "${PAPERRUST_PACK_TRACE:-}" == 1 ]] && set -x

DIR="${1:?usage: macos-smoke.sh <dir with paperrust-desktop>}"
DIR="$(cd "$DIR" && pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DESK="$DIR/paperrust-desktop"
[[ -x "$DESK" ]] || { echo "ERROR: $DESK missing" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/paperrust-smoke.XXXXXX")"
export PAPERRUST_DESKTOP_HOME="$WORK/home"
export PAPERRUST_DESKTOP_ADMIN_USER=smoke
export PAPERRUST_DESKTOP_ADMIN_EMAIL=smoke@example.invalid
PAPERRUST_DESKTOP_ADMIN_PASSWORD="smoke-$(openssl rand -hex 12)"
export PAPERRUST_DESKTOP_ADMIN_PASSWORD
export NO_COLOR=1
LOG="$WORK/run.log"
PID=""

log() { printf '\033[36m== %s\033[0m\n' "$*"; }
dump() {
	echo "---- run.log (tail)"; tail -n 60 "$LOG" 2>/dev/null || true
	for f in "$PAPERRUST_DESKTOP_HOME"/logs/*.log; do
		[[ -f "$f" ]] || continue
		echo "---- $f (tail)"; tail -n 80 "$f"
	done
}
die() {
	printf 'ERROR: %s\n' "$*" >&2
	dump >&2
	exit 1
}
cleanup() {
	if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
		"$DESK" stop >/dev/null 2>&1 || kill "$PID" 2>/dev/null || true
	fi
	# never leave a postgres of this data folder behind
	pkill -f "$PAPERRUST_DESKTOP_HOME" 2>/dev/null || true
	rm -rf "$WORK"
}
trap cleanup EXIT

log "paperrust-desktop --version / paths"
"$DESK" --version
"$DESK" paths

log "first start: paperrust-desktop run --no-browser (data in $PAPERRUST_DESKTOP_HOME)"
t0=$SECONDS
"$DESK" run --no-browser </dev/null >"$LOG" 2>&1 &
PID=$!
url=""
for _ in $(seq 1 240); do
	url="$(sed -n 's/.*PaperRust läuft: \(http[^ ]*\).*/\1/p' "$LOG" | sed -n 1p)"
	[[ -n "$url" ]] && break
	kill -0 "$PID" 2>/dev/null || die "paperrust-desktop ended before it was ready"
	sleep 1
done
[[ -n "$url" ]] || die "not ready after 240 s"
base="${url%/}"
log "ready after $((SECONDS - t0)) s: $base"

code="$(curl -s -o /dev/null -w '%{http_code}' "$base/accounts/login/")"
[[ "$code" == 200 ]] || die "login page: HTTP $code"
echo "    /accounts/login/ 200"
echo "    /__rs/health $(curl -s -o /dev/null -w '%{http_code}' "$base/__rs/health")"

token="$(curl -sf -X POST --data-urlencode "username=$PAPERRUST_DESKTOP_ADMIN_USER" \
	--data-urlencode "password=$PAPERRUST_DESKTOP_ADMIN_PASSWORD" "$base/api/token/" |
	python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("token",""))' || true)"
[[ -n "$token" ]] || die "no API token for the admin"
auth="Authorization: Token $token"
code="$(curl -s -o /dev/null -w '%{http_code}' -H "$auth" "$base/api/documents/")"
[[ "$code" == 200 ]] || die "/api/documents/: HTTP $code"
echo "    /api/documents/ 200"

sample="$REPO/upstream/src/documents/tests/samples/simple.png"
if [[ -f "$sample" ]]; then
	log "upload + OCR (simple.png: \"This is a test document.\")"
	cp "$sample" "$WORK/smoke-ocr.png"
	task="$(curl -sf -H "$auth" -F "document=@$WORK/smoke-ocr.png" -F title=smoke-ocr "$base/api/documents/post_document/" | tr -d '"' || true)"
	[[ -n "$task" ]] || die "post_document without task id"
	content=""
	for _ in $(seq 1 90); do
		content="$(curl -sf -H "$auth" "$base/api/documents/?title__icontains=smoke-ocr" |
			python3 -I -c 'import json,sys
r = json.load(sys.stdin).get("results") or []
print((r[0].get("content") or "").replace("\n", " ") if r else "")' || true)"
		[[ -n "$content" ]] && break
		sleep 2
	done
	[[ -n "$content" ]] || die "document not consumed after 180 s (task $task)"
	echo "    content: $content"
	case "$(printf '%s' "$content" | tr '[:upper:]' '[:lower:]')" in
	*test*) ;;
	*) die "OCR text does not contain 'test'" ;;
	esac
	echo "    task: $(curl -sf -H "$auth" "$base/api/tasks/?task_id=$task" | head -c 400 || true)"
else
	echo "    (upstream submodule missing – OCR upload skipped)"
fi

log "orderly stop (paperrust-desktop stop)"
t0=$SECONDS
"$DESK" stop || die "paperrust-desktop stop failed"
for _ in $(seq 1 60); do
	kill -0 "$PID" 2>/dev/null || break
	sleep 1
done
if kill -0 "$PID" 2>/dev/null; then die "paperrust-desktop still running 60 s after stop"; fi
wait "$PID" || die "paperrust-desktop exit code $?"
PID=""
left="$(pgrep -fl "$PAPERRUST_DESKTOP_HOME" || true)"
[[ -z "$left" ]] || die "processes left after stop: $left"
log "OK (stop after $((SECONDS - t0)) s)"
