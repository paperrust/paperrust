#!/usr/bin/env bash
# Remaining commands: Rust and upstream each on its own copy (pl_w7_cmd_rs / pl_w7_cmd_up) with
# the same prepared damage; afterwards compare DB content and media.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BIN="${PLCLI_BIN:-$REPO_DIR/target/debug/examples/manage.exe}"
fail=0
# m2m IDs (documents_document_tags.id) follow the CPython set order in upstream in
# add_nested_tags (pl-consumer inserts in list order) → only content compared.
IGN="documents_document.modified auditlog_logentry.timestamp auditlog_logentry.id documents_document_tags.id"
prep() { for s in rs up; do copy_env "cmd_$s" >/dev/null 2>&1; done; }
both_sql() { for s in rs up; do psql_db "pl_w7_cmd_$s" -c "$1" >/dev/null; done; }
cmpall() {
  # upstream on Windows stores str(Path) with "\\" (on Linux "/")
  for s in up rs; do psql_db pl_w7_cmd_$s -c "UPDATE documents_document SET filename=replace(filename,chr(92),'/'), archive_filename=replace(archive_filename,chr(92),'/')" >/dev/null; done
  "$PY" -I "$HERE_PLCLI/compare.py" db pl_w7_cmd_up pl_w7_cmd_rs --ignore $IGN "$@" | tail -8; local r=${PIPESTATUS[0]}
  "$PY" -I "$HERE_PLCLI/compare.py" tree "$WORKROOT/cmd_up/media" "$WORKROOT/cmd_rs/media" | tail -4 || r=1
  return $r; }
ok() { if "$@"; then echo "OK   $CASE"; else echo "FAIL $CASE"; fail=1; fi; }
up_shell() { use_env cmd_up; manage_py shell -c "$1" 2>/dev/null; }

# --- sanity checker: thumbnail missing, original changed, orphaned file, empty content
prep
for s in rs up; do M="$WORKROOT/cmd_$s/media/documents"
  rm -f "$M/thumbnails/0000003.webp"; printf 'x' >>"$M/originals/0000004.pdf"; echo orphan >"$M/originals/orphan.txt"; done
both_sql "UPDATE documents_document SET content='' WHERE id=5"
use_env cmd_rs; "$BIN" document_sanity_checker >"$WORKROOT/sanity_rs.txt"
up_shell "
from documents.sanity_checker import check_sanity
m = check_sanity()
for pk, msgs in m.iter_messages():
    for x in msgs: print(pk, x['level'], x['message'])
print('counts', m.document_error_count, m.document_warning_count, m.document_info_count, m.global_warning_count)" >"$WORKROOT/sanity_up.txt"
python - "$WORKROOT/sanity_up.txt" "$WORKROOT/sanity_rs.txt" "$WORKROOT/cmd_up" "$WORKROOT/cmd_rs" <<'PY'
import sys, re, os
up = [l.rstrip("\n") for l in open(sys.argv[1], encoding="utf8") if l.strip()]
rs = [l.rstrip("\n") for l in open(sys.argv[2], encoding="utf8")]
lv = {"40": "ERROR", "30": "WARN", "20": "INFO"}
norm = lambda s: s.replace(os.path.normpath(sys.argv[3]).replace("/", "\\"), "W").replace(sys.argv[3], "W")
up = [l for l in up if re.match(r"^(\d+|None) (10|20|30|40) ", l)]
u = sorted(f"{lv[l.split()[1]]} {l.split()[0]} {norm(' '.join(l.split()[2:]))}" for l in up)
r = []
for l in rs:
    m = re.match(r"(\w+)\s+\| (#(\d+) .*?|\(global\))\s+\| (.*)$", l)
    if m: r.append(f"{m.group(1)} {m.group(3) or 'None'} " + m.group(4).replace(os.path.normpath(sys.argv[4]).replace("/", "\\"), "W").replace(sys.argv[4], "W"))
r.sort()
print("upstream", len(u), "rust", len(r)); [print(" up:", x) for x in u if x not in r]; [print(" rs:", x) for x in r if x not in u]
sys.exit(0 if u == r else 1)
PY
r=$?; CASE="sanity_checker Meldungen"; ok test $r -eq 0
tail -1 "$WORKROOT/sanity_rs.txt"; grep counts "$WORKROOT/sanity_up.txt"

