#!/usr/bin/env bash
# Model-check the task-queue TLA+ models (Git Bash / Linux / macOS).
#
#   bash verification/task-queue/run.sh                 # all checks, ~1-2 min
#   bash verification/task-queue/run.sh --deep          # plus deeper bounds, before a release
#   bash verification/task-queue/run.sh --trace Broken_fence_only_after_heartbeat   # one full trace
#
# Every check has an expected result: configs that describe the code (`*_code*`) must pass,
# `Broken_*` configs must fail with the named property (proves the property can fail; some are the
# old code before the fixes of findings F1-F3). A new open finding gets a `Finding_*` config that
# must fail until the code is fixed (then it becomes a `*_code` config, see the skill
# .claude/skills/verify-task-queue/SKILL.md).
# Exit code 0 only if every check behaves as expected.
#
# Needs Java 11+ on PATH. Fetches the pinned TLA+ tools jar into ./tools (gitignored) and
# verifies its SHA-256. Heap and workers are kept small for a shared laptop.
set -euo pipefail

TLA_VERSION="1.7.4"
TLA_SHA256="936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
TLA_URL="https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
JAVA_OPTS=(-Xmx2g -XX:+UseParallelGC)
WORKERS=2

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"
jar="tools/tla2tools-${TLA_VERSION}.jar"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

if [[ ! -f "$jar" ]]; then
  mkdir -p tools
  echo "Fetching TLA+ tools v${TLA_VERSION} ..."
  curl -fsSL -o "$jar.part" "$TLA_URL"
  mv "$jar.part" "$jar"
fi
got="$(sha256 "$jar")"
if [[ "$got" != "$TLA_SHA256" ]]; then
  echo "SHA-256 mismatch for $jar: $got (expected $TLA_SHA256). Delete it and retry." >&2
  exit 2
fi

meta="$(mktemp -d)"
trap 'rm -rf "$meta"' EXIT

# spec | config | expected ("pass", the invariant that must be violated, or "Liveness")
checks=(
  "QueueLease     Lease_code_hang                    pass"
  "QueueLease     Lease_code_fast_fail               pass"
  "QueueLease     Lease_code_fast_fail_no_skew       pass"
  "QueueLease     Broken_fence_only_after_heartbeat  ExclusiveExecution"
  "QueueLease     Broken_no_self_fence               ExclusiveExecution"
  "QueueLease     Broken_claim_no_row_lock           ExclusiveExecution"
  "QueueLease     Broken_skew_beyond_margin          ExclusiveExecution"
  "QueueLifecycle Life_code                          pass"
  "QueueLifecycle Life_code_same_process             pass"
  "QueueLifecycle Coalesce_code                      pass"
  "QueueLifecycle Life_code_unbounded_prestart       pass"
  "QueueLifecycle Life_code_same_process_unbounded   pass"
  "QueueLifecycle Coalesce_code_unbounded            pass"
  "QueueLifecycle Broken_unfenced_mark_started       StatusConsistent"
  "QueueLifecycle Broken_attempts_token_coalesce     NoLostMerge"
  "QueueLifecycle Broken_old_token_same_process      StatusConsistent"
  "QueueLifecycle Broken_finish_no_fence             TerminalOnce"
  "QueueLifecycle Broken_hb_no_fence                 StatusConsistent"
  "QueueLifecycle Broken_reaper_no_fail              Liveness"
  "BeatReserve    Beat_code                          pass"
  "BeatReserve    Beat_code_fallback                 pass"
  "BeatReserve    Beat_fallback_crash_repeats        NoSlotTwice"
  "BeatReserve    Beat_broken_no_for_update          NoSlotTwice"
  "BeatReserve    Beat_broken_record_before_enqueue  NoLostSlot"
)
deep=(
  "QueueLease     Deep_lease_code_hang             pass"
  "QueueLifecycle Deep_life_code                   pass"
)

tlc() { # spec cfg
  java "${JAVA_OPTS[@]}" -cp "$jar" tlc2.TLC -workers "$WORKERS" -metadir "$meta/$2" -config "$2.cfg" "$1.tla" 2>&1
}

if [[ "${1:-}" == "--trace" ]]; then
  cfg="${2:?config name, e.g. Broken_fence_only_after_heartbeat}"
  spec=""
  for line in "${checks[@]}" "${deep[@]}"; do
    read -r s c _ <<<"$line"
    [[ "$c" == "$cfg" ]] && spec="$s"
  done
  [[ -n "$spec" ]] || { echo "unknown config $cfg" >&2; exit 2; }
  tlc "$spec" "$cfg" || true
  exit 0
fi
if [[ "${1:-}" == "--deep" ]]; then
  checks+=("${deep[@]}")
fi

fail=0
for line in "${checks[@]}"; do
  read -r spec cfg expect <<<"$line"
  out="$(tlc "$spec" "$cfg" || true)"
  if grep -q "Model checking completed. No error has been found." <<<"$out"; then
    got="pass"
  elif v="$(grep -oE 'Invariant [A-Za-z]+ is violated' <<<"$out" | head -1)"; [[ -n "$v" ]]; then
    got="$(awk '{print $2}' <<<"$v")"
  elif grep -q "Temporal properties were violated" <<<"$out"; then
    got="Liveness"
  elif grep -q "Deadlock reached" <<<"$out"; then
    got="DEADLOCK"
  else
    got="TLC-ERROR"
  fi
  states="$(grep -oE '[0-9]+ distinct states found' <<<"$out" | tail -1 | cut -d' ' -f1)"
  if [[ "$got" == "$expect" ]]; then
    printf 'ok    %-34s %-20s (%s states)\n' "$cfg" "$got" "${states:-?}"
  else
    printf 'FAIL  %-34s got %-16s expected %s\n' "$cfg" "$got" "$expect"
    fail=1
    if [[ "$got" == "TLC-ERROR" ]]; then echo "$out" | tail -20; fi
  fi
done

if [[ $fail -ne 0 ]]; then
  echo "Some checks did not behave as expected. See the skill .claude/skills/verify-task-queue/SKILL.md (\"Reading a result\")."
  echo "Full trace: bash verification/task-queue/run.sh --trace <config>"
  exit 1
fi
echo "All checks behaved as expected (TLA+ tools v${TLA_VERSION})."
