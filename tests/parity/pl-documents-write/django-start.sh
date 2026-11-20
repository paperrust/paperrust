#!/usr/bin/env bash
# Starts unchanged upstream (granian, 1 worker) against the W4 copy on 127.0.0.1:8130.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w4env.sh"
pidf="$W4_WORK/run/django.pid"
if [[ -f "$pidf" ]] && tasklist //FI "PID eq $(cat "$pidf")" 2>/dev/null | grep -qi granian; then
  echo "W4-Django läuft bereits (PID $(cat "$pidf"))"; exit 0
fi
[[ -d "$W4_WORK/data/media" ]] || "$W4_DIR/reset.sh"
export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT="$DJANGO_PORT" GRANIAN_WORKERS=1
detach "$pidf" "$REF_DIR/venv/Scripts/granian.exe" "$W4_WORK/logs/django.out.log" "$W4_WORK/logs/django.err.log" \
  --interface asginl --ws --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
code=000
for _ in $(seq 1 90); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DJANGO_PORT/api/" || true)
  [[ "$code" =~ ^(200|302|401|403)$ ]] && break
  sleep 1
done
[[ "$code" =~ ^(200|302|401|403)$ ]] || { echo "Start fehlgeschlagen (HTTP $code)"; tail -20 "$W4_WORK/logs/django.err.log"; exit 1; }
echo "W4-Django läuft: PID $(cat "$pidf"), http://127.0.0.1:$DJANGO_PORT"
