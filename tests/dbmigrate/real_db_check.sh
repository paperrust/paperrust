#!/usr/bin/env bash
# Test of the switch assistant with a REAL paperless-ngx database (e.g. the NAS data of the
# owner) – only on his own machine, only with a restored copy.
#
# Privacy (strict):
#   - The backup is restored into a throwaway database; the real database is never
#     touched. Before the first run of the assistant, mail passwords, OAuth
#     and API tokens, sessions, password hashes, MFA secrets, webhook headers, AI/OCR
#     keys and share links are deleted or made unusable in this copy.
#   - On screen (and in zahlen.txt) there are ONLY numbers: version, counts per object type,
#     file findings as a count, duration. No titles, contents, names, file names.
#   - The complete reports (with titles and file names) stay in the private working folder
#     (default: .work/real, ignored by git) and must not be passed on.
#   - Files are only read (checksums); nothing is copied and nothing is written.
#
# Invocation (Git Bash):
#   bash tests/dbmigrate/real_db_check.sh <backup.pgdump|backup.sql> [<media-folder>]
#   Environment: PG_BIN (pg_restore/psql), PGHOST/PGPORT (default 127.0.0.1:54329, superuser
#   PG_SUPERUSER/PG_SUPERPASS like reference/env.sh), WORK_DIR, KEEP=1 (keep throwaway DBs),
#   BIN (paperrust-migrate-db, default: target/debug).
# Procedure: (1) restore to pl_mig_real_src, (2) delete secrets, (3) detect + dry run,
#   (4) switch to a NEW database pl_mig_real_dst (source stays), (5) switch in
#   place on a second copy pl_mig_real_inplace (like Docker "just swap the image", without
#   further backup – the copy already is one), (6) output numbers, (7) clean up.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN="$(cd "$HERE" && cd "$(git rev-parse --git-common-dir)/.." && pwd)"
[[ -f "$MAIN/reference/env.sh" ]] && source "$MAIN/reference/env.sh" >/dev/null
DUMP="${1:?Aufruf: real_db_check.sh <sicherung.pgdump|.sql> [<media-ordner>]}"
MEDIA="${2:-}"
# Without a media folder no file check: otherwise the tool would take the media folder of the
# reference environment (from env.sh) as target and report foreign files.
[[ -z "$MEDIA" ]] && unset PAPERLESS_MEDIA_ROOT PAPERRUST_MEDIA_ROOT PAPERLESS_RS_MEDIA_ROOT
WORK="${WORK_DIR:-$HERE/.work/real}"
BIN="${BIN:-${CARGO_TARGET_DIR:-$MAIN/target}/debug/paperrust-migrate-db.exe}"
[[ -x "$BIN" ]] || BIN="${BIN%.exe}"
PGHOST="${PGHOST:-127.0.0.1}"; PGPORT="${PGPORT:-54329}"
export PGPASSWORD="${PG_SUPERPASS:?PG_SUPERPASS fehlt}"
SU="${PG_SUPERUSER:-postgres}"
SRC=pl_mig_real_src; DST=pl_mig_real_dst; INP=pl_mig_real_inplace
su() { "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h "$PGHOST" -p "$PGPORT" -U "$SU" -d "${DB:-postgres}" "$@"; }
q() { DB=$1 su -tA -c "$2"; }
w() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
url() { echo "postgres://$SU:$PGPASSWORD@$PGHOST:$PGPORT/$1"; }
cleanup() {
  [[ "${KEEP:-0}" == 1 ]] && { echo "KEEP=1: Wegwerf-Datenbanken bleiben ($SRC, $DST, $INP …)"; return; }
  for d in $(q postgres "SELECT datname FROM pg_database WHERE datname LIKE 'pl\_mig\_real\_%'"); do
    su -c "DROP DATABASE IF EXISTS $d WITH (FORCE)" >/dev/null 2>&1 || true; done
}
trap cleanup EXIT
[[ -x "$BIN" ]] || { echo "paperrust-migrate-db fehlt ($BIN) – cargo build -p pl-dbmigrate"; exit 2; }
rm -rf "$WORK"; mkdir -p "$WORK"; chmod 700 "$WORK" 2>/dev/null || true

# (1) restore ---------------------------------------------------------------------------
for d in $SRC $DST $INP; do su -c "DROP DATABASE IF EXISTS $d WITH (FORCE)" 2>/dev/null; done
su -c "CREATE DATABASE $SRC ENCODING 'UTF8' TEMPLATE template0"
if head -c 5 "$DUMP" | grep -q PGDMP; then
  "$PG_BIN/pg_restore" -h "$PGHOST" -p "$PGPORT" -U "$SU" --no-owner --no-privileges -d $SRC "$DUMP" >"$WORK/restore.log" 2>&1 || true
else
  DB=$SRC su -f "$DUMP" >"$WORK/restore.log" 2>&1 || true
fi
[[ -n "$(q $SRC "SELECT to_regclass('public.django_migrations')")" ]] || { echo "Zurückspielen fehlgeschlagen (Details: $WORK/restore.log)"; exit 1; }

# (2) delete secrets – before every run of the assistant ------------------------------------
DB=$SRC su <<'SQL'
DO $$
DECLARE
  -- Tabelle, Spalte, neuer Wert (SQL-Ausdruck); nur wenn die Spalte in dieser Version existiert
  t record;
BEGIN
  FOR t IN SELECT * FROM (VALUES
      ('paperless_mail_mailaccount', 'password', $v$''$v$),
      ('paperless_mail_mailaccount', 'refresh_token', 'NULL'),
      ('auth_user', 'password', $v$'!'$v$),
      ('socialaccount_socialtoken', 'token', $v$''$v$),
      ('socialaccount_socialtoken', 'token_secret', $v$''$v$),
      ('socialaccount_socialapp', 'secret', $v$''$v$),
      ('socialaccount_socialapp', 'client_id', $v$''$v$),
      ('mfa_authenticator', 'data', $v$'{}'::jsonb$v$),
      ('documents_workflowactionwebhook', 'headers', 'NULL'),
      ('documents_workflowactionwebhook', 'params', 'NULL'),
      ('documents_workflowactionwebhook', 'body', 'NULL'),
      ('documents_workflowaction', 'passwords', 'NULL'),
      ('paperless_applicationconfiguration', 'llm_api_key', 'NULL'),
      ('paperless_applicationconfiguration', 'llm_embedding_api_key', 'NULL'),
      ('paperless_applicationconfiguration', 'remote_ocr_api_key', 'NULL'),
      ('documents_sharelink', 'slug', 'md5(slug)'),
      ('documents_sharelinkbundle', 'slug', 'md5(slug)')
    ) AS v(tab, col, val)
  LOOP
    IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = t.tab AND column_name = t.col) THEN
      EXECUTE format('UPDATE public.%I SET %I = %s', t.tab, t.col, t.val);
    END IF;
  END LOOP;
  FOR t IN SELECT * FROM (VALUES ('authtoken_token'), ('django_session'), ('paperrust_device_token')) AS v(tab) LOOP
    IF to_regclass('public.' || t.tab) IS NOT NULL THEN
      EXECUTE format('DELETE FROM public.%I', t.tab);
    END IF;
  END LOOP;
