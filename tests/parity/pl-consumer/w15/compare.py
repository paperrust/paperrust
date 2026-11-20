"""Comparison of the W15 parity: pl_w15_ref (upstream) against pl_w15_rs (Rust).

    reference/venv/Scripts/python.exe tests/parity/pl-consumer/w15/compare.py

Reads out/upstream.json and out/rust.json and both DBs (read only): step results,
enqueued tasks, document fields (without timestamps), tags, workflow runs, audit log (new
entries, timestamps normalized), barcodes, PaperlessTask growth, files (original/archive:
present + SHA-256, thumbnail: present; for the check parser also SHA-256).
Known deviations from KNOWN count as "known". Result: out/report.md, exit code 1 on
differences.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

import psycopg

HERE = Path(__file__).resolve().parent
DSN = "host=127.0.0.1 port=54329 user=paperless password=paperless dbname={db}"
DOC_IDS = [9001, 9002, 9003, 9011, 9012, 9013, 9014, 9015, 9016, 9017, 9018, 9019, 9020, 9021, 9031, 9032]
AUDIT_AFTER = 918  # max(auditlog_logentry.id) in the snapshot
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
TS = re.compile(r"^\d{4}-\d\d-\d\d[ T]\d\d:\d\d:\d\d(\.\d+)?([+-]\d\d:\d\d|Z)?$")

# (section, key) → reason
KNOWN = {
    ("doc", "9002.content"): "PDF-Textebene: pdftotext der Windows-Referenz liefert \\r\\n-Zeilenenden, pl-parsers "
    "\\n (Textextraktion, nicht Aufgabenlogik; wie pl-consumer-Fälle barcode_asn)",
    ("audit", "9002"): "Folge von content",
}


def sha(p: Path):
    return hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None


def norm(v):
    if isinstance(v, str):
        v = v.replace("\\", "/")
        return "<ts>" if TS.match(v) else "<uuid>" if UUID.match(v) else v
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items()}
    return v


def snapshot(db, media_root):
    media = Path(media_root) / "documents"
    out = {"doc": {}, "tags": {}, "runs": {}, "audit": {}, "barcodes": {}, "files": {}}
    with psycopg.connect(DSN.format(db=db)) as conn, conn.cursor() as cur:
        for i in DOC_IDS:
            cur.execute(
                "select title, content, mime_type, archive_checksum, page_count, created::text, filename,"
                " archive_filename, correspondent_id, document_type_id, storage_path_id, owner_id,"
                " deleted_at is null from documents_document where id = %s",
                (i,),
            )
            r = cur.fetchone()
            names = ["title", "content", "mime_type", "archive_checksum", "page_count", "created", "filename",
                     "archive_filename", "correspondent_id", "document_type_id", "storage_path_id", "owner_id", "alive"]
            d = dict(zip(names, r)) if r else None
            if d:
                d["filename"] = norm(d["filename"])
                d["archive_filename"] = norm(d["archive_filename"])
                for k, v in d.items():
                    out["doc"][f"{i}.{k}"] = v
                out["files"][f"{i}.original"] = sha(media / "originals" / d["filename"]) if d["filename"] else None
                out["files"][f"{i}.archive"] = sha(media / "archive" / d["archive_filename"]) if d["archive_filename"] else None
                thumb = media / "thumbnails" / f"{i:07d}.webp"
                out["files"][f"{i}.thumbnail"] = (sha(thumb) if i == 9003 else thumb.is_file())
            cur.execute(
                "select t.name from documents_document_tags dt join documents_tag t on t.id = dt.tag_id"
                " where dt.document_id = %s order by t.name",
                (i,),
            )
            out["tags"][str(i)] = [x[0] for x in cur.fetchall()]
            cur.execute(
                "select workflow_id, type, deleted_at is null, count(*) from documents_workflowrun"
                " where document_id = %s group by 1, 2, 3 order by 1, 2, 3",
                (i,),
            )
            out["runs"][str(i)] = [list(x) for x in cur.fetchall()]
            cur.execute(
                "select page, value, format from documents_documentbarcode where document_id = %s order by page, id",
                (i,),
            )
            out["barcodes"][str(i)] = [list(x) for x in cur.fetchall()]
            cur.execute(
                "select l.action, l.object_repr, l.changes, l.additional_data from auditlog_logentry l"
                " join django_content_type c on c.id = l.content_type_id"
                " where c.app_label = 'documents' and c.model = 'document' and l.object_id = %s and l.id > %s"
                " order by l.id",
                (i, AUDIT_AFTER),
            )
            out["audit"][str(i)] = [
                [a, repr_, norm(ch if isinstance(ch, dict) else json.loads(ch)), norm(ad)]
                for a, repr_, ch, ad in cur.fetchall()
            ]
            cur.execute(
                "select field_id, value_date::text, deleted_at is null, transaction_id is null"
                " from documents_customfieldinstance where document_id = %s order by id",
                (i,),
            )
            out["cfi"] = out.get("cfi", {})
            out["cfi"][str(i)] = [list(x) for x in cur.fetchall()]
        cur.execute("select count(*) from documents_workflowrun where workflow_id >= 9001")
        out["runs"]["total"] = cur.fetchone()[0]
        cur.execute("select count(*) from documents_paperlesstask")
        out["paperlesstask_count"] = {"n": cur.fetchone()[0]}
    return out


def main() -> int:
    up = json.loads((HERE / "out" / "upstream.json").read_text(encoding="utf-8"))
    rs = json.loads((HERE / "out" / "rust.json").read_text(encoding="utf-8"))
    a = snapshot("pl_w15_ref", up["media_root"])
    b = snapshot("pl_w15_rs", rs["media_root"])
    a["steps"], b["steps"] = up["steps"], rs["steps"]
    a["tasks"], b["tasks"] = {"queued": up["tasks"]}, {"queued": rs["tasks"]}
    lines, diff, known, same = ["# W15-Parität", ""], 0, 0, 0
    for section in a:
        for key in sorted(set(a[section]) | set(b[section])):
            va, vb = a[section].get(key), b[section].get(key)
            if va == vb:
                same += 1
                continue
            k = KNOWN.get((section, key))
            tag = "known" if k else "DIFF"
            if k:
                known += 1
            else:
                diff += 1
            lines.append(f"- **{tag}** {section}/{key}: upstream `{json.dumps(va, ensure_ascii=False, default=str)[:400]}`"
                         f" vs rust `{json.dumps(vb, ensure_ascii=False, default=str)[:400]}`" + (f" – {k}" if k else ""))
    lines.insert(1, f"gleich: {same}, bekannt: {known}, Unterschiede: {diff}")
    (HERE / "out" / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))
    return 1 if diff else 0


if __name__ == "__main__":
    sys.exit(main())
