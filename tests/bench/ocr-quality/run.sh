#!/usr/bin/env bash
# OCR quality Tesseract vs. Windows OCR (pl-ocr, PAPERRUST_OCR_ENGINE).
#   tests/bench/ocr-quality/run.sh [runs] [languages] [jobs]
# 1. Generate corpus (synthetic, fixed seed; clean + scan-like, image PDFs) -> results/bench/ocr-quality/corpus/
# 2. Single pages: ocr_quality images -> results/bench/ocr-quality/run-<langs>/
# 3. Ingestion path: ocr_quality ingest per image PDF and engine -> run-<langs>/ingest/
# 4. score.py -> run-<langs>/table.md
# 5. Real scans (optional, git-ignored): tests/bench/ocr-quality/real/*.pdf -> real/out/<pdf>/
#    (text, page images, side-by-side.html stay there), real_report.py -> run-<langs>/real.md (numbers only)
# A debug build is enough: the time is spent in Tesseract or WinRT, not in the Rust code.
set -euo pipefail
cd "$(dirname "$0")/../../.."
RUNS=${1:-3}; LANGS=${2:-deu+eng}; JOBS=${3:-4}
ROOT=$PWD
PY=${PYTHON:-$ROOT/reference/venv/Scripts/python.exe}
[ -x "$PY" ] || PY=python3
export PAPERRUST_TESSERACT_PATH=${PAPERRUST_TESSERACT_PATH:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/bin/tesseract.exe}
export PAPERRUST_TESSDATA_DIR=${PAPERRUST_TESSDATA_DIR:-$ROOT/tests/parity/pl-ocr/.tools/tesseract/tessdata}
export PAPERRUST_PDFIUM_PATH=${PAPERRUST_PDFIUM_PATH:-$ROOT/reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll}
ENGINES=(tesseract windows)
OUT=results/bench/ocr-quality
BIN=target/debug/examples/ocr_quality
"$PY" -I -X utf8 tests/bench/ocr-quality/make_corpus.py "$OUT/corpus"
cargo build -p pl-ocr -j 2 --example ocr_quality
RUN="$OUT/run-${LANGS//+/-}"
rm -rf "$RUN"; mkdir -p "$RUN/ingest"
"$BIN" images "$OUT/corpus" "$RUN" "$RUNS" "$LANGS" "${ENGINES[@]}"
for pdf in "$OUT"/corpus/*.pdf; do
  name=$(basename "$pdf" .pdf)
  for e in "${ENGINES[@]}"; do
    "$BIN" ingest "$pdf" "$RUN/ingest/$name" "$LANGS" "$JOBS" "$e" | tee -a "$RUN/ingest.jsonl"
  done
done
"$PY" -I -X utf8 tests/bench/ocr-quality/score.py "$OUT/corpus" "$RUN" | tee "$RUN/table.md"

REAL=tests/bench/ocr-quality/real
shopt -s nullglob
reals=("$REAL"/*.pdf)
if [ ${#reals[@]} -gt 0 ]; then
  for pdf in "${reals[@]}"; do
    name=$(basename "$pdf" .pdf)
    d="$REAL/out/$name"
    rm -rf "$d"; mkdir -p "$d"
    "$BIN" pages "$pdf" "$d" 300 "$LANGS" "${ENGINES[@]}"
    for e in "${ENGINES[@]}"; do
      "$BIN" ingest "$pdf" "$d" "$LANGS" "$JOBS" "$e" | tee -a "$d/ingest.jsonl"
    done
  done
  "$PY" -I -X utf8 tests/bench/ocr-quality/real_report.py "$REAL/out" | tee "$RUN/real.md"
fi