# --- prune_audit_logs: delete tag directly (audit entries stay)
prep; both_sql "DELETE FROM documents_document_tags WHERE tag_id=15; DELETE FROM documents_tag WHERE id=15"
use_env cmd_rs; "$BIN" prune_audit_logs | tail -2
use_env cmd_up; manage_py prune_audit_logs --no-progress-bar 2>/dev/null | tail -2
CASE="prune_audit_logs"; ok cmpall

# --- empty_trash (documents 196–200 in the trash)
prep; use_env cmd_rs; "$BIN" empty_trash --doc-ids 196,197,198,199,200
up_shell "from documents.tasks import empty_trash; empty_trash([196,197,198,199,200])"
CASE="empty_trash (ohne auditlog, Known difference 7)"; IGN_SAVE="$IGN"; IGN="$IGN auditlog_logentry.object_pk auditlog_logentry.object_id auditlog_logentry.object_repr auditlog_logentry.action auditlog_logentry.changes auditlog_logentry.content_type_id auditlog_logentry.changes_text auditlog_logentry.actor_id auditlog_logentry.remote_addr auditlog_logentry.additional_data auditlog_logentry.serialized_data auditlog_logentry.cid auditlog_logentry.remote_port auditlog_logentry.actor_email"
psql_db pl_w7_cmd_up -c "DELETE FROM auditlog_logentry WHERE action=2 AND content_type_id IN (SELECT id FROM django_content_type WHERE model IN ('note','customfieldinstance'))" >/dev/null
ok cmpall; IGN="$IGN_SAVE"
psql_db pl_w7_cmd_up -c "SELECT count(*) FROM auditlog_logentry" ; psql_db pl_w7_cmd_rs -c "SELECT count(*) FROM auditlog_logentry"

# --- document_renamer with file name template
prep; export PAPERLESS_FILENAME_FORMAT="{{ created_year }}/{{ correspondent }}/{{ title }}"
use_env cmd_rs; "$BIN" document_renamer
use_env cmd_up; manage_py document_renamer --no-progress-bar >/dev/null 2>&1
unset PAPERLESS_FILENAME_FORMAT
CASE="document_renamer"; ok cmpall

# --- document_retagger (rules, without classifier), with overwrite
prep; use_env cmd_rs; "$BIN" document_retagger -c -T -t -s -f | tail -7
use_env cmd_up; manage_py document_retagger -c -T -t -s -f --no-progress-bar 2>/dev/null | tail -12
CASE="document_retagger -c -T -t -s -f"; ok cmpall
use_env cmd_rs; "$BIN" document_retagger -c --suggest | tail -3

# --- document_index reindex (sentinel file like upstream), optimize
use_env cmd_rs; "$BIN" document_index reindex && cmp "$WORKROOT/cmd_rs/data/index/.index_settings.json" "$REF_DIR/data/data/index/.index_settings.json" \
  && echo "OK   document_index reindex (Sentinel gleich)" || { echo "FAIL reindex"; fail=1; }
"$BIN" document_index reindex --if-needed; "$BIN" document_index optimize && echo "OK   optimize"
# --- classifier
use_env cmd_rs; "$BIN" document_create_classifier && ls -la "$WORKROOT/cmd_rs/data/paperrust_classifier.model" && echo "OK   classifier"
"$BIN" document_retagger -c -T --suggest | tail -2
# --- thumbnails
use_env cmd_rs; rm -f "$WORKROOT/cmd_rs/media/documents/thumbnails/0000001.webp"; "$BIN" document_thumbnails -d 1
[[ -s "$WORKROOT/cmd_rs/media/documents/thumbnails/0000001.webp" ]] && echo "OK   document_thumbnails -d 1" || { echo "FAIL thumbnails"; fail=1; }
# --- mail_fetcher without registration
"$BIN" mail_fetcher; echo "mail_fetcher exit=$?"
for s in rs up; do psql_su -c "DROP DATABASE IF EXISTS pl_w7_cmd_$s WITH (FORCE)"; done
exit $fail
