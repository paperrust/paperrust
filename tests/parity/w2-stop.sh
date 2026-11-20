#!/usr/bin/env bash
# Ends the W2 processes exclusively via the stored PIDs.  w2-stop.sh [dj|rs|all]
source "$(dirname "${BASH_SOURCE[0]}")/w2-env.sh"
what="${1:-all}"
stop() { local f="$W2_DIR/run/$1.pid"; [[ -f $f ]] || return 0; taskkill //PID "$(cat "$f")" //T //F >/dev/null 2>&1; rm -f "$f"; echo "gestoppt: $1"; }
[[ $what == all || $what == rs ]] && stop rs
[[ $what == all || $what == dj ]] && stop dj
true
