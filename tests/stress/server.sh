#!/usr/bin/env bash
# Throwaway test server for the load/concurrency suite (tests/stress).
# Thin wrapper around frontend-light/tests/server.sh with own defaults:
#   Port 58360, DB pl_stress_a (fresh from TEMPLATE paperless_snapshot), working folder under %TEMP%,
#   worker switched on ("serve --worker"), routes.toml of the repo.
#   The binary is copied to $ST_WORK/bin before the start: a running .exe locks the file under Windows,
#   otherwise other agents could not rebuild target/debug/paperrust.exe.
# Invocation: tests/stress/server.sh [start|stop|restart|reset|drop|kill|env]
#   reset = DB + data folder anew (server must be stopped), drop = stop + delete DB + working folder,
#   kill  = hard abort (taskkill /F, for scenario 5), env = print variables for stress.py.
# Environment: ST_PORT, ST_DB, ST_WORK, ST_BIN (source of the binary, default target/debug/paperrust.exe).
# Local test values only (admin/admin), never against the DBs paperless, paperless_snapshot, pl_scale_1m.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TMPW="$(cygpath -u "${TEMP:-/tmp}")"
export ST_PORT="${ST_PORT:-58360}"
export ST_DB="${ST_DB:-pl_stress_a}"
export ST_WORK="${ST_WORK:-$TMPW/paperrust-stress}"
SRC_BIN="${ST_BIN:-$REPO/target/debug/paperrust.exe}"
case "$ST_DB" in paperless|paperless_snapshot|pl_scale_1m) echo "[stress] DB $ST_DB ist tabu"; exit 2 ;; esac
case "$ST_DB" in pl_stress_*) ;; *) echo "[stress] DB-Name muss mit pl_stress_ beginnen"; exit 2 ;; esac

export F0_PORT="$ST_PORT" F0_DB="$ST_DB" F0_WORK="$ST_WORK"
export F0_BIN="$ST_WORK/bin/paperrust.exe"
export F0_ROUTES="${ST_ROUTES:-$REPO/routes.toml}"
export F0_ARGS="${ST_ARGS:-serve --worker}"
F0="$REPO/frontend-light/tests/server.sh"
export PAPERLESS_TASK_WORKERS="${PAPERLESS_TASK_WORKERS:-${ST_WORKERS:-2}}"
# PDF/OCR components (without PDFium every PDF processing fails with "PDFium not loaded").
# Default: the local desktop installation; overridable via ST_TOOLS or the PAPERRUST_* variables.
TOOLS="${ST_TOOLS:-$(cygpath -u "${LOCALAPPDATA:-C:/Users/$USERNAME/AppData/Local}")/Programs/PaperRust}"
if [[ -z "${PAPERRUST_PDFIUM_PATH:-}" && -f "$TOOLS/pdfium/pdfium.dll" ]]; then
  export PAPERRUST_PDFIUM_PATH="$(cygpath -w "$TOOLS/pdfium/pdfium.dll")"
fi
if [[ -z "${PAPERRUST_TESSERACT_PATH:-}" && -f "$TOOLS/tesseract/tesseract.exe" ]]; then
  export PAPERRUST_TESSERACT_PATH="$(cygpath -w "$TOOLS/tesseract/tesseract.exe")"
  export PAPERRUST_TESSDATA_DIR="$(cygpath -w "$TOOLS/tesseract/tessdata")"
fi

copy_bin() {
  mkdir -p "$ST_WORK/bin"
  if [[ ! -f "$F0_BIN" || "$SRC_BIN" -nt "$F0_BIN" ]]; then cp -f "$SRC_BIN" "$F0_BIN" || exit 1; fi
}

pid() { cat "$ST_WORK/run/rust.pid" 2>/dev/null; }

case "${1:-start}" in
  start) copy_bin; [[ -d "$ST_WORK/data" ]] || bash "$F0" reset; bash "$F0" start ;;
  stop) bash "$F0" stop ;;
  kill) p=$(pid); [[ -n "$p" ]] && taskkill //PID "$p" //T //F >/dev/null 2>&1 && echo "[stress] hart beendet (PID $p)"; rm -f "$ST_WORK/run/rust.pid" ;;
  restart) bash "$F0" stop; copy_bin; bash "$F0" start ;;
  reset) bash "$F0" stop; bash "$F0" reset ;;
  drop) bash "$F0" drop; rm -rf "$ST_WORK"; echo "[stress] $ST_WORK gelöscht" ;;
  env)
    source "$REPO/reference/env.sh" >/dev/null 2>&1
    echo "ST_URL=http://127.0.0.1:$ST_PORT"
    echo "ST_PID=$(pid)"
    echo "ST_MEDIA=$(cygpath -w "$ST_WORK/data/media")"
    echo "ST_CONSUME=$(cygpath -w "$ST_WORK/data/consume")"
    echo "ST_LOG=$(cygpath -w "$ST_WORK/logs")"
    echo "ST_PSQL=$(cygpath -w "$PG_BIN/psql.exe")"
    echo "ST_DSN=host=127.0.0.1 port=$PGPORT user=$PGUSER password=$PGPASSWORD dbname=$ST_DB"
    ;;
  *) echo "usage: server.sh [start|stop|restart|reset|drop|kill|env]"; exit 2 ;;
esac
