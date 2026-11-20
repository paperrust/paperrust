#!/usr/bin/env bash
# Release gates for paperless-rs (non-interactive).
#
#   scripts/run-gates.sh                 # all gates
#   scripts/run-gates.sh --only fmt,clippy
#   scripts/run-gates.sh --skip semgrep
#   GATES_OFFLINE=1 scripts/run-gates.sh # no advisory-db / registry fetches
#   GATES_PACKAGES="-p pl-core -p pl-authz" scripts/run-gates.sh --only clippy
#
# Gates (in order): gitleaks, fmt, clippy, audit, deny, machete, semgrep
#
# Exit codes: 0 all green
#             1 at least one gate red (takes precedence)
#             2 no gate red, but at least one tool missing (gate not verified)
# Tools are never installed by this script; missing ones are listed (install hints in
# docs/contract/testing.md, "Release-Gates").
#
# Logs + summary: results/gates/<stamp>/<gate>.log, results/gates/<stamp>/summary.md,
# results/gates/latest.md, results/gates/history.jsonl
#
# Resource note (shared 15 GB laptop): clippy builds the workspace with -j ${GATES_JOBS:-2}
# in the shared CARGO_TARGET_DIR. Run it only when the coordinator schedules the gates.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"
JOBS="${GATES_JOBS:-2}"
PKGS="${GATES_PACKAGES:---workspace}"
OFFLINE="${GATES_OFFLINE:-}"
ALL_GATES="gitleaks fmt clippy audit deny machete semgrep"
ONLY=""
SKIP=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --only) ONLY="${2//,/ }"; shift 2 ;;
    --skip) SKIP="${2//,/ }"; shift 2 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

STAMP="$(date +%Y-%m-%d_%H%M%S)"
OUT="$REPO/results/gates/$STAMP"
mkdir -p "$OUT"
declare -A RESULT DETAIL
ORDER=()

want() {
  local g="$1"
  if [[ -n "$ONLY" && " $ONLY " != *" $g "* ]]; then return 1; fi
  if [[ " $SKIP " == *" $g "* ]]; then return 1; fi
  return 0
}

have() { command -v "$1" >/dev/null 2>&1; }
have_cargo_sub() { cargo "$1" --version >/dev/null 2>&1; }

record() { # gate status detail
  ORDER+=("$1"); RESULT[$1]="$2"; DETAIL[$1]="$3"
  printf '%-9s %-8s %s\n' "$1" "$2" "$3"
}

run_gate() { # gate cmd...
  local g="$1"; shift
  local log="$OUT/$g.log"
  local t0=$SECONDS
  echo "\$ $*" >"$log"
  "$@" >>"$log" 2>&1
  local rc=$?
  local dt=$((SECONDS - t0))
  if [[ $rc -eq 0 ]]; then
    record "$g" green "${dt}s"
  else
    record "$g" red "exit $rc, ${dt}s, see results/gates/$STAMP/$g.log: $(grep -m1 -iE 'error|leak|warning|fail' "$log" | cut -c1-140)"
  fi
}

missing() { record "$1" missing "$2"; }

# --- gitleaks ---------------------------------------------------------------
if want gitleaks; then
  if have gitleaks; then
    CFG="$REPO/tests/security/gitleaks.toml"
    if gitleaks dir --help >/dev/null 2>&1; then
      run_gate gitleaks gitleaks dir "$REPO" --config "$CFG" --redact --no-banner --exit-code 1
    else
      run_gate gitleaks gitleaks detect --source "$REPO" --no-git --config "$CFG" --redact --no-banner --exit-code 1
    fi
  else
    missing gitleaks "gitleaks not on PATH"
  fi
fi

# --- rustfmt ----------------------------------------------------------------
if want fmt; then
  if have_cargo_sub fmt; then run_gate fmt cargo fmt --all -- --check
  else missing fmt "cargo fmt (rustfmt component) missing"; fi
fi

# --- clippy -----------------------------------------------------------------
if want clippy; then
  if have_cargo_sub clippy; then
    # shellcheck disable=SC2086
    run_gate clippy cargo clippy $PKGS --all-targets -j "$JOBS" -- -D warnings
  else missing clippy "cargo clippy component missing"; fi
fi

# --- cargo audit --------------------------------------------------------------
if want audit; then
  if have_cargo_sub audit; then
    if [[ -n "$OFFLINE" ]]; then run_gate audit cargo audit --no-fetch --deny warnings
    else run_gate audit cargo audit --deny warnings; fi
  else missing audit "cargo-audit not installed"; fi
fi

# --- cargo deny ---------------------------------------------------------------
if want deny; then
  if have_cargo_sub deny; then
    if [[ -n "$OFFLINE" ]]; then run_gate deny cargo deny --offline check
    else run_gate deny cargo deny check; fi
  else missing deny "cargo-deny not installed"; fi
fi

# --- cargo machete --------------------------------------------------------------
if want machete; then
  if have_cargo_sub machete || have cargo-machete; then run_gate machete cargo machete "$REPO/crates"
  else missing machete "cargo-machete not installed"; fi
fi

# --- semgrep ------------------------------------------------------------------
if want semgrep; then
  if have semgrep; then
    run_gate semgrep semgrep scan --config p/rust --config p/secrets --config p/owasp-top-ten \
      --error --metrics=off --exclude upstream --exclude target --exclude reference --exclude tests/.venv \
      --exclude results "$REPO/crates" "$REPO/tests" "$REPO/scripts"
  else
    missing semgrep "semgrep not on PATH"
  fi
fi

# --- summary ------------------------------------------------------------------
red=0; miss=0; green=0
{
  echo "# Release-Gates $STAMP"
  echo
  echo "| Gate | Ergebnis | Detail |"
  echo "|---|---|---|"
  for g in "${ORDER[@]}"; do
    echo "| $g | ${RESULT[$g]} | ${DETAIL[$g]//|/\\|} |"
  done
} >"$OUT/summary.md"
for g in "${ORDER[@]}"; do
  case "${RESULT[$g]}" in red) red=$((red+1));; missing) miss=$((miss+1));; green) green=$((green+1));; esac
done
cp "$OUT/summary.md" "$REPO/results/gates/latest.md"
printf '{"stamp":"%s","green":%d,"red":%d,"missing":%d}\n' "$STAMP" "$green" "$red" "$miss" >>"$REPO/results/gates/history.jsonl"
echo "summary: results/gates/$STAMP/summary.md (green=$green red=$red missing=$miss)"
if [[ $red -gt 0 ]]; then exit 1; fi
if [[ $miss -gt 0 ]]; then exit 2; fi
exit 0
