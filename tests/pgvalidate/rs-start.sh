#!/usr/bin/env bash
# Starts the own PaperRust server (copy of the binary) on 127.0.0.1:8090 against pl_pgvalidate.
set -e
source "$(dirname "${BASH_SOURCE[0]}")/rs-env.sh"
LOG="${1:-results/pgvalidate/logs/rs.log}"
detach results/pgvalidate/bin/rs.pid results/pgvalidate/bin/paperrust-pgv.exe "$LOG" "$LOG.err"
for i in $(seq 1 50); do curl -s -o /dev/null http://127.0.0.1:8090/api/ && break; sleep 0.3; done
echo "pid $(cat results/pgvalidate/bin/rs.pid)"
