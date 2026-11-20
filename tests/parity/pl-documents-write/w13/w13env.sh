#!/usr/bin/env bash
# Environment of the own reference copy of W13 (PDF methods of bulk_edit): DB pl_w13_ref (from
# paperless_snapshot), own data directory work/data (from reference/snapshot/data.tar),
# Django on 127.0.0.1:8150 with overlay settings (Celery tasks are recorded, not
# executed). Never write to the reference DB `paperless`.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../reference" && pwd)/env.sh"
W13_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export W13_DIR
W13_WORK="$W13_DIR/work"
export W13_WORK
mkdir -p "$W13_WORK/run" "$W13_WORK/logs" "$W13_WORK/data"
export PAPERLESS_DBNAME=pl_w13_ref
export PGDATABASE=pl_w13_ref
export PAPERLESS_DATA_DIR="$(cygpath -w "$W13_WORK/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W13_WORK/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W13_WORK/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W13_WORK/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W13_WORK/logs")"
export PAPERLESS_REDIS_PREFIX=w13
export DJANGO_PORT=8150
export PAPERLESS_URL="http://127.0.0.1:8150"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8150,http://localhost:8150"
# Overlay: records published Celery tasks in W13_TASK_LOG (JSON lines)
export PYTHONPATH="$(cygpath -w "$W13_DIR/overlay");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pl_w13_settings
export W13_TASK_LOG="$(cygpath -w "$W13_WORK/run/tasks.jsonl")"
