#!/usr/bin/env bash
# W23: standalone E2E without Django AND without Redis (browser, all click paths of tests/e2e/e2e.mjs).
#   DB        pl_w23_e2e   (fresh from TEMPLATE paperless_snapshot)
#   Data      target/w23e2e/{data,media,consume,scratch,logs}  (from reference/snapshot/data.tar)
#   PaperRust 127.0.0.1:8192, serve --worker (since 2026-11-14 without Django/Celery/Redis code)
#   Cache     PAPERLESS_CACHE_BACKEND NOT set -> local cache (pl_core::cache).
# Reference DB `paperless`, ports 8000/8100/8101 and other agent instances stay untouched.
#   w23-e2e.sh [run|start|stop]   (default run = reset + start + click paths + stop)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../../reference/env.sh"
W="$REPO_DIR/target/w23e2e"
OUT="${W23_OUT:-$REPO_DIR/results/e2e/standalone-w23}"
[[ "$OUT" == /* ]] || OUT="$REPO_DIR/$OUT"
PORT=8192
BIN="${W23_BIN:-$REPO_DIR/results/bin/paperrust-w23.exe}"
PIDF="$W/run/rust.pid"
mkdir -p "$W/run" "$W/logs"

export PAPERLESS_DBNAME=pl_w23_e2e
export PAPERRUST_BIND="127.0.0.1:$PORT"
export PAPERLESS_URL="http://127.0.0.1:$PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$PORT,http://localhost:$PORT"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
R="${W23_ROUTES:-$REPO_DIR/results/parity/w23/routes-w23.toml}"; [[ "$R" == /* ]] || R="$REPO_DIR/$R"
export PAPERRUST_ROUTES="$(cygpath -w "$R")"
export PAPERRUST_QUEUE_SHUTDOWN_GRACE=5 PAPERLESS_CONSUMER_STABILITY_DELAY=1
unset PAPERLESS_CACHE_BACKEND
export PAPERLESS_DATA_DIR="$(cygpath -w "$W/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W/logs")"
export PAPERLESS_STATICDIR="$(cygpath -w "$REF_DIR/data/static")"
export PAPERRUST_PDFIUM_PATH="$(cygpath -w "$REF_DIR/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")"
export PAPERRUST_TESSERACT_PATH="$(cygpath -w "$REPO_DIR/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe")"
export PAPERRUST_TESSDATA_DIR="$(cygpath -w "$REPO_DIR/tests/parity/pl-ocr/.tools/tesseract/tessdata")"
export PAPERRUST_BRANDING=off
export PAPERRUST_LOG="info,paperrust::access=info"
export NO_COLOR=1

alive() { [[ -f "$PIDF" ]] && tasklist //FI "PID eq $(cat "$PIDF")" 2>/dev/null | grep -qi paperrust; }

reset() {
  env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -q \
    -c "DROP DATABASE IF EXISTS pl_w23_e2e WITH (FORCE)" \
    -c "CREATE DATABASE pl_w23_e2e TEMPLATE paperless_snapshot OWNER paperless" || return 1
  rm -rf "$W/data"; mkdir -p "$W/data/consume" "$W/data/scratch"
  tar xf "$REPO_DIR/reference/snapshot/data.tar" -C "$W/data"
  echo "[w23] pl_w23_e2e + $W/data zurückgesetzt"
}

start() {
  if alive; then echo "[w23] läuft bereits (PID $(cat "$PIDF"))"; return 0; fi
  detach "$PIDF" "$BIN" "$W/logs/rust.out.log" "$W/logs/rust.err.log" serve --worker
  for _ in $(seq 1 90); do
    c=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/" || true)
    [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "[w23] PaperRust :$PORT PID $(cat "$PIDF")"; return 0; }
    sleep 1
  done
  echo "[w23] Start fehlgeschlagen"; return 1
}

stop() {
  if alive; then taskkill //PID "$(cat "$PIDF")" //T //F >/dev/null 2>&1 && echo "[w23] gestoppt (PID $(cat "$PIDF"))"; fi
  rm -f "$PIDF"
}

case "${1:-run}" in
  start) start ;;
  stop) stop ;;
  reset) reset ;;
  run)
    stop; reset || exit 1; start || exit 1
    cd "$REPO_DIR/tests/e2e"
    export PLAYWRIGHT_BROWSERS_PATH="$REPO_DIR/tests/e2e/.browsers"
    rm -rf "$OUT"; mkdir -p "$OUT"
    node e2e.mjs --side standalone-w23 --base "http://127.0.0.1:$PORT" --out "$OUT" --upload 1 | tee "$OUT/run.log"
    stop
    cp "$W/logs/rust.out.log" "$OUT/rust-access.log" 2>/dev/null
    python -I "$HERE/w23_e2e_summary.py" "$OUT" | tee "$OUT/summary.md"
    ;;
esac
