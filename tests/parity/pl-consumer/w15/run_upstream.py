"""Upstream run of the W15 parity (unchanged upstream code, DB pl_w15_ref).

    tests/parity/pl-consumer/w15/reset_db.sh
    tests/parity/pl-consumer/w15/run_upstream.sh

Places the files from fixtures.json in its own media folder and calls the upstream functions
directly (Celery eager, cache/channels in memory – overlay/pl_consumer_settings.py):
`update_document_content_maybe_archive_file`, `check_scheduled_workflows`,
`bulk_update_documents`. `apply_ai_suggestions.delay_on_commit` is intercepted (enqueued
tasks are compared, not executed). For `fake_archive.filename` a check parser is
used (the reference without tesseract/ghostscript produces no archive). Result:
out/upstream.json {steps: {name: {ok|error_type, error}}, tasks: [...], media_root}.
"""

import json
import os
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main() -> int:
    fx = json.loads((HERE / "fixtures.json").read_text(encoding="utf-8"))
    root = Path(os.environ["PL_W15_UP_ROOT"])

    import django

    django.setup()
    from django.conf import settings

    originals = Path(settings.ORIGINALS_DIR)
    for f in fx["files"]:
        p = originals / f["filename"]
        p.parent.mkdir(parents=True, exist_ok=True)
        if "sample" in f:
            p.write_bytes((HERE.parent.parent / f["sample"]).read_bytes())
        else:
            p.write_text(f["text"], encoding="utf-8", newline="")

    import documents.tasks as tasks

    fake = fx["fake_archive"]

    class FakeParser:
        def __init__(self, *a, **kw):
            self._dir = Path(settings.SCRATCH_DIR) / "w15fake"
            self._dir.mkdir(parents=True, exist_ok=True)
            self._archive = None

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def configure(self, ctx):
            pass

        def parse(self, path, mime_type, produce_archive=True):
            self._archive = self._dir / "archive.pdf"
            shutil.copyfile(path, self._archive)

        def get_text(self):
            return fake["text"]

        def get_archive_path(self):
            return self._archive

        def get_thumbnail(self, path, mime_type):
            t = self._dir / "thumb.webp"
            t.write_bytes(fake["thumbnail"].encode())
            return t

        def get_date(self):
            return None

    real_registry = tasks.get_parser_registry

    class Registry:
        def get_parser_for_file(self, mime_type, filename, path=None, **kw):
            if filename == fake["filename"]:
                return FakeParser
            return real_registry().get_parser_for_file(mime_type, filename, path, **kw)

    tasks.get_parser_registry = lambda: Registry()
    real_spa = tasks.should_produce_archive
    tasks.should_produce_archive = lambda parser, mime, path: True if isinstance(parser, FakeParser) else real_spa(parser, mime, path)

    queued = []

    def fake_delay_on_commit(*args, **kwargs):
        queued.append({"task": "documents.tasks.apply_ai_suggestions", "kwargs": kwargs})

    tasks.apply_ai_suggestions.delay_on_commit = fake_delay_on_commit

    from django.test import override_settings

    out = {}
    with override_settings(OCR_MODE="off", ARCHIVE_FILE_GENERATION="never"):
        for st in fx["steps"]:
            try:
                if st["op"] == "reprocess":
                    tasks.update_document_content_maybe_archive_file(st["id"])
                elif st["op"] == "scheduled":
                    tasks.check_scheduled_workflows()
                elif st["op"] == "bulk_update":
                    tasks.bulk_update_documents(st["ids"])
                out[st["name"]] = {"ok": True}
            except Exception as e:  # noqa: BLE001
                out[st["name"]] = {"ok": False, "error_type": type(e).__name__, "error": str(e)}
            print(st["name"], out[st["name"]])
    res = {"media_root": str(root / "media"), "steps": out, "tasks": queued}
    (HERE / "out").mkdir(exist_ok=True)
    (HERE / "out" / "upstream.json").write_text(json.dumps(res, indent=1, ensure_ascii=False), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
