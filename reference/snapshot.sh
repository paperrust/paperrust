#!/usr/bin/env bash
# Freezes the current state as a snapshot: template DB paperless_snapshot (fast reset),
# snapshot/paperless.dump (pg_dump -Fc, durable) and snapshot/data.tar (data/data + data/media).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$REF_DIR"; mkdir -p snapshot
SUPER=(env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -q -U "$PG_SUPERUSER" -d postgres)
"$PG_BIN/pg_dump" -Fc --no-owner -f snapshot/paperless.dump paperless
"${SUPER[@]}" -c "DROP DATABASE IF EXISTS paperless_snapshot WITH (FORCE);" \
              -c "CREATE DATABASE paperless_snapshot OWNER paperless TEMPLATE paperless STRATEGY FILE_COPY;" \
              -c "ALTER DATABASE paperless_snapshot WITH ALLOW_CONNECTIONS false;"
tar -cf snapshot/data.tar -C data data media
sha256sum snapshot/paperless.dump snapshot/data.tar > snapshot/SHA256SUMS
echo "[snapshot] $(du -sh snapshot | cut -f1) in reference/snapshot, Template-DB paperless_snapshot"
