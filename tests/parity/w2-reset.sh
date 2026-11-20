#!/usr/bin/env bash
# Restores ONLY the W2 copy: DB pl_w2_ref (from pl_w2_snapshot), tests/parity/.w2/data
# (from reference/snapshot/data.tar) and the Redis keys with prefix "w2".
# The shared reference (DB paperless, reference/data, remaining Redis keys) stays untouched.
# Logic in w2_reset.py (one process; process starts are the bottleneck on Windows).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python -I "$here/w2_reset.py"
