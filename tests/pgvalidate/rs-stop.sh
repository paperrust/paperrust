#!/usr/bin/env bash
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
PIDF="${PGV_RS_PIDFILE:-results/pgvalidate/bin/rs.pid}"
pid=$(cat "$PIDF") && taskkill //PID "$pid" //F && rm -f "$PIDF"
