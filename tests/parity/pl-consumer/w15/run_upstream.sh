#!/usr/bin/env bash
# upstream run of the W15 parity: DB pl_w15_ref, own temp data directory.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../../../../reference/env.sh"
ROOT="$(mktemp -d)"
export PL_W15_UP_ROOT="$(cygpath -w "$ROOT")"
export PAPERLESS_DBNAME=pl_w15_ref
export PAPERLESS_DATA_DIR="$(cygpath -w "$ROOT/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$ROOT/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$ROOT/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$ROOT/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$ROOT/log")"
export PAPERLESS_REDIS="redis://127.0.0.1:1"
export PAPERLESS_THUMBNAIL_FONT_NAME="C:/Windows/Fonts/arial.ttf"
export PYTHONPATH="$(cygpath -w "$HERE/../overlay");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pl_consumer_settings
mkdir -p "$ROOT/data" "$ROOT/media/documents/originals" "$ROOT/media/documents/archive" "$ROOT/media/documents/thumbnails" "$ROOT/consume" "$ROOT/scratch" "$ROOT/log"
"$PY" "$(cygpath -w "$HERE/run_upstream.py")"
echo "upstream root: $ROOT"
