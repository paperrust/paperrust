#!/usr/bin/env bash
# E2E scan-intake: PaperRust against throwaway DB pl_scan_e2e, SMTP intake on, light UI.
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
WT=$REPO/.claude/worktrees/scan-intake-2bc79d
E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-scan-e2e}"
w() { cygpath -w "$1"; }
export PAPERLESS_DBENGINE=postgresql PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=54329
export PAPERLESS_DBNAME=pl_scan_e2e PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
export PAPERLESS_SECRET_KEY="paperless-rs-reference-secret-key-local-only-0123456789"
export PAPERLESS_TIME_ZONE=Europe/Zurich PAPERLESS_OCR_LANGUAGE=eng
export PAPERLESS_DATA_DIR="$(w "$E2E/run/data")" PAPERLESS_MEDIA_ROOT="$(w "$E2E/run/media")"
export PAPERLESS_CONSUMPTION_DIR="$(w "$E2E/run/consume")" PAPERLESS_SCRATCH_DIR="$(w "$E2E/run/scratch")"
export PAPERLESS_LOGGING_DIR="$(w "$E2E/run/logs")"
export PAPERLESS_URL="http://127.0.0.1:8131" PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8131"
export PAPERRUST_QUEUE_SHUTDOWN_GRACE=5
export PAPERRUST_BIND=127.0.0.1:8131
export PAPERRUST_ROUTES="$(w "$WT/routes.toml")"
export PAPERRUST_LIGHT_UI_DIR="$(w "$WT/frontend-light/dist")"
export PAPERRUST_PDFIUM_PATH="$(w "$REPO/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")"
export PAPERRUST_TESSERACT_PATH="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe")"
export PAPERRUST_TESSDATA_DIR="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/tessdata")"
export PAPERRUST_BRANDING=off
export PAPERRUST_LOG="info,paperrust::access=info"
export PAPERRUST_SMTP_INTAKE=on PAPERRUST_SMTP_INTAKE_LISTEN=127.0.0.1:2531
EXE="${PAPERRUST_EXE:-$E2E/bin/paperrust-scan.exe}"
if [ "${1:-}" = migrate ]; then exec "$EXE" manage migrate; fi
exec "$EXE" serve --worker
