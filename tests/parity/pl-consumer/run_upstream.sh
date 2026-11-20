#!/usr/bin/env bash
# upstream run of the pl-consumer parity: own DB pl_consumer_test_up, own temp data directory.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../../../reference/env.sh"
ROOT="$(mktemp -d)"
export PL_CONSUMER_UP_ROOT="$(cygpath -w "$ROOT")"
export PAPERLESS_DBNAME="${PL_CONSUMER_DB:-pl_consumer_test}_up"
export PAPERLESS_DATA_DIR="$(cygpath -w "$ROOT/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$ROOT/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$ROOT/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$ROOT/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$ROOT/log")"
export PAPERLESS_REDIS="redis://127.0.0.1:1"
export PAPERLESS_THUMBNAIL_FONT_NAME="C:/Windows/Fonts/arial.ttf"
export PYTHONPATH="$(cygpath -w "$HERE/overlay");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pl_consumer_settings
mkdir -p "$ROOT/data" "$ROOT/media" "$ROOT/consume" "$ROOT/scratch" "$ROOT/log"
"$PY" "$(cygpath -w "$HERE/run_upstream.py")"
echo "upstream root: $ROOT"
