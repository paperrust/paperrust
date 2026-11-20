#!/usr/bin/env bash
# Environment of the own reference copy of W4 (pl-documents-write): DB pl_w4_ref (from
# paperless_snapshot), own data directory work/data (from reference/snapshot/data.tar),
# Django on 127.0.0.1:8130, Redis prefix w4. Never write to the reference DB `paperless`.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../reference" && pwd)/env.sh"
W4_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export W4_DIR
W4_WORK="$W4_DIR/work"
export W4_WORK
mkdir -p "$W4_WORK/run" "$W4_WORK/logs" "$W4_WORK/data"
export PAPERLESS_DBNAME=pl_w4_ref
export PGDATABASE=pl_w4_ref
export PAPERLESS_DATA_DIR="$(cygpath -w "$W4_WORK/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W4_WORK/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W4_WORK/data/consume")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W4_WORK/logs")"
export PAPERLESS_REDIS_PREFIX=w4
export DJANGO_PORT=8130
export PAPERLESS_URL="http://127.0.0.1:8130"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8130,http://localhost:8130"
