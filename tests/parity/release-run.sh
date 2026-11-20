#!/usr/bin/env bash
# Release parity run: Django v3.4.2 (A) against PaperRust standalone (B), each on its own database.
#
#   tests/parity/release-run.sh read  [run_parity args]   # all read cases (--no-write)
#   tests/parity/release-run.sh write [run_parity args]   # all write cases (--write-only)
#   tests/parity/release-run.sh all                       # read, then write
#
# Why two databases: since 2026-11-14 B runs without Django and restructures its tables
# (data-model stages 1-5, tag adjacency list), so A and B cannot share one database any more.
#
#   read   A = Django :8000 on DB paperless (reference/reset.sh before the run; started with
#              reference/django-start.sh if not running and stopped again afterwards)
#          B = PaperRust :58431 on pl_relrun_b (<- paperless_snapshot + `paperrust manage migrate`)
#   write  A = Django :8150 on its own W2 copy (W2_NAME=w2rel, tests/parity/w2-start.sh dj)
#          B = PaperRust :58441 on pl_relrun_bw (<- migrated template pl_relrun_bw_tpl)
#          reset = tests/parity/release_reset.py (both copies), SQL steps per side
#          (tests/parity/release_harness.py), OAuth test values for both sides
#
# Environment: PAPERRUST_BIN (default $CARGO_TARGET_DIR/debug/paperrust.exe, copied before the run),
# RELRUN_WORK (work dir, short path because of MAX_PATH; default %LOCALAPPDATA%\Temp\prrel),
# RELRUN_RESULTS (default results/parity/release-<date>), KEEP=1 keeps databases and work dir.
# Never touches paperless/paperless_snapshot/pl_scale_1m except via reference/reset.sh. Processes are
# stopped only by their stored PIDs.
set -uo pipefail
MODE="${1:-all}"; shift || true
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
source "$REPO/reference/env.sh"
# PAPERLESS_REDIS stays set: B ignores it (retired, warning only), and externally_configured_variables then matches A
WORK="${RELRUN_WORK:-$(cygpath -u "${LOCALAPPDATA:-$HOME}")/Temp/prrel}"
RESULTS="${RELRUN_RESULTS:-$REPO/results/parity/release-$(date +%F)}"
SRC_BIN="${PAPERRUST_BIN:-${CARGO_TARGET_DIR:-$REPO/target}/debug/paperrust.exe}"
BIN="$WORK/paperrust-relrun.exe"
export W2_NAME=w2rel W2_DJ_PORT=8150 W2_RS_PORT=58441
OAUTH=(PAPERLESS_GMAIL_OAUTH_CLIENT_ID=gaps-test-gmail-id PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET=gaps-test-gmail-secret
       PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID=gaps-test-outlook-id PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET=gaps-test-outlook-secret)
ADMIN_TOKEN=a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001   # local test value (reference/users.json)

psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q "$@"; }
for db in pl_relrun_b pl_relrun_bw pl_relrun_bw_tpl; do
  case "$db" in paperless|paperless_snapshot|pl_scale_1m) echo "refusing $db"; exit 2;; esac
