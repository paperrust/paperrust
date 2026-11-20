#!/usr/bin/env bash
# upstream run of the mail worker parity (own DB pl_w6_test_up, own temp directory).
# Called by crates/pl-mail/tests/worker_parity.rs; PL_W6_OUT must be set.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/../../../reference/env.sh"
ROOT="$(mktemp -d)"
export PAPERLESS_DBNAME="${PL_W6_UP_DB:-pl_w6_test_up}"
case "$PAPERLESS_DBNAME" in pl_w6_test*) ;; *) echo "refusing $PAPERLESS_DBNAME" >&2; exit 1;; esac
export PAPERLESS_DATA_DIR="$(cygpath -w "$ROOT/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$ROOT/media")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$ROOT/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$ROOT/log")"
export PAPERLESS_REDIS="redis://127.0.0.1:1"
export PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS=true
export PYTHONPATH="$(cygpath -w "$HERE/overlay");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pl_w6_settings
mkdir -p "$ROOT/data" "$ROOT/media" "$ROOT/scratch" "$ROOT/log"
"$PY" "$(cygpath -w "$HERE/run_upstream.py")"
rm -rf "$ROOT"