END $$;
SQL
echo "Geheimnisse in der Wegwerf-Kopie gelöscht (Mail-Passwörter, Token, Sitzungen, Passwort-Hashes, Schlüssel, Freigabe-Links)."
su -c "CREATE DATABASE $INP TEMPLATE $SRC" -c "CREATE DATABASE $DST ENCODING 'UTF8' TEMPLATE template0"

run() {  # $1 = name, rest = arguments; output (contains titles) only into the private log file
  local name=$1; shift; local rc=0
  local t0=$SECONDS
  "$BIN" "$@" >"$WORK/$name.log" 2>&1 || rc=$?
  echo "$name $rc $((SECONDS - t0))" >>"$WORK/laeufe.txt"
}
MEDIA_ARGS=(); [[ -n "$MEDIA" ]] && MEDIA_ARGS=(--media "$(w "$MEDIA")" --target-media "$(w "$MEDIA")")
# (3) detection + dry run, (4) new database, (5) in place
run detect detect --from "$(url $SRC)"
run probelauf switch --dry-run --quiet --from "$(url $SRC)" --target "$(url $DST)" "${MEDIA_ARGS[@]}" --report-dir "$(w "$WORK/probelauf")"
run neue-db switch --quiet --from "$(url $SRC)" --target "$(url $DST)" "${MEDIA_ARGS[@]}" --report-dir "$(w "$WORK/neue-db")"
# --no-backup: pl_mig_real_inplace is itself already a throwaway copy (no further copy of the data)
run an-ort-und-stelle switch --quiet --no-backup --from "$(url $INP)" --target "$(url $INP)" \
  "${MEDIA_ARGS[@]}" --report-dir "$(w "$WORK/an-ort-und-stelle")"

