#!/usr/bin/env bash
# Shared helpers: lock, memory check (Jetson is shared, see ~/jobs/REGELN.md)
LOCK=~/jobs/paperrust_bench.lock
take_lock() {
  echo "$$ $(date -Is) PAPERRUST BENCHMARK $1" > "$LOCK"
  trap 'rm -f "$LOCK"' EXIT
}
mem_avail() { awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo; }
wait_mem() { # waits until >= $1 MB free
  local need=${1:-2000}
  while [ "$(mem_avail)" -lt "$need" ]; do echo "warte auf RAM ($(mem_avail) MB)"; sleep 30; done
}
foreign_mpc() { ls ~/jobs/mpc_*.lock 2>/dev/null | grep -v mpc_luecke_ | head -1; }
