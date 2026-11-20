"""Upstream run for the pl-consumer parity (unchanged upstream code, own DB copy).

Invocation (Git Bash, from the repo):
    tests/parity/pl-consumer/reset_db.sh
    tests/parity/pl-consumer/run_upstream.sh

Reads cases.json, creates the files in its own temp inbox and calls for every case
`documents.tasks.consume_file.apply(...)` (Celery eager, only in this process). Database
`pl_consumer_test_up` (copy of paperless_snapshot), media/data in a temp folder,
Redis/channels/cache in memory (see overlay/pl_consumer_settings.py). Result:
out/upstream_results.json {case: {result, error, new_ids}} + media_root.

Barcodes: `scan_pdf` runs unchanged; only `pdf2image.convert_from_path` (needs poppler, not present in the
Windows reference) is replaced by pypdfium2 with the same DPI – like
tests/parity/pl-pdfops/gen_reference.py. Cases can set Django settings (`settings`) and
ApplicationConfiguration columns (`app_config`, afterwards NULL again).
"""

import datetime
import json
import os
import sys
import tempfile
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main() -> int:
    cases = json.loads((HERE / "cases.json").read_text(encoding="utf-8"))["cases"]
    root = Path(os.environ["PL_CONSUMER_UP_ROOT"])
    consume = root / "consume"
    consume.mkdir(parents=True, exist_ok=True)

    import django

    django.setup()
    from django.test import override_settings

    from documents.data_models import ConsumableDocument
    from documents.data_models import DocumentMetadataOverrides
    from documents.data_models import DocumentSource
    from documents.tasks import consume_file
    from documents.models import Document
    from paperless.models import ApplicationConfiguration

    import pypdfium2 as pdfium
    from PIL import Image

    import documents.barcodes as bc_mod

    def fake_convert_from_path(pdf_path, dpi, output_folder, first_page, last_page, **_kw):
        pdoc = pdfium.PdfDocument(str(pdf_path))
        try:
            page = pdoc[first_page - 1]
            img = page.render(scale=dpi / 72).to_pil().convert("RGB")
        finally:
            pdoc.close()
        path = Path(output_folder) / f"{uuid.uuid4()}-{first_page:02d}.ppm"
        img.save(path)
        im = Image.open(path)
        im.load()
        return [im]

    bc_mod.convert_from_path = fake_convert_from_path

    def max_id():
        return Document.global_objects.order_by("-id").values_list("id", flat=True).first() or 0

    out = {}
    for i, c in enumerate(cases):
        p = consume / c["filename"]
        if "sample" in c:
            p.write_bytes((HERE.parent / c["sample"]).read_bytes())
        else:
            p.write_text(c["content"], encoding="utf-8", newline="")
        os.utime(p, (c["mtime"], c["mtime"]))
        ov = dict(c.get("overrides") or {})
        if "created" in ov:
            ov["created"] = datetime.date.fromisoformat(ov["created"])
        if "custom_fields" in ov:
            ov["custom_fields"] = {int(k): v for k, v in ov["custom_fields"].items()}
        overrides = DocumentMetadataOverrides(**ov)
        doc = ConsumableDocument(source=DocumentSource(c["source"]), original_file=p)
        settings_ctx = {
            "CONSUMER_DELETE_DUPLICATES": bool(c.get("delete_duplicates")),
            "FILENAME_FORMAT": c.get("filename_format"),
        }
        if "ocr_mode" in c:
            settings_ctx["OCR_MODE"] = c["ocr_mode"]
        if "archive_file_generation" in c:
            settings_ctx["ARCHIVE_FILE_GENERATION"] = c["archive_file_generation"]
        settings_ctx.update(c.get("settings") or {})
        if c.get("app_config"):
            ApplicationConfiguration.objects.all().update(**c["app_config"])
        before = max_id()
        with override_settings(**settings_ctx):
            r = consume_file.apply(
                kwargs={"input_doc": doc, "overrides": overrides},
                task_id=f"parity-{i:02d}",
            )
        entry = {"state": r.state}
        if r.successful():
            entry["result"] = r.result
        else:
            entry["error"] = str(r.result)
            entry["error_type"] = type(r.result).__name__
        entry["new_ids"] = list(
            Document.global_objects.filter(id__gt=before).order_by("id").values_list("id", flat=True),
        )
        if c.get("app_config"):
            ApplicationConfiguration.objects.all().update(**{k: None for k in c["app_config"]})
        entry["file_left"] = p.exists()
        out[c["name"]] = entry
        print(c["name"], json.dumps(entry, ensure_ascii=False, default=str))
    res = {"media_root": str(root / "media"), "cases": out}
    (HERE / "out").mkdir(exist_ok=True)
    (HERE / "out" / "upstream_results.json").write_text(
        json.dumps(res, indent=1, ensure_ascii=False, default=str),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
