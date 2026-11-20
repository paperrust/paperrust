#!/usr/bin/env bash
# Stops PostgreSQL of the reference – only via the PID of its own postmaster
# (reference/pg/cluster/postmaster.pid, mirrored in reference/run/postgres.pid).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
if [[ ! -f "$PGDATA/postmaster.pid" ]]; then echo "PostgreSQL läuft nicht."; rm -f "$REF_DIR/run/postgres.pid"; exit 0; fi
pid=$(head -1 "$PGDATA/postmaster.pid" | tr -d '\r')
# pg_ctl stop signals exactly this PID (from postmaster.pid) – fast shutdown.
"$PG_BIN/pg_ctl" -D "$PGDATA" -m fast -w -t 60 stop >/dev/null
rm -f "$REF_DIR/run/postgres.pid"
echo "PostgreSQL (PID $pid) gestoppt."
