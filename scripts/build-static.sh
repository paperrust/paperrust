#!/usr/bin/env bash
# STATIC_ROOT for PaperRust (PAPERLESS_STATICDIR), without Angular: since 2026-11-13 PaperRust serves only
# its own UI (frontend-light → <STATICDIR>/../light, /rs/app/). What server pages need stays under /static/:
#   base.css, bootstrap.min.css   login/account pages (upstream/src/documents/static, color PaperRust)
#   paperless/img/favicon.ico     favicon of the account pages without branding layer
#   drf_spectacular_sidecar/, rest_framework/   Swagger UI (/api/schema/view/) and DRF pages
#                                 (from EXTRA_STATIC, see scripts/fetch-extra-static.sh)
# Plus .br/.gz like whitenoise (scripts/compress-static.mjs, needs node; without node uncompressed).
#
#   bash scripts/build-static.sh [target]      Default: results/static
#   EXTRA_STATIC=<folder|none>                 Default: reference/data/static (if present)
#   UPSTREAM_DIR=<folder>                      Default: upstream (submodule)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
OUT="${1:-${STATIC_OUT:-$REPO/results/static}}"
log() { echo "[static] $*"; }
die() { echo "[static] FEHLER: $*" >&2; exit 1; }

SRC="${UPSTREAM_DIR:-$REPO/upstream}/src/documents/static"
[[ -f "$SRC/base.css" ]] || die "$SRC fehlt (git submodule update --init)"

rm -rf "$OUT"; mkdir -p "$OUT/paperless/img"
cp "$SRC/"* "$OUT/"
cp "$HERE/static-assets/favicon.ico" "$OUT/paperless/img/favicon.ico"
# upstream green → PaperRust rust (docs/BRANDING.md)
sed -i -e 's/23, 84, 31; \/\* #17541f \*\//183, 65, 14; \/* #B7410E *\//' -e 's/15, 56, 20;/128, 45, 10;/' \
  -e 's/#17541f/#B7410E/g' -e 's/#0f3614/#a1390c/g' -e 's/#0c2c10/#8e320b/g' -e 's/#09220d/#772a09/g' "$OUT/base.css"
grep -qi "17541f\|0f3614" "$OUT/base.css" && die "base.css: upstream-Grün nicht vollständig ersetzt"

EXTRA="${EXTRA_STATIC:-$REPO/reference/data/static}"
if [[ "$EXTRA" != none && -d "$EXTRA" ]]; then
  for n in ${EXTRA_STATIC_DIRS:-drf_spectacular_sidecar rest_framework}; do
    [[ -d "$EXTRA/$n" ]] && cp -r "$EXTRA/$n" "$OUT/$n"
  done
  log "Drittanbieter-Statics aus $EXTRA übernommen"
else
  log "ohne Swagger-/DRF-Statics (EXTRA_STATIC fehlt; scripts/fetch-extra-static.sh <ordner>)"
fi

if command -v node >/dev/null 2>&1; then
  node "$HERE/compress-static.mjs" "$OUT"
else
  log "node fehlt – keine Vorkomprimate (pl-server liefert dann unkomprimiert)"
fi
log "fertig: $OUT  ->  PAPERLESS_STATICDIR=$OUT"
