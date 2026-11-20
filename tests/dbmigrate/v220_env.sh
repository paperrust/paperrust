# Environment for upstream paperless-ngx v2.20.15 with the reference venv (test fixture only).
# Expected: reference/env.sh sourced, V220_SRC = unpacked `git archive v2.20.15 src`, WORK.
export PYTHONPATH="$(cygpath -w "$V220_SRC/src");$(cygpath -w "$HERE/stubs")"
export DJANGO_SETTINGS_MODULE=v220_settings
export PAPERLESS_DATA_DIR="$(cygpath -w "$WORK/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$WORK/media")"
mkdir -p "$WORK/data" "$WORK/media"
export V220_SRC_DIR="$(cygpath -w "$V220_SRC/src")"
