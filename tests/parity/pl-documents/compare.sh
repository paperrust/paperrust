#!/usr/bin/env bash
# Parity comparison pl-documents: same request to Django and to PaperRust, compare responses
# (status, content type, JSON in key order). Read-only requests only.
#
#   DJANGO=http://127.0.0.1:8000 RUST=http://127.0.0.1:8080 TOKEN=<api-token> \
#     tests/parity/pl-documents/compare.sh [DOC_ID]
#
# Note: `next`/`previous` contain the host – both servers must be addressed with the same `Host`
# (header `HOST_HEADER`, default 127.0.0.1:8000).
set -u
DJANGO="${DJANGO:-http://127.0.0.1:8000}"
RUST="${RUST:-http://127.0.0.1:8080}"
TOKEN="${TOKEN:?TOKEN fehlt}"
HOST_HEADER="${HOST_HEADER:-127.0.0.1:8000}"
DOC="${1:-1}"
OUT="$(mktemp -d)"
PY="${PY:-python}"

paths=(
  "/api/documents/"
  "/api/documents/?page_size=3"
  "/api/documents/?page=2&page_size=2"
  "/api/documents/?page=999"
  "/api/documents/?fields=id,title,tags"
  "/api/documents/?fields=dgfhs"
  "/api/documents/?truncate_content=true"
  "/api/documents/?full_perms=true"
  "/api/documents/?ordering=title"
  "/api/documents/?ordering=-num_notes"
  "/api/documents/?ordering=correspondent__name"
  "/api/documents/?search=test"
  "/api/documents/?id__in=1,2,3"
  "/api/documents/?include_selection_data=true"
  "/api/documents/?tags__id__all=1&ordering=-custom_field_1"
  "/api/documents/?custom_field_query=%5B1%2C%22exists%22%2Ctrue%5D"
  "/api/documents/?query=invoice"
  "/api/documents/?text=invoice&ordering=title"
  "/api/documents/?more_like_id=${DOC}"
  "/api/trash/"
  "/api/documents/next_asn/"
  "/api/documents/${DOC}/"
  "/api/documents/${DOC}/?full_perms=true"
  "/api/documents/${DOC}/?version="
  "/api/documents/${DOC}/root/"
  "/api/documents/${DOC}/notes/"
  "/api/documents/${DOC}/share_links/"
  "/api/documents/${DOC}/history/"
  "/api/documents/999999/"
  "/api/documents/abc/"
)

fetch() { # base path accept outfile
  curl -s -o "$4.body" -w '%{http_code} %{content_type}' \
    -H "Authorization: Token $TOKEN" -H "Host: $HOST_HEADER" -H "Accept: $3" "$1$2" > "$4.meta"
}

same=0; diff_n=0
for accept in "application/json" "application/json; version=9"; do
  for p in "${paths[@]}"; do
    n=$((same + diff_n))
    fetch "$DJANGO" "$p" "$accept" "$OUT/d$n"
    fetch "$RUST" "$p" "$accept" "$OUT/r$n"
    if cmp -s "$OUT/d$n.meta" "$OUT/r$n.meta" && \
       "$PY" -c 'import json,sys
a,b=(open(f,"rb").read() for f in sys.argv[1:3])
try: sys.exit(0 if json.loads(a,object_pairs_hook=list)==json.loads(b,object_pairs_hook=list) else 1)
except ValueError: sys.exit(0 if a==b else 1)' "$OUT/d$n.body" "$OUT/r$n.body"; then
      same=$((same + 1))
    else
      diff_n=$((diff_n + 1))
      echo "DIFF [$accept] $p  django=$(cat "$OUT/d$n.meta") rust=$(cat "$OUT/r$n.meta")"
      echo "  django: $(head -c 400 "$OUT/d$n.body")"
      echo "  rust:   $(head -c 400 "$OUT/r$n.body")"
    fi
  done
done
echo "same=$same different=$diff_n (Rohdaten: $OUT)"
[ "$diff_n" -eq 0 ]
