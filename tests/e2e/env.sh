#!/usr/bin/env bash
# Shared environment of the browser E2E suite (A = Django :8170, B = PaperRust :8171), DB pl_e2e.
# Local test values only. Sourced by reset.sh / start-*.sh / stop.sh / run.sh.
E2E_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$E2E_DIR/../.." && pwd)"
source "$REPO/reference/env.sh"          # PY, PG_BIN, detach(), PAPERLESS_* of the reference …
WORK="$E2E_DIR/.work"                    # own data folder (gitignored)
RUN="$WORK/run"; LOGS="$WORK/logs"
mkdir -p "$RUN" "$LOGS"
w() { cygpath -w "$1"; }
export E2E_DIR REPO WORK RUN LOGS
export E2E_DB=pl_e2e E2E_A_PORT=8170 E2E_B_PORT=8171 E2E_REDIS_DB=5
# … and here redirect everything to the own instance
export PAPERLESS_DBNAME=$E2E_DB
export PAPERLESS_REDIS="redis://127.0.0.1:$REDIS_PORT/$E2E_REDIS_DB"   # own Garnet DB (cache/Celery queue)
export PAPERLESS_DATA_DIR="$(w "$WORK/data/data")" PAPERLESS_MEDIA_ROOT="$(w "$WORK/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(w "$WORK/data/consume")" PAPERLESS_SCRATCH_DIR="$(w "$WORK/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(w "$LOGS")"
# STATICDIR stays reference/data/static (read only; collectstatic already ran there)
export PAPERLESS_URL="http://127.0.0.1:$E2E_A_PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$E2E_A_PORT,http://127.0.0.1:$E2E_B_PORT"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERRUST_EXE="${PAPERRUST_EXE:-$REPO/results/bin/paperrust-e2e.exe}"
# pid_alive <pidfile> <image-substring>
pid_alive() { [[ -f "$1" ]] && tasklist //FI "PID eq $(cat "$1")" 2>/dev/null | grep -qi "$2"; }
wait_http() { # wait_http <url> <seconds>
  local code=""; for _ in $(seq 1 "$2"); do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$1" || true)
    [[ "$code" =~ ^(200|302|401|403)$ ]] && return 0; sleep 1; done
  echo "keine Antwort von $1 (HTTP $code)" >&2; return 1; }
