#!/usr/bin/env bash
# Ends the W8 processes exclusively via the stored PIDs.
source "$(dirname "${BASH_SOURCE[0]}")/w8-env.sh"
for n in dj mock; do
  f="$W8_DIR/run/$n.pid"; [[ -f $f ]] || continue
  taskkill //PID "$(cat "$f")" //T //F >/dev/null 2>&1; rm -f "$f"; echo "gestoppt: $n"
done
true
