#!/usr/bin/env bash
# Stops Django (granian) – only via the stored PID (incl. its worker child processes).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
pidf="$REF_DIR/run/django$REF_SUFFIX.pid"
[[ -f "$pidf" ]] || { echo "Django läuft nicht (keine PID-Datei)."; exit 0; }
pid=$(cat "$pidf")
if tasklist //FI "PID eq $pid" 2>/dev/null | grep -qi granian; then
  # /T: whole process tree from this PID (granian launcher -> python -> worker)
  taskkill //PID "$pid" //T //F >/dev/null 2>&1 || true
  sleep 1
  if tasklist //FI "PID eq $pid" 2>/dev/null | grep -qi granian; then echo "Stoppen von PID $pid fehlgeschlagen"; exit 1; fi
  echo "Django (PID $pid + Worker) gestoppt."
else
  echo "Prozess $pid ist kein granian mehr – nichts gestoppt."
fi
rm -f "$pidf"
