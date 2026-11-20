"""Compares the results of the upstream run (pl_consumer_test_up) with the Rust run
(pl_consumer_test) for the cases from cases.json.

    reference/venv/Scripts/python.exe tests/parity/pl-consumer/compare.py

Reads out/upstream_results.json and out/rust_results.json, reads both DBs directly (psycopg, read
only) and compares per case: task result/error text, document fields, tags, custom fields,
object permissions, workflow runs, audit log (without timestamps), existing files (original checksum).
Known deviations (contract) are counted as "known", not as errors.
Result: out/report.md, exit code 1 on differences.

DB names: PL_CONSUMER_DB (default pl_consumer_test → Rust pl_consumer_test, upstream …_up).
All documents newly created in a case (`new_ids`, e.g. parts of a barcode split)
are compared pairwise. `normalize: ["checksum"]` replaces checksums of files that
pikepdf/lopdf write differently byte-wise (partial PDFs) with "<pdf>" (content/pages are
still compared).
"""

import hashlib
import json
import os
import re
import sys
from pathlib import Path

import psycopg

HERE = Path(__file__).resolve().parent
DSN = "host=127.0.0.1 port=54329 user=paperless password=paperless dbname={db}"
DB_RUST = os.environ.get("PL_CONSUMER_DB", "pl_consumer_test")
DB_UP = DB_RUST + "_up"

DOC_FIELDS = [
    "title", "content", "mime_type", "checksum", "archive_checksum", "page_count", "created",
    "filename", "archive_filename", "original_filename", "archive_serial_number",
    "correspondent_id", "document_type_id", "storage_path_id", "owner_id", "root_document_id",
    "version_label", "version_index", "deleted_at_is_null",
]

