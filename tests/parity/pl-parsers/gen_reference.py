"""Creates reference.json with the upstream results (paperless-ngx v3.4.2) for samples/.

Invocation (from the repo root directory, Git Bash):

    source reference/env.sh   # PATH/MAGIC for libmagic, PYTHONPATH, PAPERLESS_*
    reference/venv/Scripts/python.exe tests/parity/pl-parsers/gen_reference.py

Without Tika/Gotenberg server: the mail parser gets `tika_parse` replaced by a placeholder
(returns "TIKAHTML" + length of the HTML), `generate_pdf` is skipped. This makes the
formatting (header lines, attachment list, whitespace handling) exactly comparable; the
HTML->text conversion itself is tested by Rust with its own expectations.

Nothing is written to the database (no model access).
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "reference_settings")
for p in (REPO / "reference" / "overlay", REPO / "upstream" / "src"):
    if str(p) not in sys.path:
        sys.path.insert(0, str(p))

import django  # noqa: E402

django.setup()

import magic  # noqa: E402
from django.conf import settings  # noqa: E402

from documents import parsers as doc_parsers  # noqa: E402
from documents.parsers import ParseError  # noqa: E402
from paperless.parsers import utils as putils  # noqa: E402
from paperless.parsers.mail import MailDocumentParser  # noqa: E402
from paperless.parsers.registry import get_parser_registry  # noqa: E402

SAMPLES = HERE / "samples"
TEXT_SUFFIXES = {".txt", ".csv", ".md", ".html", ".xml", ".bin"}


def parser_name(mime: str, filename: str, path: Path) -> str | None:
    cls = get_parser_registry().get_parser_for_file(mime, filename, path)
    return None if cls is None else cls.__name__


def mail_results(path: Path) -> dict:
    out: dict = {}

    def fake_tika(self, html: str) -> str:
        return f"TIKAHTML{len(html)}"

    MailDocumentParser.tika_parse = fake_tika  # type: ignore[method-assign]
    MailDocumentParser.generate_pdf = lambda self, mail, layout=None: Path("unused.pdf")  # type: ignore[method-assign]
    with MailDocumentParser() as p:
        try:
            p.parse(path, "message/rfc822")
            out["text"] = p.get_text()
            d = p.get_date()
            out["date"] = None if d is None else d.isoformat()
        except ParseError as e:
            out["parse_error"] = str(e).replace(str(path), "<path>")
        out["metadata"] = p.extract_metadata(path, "message/rfc822")
        try:
            mail = MailDocumentParser.parse_file_to_message(path)
            out["html_len"] = len(mail.html)
            out["html"] = mail.html
            out["plain"] = mail.text
            out["attachments"] = [
                {"filename": a.filename, "size": a.size, "content_id": a.content_id}
                for a in mail.attachments
            ]
        except ParseError:
            pass
    return out


def main() -> None:
    result: dict = {
        "upstream": "paperless-ngx v3.4.2",
        "libmagic": magic.version() if hasattr(magic, "version") else None,
        "time_zone": settings.TIME_ZONE,
        "supported_extensions": {},
        "default_extension": {},
        "files": {},
    }
    for tika in (False, True):
        settings.TIKA_ENABLED = tika
        result["supported_extensions"]["tika" if tika else "no_tika"] = sorted(
            doc_parsers.get_supported_file_extensions(),
        )
    mimes_seen: set[str] = set()
    for path in sorted(SAMPLES.iterdir()):
        if not path.is_file():
            continue
        mime = magic.from_file(str(path), mime=True)
        mimes_seen.add(mime)
        entry: dict = {"mime": mime}
        for tika in (False, True):
            settings.TIKA_ENABLED = tika
            entry["parser_tika" if tika else "parser_no_tika"] = parser_name(mime, path.name, path)
        settings.TIKA_ENABLED = False
        if path.suffix.lower() in TEXT_SUFFIXES or mime.startswith("text/"):
            entry["text_read"] = putils.read_file_handle_unicode_errors(path)
        if mime == "message/rfc822" or path.suffix == ".eml":
            entry["mail"] = mail_results(path)
        if mime == "application/pdf" or path.suffix == ".pdf":
            entry["pdf_page_count"] = putils.get_page_count_for_pdf(path)
            entry["pdf_tagged"] = putils.is_tagged_pdf(path)
            entry["pdf_metadata"] = putils.extract_pdf_metadata(path)
        result["files"][path.name] = entry
    # MIME samples: libmagic result only
    result["mime_probes"] = {
        f.name: magic.from_file(str(f), mime=True)
        for f in sorted((SAMPLES / "mime").iterdir())
        if f.is_file()
    }
    # Python mimetypes without Windows registry/OS files (like a fresh Linux without /etc/mime.types)
    import mimetypes

    mt = mimetypes.MimeTypes(filenames=())
    all_mimes = sorted(
        {m for cls in get_parser_registry().all_parsers() for m in cls.supported_mime_types()},
    )
    result["mimetypes_default_all_extensions"] = {
        m: mt.guess_all_extensions(m) for m in all_mimes
    }
    result["mimetypes_default_guess_extension"] = {
        m: mt.guess_extension(m) for m in sorted(mimes_seen | set(all_mimes))
    }
    for tika in (False, True):
        settings.TIKA_ENABLED = tika
        result["default_extension"]["tika" if tika else "no_tika"] = {
            m: doc_parsers.get_default_file_extension(m) for m in sorted(mimes_seen)
        }
    settings.TIKA_ENABLED = False
    out = HERE / "reference.json"
    out.write_text(json.dumps(result, ensure_ascii=False, indent=1, sort_keys=False) + "\n", encoding="utf-8")
    print("geschrieben:", out, len(result["files"]), "Dateien")


if __name__ == "__main__":
    main()
