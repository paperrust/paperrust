#!/usr/bin/env bash
# Own, short-lived Django (upstream v3.4.2 unchanged) on 127.0.0.1:8091 against pl_pgvalidate.
# PGV_DJ_SQL_LOG=<file> switches the SQL logger on (see pgv_settings.py). PID: results/pgvalidate/bin/dj.pid
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
source reference/env.sh
ROOT="$(pwd)"
export PAPERLESS_DBNAME=pl_pgvalidate
export PAPERLESS_REDIS_PREFIX=pgv
DJ_PORT="${PGV_DJ_PORT:-8091}"
DJ_PIDF="${PGV_DJ_PIDFILE:-$ROOT/results/pgvalidate/bin/dj.pid}"
export PAPERLESS_URL=http://127.0.0.1:$DJ_PORT
export PAPERLESS_CSRF_TRUSTED_ORIGINS=http://127.0.0.1:$DJ_PORT
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$ROOT/results/pgvalidate/logs/django")"
mkdir -p "$ROOT/results/pgvalidate/logs/django"
export PYTHONPATH="$(cygpath -w "$ROOT/tests/pgvalidate");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pgv_settings
[[ -n "${PGV_DJ_SQL_LOG:-}" ]] && export PGV_DJ_SQL_LOG="$(cygpath -w "$PGV_DJ_SQL_LOG")"
export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT=$DJ_PORT GRANIAN_WORKERS=1
detach "$DJ_PIDF" "$REF_DIR/venv/Scripts/granian.exe" \
  "$ROOT/results/pgvalidate/logs/dj.out.log" "$ROOT/results/pgvalidate/logs/dj.err.log" \
  --interface asginl --ws --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
for _ in $(seq 1 90); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DJ_PORT/api/" || true)
  [[ "$code" =~ ^(200|302|401|403)$ ]] && break
  sleep 1
done
echo "django :$DJ_PORT HTTP $code pid $(cat "$DJ_PIDF")"
