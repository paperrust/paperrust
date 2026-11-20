#!/usr/bin/env bash
# Shared environment of the reference environment (sourced by all reference/*.sh).
# Local test values only – never use in production.
REF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$REF_DIR/.." && pwd)"
export REF_DIR REPO_DIR

# --- PostgreSQL -------------------------------------------------------------
export PG_BIN="$REF_DIR/pg/pgsql/bin"
export PGDATA="$REF_DIR/pg/cluster"
export PGHOST=127.0.0.1
export PGPORT=54329
export PGUSER=paperless
export PGPASSWORD=paperless
export PGDATABASE=paperless
export PG_SUPERUSER=postgres
export PG_SUPERPASS=postgres-local-only

# --- Redis-kompatibler Server (Microsoft Garnet) ----------------------------
export REDIS_PORT=63799
export GARNET_DIR="$REF_DIR/bin/garnet"

# --- paperless-ngx (upstream v3.4.2, unchanged) ---------------------------
export PY="$REF_DIR/venv/Scripts/python.exe"
# libmagic (for python-magic): native UCRT64 DLL from the official MSYS2 repo, at the front of PATH,
# so that Git Bash msys-magic-1.dll is not loaded by accident. MAGIC = signature database.
export PATH="$REF_DIR/bin/libmagic:$PATH"
export MAGIC="$(cygpath -w "$REF_DIR/bin/libmagic/magic.mgc")"
export PYTHONPATH="$(cygpath -w "$REF_DIR/overlay");$(cygpath -w "$REPO_DIR/upstream/src")"
export PYTHONUTF8=1
export PYTHONIOENCODING=utf-8
export DJANGO_SETTINGS_MODULE=reference_settings
export PAPERLESS_DBENGINE=postgresql
export PAPERLESS_DBHOST=127.0.0.1
export PAPERLESS_DBPORT=54329
export PAPERLESS_DBNAME=paperless
export PAPERLESS_DBUSER=paperless
export PAPERLESS_DBPASS=paperless
export PAPERLESS_REDIS="redis://127.0.0.1:$REDIS_PORT"
export PAPERLESS_SECRET_KEY="paperless-rs-reference-secret-key-local-only-0123456789"
export PAPERLESS_TIME_ZONE=Europe/Zurich
export PAPERLESS_DATA_DIR="$(cygpath -w "$REF_DIR/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$REF_DIR/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$REF_DIR/data/consume")"
export PAPERLESS_STATICDIR="$(cygpath -w "$REF_DIR/data/static")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$REF_DIR/logs")"
export PAPERLESS_URL="http://127.0.0.1:8000"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8000,http://localhost:8000"
export PAPERLESS_DEBUG=false
export PAPERLESS_OCR_LANGUAGE=eng
export DJANGO_PORT=8000

# --- Profile "test" (only for tests/upstream-adapter) -----------------------------------------
# REF_PROFILE=test: second Django instance on 127.0.0.1:8001 against the test DB test_paperless
# created by pytest-django, with the test SECRET_KEY from upstream/pyproject.toml and "testserver" in
# ALLOWED_HOSTS. Own data/media folders (data-test/), own PID/log files (suffix -test).
REF_SUFFIX=""
if [[ "${REF_PROFILE:-}" == "test" ]]; then
  REF_SUFFIX="-test"
  export PAPERLESS_DBNAME=test_paperless
  export PAPERLESS_SECRET_KEY="test-secret-key-do-not-use-in-production"
  export PAPERLESS_ALLOWED_HOSTS="testserver,127.0.0.1,localhost"
  export PAPERLESS_DATA_DIR="$(cygpath -w "$REF_DIR/data-test/data")"
  export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$REF_DIR/data-test/media")"
  export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$REF_DIR/data-test/consume")"
  export PAPERLESS_REDIS_PREFIX="test"
  export DJANGO_PORT=8001
  export PAPERLESS_URL="http://127.0.0.1:8001"
  export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8001,http://localhost:8001,http://testserver"
fi
export REF_SUFFIX

# detach <pidfile> <exe> <stdout-log> <stderr-log> <arg>... : starts <exe> detached (own
# hidden console) and writes the PID to <pidfile>. Needed on Windows so that services survive the
# end of the calling shell/console. powershell deliberately gets NO pipe as
# stdout/stderr (only NUL), otherwise the service inherits the pipe and the caller hangs.
detach() {
  local rawpid="$1" pidf exe out err; pidf=$(cygpath -w "$1"); exe=$(cygpath -w "$2"); out=$(cygpath -w "$3"); err=$(cygpath -w "$4"); shift 4
  local args="" a
  for a in "$@"; do a=${a//\'/\'\'}; args+="${args:+,}'\"$a\"'"; done
  rm -f "$rawpid"
  powershell.exe -NoProfile -NonInteractive -Command \
    "\$p = Start-Process -WindowStyle Hidden -PassThru -FilePath '$exe' -RedirectStandardOutput '$out' -RedirectStandardError '$err' ${args:+-ArgumentList $args}; [IO.File]::WriteAllText('$pidf', [string]\$p.Id)" \
    </dev/null >/dev/null 2>&1
}
