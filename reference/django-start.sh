#!/usr/bin/env bash
# Starts paperless-ngx v3.4.2 (unchanged) like in the Docker image: granian, ASGI (asginl), WebSockets,
# 1 worker – here without uvloop (not available on Windows) -> asyncio loop.
# 127.0.0.1:8000 (profile test: REF_PROFILE=test -> :8001). PID: run/django<suffix>.pid
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$REF_DIR"; mkdir -p run logs data/static
# like docker init-folders (also for profile test): data/index, media, consume
mkdir -p "$(cygpath -u "$PAPERLESS_DATA_DIR")/index" "$(cygpath -u "$PAPERLESS_MEDIA_ROOT")" "$(cygpath -u "$PAPERLESS_CONSUMPTION_DIR")"
pidf="$REF_DIR/run/django$REF_SUFFIX.pid"
if [[ -f "$pidf" ]] && tasklist //FI "PID eq $(cat "$pidf")" 2>/dev/null | grep -qi granian; then
  echo "Django läuft bereits (PID $(cat "$pidf"), Port $DJANGO_PORT)"; exit 0
fi
"$PG_BIN/pg_isready" -q -h 127.0.0.1 -p "$PGPORT" || ./pg-start.sh >/dev/null
./redis-start.sh >/dev/null
# static files (incl. frontend from reference/frontend) via collectstatic as in the image
if [[ ! -f data/static/.collected || ( -f frontend/en-US/main.js && ! -f data/static/frontend/en-US/main.js ) ]]; then
  "$PY" "$REPO_DIR/upstream/src/manage.py" collectstatic --no-input --clear > logs/collectstatic.log 2>&1
  touch data/static/.collected
fi
export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$DJANGO_PORT" GRANIAN_WORKERS=1
detach "$pidf" "$REF_DIR/venv/Scripts/granian.exe" "$REF_DIR/logs/django$REF_SUFFIX.out.log" "$REF_DIR/logs/django$REF_SUFFIX.err.log" \
  --interface asginl --ws --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
for _ in $(seq 1 90); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DJANGO_PORT/api/" || true)
  [[ "$code" =~ ^(200|302|401|403)$ ]] && break
  sleep 1
done
[[ "$code" =~ ^(200|302|401|403)$ ]] || { echo "Django-Start fehlgeschlagen (HTTP $code), siehe logs/django$REF_SUFFIX.*.log"; tail -20 "$REF_DIR/logs/django$REF_SUFFIX.err.log"; exit 1; }
echo "Django läuft: PID $(cat "$pidf"), http://127.0.0.1:$DJANGO_PORT"
