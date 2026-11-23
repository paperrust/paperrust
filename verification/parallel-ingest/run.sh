#!/usr/bin/env bash
# Model-check the parallel-ingest TLA+ models (Git Bash / Linux / macOS).
#
#   bash verification/parallel-ingest/run.sh          # all checks, ~1 min
#   bash verification/parallel-ingest/run.sh --deep   # plus 4 concurrent tasks, ~+11 min (44 million states)
#   bash verification/parallel-ingest/run.sh --trace Broken_no_asn_lock   # print one full trace
#
# Every check has an expected result: the configs that describe the code must pass, every
# "Broken_*"/"Tag_broken_*"/"Scope_*" config must fail with the named invariant (this proves the
# property can fail). Exit code 0 only if every check behaves as expected.
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

# spec | config | expected ("pass" or the invariant that must be violated)
checks=(
  "ParallelIngest Ingest_code                 pass"
  "ParallelIngest Ingest_code_keepdups        pass"
  "ParallelIngest Broken_no_asn_lock          NoRawUniqueError"
  "ParallelIngest Broken_no_sum_lock          NoDuplicateStored"
  "ParallelIngest Broken_shrink_no_source     NoDuplicateStored"
  "ParallelIngest Broken_lock_after_find      NoRawUniqueError"
  "ParallelIngest Broken_lock_after_find_dups NoDuplicateStored"
  "ParallelIngest Broken_repeatable_read      NoRawUniqueError"
  "ParallelIngest Broken_repeatable_read_dups NoDuplicateStored"
  "ParallelIngest Broken_recheck_after_insert RejectionJustified"
  "ParallelIngest Broken_no_lock_no_index     UniqueAsn"
  "ParallelIngest Scope_external_api_writer   NoRawUniqueError"
  "TagGetOrCreate Tag_code                    pass"
  "TagGetOrCreate Tag_broken_no_lock_case     OneTagPerName"
  "TagGetOrCreate Tag_broken_no_lock_same     NoRawUniqueError"
  "TagGetOrCreate Tag_broken_no_lock_ids      SameTagForSameName"
  "TagGetOrCreate Tag_broken_repeatable_read  NoRawUniqueError"
)

tlc() { # spec cfg
  java "${JAVA_OPTS[@]}" -cp "$jar" tlc2.TLC -workers "$WORKERS" -metadir "$meta/$2" -config "$2.cfg" "$1.tla" 2>&1
}

if [[ "${1:-}" == "--trace" ]]; then
  cfg="${2:?config name, e.g. Broken_no_asn_lock}"
  spec="ParallelIngest"; [[ "$cfg" == Tag_* ]] && spec="TagGetOrCreate"
  tlc "$spec" "$cfg" || true
  exit 0
fi
if [[ "${1:-}" == "--deep" ]]; then
  checks+=("ParallelIngest Deep_ingest_code            pass")
fi

fail=0
for line in "${checks[@]}"; do
  read -r spec cfg expect <<<"$line"
  out="$(tlc "$spec" "$cfg" || true)"
  if grep -q "Model checking completed. No error has been found." <<<"$out"; then
    got="pass"
  elif v="$(grep -oE 'Invariant [A-Za-z]+ is violated' <<<"$out" | head -1)"; [[ -n "$v" ]]; then
    got="$(awk '{print $2}' <<<"$v")"
  elif grep -q "Deadlock reached" <<<"$out"; then
    got="DEADLOCK"
  else
    got="TLC-ERROR"
  fi
  states="$(grep -oE '[0-9]+ distinct states found' <<<"$out" | tail -1 | cut -d' ' -f1)"
  if [[ "$got" == "$expect" ]]; then
    printf 'ok    %-28s %-20s (%s states)\n' "$cfg" "$got" "${states:-?}"
  else
    printf 'FAIL  %-28s got %-16s expected %s\n' "$cfg" "$got" "$expect"
    fail=1
    if [[ "$got" == "TLC-ERROR" ]]; then echo "$out" | tail -20; fi
  fi
done

if [[ $fail -ne 0 ]]; then
  echo "Some checks did not behave as expected. See the skill .claude/skills/verify-parallel-ingest/SKILL.md (\"Reading a result\")."
  echo "Full trace: bash verification/parallel-ingest/run.sh --trace <config>"
  exit 1
fi
echo "All checks behaved as expected (TLA+ tools v${TLA_VERSION})."
