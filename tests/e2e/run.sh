#!/usr/bin/env bash
# Complete browser E2E round:
#   1. Reset (DB pl_e2e, data folder), start Django :8170, click paths against A
#   2. Reset, start PaperRust :8171 (branding off, Django fallback :8170), same click paths against B
#   3. Comparison -> results/e2e/<date>/report.md (+ A/, B/, diff/)
#   4. Reset, PaperRust with branding (default) -> results/e2e/<date>/branding/*.png
#   5. end own processes by PID (DB pl_e2e stays)
# Options via environment: E2E_DATE (folder name), E2E_UPLOAD_RUST=0 (upload route not on rust),
# E2E_KEEP=1 (leave servers running at the end).
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
cd "$E2E_DIR"
export PLAYWRIGHT_BROWSERS_PATH="$E2E_DIR/.browsers"
DATE="${E2E_DATE:-$(date +%F)}"
REP="$REPO/results/e2e/$DATE"
mkdir -p "$REP"
T0=$(date +%s)
log() { echo "[e2e $(date +%T)] $*"; }
ensure_django() { # Django must run for A and as fallback for B; restart on failure
  if ! curl -s -o /dev/null -m 5 "http://127.0.0.1:$E2E_A_PORT/api/"; then log "Django :$E2E_A_PORT antwortet nicht -> Neustart"; ./stop.sh django; ./start-django.sh || exit 1; fi; }
warm() { # after a reset: Django rebuilds DB connections; wait up to 200
  for _ in $(seq 1 30); do
    c=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001" "http://127.0.0.1:$E2E_A_PORT/api/documents/?page_size=1")
    [[ "$c" == 200 ]] && return 0; sleep 1; done; log "Warnung: Django nach Reset nicht bereit"; }
trap './stop.sh rust >/dev/null; [[ "${E2E_KEEP:-0}" == 1 ]] || ./stop.sh django' EXIT

./stop.sh all
log "Reset + Django (A)"; ./reset.sh && ./start-django.sh || exit 1; warm
rm -f "$LOGS/watchdog.log"
E2E_PHASE=A ./watchdog.sh & WD=$!
tA=$(date +%s)
node e2e.mjs --side A --base "http://127.0.0.1:$E2E_A_PORT" --out "$WORK/A" --upload 0 | tee "$WORK/A.log"
tA=$(( $(date +%s) - tA ))
touch "$RUN/watchdog.stop"; wait $WD

log "Reset + PaperRust (B, Branding aus)"; ./stop.sh django; ./reset.sh; ./start-django.sh || exit 1; warm
./start-rust.sh off || exit 1
UP=0; grep -q '"documents.post_document" = "rust"' "$WORK/routes.toml" && UP=1
ensure_django
E2E_PHASE=B ./watchdog.sh & WD=$!
tB=$(date +%s)
node e2e.mjs --side B --base "http://127.0.0.1:$E2E_B_PORT" --out "$WORK/B" --upload "$UP" | tee "$WORK/B.log"
tB=$(( $(date +%s) - tB ))
touch "$RUN/watchdog.stop"; wait $WD
./stop.sh rust
cp "$LOGS/rust-off.out.log" "$REP/rust-access-B.log" 2>/dev/null
cp "$WORK/routes.toml" "$REP/routes-B.toml"

log "Vergleich"
node compare.mjs --a "$WORK/A" --b "$WORK/B" --report "$REP" --rustlog "$LOGS/rust-off.out.log"

log "Reset + PaperRust (Branding an)"; ./stop.sh django; ./reset.sh; ./start-django.sh || exit 1; warm
./start-rust.sh on || exit 1
node e2e.mjs --side B-branding --base "http://127.0.0.1:$E2E_B_PORT" --out "$REP/branding" --flow branding | tee "$WORK/branding.log"
rm -f "$REP/branding/requests.json"
./stop.sh rust

T=$(( $(date +%s) - T0 ))
{ echo; echo "## Laufzeit"; echo; echo "- gesamt ${T} s (Klickwege A ${tA} s, B ${tB} s; Rest: Resets, Serverstarts, Vergleich, Branding-Lauf)"
  echo; echo "## Wächter (Django-Neustarts während der Klickwege)"; echo; if [[ -s "$LOGS/watchdog.log" ]]; then echo '```'; cat "$LOGS/watchdog.log"; echo '```'; else echo "- keine"; fi
  echo "- Upload-Schritt in B: $([[ $UP == 1 ]] && echo 'ausgeführt (documents.post_document = rust, PAPERRUST_QUEUE=postgres, eingebetteter Worker)' || echo 'übersprungen')"; } >> "$REP/report.md"
log "fertig in ${T} s -> $REP/report.md"
