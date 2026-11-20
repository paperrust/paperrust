#!/usr/bin/env bash
# Starts PaperRust on 127.0.0.1:8130 with embedded worker against pl_w10_test, AI only against the
# local mock (tests/parity/pl-ai/mock_llm.py on 127.0.0.1:8131, started by run_mock.sh).
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
W=$(cd "$(dirname "$0")" && pwd); E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-w10-e2e}"
w() { cygpath -w "$1"; }
export PAPERLESS_DBENGINE=postgresql PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=54329
export PAPERLESS_DBNAME=pl_w10_test PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
export PAPERLESS_SECRET_KEY="paperless-rs-reference-secret-key-local-only-0123456789"
export PAPERLESS_TIME_ZONE=Europe/Zurich PAPERLESS_OCR_LANGUAGE=eng
export PAPERLESS_DATA_DIR="$(w "$E2E/data")" PAPERLESS_MEDIA_ROOT="$(w "$E2E/media")"
export PAPERLESS_CONSUMPTION_DIR="$(w "$E2E/consume")" PAPERLESS_SCRATCH_DIR="$(w "$E2E/scratch")"
export PAPERLESS_LOGGING_DIR="$(w "$E2E/logs")"
export PAPERLESS_URL="http://127.0.0.1:8130" PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8130"
export PAPERLESS_CONSUMER_STABILITY_DELAY=1
export PAPERRUST_QUEUE_SHUTDOWN_GRACE=5
export PAPERRUST_BIND=127.0.0.1:8130
export PAPERRUST_ROUTES="$(w "$W/routes.toml")"
export PAPERRUST_PDFIUM_PATH="$(w "$REPO/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")"
export PAPERRUST_TESSERACT_PATH="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe")"
export PAPERRUST_TESSDATA_DIR="$(w "$REPO/tests/parity/pl-ocr/.tools/tesseract/tessdata")"
export PAPERRUST_BRANDING=off
# AI: local mock only
export PAPERLESS_AI_ENABLED=true
export PAPERLESS_AI_LLM_BACKEND=ollama PAPERLESS_AI_LLM_MODEL=llama3.1
export PAPERLESS_AI_LLM_ENDPOINT=http://127.0.0.1:8131
export PAPERLESS_AI_LLM_EMBEDDING_BACKEND=ollama PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT=http://127.0.0.1:8131
export PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE=256 PAPERLESS_AI_LLM_CONTEXT_SIZE=4096
export PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS=true
# Extension: automation "suggest" after ingestion + tag trigger every minute
export PAPERRUST_AI_AUTO_MODE=suggest
export PAPERRUST_AI_AUTO_TRIGGER_TAG=w10-ai-trigger
export PAPERRUST_AI_TRIGGER_TAG_CRON="* * * * *"
export PAPERRUST_LOG="info,paperrust::access=info,pl_ai=debug"
EXE="${PAPERRUST_EXE:-$REPO/target/debug/paperrust.exe}"
# serve no longer starts on a DB without PaperRust migrations (test log 2026-11-16 B-312)
PAPERRUST_DATAMODEL_BACKUP=off "$EXE" manage migrate --no-input || exit 1
exec "$EXE" serve --worker
