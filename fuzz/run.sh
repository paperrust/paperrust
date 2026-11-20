#!/usr/bin/env bash
# Fuzz run with low priority, at most 2 cores (2 lanes of targets running one after the other).
#   fuzz/run.sh <seconds-per-target> [target ...]     (default: all targets, 1800 s per target)
# Build beforehand: CARGO_TARGET_DIR=…/target CARGO_BUILD_JOBS=2 cargo +nightly-2026-10-25 fuzz build -s none -a
# Result: results/release/security/fuzz/<target>.log, crashes/hangs under fuzz/artifacts/<target>/.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
SECS="${1:-1800}"; shift || true
TARGETS=("$@")
[[ ${#TARGETS[@]} -eq 0 ]] && TARGETS=(pdf_text office mail image zip_import docfilter templating)
BIN="${FUZZ_BIN:-$REPO/target/x86_64-pc-windows-msvc/release}"
OUT="$REPO/results/release/security/fuzz"
mkdir -p "$OUT"

lane() {
  for t in "$@"; do
    mkdir -p "$HERE/corpus/$t" "$HERE/artifacts/$t"
    echo "[fuzz] $t start $(date +%H:%M:%S) ${SECS}s" >>"$OUT/run.log"
    # Windows priority "Low" (BelowNormal/Idle via PowerShell); one process = one core
    local exe args
    exe=$(cygpath -w "$BIN/$t.exe")
    args="-max_total_time=$SECS -rss_limit_mb=2048 -malloc_limit_mb=2048 -timeout=30 -print_final_stats=1 -artifact_prefix=$(cygpath -w "$HERE/artifacts/$t")\ $(cygpath -w "$HERE/corpus/$t")"
    powershell.exe -NoProfile -NonInteractive -Command       "\$p = Start-Process -FilePath '$exe' -ArgumentList '$args' -NoNewWindow -PassThru -RedirectStandardError '$(cygpath -w "$OUT/$t.log")' -RedirectStandardOutput '$(cygpath -w "$OUT/$t.out.log")'; \$p.PriorityClass = 'Idle'; \$p.WaitForExit(); exit \$p.ExitCode" </dev/null
    echo "[fuzz] $t ende $(date +%H:%M:%S) exit $?" >>"$OUT/run.log"
  done
}

half=$(( (${#TARGETS[@]} + 1) / 2 ))
lane "${TARGETS[@]:0:$half}" &
lane "${TARGETS[@]:$half}" &
wait
echo "[fuzz] fertig $(date +%H:%M:%S)" >>"$OUT/run.log"
