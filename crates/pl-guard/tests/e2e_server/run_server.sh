#!/usr/bin/env bash
# E2E protection against ransomware: PaperRust against throwaway DB pl_guard_e2e (copy of paperless_snapshot),
# copy of the reference media with marker .paperrust-test-media, guard with a short review,
# backup into a local folder. Procedure and result: README.md next to it.
#   E2E_DIR=… PAPERRUST_EXE=… bash run_server.sh [migrate | manage <args…>]
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
WT="${WT:-$REPO/.claude/worktrees/ransomware-guard-788426}"
E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-guard-e2e}"
w() { cygpath -w "$1"; }
export PAPERLESS_DBENGINE=postgresql PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT=54329
export PAPERLESS_DBNAME=pl_guard_e2e PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
export PAPERLESS_SECRET_KEY="paperless-rs-reference-secret-key-local-only-0123456789"
export PAPERLESS_TIME_ZONE=Europe/Zurich PAPERLESS_OCR_LANGUAGE=eng
export PAPERLESS_DATA_DIR="$(w "$E2E/run/data")" PAPERLESS_MEDIA_ROOT="$(w "$E2E/run/media")"
export PAPERLESS_CONSUMPTION_DIR="$(w "$E2E/run/consume")" PAPERLESS_SCRATCH_DIR="$(w "$E2E/run/scratch")"
export PAPERLESS_LOGGING_DIR="$(w "$E2E/run/logs")"
export PAPERLESS_URL="http://127.0.0.1:8141" PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8141"
export PAPERRUST_QUEUE_SHUTDOWN_GRACE=5
export PAPERRUST_BIND=127.0.0.1:8141
export PAPERRUST_ROUTES="$(w "$WT/routes.toml")"
export PAPERRUST_LIGHT_UI_DIR="$(w "$WT/frontend-light/dist")"
export PAPERRUST_BRANDING=off
export PAPERRUST_LOG="info"
# Guard: threshold 10, review every 20 s
export PAPERRUST_GUARD_THRESHOLD=10 PAPERRUST_GUARD_SCAN_INTERVAL=20
# Backup into a folder
export PAPERRUST_BACKUP_TARGET="$(w "$E2E/backup")"
EXE="${PAPERRUST_EXE:-$E2E/bin/paperrust-guard.exe}"
case "${1:-}" in
  migrate) exec "$EXE" manage migrate ;;
  manage) shift; exec "$EXE" manage "$@" ;;
esac
exec "$EXE" serve --worker
