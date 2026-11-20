#!/usr/bin/env bash
# Parity run per restructuring stage (docs/DATAMODEL-V2.md §6): PaperRust on a copy of the reference data
# with stages 1..N-1 (A) against PaperRust with stages 1..N (B), all read cases of all crates
# (tests/parity/harness/run_parity.py). Same program on both sides – what is compared is the
# state of the data model.
#
#   bash tests/datamodel/parity_stage.sh <N> [further run_parity arguments]
# Read cases only: the harness resets write cases before every case with reference/reset.sh – that hits
# the shared reference DB `paperless`, not the two copies here. Therefore always --no-write.
#   PAPERRUST_BIN    program (default target/debug/paperrust.exe), KEEP=1 keep copies
#   PAPERRUST_BIN_A  other program for side A (stages without migration, e.g. 7: same data model,
#                    old against new code); N greater than the last stage = both sides fully restructured
set -uo pipefail
N="$1"; shift
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
REFD="${REFERENCE_DIR:-$ROOT/reference}"
[[ -x "$REFD/venv/Scripts/python.exe" ]] || REFD="$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir)/../reference"
source "$REFD/env.sh"
WORKROOT="${DM_WORKROOT:-$(cygpath -u "${LOCALAPPDATA:-$HOME}")/Temp/paperrust-dm}"
PR="${PAPERRUST_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/paperrust.exe}"
PRA="${PAPERRUST_BIN_A:-$PR}"
psql_su() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q "$@"; }
unset PAPERLESS_REDIS
declare -A PORT=([a]=58411 [b]=58412)
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done; }
trap cleanup EXIT
for side in a b; do
  db="pl_w7_par_$side"; w="$WORKROOT/par_$side"
  psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"
  psql_su -c "CREATE DATABASE $db OWNER paperless TEMPLATE paperless_snapshot"
  rm -rf "$w"; mkdir -p "$w/consume" "$w/logs"; tar -xf "$REF_DIR/snapshot/data.tar" -C "$w"; mkdir -p "$w/data" "$w/media"
  export PAPERLESS_DBNAME="$db" PAPERLESS_DATA_DIR="$(cygpath -w "$w/data")" PAPERLESS_MEDIA_ROOT="$(cygpath -w "$w/media")"
  export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$w/consume")" PAPERLESS_LOGGING_DIR="$(cygpath -w "$w/logs")"
  export PAPERRUST_DATAMODEL_BACKUP=off
  if [[ $side == a ]]; then
    # Mark stage N and later as applied → they stay out
    "$PRA" manage migrate --no-input >/dev/null 2>&1 || true   # creates paperrust_migrations (all stages)
    "$PRA" manage datamodel rollback --to $((N - 1)) >"$w/rollback.log" 2>&1 || { echo "rollback A failed"; tail -5 "$w/rollback.log"; exit 2; }
  else
    "$PR" manage migrate --no-input >"$w/migrate.log" 2>&1 || { echo "migrate B failed"; tail -5 "$w/migrate.log"; exit 2; }
  fi
  X="$PR"; [[ $side == a ]] && X="$PRA"
  "$X" manage datamodel status 2>/dev/null | tail -n +2 | sed "s/^/[$side] /"
  PAPERRUST_BIND="127.0.0.1:${PORT[$side]}" PAPERRUST_ROUTES="$(cygpath -w "$ROOT/results/parity/routes-all.toml")" \
    PAPERRUST_BRANDING=off PAPERRUST_UPDATE_CHECK=off PAPERRUST_LOG=warn NO_COLOR=1 \
    "$X" serve >"$w/serve.log" 2>&1 &
  pids+=($!)
done
for side in a b; do
  for _ in $(seq 1 120); do curl -sf "http://127.0.0.1:${PORT[$side]}/api/" -o /dev/null -H "Authorization: Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001" && break; sleep 1; done
done
"$PY" -I "$ROOT/tests/parity/harness/run_parity.py" --a-url "http://127.0.0.1:${PORT[a]}" --b-url "http://127.0.0.1:${PORT[b]}" \
  --no-write --results-dir "$(cygpath -w "$WORKROOT/par_results_$N")" --users-file "$(cygpath -w "$REFD/users.json")" "$@"
rc=$?
if [[ "${KEEP:-0}" != 1 ]]; then
  cleanup; sleep 2
  for side in a b; do psql_su -c "DROP DATABASE IF EXISTS pl_w7_par_$side WITH (FORCE)"; rm -rf "$WORKROOT/par_$side"; done
fi
exit $rc
