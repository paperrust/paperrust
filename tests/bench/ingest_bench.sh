#!/usr/bin/env bash
# Ingest measurement (pl-ocr parse_document): scan-like 16-page PDF (300 dpi, grayscale, images only),
# generated from upstream example PDFs; runs with jobs=1 (serial behavior as before) and jobs=N.
#   tests/bench/ingest_bench.sh [pages] [runs] [jobs...]
# Results: results/bench/ingest/runs.jsonl (JSON per run), input.json
set -euo pipefail
cd "$(dirname "$0")/../.."
PAGES=${1:-16}; RUNS=${2:-3}
if [ $# -gt 2 ]; then JOBS=("${@:3}"); else JOBS=(1 4 8); fi
ROOT=$PWD
export PAPERRUST_PDFIUM_PATH=${PAPERRUST_PDFIUM_PATH:-$ROOT/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll}
export PAPERRUST_TESSERACT_PATH=${PAPERRUST_TESSERACT_PATH:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe}
export PAPERRUST_TESSDATA_DIR=${PAPERRUST_TESSDATA_DIR:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/tessdata}
OUT=results/bench/ingest; mkdir -p "$OUT"
cargo build -p pl-ocr --release --example ingest_bench
BIN=target/release/examples/ingest_bench
U=upstream/src
INPUT="$OUT/scan-${PAGES}p.pdf"
"$BIN" make "$INPUT" "$PAGES" 300 \
  $U/paperless_testing/sample_files/multi-page-digital.pdf \
  $U/paperless_testing/sample_files/simple-digital.pdf \
  $U/paperless/tests/samples/mail/html.eml.pdf \
  $U/paperless/tests/samples/mail/simple_text.eml.pdf \
  $U/documents/tests/samples/documents/originals/0000004.pdf \
  $U/documents/tests/samples/documents/originals/0000006.pdf > "$OUT/input.json"
cat "$OUT/input.json"
: > "$OUT/runs.jsonl"
: > "$OUT/peak_mem.txt"
for j in "${JOBS[@]}"; do
  STOP="$OUT/.stop"; rm -f "$STOP" "$OUT/.peak"
  powershell -NoProfile -ExecutionPolicy Bypass -File tests/bench/memsample.ps1 -Out "$OUT/.peak" -Stop "$STOP" &
  SPID=$!
  "$BIN" run "$INPUT" "$j" "$RUNS" | tee -a "$OUT/runs.jsonl"
  touch "$STOP"; wait "$SPID" || true
  echo "jobs=$j peak_mb=$(cat "$OUT/.peak" 2>/dev/null | tr -d '
 ')" | tee -a "$OUT/peak_mem.txt"
  rm -f "$STOP" "$OUT/.peak"
done
