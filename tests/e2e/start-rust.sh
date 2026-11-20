#!/usr/bin/env bash
# B: PaperRust on :8171 (standalone, no forwarding to Django :8170), same DB pl_e2e.
# start-rust.sh [off|on]   (branding; default off = comparison run)
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BR="${1:-off}"
pidf="$RUN/rust.pid"
if pid_alive "$pidf" paperrust; then echo "PaperRust-E2E läuft bereits (PID $(cat "$pidf"))"; exit 0; fi
# routes.toml: all routes that are already "rust" in the main routes.toml, + upload (if E2E_UPLOAD_RUST=1)
{ echo "# generiert von tests/e2e/start-rust.sh aus routes.toml – nicht von Hand ändern"; echo "[routes]"
  grep -E '^"[^"]+"[[:space:]]*=[[:space:]]*"rust"' "$REPO/routes.toml"
  if [[ "${E2E_UPLOAD_RUST:-1}" == 1 ]] && ! grep -q '^"documents.post_document"' "$REPO/routes.toml"; then
    echo '"documents.post_document" = "rust"'; fi
} > "$WORK/routes.toml"
export PAPERRUST_BIND=127.0.0.1:$E2E_B_PORT
export PAPERRUST_ROUTES="$(w "$WORK/routes.toml")"
export PAPERRUST_QUEUE_SHUTDOWN_GRACE=5 PAPERLESS_CONSUMER_STABILITY_DELAY=1
export PAPERRUST_PDFIUM_PATH="$(w "$REPO/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")"
export PAPERRUST_TESSERACT_PATH="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe")"
export PAPERRUST_TESSDATA_DIR="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/tessdata")"
export PAPERRUST_BRANDING=$BR
export PAPERRUST_LOG="info,paperrust::access=info"
# Since 2026-11-17 the server no longer starts on a DB with open migrations → migrate beforehand
# (test copy, hence without pg_dump backup).
PAPERRUST_DATAMODEL_BACKUP=off "$PAPERRUST_EXE" manage migrate --no-input >"$LOGS/rust-migrate.log" 2>&1 \
  || { echo "manage migrate fehlgeschlagen (siehe $LOGS/rust-migrate.log)"; tail -5 "$LOGS/rust-migrate.log"; exit 1; }
detach "$pidf" "$PAPERRUST_EXE" "$LOGS/rust-$BR.out.log" "$LOGS/rust-$BR.err.log" serve --worker
wait_http "http://127.0.0.1:$E2E_B_PORT/api/" 60
echo "PaperRust-E2E ($BR): PID $(cat "$pidf"), http://127.0.0.1:$E2E_B_PORT"
