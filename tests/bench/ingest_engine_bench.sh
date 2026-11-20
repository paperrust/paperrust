#!/usr/bin/env bash
# A/B measurement of the Tesseract paths (pl-ocr): variants alternating per run (noise of the machine
# hits all equally), jobs=1 and jobs=12.
#   tests/bench/ingest_engine_bench.sh [runs] [baseline-binary]
# Variants: lib (libtesseract in process, default if found), cli-tiff (PAPERRUST_TESSERACT_LIB=off),
# optionally baseline (older ingest_bench binary, e.g. state 483d829: program + PNM).
# Output: results/bench/ingest/runs-engine.jsonl (appended, field "variant").
set -euo pipefail
cd "$(dirname "$0")/../.."
RUNS=${1:-3}; BASE=${2:-}
ROOT=$PWD
export PAPERRUST_PDFIUM_PATH=${PAPERRUST_PDFIUM_PATH:-$ROOT/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll}
export PAPERRUST_TESSERACT_PATH=${PAPERRUST_TESSERACT_PATH:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe}
export PAPERRUST_TESSDATA_DIR=${PAPERRUST_TESSDATA_DIR:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/tessdata}
OUT=results/bench/ingest; INPUT=$OUT/scan-16p.pdf
[ -f "$INPUT" ] || { echo "missing $INPUT – run tests/bench/ingest_bench.sh first"; exit 2; }
cargo build -p pl-ocr --release --example ingest_bench -j 4
BIN=target/release/examples/ingest_bench
tag() { sed "s/^{/{\"variant\":\"$1\",/"; }
for j in 1 12; do
  for r in $(seq 1 "$RUNS"); do
    if [ -n "$BASE" ]; then "$BASE" run "$INPUT" "$j" 1 | tag baseline | tee -a "$OUT/runs-engine.jsonl"; fi
    PAPERRUST_TESSERACT_LIB=off "$BIN" run "$INPUT" "$j" 1 | tag cli-tiff | tee -a "$OUT/runs-engine.jsonl"
    # lib: two runs in the same process – run 0 cold (instances are initialized), run 1 warm (pool
    # as in the server across documents)
    "$BIN" run "$INPUT" "$j" 2 | tag lib | tee -a "$OUT/runs-engine.jsonl"
  done
done
