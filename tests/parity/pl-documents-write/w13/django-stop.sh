#!/usr/bin/env bash
# Stops the W13 Django instance only via the stored PID.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w13env.sh"
pidf="$W13_WORK/run/django.pid"
[[ -f "$pidf" ]] || { echo "läuft nicht"; exit 0; }
pid=$(cat "$pidf")
if tasklist //FI "PID eq $pid" 2>/dev/null | grep -qi granian; then
  taskkill //PID "$pid" //T //F >/dev/null 2>&1 || true
  echo "gestoppt ($pid)"
fi
rm -f "$pidf"