# (6) numbers only ------------------------------------------------------------------
PYTHON="${PY:-python}"
"$PYTHON" -I - "$WORK" <<'PY' | tee "$WORK/zahlen.txt"
import json, os, sys
work = sys.argv[1]
runs = {}
for line in open(os.path.join(work, "laeufe.txt"), encoding="utf-8"):
    n, rc, s = line.split()
    runs[n] = (int(rc), int(s))
try:
    det = json.load(open(os.path.join(work, "detect.log"), encoding="utf-8"))
    print(f"Quelle: {det.get('kind')}, Schema {det.get('schema')}, nachzuholen {len(det.get('catchup') or [])}, neuere bekannte {len(det.get('newer') or [])}")
except Exception:
    print("Quelle: detect nicht lesbar (Exit %s)" % runs.get("detect", ("?",))[0])
for name in ("probelauf", "neue-db", "an-ort-und-stelle"):
    rc, s = runs.get(name, (None, 0))
    p = os.path.join(work, name, "umstieg-bericht.json")
    if not os.path.exists(p):
        print(f"\n[{name}] kein Bericht (Exit {rc}, {s} s)")
        continue
    r = json.load(open(p, encoding="utf-8"))
    print(f"\n[{name}] Ergebnis {r['verdict']} (Exit {rc}, {s} s)")
    for c in r.get("counts", []):
        print(f"  {c['label']:<28} Quelle {str(c['source']):>8}  Ziel {str(c['target']):>8}  {c['level']}")
    for side in ("source", "target"):
        f = (r.get("files") or {}).get(side)
        if f:
            kinds = {}
            for pr in f.get("problem_list", []):
                kinds[pr.get("issue", "?").split(":")[0]] = kinds.get(pr.get("issue", "?").split(":")[0], 0) + 1
            print(f"  Dateien {'Quelle' if side == 'source' else 'Ziel'}: {f.get('originals_checked')} Originale, "
                  f"{f.get('archives_checked')} Archiv, {f.get('thumbnails_checked')} Vorschau, {f.get('ok')} in Ordnung, "
                  f"{f.get('problems')} Befunde {sorted(kinds.items())}, Prüfsummen MD5 {f.get('md5_checksums')} / SHA-256 {f.get('sha256_checksums')}")
    lv = {}
    for x in r.get("findings", []):
        lv[x["level"]] = lv.get(x["level"], 0) + 1
    print(f"  Befunde je Stufe: {dict(sorted(lv.items()))}; Schritte FAIL: {sum(1 for s in r.get('steps', []) if s['level'] == 'FAIL')}")
print("\nVollständige Berichte (mit Titeln/Dateinamen – NICHT weitergeben):", work)
PY
