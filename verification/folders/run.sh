#!/usr/bin/env bash
# Model-check the folder-view TLA+ models (Git Bash / Linux / macOS).
#
#   bash verification/folders/run.sh          # all checks
#   bash verification/folders/run.sh --deep   # plus 3 concurrent requests on tree and documents
#   bash verification/folders/run.sh --trace Upload_code   # print one full trace
#
# Every check has an expected result: the *_code configs must pass (except the reported finding
# Upload_code, which must fail with NoIngestFkError until the code is fixed), every "Broken_*" /
# "Names_broken_*" config must fail with the named invariant (this proves the property can fail).
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

# spec | config | expected ("pass" or the invariant that must be violated)
checks=(
  "FolderTree  Tree_code                     pass"
  "FolderTree  Docs_code                     pass"
  "FolderTree  Upload_code                   pass"
  "FolderTree  Upload_fix_keyshare           pass"
  "FolderTree  Broken_upload_no_keyshare     NoIngestFkError"
  "FolderTree  Broken_no_tree_lock           Acyclic"
  "FolderTree  Broken_no_tree_lock_deadlock  NoRawError"
  "FolderTree  Broken_check_before_lock      Acyclic"
  "FolderTree  Broken_repeatable_read        Acyclic"
  "FolderTree  Broken_delete_no_for_update   DeleteHonorsContents"
  "FolderTree  Broken_delete_no_locks        NoRawError"
  "FolderNames Names_code                    pass"
  "FolderNames Names_broken_no_index         UniqueSiblingNames"
)

tlc() { # spec cfg
  java "${JAVA_OPTS[@]}" -cp "$jar" tlc2.TLC -workers "$WORKERS" -metadir "$meta/$2" -config "$2.cfg" "$1.tla" 2>&1
}

if [[ "${1:-}" == "--trace" ]]; then
  cfg="${2:?config name, e.g. Upload_code}"
  spec="FolderTree"; [[ "$cfg" == Names_* ]] && spec="FolderNames"
  tlc "$spec" "$cfg" || true
  exit 0
fi
if [[ "${1:-}" == "--deep" ]]; then
  checks+=("FolderTree  Deep_tree_code                pass")
  checks+=("FolderTree  Deep_docs_code                pass")
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
    tag=""; [[ "$cfg" == *_code && "$expect" != "pass" ]] && tag="  <- known finding (README \"Findings\")"
    printf 'ok    %-28s %-20s (%s states)%s\n' "$cfg" "$got" "${states:-?}" "$tag"
  else
    printf 'FAIL  %-28s got %-16s expected %s\n' "$cfg" "$got" "$expect"
    fail=1
    if [[ "$got" == "TLC-ERROR" ]]; then echo "$out" | tail -20; fi
  fi
done

if [[ $fail -ne 0 ]]; then
  echo "Some checks did not behave as expected. See the skill .claude/skills/verify-folders/SKILL.md (\"Reading a result\")."
  echo "Full trace: bash verification/folders/run.sh --trace <config>"
  exit 1
fi
echo "All checks behaved as expected (TLA+ tools v${TLA_VERSION})."
