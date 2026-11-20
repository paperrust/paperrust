#!/usr/bin/env bash
# W23: own reference copy for the document action routes (delete/rotate/merge/merge_as_versions/
# edit_pdf/remove_password) and versions (versions/<id>/, update_version/).
# DB pl_w23_ref (from paperless_snapshot), data work/data (from reference/snapshot/data.tar),
# Django 127.0.0.1:8190 with the W13 overlay (Celery tasks are recorded, not executed).
# Rust side runs in-process in the test (crates/pl-documents-write/tests/w23_parity.rs).
# Never write to the reference DB `paperless`.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../reference" && pwd)/env.sh"
W23_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export W23_DIR
W23_WORK="$W23_DIR/work"
export W23_WORK
mkdir -p "$W23_WORK/run" "$W23_WORK/logs" "$W23_WORK/data"
export PAPERLESS_DBNAME=pl_w23_ref
export PGDATABASE=pl_w23_ref
export PAPERLESS_DATA_DIR="$(cygpath -w "$W23_WORK/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W23_WORK/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W23_WORK/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W23_WORK/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W23_WORK/logs")"
export PAPERLESS_REDIS_PREFIX=w23
export DJANGO_PORT=8190
export PAPERLESS_URL="http://127.0.0.1:8190"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8190,http://localhost:8190"
export PYTHONPATH="$(cygpath -w "$W23_DIR/../w13/overlay");$PYTHONPATH"
export DJANGO_SETTINGS_MODULE=pl_w13_settings
export W13_TASK_LOG="$(cygpath -w "$W23_WORK/run/tasks.jsonl")"
# Rust side: search index synchronous like upstream in the request (queue is always PostgreSQL)
export PAPERRUST_INDEX_WRITER=rust
