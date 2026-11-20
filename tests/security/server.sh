#!/usr/bin/env bash
# Local PaperRust test server for the security suite (tests/security), WITHOUT Django.
#   DB      pl_sec_matrix (fresh from TEMPLATE paperless_snapshot), 127.0.0.1:54329; `start` migrates beforehand
#   Data    tests/security/.work/{data,logs}  (from reference/snapshot/data.tar)
#   Server  127.0.0.1:8370 (SEC_PORT)
#   Binary  is copied to results/bin/paperrust-sec.exe before the start (SEC_SRC, default
#           target/debug/paperrust.exe), so that a parallel build does not lock the running program.
#   server.sh [start|stop|reset|drop|routes]   (drop = stop + delete DB; routes = active routes from the log)
# Local test values only, never against the DBs paperless/paperless_snapshot/pl_scale_1m.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
W="${SEC_WORK:-$HERE/.work}"
PORT="${SEC_PORT:-8370}"
DB="${SEC_DB:-pl_sec_matrix}"
case "$DB" in pl_sec_*) ;; *) echo "[sec] SEC_DB muss mit pl_sec_ beginnen"; exit 2 ;; esac
SRC="${SEC_SRC:-$REPO_DIR/target/debug/paperrust.exe}"
BIN="$REPO_DIR/results/bin/paperrust-sec.exe"
I="${SEC_INSTANCE:-}"   # second instance on the same DB: SEC_INSTANCE=2 SEC_PORT=8371 (throttling across processes)
PIDF="$W/run/rust$I.pid"
mkdir -p "$W/run" "$W/logs"

export PAPERLESS_DBNAME=$DB
export PAPERLESS_RS_BIND="127.0.0.1:$PORT"
export PAPERLESS_URL="http://127.0.0.1:$PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8370,http://localhost:8370,http://127.0.0.1:8371,http://localhost:8371,http://127.0.0.1:8372,http://localhost:8372"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
unset PAPERLESS_RS_ROUTES PAPERRUST_ROUTES PAPERLESS_CACHE_BACKEND
export PAPERLESS_DATA_DIR="$(cygpath -w "$W/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W/logs")"
export PAPERRUST_UPDATE_CHECK=false
export PAPERRUST_BRANDING=off
export PAPERLESS_RS_LOG="info"
export NO_COLOR=1
# For the login tests: account throttling and social/OIDC against the local fake provider
export PAPERLESS_ACCOUNT_ALLOW_SIGNUPS="${SEC_SIGNUPS:-false}"
# SEC_OIDC=1: social/OIDC against the fake provider tests/parity/pl-accounts/w12_sinks.py (IdP :8141,
# SMTP sink :8142), automatic registration on
if [[ "${SEC_OIDC:-}" == 1 ]]; then
  export PAPERLESS_SOCIALACCOUNT_PROVIDERS='{"openid_connect":{"OAUTH_PKCE_ENABLED":true,"APPS":[{"provider_id":"fakeidp","name":"Fake IdP","client_id":"w12client","secret":"w12secret","settings":{"server_url":"http://127.0.0.1:8141"}}]}}'
  export PAPERLESS_APPS="allauth.socialaccount.providers.openid_connect" PAPERLESS_SOCIAL_AUTO_SIGNUP=true
  export PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL=http
  export PAPERLESS_EMAIL_HOST=127.0.0.1 PAPERLESS_EMAIL_PORT=8142 PAPERLESS_EMAIL_FROM="paperless@example.invalid"
fi

alive() { [[ -f "$PIDF" ]] && tasklist //FI "PID eq $(cat "$PIDF")" 2>/dev/null | grep -qi paperrust; }
psql_su() { env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -q "$@"; }

reset() {
  psql_su -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB TEMPLATE paperless_snapshot OWNER paperless" || return 1
  rm -rf "$W/data"; mkdir -p "$W/data/consume" "$W/data/scratch"
  tar xf "$REPO_DIR/reference/snapshot/data.tar" -C "$W/data"
  echo "[sec] $DB + $W/data zurückgesetzt"
}

start() {
  if alive; then echo "[sec] läuft bereits (PID $(cat "$PIDF"))"; return 0; fi
  [[ -z "$I" ]] && { cp -f "$SRC" "$BIN" || return 1; }
  [[ -n "$I" ]] || PAPERRUST_DATAMODEL_BACKUP=off "$BIN" manage migrate --no-input >"$W/logs/migrate.log" 2>&1 \
    || { echo "[sec] manage migrate fehlgeschlagen (siehe $W/logs/migrate.log)"; tail -5 "$W/logs/migrate.log"; return 1; }
  detach "$PIDF" "$BIN" "$W/logs/rust$I.out.log" "$W/logs/rust$I.err.log" ${SEC_ARGS:-serve}
  for _ in $(seq 1 120); do
    c=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/" || true)
    [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "[sec] PaperRust :$PORT PID $(cat "$PIDF")"; return 0; }
    sleep 1
  done
  echo "[sec] Start fehlgeschlagen"; tail -20 "$W/logs/rust$I.err.log"; return 1
}

stop() {
  if alive; then taskkill //PID "$(cat "$PIDF")" //T //F >/dev/null 2>&1 && echo "[sec] gestoppt (PID $(cat "$PIDF"))"; fi
  rm -f "$PIDF"
}

routes() {
  cat "$W/logs/rust.out.log" "$W/logs/rust.err.log" 2>/dev/null | grep "rust route active" \
    | sed -E 's/.*id="?([^ "]+)"? method=([A-Z]+) path="?([^ "]+)"?.*/\1\t\2\t\3/' | sort -u
}

case "${1:-start}" in
  start) start ;;
  stop) stop ;;
  reset) reset ;;
  routes) routes ;;
  drop) stop; psql_su -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" && echo "[sec] $DB gelöscht"; rm -rf "$W" ;;
  *) echo "usage: server.sh [start|stop|reset|drop|routes]"; exit 2 ;;
esac
