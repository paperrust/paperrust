#!/usr/bin/env bash
# Starts the Redis-compatible server (Microsoft Garnet, MIT) on 127.0.0.1:63799.
# In memory only (no persistence), Lua on (Celery/kombu, channels_redis). PID: run/redis.pid
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
mkdir -p "$REF_DIR/run" "$REF_DIR/logs"
pidf="$REF_DIR/run/redis.pid"
if [[ -f "$pidf" ]] && tasklist //FI "PID eq $(cat "$pidf")" 2>/dev/null | grep -qi GarnetServer; then
  echo "Garnet läuft bereits (PID $(cat "$pidf"))"; exit 0
fi
export DOTNET_ROOT="$(cygpath -w "$REF_DIR/bin/dotnet")" DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_gcServer=0
detach "$pidf" "$GARNET_DIR/net10.0/GarnetServer.exe" "$REF_DIR/logs/redis.out.log" "$REF_DIR/logs/redis.err.log" \
  --bind 127.0.0.1 --port "$REDIS_PORT" --lua --memory 128m --index 16m --page 4m \
  --logger-level Warning
for _ in $(seq 1 30); do
  "$PY" -I -c "import socket,sys; s=socket.create_connection(('127.0.0.1',$REDIS_PORT),1); s.sendall(b'PING\r\n'); sys.exit(0 if s.recv(16).startswith(b'+PONG') else 1)" 2>/dev/null && break
  sleep 1
done
"$PY" -I -c "import socket; s=socket.create_connection(('127.0.0.1',$REDIS_PORT),1); s.sendall(b'PING\r\n'); assert s.recv(16).startswith(b'+PONG')" \
  || { echo "Garnet-Start fehlgeschlagen, siehe logs/redis.*.log"; tail -5 "$REF_DIR/logs/redis.err.log" "$REF_DIR/logs/redis.out.log"; exit 1; }
echo "Garnet läuft: PID $(cat "$pidf"), redis://127.0.0.1:$REDIS_PORT"
