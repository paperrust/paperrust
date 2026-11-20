#!/usr/bin/env bash
# Rebuilds the reference data completely: recreate DB, migrate, schema dump, seed.py,
# full-text index, snapshot (snapshot/ + template DB paperless_snapshot). Takes ~2-3 min.
# Django is stopped for this and restarted at the end if it was running.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$REF_DIR"
was_running=0
if [[ -f run/django.pid ]]; then was_running=1; ./django-stop.sh; fi
./pg-start.sh >/dev/null
./redis-start.sh >/dev/null
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -U "$PG_SUPERUSER" -d postgres)
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS paperless WITH (FORCE);" \
              -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0;"
rm -rf data/data data/media data/consume
# like docker init-folders: consume, data/index
mkdir -p data/data/index data/media data/consume data/static logs
echo "[seed] migrate"; "$PY" "$REPO_DIR/upstream/src/manage.py" migrate --no-input > logs/migrate.log 2>&1
echo "[seed] schema dump"
"$PG_BIN/pg_dump" --schema-only --no-owner --no-privileges --restrict-key=paperlessrsref -f schema/postgres.sql paperless
"$PG_BIN/psql" -Atc "select app||'.'||name from django_migrations order by app, name" > schema/migrations.txt
echo "[seed] seed.py"; "$PY" -I seed.py | tee logs/seed.log
echo "[seed] document_index reindex"
"$PY" "$REPO_DIR/upstream/src/manage.py" document_index reindex --no-progress-bar > logs/index.log 2>&1
"$PY" -I -c "import redis; redis.Redis(port=$REDIS_PORT).flushall()"
./snapshot.sh
[[ $was_running == 1 ]] && ./django-start.sh || true
echo "[seed] fertig"
