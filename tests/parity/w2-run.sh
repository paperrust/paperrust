#!/usr/bin/env bash
# Harness write run against the W2 copy (A = Django :8100, B = PaperRust :8101, reset = w2-reset.sh).
#   tests/parity/w2-run.sh --crate pl-workflows [--case 'workflows.*'] ...
# Reports to results/parity/w2/ (not into the shared latest.md/history.jsonl).
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/w2-env.sh"
cd "$W2_REPO"
exec python tests/parity/harness/run_parity.py --write-only --results-dir results/parity/w2 "$@"