TS = re.compile(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d(\.\d+)?$")


def norm_changes(ch):
    """Timestamps (added, created of the CF instance, deleted_at …) → "<ts>"."""
    if not isinstance(ch, dict):
        return ch
    out = {}
    for k, v in ch.items():
        if isinstance(v, list):
            out[k] = ["<ts>" if isinstance(x, str) and TS.match(x) and k != "created" else x for x in v]
            if k == "created" and all(isinstance(x, str) for x in v):
                out[k] = ["<ts>" if TS.match(x) else x for x in v]
        else:
            out[k] = v
    return out


def winpath(v):
    """upstream on Windows stores paths with "\\" (str(Path)); Linux/Docker and Rust with "/"."""
    if isinstance(v, str):
        return v.replace("\\", "/")
    if isinstance(v, list):
        return [winpath(x) for x in v]
    if isinstance(v, dict):
        return {k: winpath(x) for k, x in v.items()}
    return v


def snapshot(db, doc_id, media_root, normalize=()):
    with psycopg.connect(DSN.format(db=db)) as conn, conn.cursor() as cur:
        cur.execute(
            "select title, content, mime_type, checksum, archive_checksum, page_count, created::text,"
            " filename, archive_filename, original_filename, archive_serial_number, correspondent_id,"
            " document_type_id, storage_path_id, owner_id, root_document_id, version_label, version_index,"
            " deleted_at is null from documents_document where id = %s",
            (doc_id,),
        )
        row = cur.fetchone()
        if row is None:
            return None
        snap = dict(zip(DOC_FIELDS, row))
        cur.execute("select tag_id from documents_document_tags where document_id = %s order by tag_id", (doc_id,))
        snap["tags"] = [r[0] for r in cur.fetchall()]
        cur.execute(
            "select field_id, value_text, value_bool, value_url, value_date::text, value_int, value_float,"
            " value_monetary, value_document_ids::text, value_select, value_long_text, deleted_at is null"
            " from documents_customfieldinstance where document_id = %s order by field_id",
            (doc_id,),
        )
        snap["custom_fields"] = [list(r) for r in cur.fetchall()]
        cur.execute(
            "select u.user_id, p.codename from guardian_userobjectpermission u join auth_permission p on p.id = u.permission_id"
            " where u.object_pk = %s and u.content_type_id = 17 order by 1, 2",
            (str(doc_id),),
        )
        snap["user_perms"] = [list(r) for r in cur.fetchall()]
        cur.execute(
            "select g.group_id, p.codename from guardian_groupobjectpermission g join auth_permission p on p.id = g.permission_id"
            " where g.object_pk = %s and g.content_type_id = 17 order by 1, 2",
            (str(doc_id),),
        )
        snap["group_perms"] = [list(r) for r in cur.fetchall()]
        cur.execute(
            "select workflow_id, type from documents_workflowrun where document_id = %s order by id",
            (doc_id,),
        )
        snap["workflow_runs"] = [list(r) for r in cur.fetchall()]
        cur.execute(
            "select c.model, l.action, l.object_repr, l.changes, l.additional_data, l.actor_id"
            " from auditlog_logentry l join django_content_type c on c.id = l.content_type_id"
            " where (c.model = 'document' and l.object_id = %s)"
            "    or (c.model = 'customfieldinstance' and l.object_id in"
            "        (select id from documents_customfieldinstance where document_id = %s))"
            " order by l.id",
            (doc_id, doc_id),
        )
        snap["audit"] = [[m, a, r, norm_changes(ch), ad, ac] for (m, a, r, ch, ad, ac) in cur.fetchall()]
        cur.execute(
            "select page, value, format from documents_documentbarcode where document_id = %s order by id",
            (doc_id,),
        )
        snap["barcodes"] = [list(r) for r in cur.fetchall()]
    media = Path(media_root)
    files = {}
    if snap["filename"]:
        p = media / "documents" / "originals" / snap["filename"]
        files["original_sha256"] = hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None
    if snap["archive_filename"]:
        files["archive_exists"] = (media / "documents" / "archive" / snap["archive_filename"]).is_file()
    files["thumbnail_exists"] = (media / "documents" / "thumbnails" / f"{doc_id:07}.webp").is_file()
    snap["files"] = files
    for k in ("filename", "archive_filename"):
        snap[k] = winpath(snap[k])
    if "checksum" in normalize:
        snap["checksum"] = "<pdf>"
        if files.get("original_sha256"):
            files["original_sha256"] = "<pdf>"
        for e in snap["audit"]:
            if isinstance(e[3], dict) and "checksum" in e[3]:
                e[3]["checksum"] = [x if x == "None" else "<pdf>" for x in e[3]["checksum"]]
    snap["audit"] = [[m, a, r, {f: (winpath(v) if f in ("filename", "archive_filename") else v) for f, v in ch.items()} if isinstance(ch, dict) else ch, ad, ac]
                     for (m, a, r, ch, ad, ac) in snap["audit"]]
    return snap


def flatten(prefix, v, out):
    if isinstance(v, dict):
        for k in v:
            flatten(f"{prefix}.{k}" if prefix else k, v[k], out)
    else:
        out[prefix] = v


def main() -> int:
    cases = json.loads((HERE / "cases.json").read_text(encoding="utf-8"))["cases"]
    up = json.loads((HERE / "out" / "upstream_results.json").read_text(encoding="utf-8"))
    rs = json.loads((HERE / "out" / "rust_results.json").read_text(encoding="utf-8"))
    lines = ["# pl-consumer Parität (upstream vs. Rust)", ""]
    total_diffs = 0
    for c in cases:
        name = c["name"]
        u, r = up["cases"][name], rs["cases"][name]
        diffs = []
        if u.get("result") != r.get("result"):
            diffs.append(("result", u.get("result"), r.get("result")))
        if u.get("error") != r.get("error"):
            diffs.append(("error", u.get("error"), r.get("error")))
        if u.get("file_left") != r.get("file_left"):
            diffs.append(("file_left", u.get("file_left"), r.get("file_left")))
        uid = (u.get("result") or {}).get("document_id")
        rid = (r.get("result") or {}).get("document_id")
        pairs = []
        if "new_ids" in u and "new_ids" in r:
            if len(u["new_ids"]) != len(r["new_ids"]):
                diffs.append(("new_ids.count", len(u["new_ids"]), len(r["new_ids"])))
            single = len(u["new_ids"]) == 1 and len(r["new_ids"]) == 1
            for i, (a, b) in enumerate(zip(u["new_ids"], r["new_ids"])):
                pairs.append(("" if single else f"doc{i}.", a, b))
        elif uid and rid:
            pairs.append(("", uid, rid))
        norm = tuple(c.get("normalize", ()))
        for prefix, a, b in pairs:
            su = snapshot(DB_UP, a, up["media_root"], norm)
            sr = snapshot(DB_RUST, b, rs["media_root"], norm)
            fu, fr = {}, {}
            flatten("", su, fu)
            flatten("", sr, fr)
            for k in sorted(set(fu) | set(fr)):
                if fu.get(k) != fr.get(k):
                    diffs.append((prefix + k, fu.get(k), fr.get(k)))
        known = c.get("known_diffs", {})

        def is_known(key):
            return key in known or key.split(".", 1)[-1] in known and key.startswith("doc")

        real = [d for d in diffs if not is_known(d[0])]
        total_diffs += len(real)
        status = "OK" if not real else f"{len(real)} Unterschiede"
        if len(diffs) != len(real):
            status += f" (+{len(diffs) - len(real)} bekannt: {', '.join(sorted({d[0] for d in diffs if is_known(d[0])}))})"
        if norm:
            status += f" [normalisiert: {', '.join(norm)}]"
        if pairs and len(pairs) > 1:
            status += f" [{len(pairs)} Dokumente]"
        lines.append(f"## {name}: {status}")
        for k, a, b in real:
            lines.append(f"- `{k}`\n  - upstream: `{json.dumps(a, ensure_ascii=False, default=str)}`\n  - rust:     `{json.dumps(b, ensure_ascii=False, default=str)}`")
        lines.append("")
    lines.insert(1, f"Unterschiede gesamt: {total_diffs}")
    (HERE / "out" / "report.md").write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))
    return 1 if total_diffs else 0


if __name__ == "__main__":
    sys.exit(main())
