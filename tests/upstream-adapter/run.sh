#!/usr/bin/env bash
# Upstream-Test-Adapter: runs upstream's DRF API tests (src/*/tests/test_api_*.py) in-process
# (baseline) and/or over HTTP against a live server, then compares the JUnit results.
#
#   tests/upstream-adapter/run.sh baseline [pytest args]   # in-process Django (reference)
#   tests/upstream-adapter/run.sh http     [pytest args]   # requests -> $UPSTREAM_ADAPTER_TARGET
#   tests/upstream-adapter/run.sh compare                   # diff of the two latest runs
#   tests/upstream-adapter/run.sh selftest                  # adapter mechanics (toy app, no paperless)
#
# Prerequisites (see docs/contract/testing.md, "Upstream-Test-Adapter"):
#   * Python with upstream's runtime + test dependencies (pytest-django, pytest-env, pytest-mock,
#     pytest-httpx, factory-boy, faker, ...). Default: reference venv ($PY from reference/env.sh).
#   * PostgreSQL from reference/env.sh. pytest-django creates/keeps the test DB "test_<PAPERLESS_DBNAME>"
#     (--reuse-db); the live server must point to THAT database and use SECRET_KEY
#     "test-secret-key-do-not-use-in-production" (upstream [tool.pytest_env]).
#   * http mode: the target (PaperRust, routes not on rust are proxied to a Django that must
#     also use the test DB) is started by the reference agent/coordinator, not by this script.
# Exit: pytest's exit code; 2 if prerequisites are missing.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MODE="${1:-}"; shift || true
OUT="$REPO/results/upstream-adapter"
case "${OSTYPE:-}" in msys*|cygwin*|win*) SEP=";" ;; *) SEP=":" ;; esac
np() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf "%s" "$1"; fi; }  # native path for Python
export PYTHONDONTWRITEBYTECODE=1  # never write __pycache__ into upstream/
mkdir -p "$OUT"

if [[ "$MODE" == "selftest" ]]; then
  VPY="${ADAPTER_PY:-$REPO/tests/.venv/Scripts/python.exe}"
  [[ -x "$VPY" ]] || VPY="$REPO/tests/.venv/bin/python"
  [[ -x "$VPY" ]] || { echo "tests/.venv missing (django, djangorestframework, pytest)"; exit 2; }
  cd "$HERE/selftest" && PYTHONPATH="$(np "$HERE")$SEP$(np "$HERE/selftest")" "$VPY" -m pytest -p paperless_http_adapter -q -p no:cacheprovider "$@"
  exit $?
fi

if [[ "$MODE" == "compare" ]]; then
  B="$(ls -t "$OUT"/baseline-*.xml 2>/dev/null | head -1)"; H="$(ls -t "$OUT"/http-*.xml 2>/dev/null | head -1)"
  [[ -n "$B" && -n "$H" ]] || { echo "need baseline-*.xml and http-*.xml in $OUT"; exit 2; }
  exec python "$HERE/compare_junit.py" "$B" "$H" --out "$OUT"
fi

[[ "$MODE" == "baseline" || "$MODE" == "http" ]] || { sed -n '2,20p' "$0"; exit 2; }

if [[ -f "$REPO/reference/env.sh" ]]; then
  # shellcheck disable=SC1091
  source "$REPO/reference/env.sh"
fi
PYBIN="${ADAPTER_PY:-${PY:-python}}"
"$PYBIN" -c "import pytest_django, pytest_env, django, rest_framework" 2>/dev/null || {
  echo "missing test dependencies in $PYBIN (pytest-django, pytest-env, ...); see docs/contract/testing.md"; exit 2; }

# upstream's own settings, not the reference overlay; PostgreSQL from env.sh stays
export DJANGO_SETTINGS_MODULE=paperless.settings
export PYTHONPATH="$(np "$HERE")$SEP$(np "$REPO/upstream/src")"
STAMP="$(date +%Y-%m-%d_%H%M%S)"
TESTS=( "$REPO"/upstream/src/*/tests/test_api_*.py )
ARGS=( -p paperless_http_adapter -o addopts= -p no:cacheprovider -p no:randomly --ds=paperless.settings --reuse-db
       --junitxml="$(np "$OUT/$MODE-$STAMP.xml")" -o junit_family=legacy -q )

if [[ "$MODE" == "http" ]]; then
  : "${UPSTREAM_ADAPTER_TARGET:=http://127.0.0.1:8080}"
  export UPSTREAM_ADAPTER_TARGET UPSTREAM_ADAPTER_LOG="$(np "$OUT/http-$STAMP.requests.jsonl")"
  ARGS+=( -p no:xdist )
else
  unset UPSTREAM_ADAPTER_TARGET
fi

cd "$REPO/upstream" || exit 2
"$PYBIN" -m pytest "${ARGS[@]}" "$@" "${TESTS[@]}"
RC=$?
echo "junit: $OUT/$MODE-$STAMP.xml"
exit $RC
