#!/usr/bin/env bash
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
PIDF="${PGV_DJ_PIDFILE:-results/pgvalidate/bin/dj.pid}"
pid=$(cat "$PIDF") && taskkill //PID "$pid" //T //F && rm -f "$PIDF"
