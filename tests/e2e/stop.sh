#!/usr/bin/env bash
# Ends only the own processes of this suite via stored PID. stop.sh [rust|django|all]
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
stop_one() { local f="$RUN/$1.pid"; [[ -f "$f" ]] || return 0
  if pid_alive "$f" "$2"; then taskkill //PID "$(cat "$f")" //T //F >/dev/null 2>&1 && echo "gestoppt: $1 (PID $(cat "$f"))"; fi
  rm -f "$f"; }
case "${1:-all}" in
  rust) stop_one rust paperrust ;;
  django) stop_one django granian ;;
  *) stop_one rust paperrust; stop_one django granian ;;
esac
