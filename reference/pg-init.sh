#!/usr/bin/env bash
# Creates the PostgreSQL cluster of the reference once (reference/pg/cluster).
# Prerequisite: EDB binaries under reference/pg/pgsql (see README, "Einrichtung").
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
if [[ -f "$PGDATA/PG_VERSION" ]]; then echo "Cluster existiert bereits: $PGDATA"; exit 0; fi
mkdir -p "$REF_DIR/run" "$REF_DIR/logs"
pw="$REF_DIR/run/pwfile.tmp"; printf '%s' "$PG_SUPERPASS" > "$pw"
"$PG_BIN/initdb" -D "$PGDATA" -U "$PG_SUPERUSER" --pwfile="$pw" -E UTF8 \
  --locale-provider=icu --icu-locale=en-US --locale=en-US --lc-messages=C \
  --auth=scram-sha-256 --no-instructions
rm -f "$pw"
cat >> "$PGDATA/postgresql.conf" <<CONF

# --- paperless-rs Referenz (RAM-sparsam, nur lokal) ---
listen_addresses = '127.0.0.1'
port = $PGPORT
max_connections = 100
shared_buffers = 32MB
work_mem = 4MB
maintenance_work_mem = 32MB
effective_cache_size = 256MB
max_wal_size = 256MB
min_wal_size = 64MB
wal_buffers = 1MB
max_worker_processes = 4
max_parallel_workers = 2
max_parallel_workers_per_gather = 1
autovacuum_max_workers = 2
timezone = 'UTC'
log_timezone = 'UTC'
lc_messages = 'C'
logging_collector = off
log_line_prefix = '%m [%p] %u@%d '
# Wegwerf-Testcluster: Haltbarkeit gegen Geschwindigkeit (Reset/Seed). Nach Absturz: seed.sh/reset.sh.
fsync = off
synchronous_commit = off
full_page_writes = off
CONF
cat > "$PGDATA/pg_hba.conf" <<HBA
# Nur lokal (127.0.0.1), Passwort (SCRAM). Keine anderen Hosts.
# TYPE  DATABASE  USER  ADDRESS        METHOD
host    all       all   127.0.0.1/32   scram-sha-256
HBA
"$REF_DIR/pg-start.sh" </dev/null
PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -v ON_ERROR_STOP=1 -U "$PG_SUPERUSER" -d postgres -q <<SQL
CREATE ROLE paperless LOGIN PASSWORD 'paperless' CREATEDB;
CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0;
SQL
echo "Cluster angelegt, Rolle/DB 'paperless' erstellt."
