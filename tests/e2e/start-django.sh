#!/usr/bin/env bash
# A: unchanged paperless-ngx v3.4.2 (granian/ASGI like reference/django-start.sh) on :8170 against pl_e2e.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
pidf="$RUN/django.pid"
if pid_alive "$pidf" granian; then echo "Django-E2E läuft bereits (PID $(cat "$pidf"))"; exit 0; fi
export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT=$E2E_A_PORT GRANIAN_WORKERS=1
ts=$(date +%H%M%S)
detach "$pidf" "$REF_DIR/venv/Scripts/granian.exe" "$LOGS/django-$ts.out.log" "$LOGS/django-$ts.err.log" \
  --interface asginl --ws --loop asyncio --working-dir "$(w "$REPO/upstream/src")" "paperless.asgi:application"
wait_http "http://127.0.0.1:$E2E_A_PORT/api/" 120
echo "Django-E2E: PID $(cat "$pidf"), http://127.0.0.1:$E2E_A_PORT"
