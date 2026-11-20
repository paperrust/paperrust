#!/usr/bin/env bash
# W22: PaperRust without Django (frontend + API from Rust). Own DB pl_w22 (copy of paperless_snapshot),
# own data folder target/w22, port 8084 (parity, without worker) or 8085 (E2E, serve --worker).
# The reference DB `paperless` and ports 8000/8100/8101 stay untouched.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
export W22_WORK="${W22_WORK:-$REPO_DIR/target/w22}"
export PAPERLESS_DBNAME=pl_w22
export W22_PORT="${W22_PORT:-8084}"
export PAPERRUST_BIND="127.0.0.1:$W22_PORT"
export PAPERLESS_URL="http://127.0.0.1:$W22_PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$W22_PORT,http://localhost:$W22_PORT"
export PAPERRUST_ROUTES="$(cygpath -w "$REPO_DIR/results/parity/w22/routes-w22.toml")"
# PAPERLESS_REDIS from reference/env.sh applies only to the reference (PaperRust would only log a hint)
unset PAPERLESS_REDIS
export PAPERLESS_DATA_DIR="$(cygpath -w "$W22_WORK/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W22_WORK/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W22_WORK/consume")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W22_WORK/logs")"
export PAPERLESS_STATICDIR="$(cygpath -w "$REF_DIR/data/static")"
export PAPERRUST_BRANDING="${PAPERRUST_BRANDING:-off}"
export PAPERRUST_LOG="${PAPERRUST_LOG:-info}"
W22_BIN="$REPO_DIR/results/bin/paperrust-w22.exe"
