#!/usr/bin/env bash
# Security check for the release gate (docs/RELEASE-GATE.md section 4) in one run, local only:
#   1. Throwaway DB pl_sec_matrix + server :8370 (current target/debug/paperrust.exe), second instance :8371,
#      OIDC instance :8372 with fake provider (w12_sinks.py, IdP :8141, SMTP :8142)
#   2. Permission matrix (matrix.py), then DB fresh
#   3. Security suite (run_security.py) incl. login/session, OIDC and input tests
#   4. Clean up (stop servers, delete DBs)
# Results: results/release/security/ (matrix.*), results/security/ (suite runs).
# Dependencies/secrets/fuzzing run separately (see results/release/security/BERICHT.md).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
export SEC_DB=pl_sec_matrix TARGET=rust SECURITY_URL=http://127.0.0.1:8370
rc=0

bash "$HERE/server.sh" stop >/dev/null 2>&1
bash "$HERE/server.sh" reset && bash "$HERE/server.sh" start || exit 2
python "$REPO/tests/security/matrix.py" || rc=1

bash "$HERE/server.sh" stop; bash "$HERE/server.sh" reset && bash "$HERE/server.sh" start || exit 2
SEC_INSTANCE=2 SEC_PORT=8371 bash "$HERE/server.sh" start || exit 2
mkdir -p "$HERE/.work/sinks"
python -I "$REPO/tests/parity/pl-accounts/w12_sinks.py" "$(cygpath -w "$HERE/.work/sinks")" &
SINKS=$!
SEC_OIDC=1 SEC_INSTANCE=3 SEC_PORT=8372 bash "$HERE/server.sh" start || exit 2

# Suite against :8370 (rate limit test last), login flows against the OIDC instance (own IP windows)
python "$HERE/run_security.py" -k Csrf -k Auth -k Headers -k Leaks -k Permissions -k Traversal -k Limits \
  -k FileResponses -k OpenRedirect -k StoragePath -k TemplateSandbox -k ZipSlip -k TokenRateLimit || rc=1
SECURITY_URL=http://127.0.0.1:8372 SECURITY_URL2=http://127.0.0.1:8371 SECURITY_OIDC_URL=http://127.0.0.1:8372 \
  python "$HERE/run_security.py" -k CsrfForm -k Sessions -k Tokens -k TwoFactor -k PasswordReset -k Oidc -k ZThrottling || rc=1

kill "$SINKS" 2>/dev/null
SEC_INSTANCE=3 SEC_PORT=8372 bash "$HERE/server.sh" stop
SEC_INSTANCE=2 SEC_PORT=8371 bash "$HERE/server.sh" stop
bash "$HERE/server.sh" drop
exit $rc
