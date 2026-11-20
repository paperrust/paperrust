#!/usr/bin/env bash
# Local LLM/embedding mock (only 127.0.0.1:8131), request log in $E2E/mock.log.
REPO="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
E2E="${E2E_DIR:-${TMPDIR:-/tmp}/paperrust-w10-e2e}"
exec "$REPO/reference/venv/Scripts/python.exe" -I "$REPO/tests/parity/pl-ai/mock_llm.py" 8131 "$(cygpath -w "$E2E/mock.log")"
