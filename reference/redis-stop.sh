#!/usr/bin/env bash
# Stops Garnet – only via the stored PID (run/redis.pid).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
pidf="$REF_DIR/run/redis.pid"
[[ -f "$pidf" ]] || { echo "Garnet läuft nicht (keine PID-Datei)."; exit 0; }
pid=$(cat "$pidf")
if tasklist //FI "PID eq $pid" 2>/dev/null | grep -qi GarnetServer; then
  taskkill //PID "$pid" //F >/dev/null && echo "Garnet (PID $pid) gestoppt."
else
  echo "Prozess $pid ist kein GarnetServer mehr – nichts gestoppt."
fi
rm -f "$pidf"
