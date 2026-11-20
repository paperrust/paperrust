#!/usr/bin/env bash
# W2 write parity: own reference copy (never the shared reference on :8000 / DB paperless).
#   DB        pl_w2_ref   (from own template pl_w2_snapshot <- paperless_snapshot)
#   Data      tests/parity/.w2/data/{data,media,consume}   (from reference/snapshot/data.tar)
#   Redis     shared Garnet, but prefix "w2" (reset deletes only w2* keys)
#   Django    127.0.0.1:8100 (upstream unchanged, reference/venv + reference/overlay)
#   PaperRust 127.0.0.1:8101 (binary copy tests/parity/.w2/bin) -> proxy to :8100,
#             routes from tests/parity/.w2/routes.toml (W2_ROUTES overrides)
# Sourced by w2-start.sh / w2-stop.sh / w2-reset.sh.
#
# Further, independent copies (e.g. in parallel to a running W2): W2_NAME=<name> and own
# ports, e.g. `W2_NAME=w2sync W2_DJ_PORT=8110 W2_RS_PORT=8111` → DB pl_w2sync_ref, template
# pl_w2sync_snapshot, directory tests/parity/.w2sync, Redis prefix w2sync.
#
# PARITY_SYNC_CAPTURE=on (ARCHITECTURE-V2 §4.3): reset from a template with applied
# sync change capture (<template>_sync = template + crates/pl-sync/migrations/0001_paperrust_sync.sql),
# i.e. Django (A) and PaperRust (B) write with active triggers. Default off.
W2_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$W2_REPO/reference/env.sh"
export W2_NAME="${W2_NAME:-w2}"
export W2_DIR="$W2_REPO/tests/parity/.$W2_NAME"
export W2_DB="${W2_DB:-pl_${W2_NAME}_ref}"
export W2_TPL="${W2_TPL:-pl_${W2_NAME}_snapshot}"
export W2_DJ_PORT="${W2_DJ_PORT:-8100}"
export W2_RS_PORT="${W2_RS_PORT:-8101}"
export PARITY_SYNC_CAPTURE="${PARITY_SYNC_CAPTURE:-off}"

export PAPERLESS_DBNAME="$W2_DB"
export PGDATABASE="$W2_DB"
export PAPERLESS_REDIS_PREFIX="$W2_NAME"
export PAPERLESS_DATA_DIR="$(cygpath -w "$W2_DIR/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W2_DIR/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W2_DIR/data/consume")"
# use the collectstatic result of the reference read-only (frontend delivery)
export PAPERLESS_STATICDIR="$(cygpath -w "$REF_DIR/data/static")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W2_DIR/logs")"
export PAPERLESS_URL="http://127.0.0.1:$W2_DJ_PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$W2_DJ_PORT,http://127.0.0.1:$W2_RS_PORT,http://localhost:$W2_DJ_PORT"

export PAPERRUST_BIND="127.0.0.1:$W2_RS_PORT"
export PAPERRUST_ROUTES="$(cygpath -w "${W2_ROUTES:-$W2_DIR/routes.toml}")"
export PAPERRUST_LOG="${PAPERRUST_LOG:-info}"
export PAPERRUST_BRANDING=off
export NO_COLOR=1

# Harness settings for a W2 run
export PARITY_A_URL="http://127.0.0.1:$W2_DJ_PORT"
export PARITY_B_URL="http://127.0.0.1:$W2_RS_PORT"
export PARITY_RESET_CMD="python -I $(cygpath -w "$W2_REPO/tests/parity/w2_reset.py")"
export PARITY_SQL_DB="$W2_DB"
