#!/usr/bin/env bash
# Watchdog during the click paths: if Django (:8170) no longer answers and the granian process is gone,
# it is restarted and the incident written to .work/logs/watchdog.log (goes into the report).
# Runs until .work/run/watchdog.stop exists.
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$E2E_DIR"; rm -f "$RUN/watchdog.stop"
while [[ ! -f "$RUN/watchdog.stop" ]]; do
  if ! curl -s -o /dev/null -m 5 "http://127.0.0.1:$E2E_A_PORT/api/"; then
    sleep 3
    if ! curl -s -o /dev/null -m 5 "http://127.0.0.1:$E2E_A_PORT/api/" && [[ ! -f "$RUN/watchdog.stop" ]]; then
      echo "$(date +%T) Django :$E2E_A_PORT nicht erreichbar (Phase ${E2E_PHASE:-?}) -> Neustart; letzte Zeilen granian:" >> "$LOGS/watchdog.log"
      tail -n 4 "$(ls -t "$LOGS"/django-*.out.log | head -1)" >> "$LOGS/watchdog.log" 2>/dev/null
      ./stop.sh django >/dev/null; ./start-django.sh >> "$LOGS/watchdog.log" 2>&1
    fi
  fi
  sleep 3
done