done
kill_pid() { [[ -f "$1" ]] || return 0; taskkill //PID "$(cat "$1")" //T //F >/dev/null 2>&1; rm -f "$1"; }
wait_api() {  # port
  for _ in $(seq 1 120); do
    curl -sf -o /dev/null -H "Authorization: Token $ADMIN_TOKEN" "http://127.0.0.1:$1/api/" && return 0; sleep 1
  done
  echo "no answer on :$1"; return 1
}
# b_env <db> <dir> <port> <paperless_url>: environment of a B process / of `manage migrate`
b_env() {
  export PAPERLESS_DBNAME="$1" PAPERLESS_DATA_DIR="$(cygpath -w "$2/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$2/media")"
  export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$2/consume")" PAPERLESS_LOGGING_DIR="$(cygpath -w "$2/logs")"
  export PAPERRUST_DATAMODEL_BACKUP=off PAPERLESS_URL="$4"
  export PAPERLESS_CSRF_TRUSTED_ORIGINS="$4,http://127.0.0.1:$3,http://localhost:${4##*:}"
  export PAPERRUST_BIND="127.0.0.1:$3" PAPERRUST_ROUTES="$(cygpath -w "$REPO/routes.toml")"
  export PAPERRUST_BRANDING=off PAPERRUST_LOG=warn NO_COLOR=1
}
# fresh_copy <db> <dir>: DB from paperless_snapshot + PaperRust migrations, files from data.tar
fresh_copy() {
  psql_su -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless TEMPLATE paperless_snapshot"
  rm -rf "$2"; mkdir -p "$2/consume" "$2/logs"; tar -xf "$REF_DIR/snapshot/data.tar" -C "$2"; mkdir -p "$2/data" "$2/media"
  "$BIN" manage migrate --no-input >"$2/logs/migrate.log" 2>&1 || { echo "migrate $1 failed"; tail -20 "$2/logs/migrate.log"; return 1; }
}

STARTED_REF=0; STARTED_REDIS=0
cleanup() {
  kill_pid "$WORK/b.pid"; kill_pid "$WORK/bw.pid"
  [[ -d "$REPO/tests/parity/.$W2_NAME/run" ]] && bash "$HERE/w2-stop.sh" dj >/dev/null
  [[ $STARTED_REF == 1 ]] && bash "$REF_DIR/django-stop.sh"
  [[ $STARTED_REF == 1 || $STARTED_REDIS == 1 ]] && bash "$REF_DIR/redis-stop.sh"
  if [[ "${KEEP:-0}" != 1 ]]; then
    sleep 1
    for db in pl_relrun_b pl_relrun_bw pl_relrun_bw_tpl "pl_${W2_NAME}_ref" "pl_${W2_NAME}_snapshot"; do
      psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" 2>/dev/null
    done
    rm -rf "$WORK" "$REPO/tests/parity/.$W2_NAME"
  fi
}
trap cleanup EXIT

mkdir -p "$WORK"
cp "$SRC_BIN" "$BIN" || { echo "binary $SRC_BIN missing (cargo build -p pl-server --bin paperrust -j 2)"; exit 2; }
echo "B binary: $SRC_BIN ($(stat -c %y "$SRC_BIN" | cut -d. -f1)), git $(git -C "$REPO" rev-parse --short HEAD)"
rc=0

run_read() {
  if ! curl -s -o /dev/null "http://127.0.0.1:$DJANGO_PORT/api/"; then
    bash "$REF_DIR/django-start.sh" || return 2; STARTED_REF=1
  fi
  bash "$REF_DIR/reset.sh" || return 2
  ( b_env pl_relrun_b "$WORK/b" 58431 "http://127.0.0.1:$DJANGO_PORT"
    fresh_copy pl_relrun_b "$WORK/b" || exit 2
    detach "$WORK/b.pid" "$BIN" "$WORK/b/logs/serve.out.log" "$WORK/b/logs/serve.err.log" serve ) || return 2
  wait_api 58431 || return 2
  PARITY_SENSITIVE_PATHS="$(cygpath -w "$WORK")" "$PY" -I "$REPO/tests/parity/harness/run_parity.py" \
    --a-url "http://127.0.0.1:$DJANGO_PORT" --b-url http://127.0.0.1:58431 --no-write \
    --results-dir "$(cygpath -w "$RESULTS-read")" --users-file "$(cygpath -w "$REF_DIR/users.json")" "$@"
  local r=$?
  kill_pid "$WORK/b.pid"
  return $r
}

run_write() {
  # Django A needs the Redis-compatible cache (Garnet, sessions/cache) - without it every session save is a 500
  if ! (exec 3<>/dev/tcp/127.0.0.1/63799) 2>/dev/null; then
    bash "$REF_DIR/redis-start.sh" >/dev/null || return 2; STARTED_REDIS=1
  fi
  ( export "${OAUTH[@]}"; bash "$HERE/w2-start.sh" dj ) || return 2
  ( b_env pl_relrun_bw_tpl "$WORK/bw" 58441 "http://127.0.0.1:$W2_DJ_PORT"
    fresh_copy pl_relrun_bw_tpl "$WORK/bw" || exit 2
    psql_su -c "ALTER DATABASE pl_relrun_bw_tpl WITH ALLOW_CONNECTIONS false" ) || return 2
  rm -f "$WORK/bw.manifest.json"
  export RELRUN_B_DB=pl_relrun_bw RELRUN_B_TPL=pl_relrun_bw_tpl RELRUN_B_PORT=58441 RELRUN_B_DIR="$(cygpath -w "$WORK/bw")"
  # first reset creates pl_relrun_bw and the files; B starts afterwards (RELRUN_B_PORT=0: no warm-up)
  ( export RELRUN_B_PORT=0; "$PY" -I "$HERE/release_reset.py" ) || return 2
  # same PAPERLESS_* environment as A (w2-env.sh sets the Redis prefix), so externally_configured_variables match
  ( b_env pl_relrun_bw "$WORK/bw" 58441 "http://127.0.0.1:$W2_DJ_PORT"; export "${OAUTH[@]}" PAPERLESS_REDIS_PREFIX="$W2_NAME"
    detach "$WORK/bw.pid" "$BIN" "$WORK/bw-serve.out.log" "$WORK/bw-serve.err.log" serve ) || return 2
  wait_api 58441 || return 2
  PARITY_SQL_DB_A="pl_${W2_NAME}_ref" PARITY_SQL_DB_B=pl_relrun_bw \
  PARITY_RESET_CMD="$(cygpath -w "$PY") -I $(cygpath -w "$HERE/release_reset.py")" \
  PARITY_SENSITIVE_PATHS="$(cygpath -w "$WORK");$(cygpath -w "$REPO/tests/parity/.$W2_NAME")" \
    "$PY" -I "$HERE/release_harness.py" --a-url "http://127.0.0.1:$W2_DJ_PORT" --b-url http://127.0.0.1:58441 \
    --write-only --results-dir "$(cygpath -w "$RESULTS-write")" --users-file "$(cygpath -w "$REF_DIR/users.json")" "$@"
  local r=$?
  kill_pid "$WORK/bw.pid"; bash "$HERE/w2-stop.sh" dj >/dev/null
  return $r
}

case "$MODE" in
  read)  run_read "$@"; rc=$? ;;
  write) run_write "$@"; rc=$? ;;
  all)   run_read "$@"; r1=$?; run_write "$@"; r2=$?; rc=$(( r1 > r2 ? r1 : r2 )) ;;
  *) echo "usage: $0 read|write|all [run_parity args]"; exit 2 ;;
esac
exit $rc
