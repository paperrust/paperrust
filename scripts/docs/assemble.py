"""Assemble the English PaperRust user pages from split upstream bodies.
python -I assemble.py <pages_dir> <docs/user/en>
"""
import os
import re
import sys

P, OUT = sys.argv[1], sys.argv[2]

ORIGIN = ("> Based on the paperless-ngx v3.4.2 documentation (GPL-3.0, © the paperless-ngx authors), "
          "adapted for PaperRust. Lines marked **PaperRust** describe where PaperRust differs.")


def nav(page, title):
    return (f"# {title}\n\n[Deutsch](../{page}) · [Documentation overview](README.md)\n\n{ORIGIN}\n\n")


HEADERS = {
    "usage.md": nav("usage.md", "Usage") ,
    "mail.md": nav("mail.md", "Incoming mail"),
    "permissions.md": nav("permissions.md", "Users, groups and permissions"),
    "workflows.md": nav("workflows.md", "Workflows"),
    "custom-fields.md": nav("custom-fields.md", "Custom fields"),
    "advanced.md": nav("advanced.md", "Advanced topics"),
    "storage-paths.md": nav("storage-paths.md", "File names and storage paths"),
    "api.md": nav("api.md", "REST API"),
}


def note(kind, text):
    return "\n".join([f"> [!{kind}]"] + ["> " + l if l else ">" for l in text.strip().split("\n")]) + "\n"


NOTES = {
    "configuration.md": [
        (r"^### Message Broker", note("NOTE", """**PaperRust:** needs no broker at all (no Redis, no Celery; the task queue lives in PostgreSQL).
After switching from paperless-ngx the Redis/Valkey service and `PAPERLESS_REDIS` can be removed;
if the variable stays set, only a hint is logged. The upstream text below describes paperless-ngx.""")),
        (r"^### Database", note("IMPORTANT", """**PaperRust:** only **PostgreSQL ≥ 14** is supported (tested with 18). The text below
describes upstream, which also accepts SQLite and MariaDB. To move an existing SQLite/MariaDB
installation, see [Switching from paperless-ngx](migration.md).""")),
        (r"^### Tika", note("NOTE", """**PaperRust:** Tika and Gotenberg are not needed. PaperRust reads Office documents (DOCX, XLSX,
PPTX, ODT/ODS/ODP, RTF) and mails (EML, HTML) itself. Differences: no PDF rendition of Office
files (the original is stored and searchable), thumbnails are drawn from the text, text layout
can differ from Tika. Details: `docs/contract/pl-parsers.md`.""")),
        (r"^## OCR settings", note("NOTE", """**PaperRust:** OCR runs with Tesseract and PDFium instead of OCRmyPDF, Ghostscript and unpaper.
Not carried out yet: cleaning (`PAPERLESS_OCR_CLEAN`) and deskewing (`PAPERLESS_OCR_DESKEW`);
PDF/A output only stamps the metadata. HEIC images cannot be read. Details:
`docs/contract/pl-ocr.md`, "Known differences".""")),
        (r"^## Binaries", note("NOTE", """**PaperRust:** the external programs PaperRust uses are Tesseract (`PAPERRUST_TESSERACT_PATH`,
`PAPERRUST_TESSDATA_DIR`) and the PDFium library (`PAPERRUST_PDFIUM_PATH`), see
[PaperRust settings](#paperrust-settings).""")),
        (r"^## Docker-specific options", note("NOTE", """**PaperRust:** these options belong to the upstream container image (`ghcr.io/paperless-ngx/paperless-ngx`).
A PaperRust container image is planned but not published yet.""")),
        (r"^### Encrypted Emails", note("WARNING", """**PaperRust:** GPG decryption of mails is not implemented; encrypted mails are processed unchanged.""")),
    ],
    "usage.md": [
        (r"^## Adding documents to", note("NOTE", """**PaperRust:** `paperrust serve --worker` watches the consumption directory itself (no separate
`document_consumer`, no Celery). The folder is polled; `PAPERLESS_CONSUMER_POLLING_INTERVAL=0` means
once per second.""")),
        (r"^## Document Trash", None),  # anchor only (see ANCHORS)
    ],
    "mail.md": [
        (r"^#### OAuth Email Setup", note("NOTE", """**PaperRust:** the OAuth callback (`/api/oauth/callback/`, Gmail/Outlook) is still served by
Django. Without Django, set up mail accounts with password authentication.""")),
    ],
    "advanced.md": [
        (r"^## Barcodes", note("NOTE", """**PaperRust:** barcodes are rendered with PDFium and read with rxing (instead of
poppler/zxing-cpp); without PDFium barcode detection is off. Split documents have the same pages
but different bytes, hence different checksums, than upstream.""")),
        (r"^## SSO and third party authentication", note("NOTE", """**PaperRust:** OpenID Connect (`openid_connect`) is served by PaperRust. Other social providers and
allauth "headless" flows are still served by Django (mixed operation only).""")),
        (r"^## Decryption of encrypted emails", note("WARNING", """**PaperRust:** not implemented. Encrypted mails are processed unchanged.""")),
    ],
    "storage-paths.md": [
        (r"^### Filename Templates", note("NOTE", """**PaperRust:** templates are evaluated by a Rust port of the Jinja2 sandbox with the same filters
(`pl-templating`); results are checked against upstream. Known edge cases: `docs/contract/pl-templating.md`.""")),
    ],
}

# extra anchors to place before headings (both languages use the same ids)
ANCHORS = {
    "usage.md": [(r"^## Document Trash", ["papierkorb", "document-trash"]),
                 (r"^## Terms and definitions", ["terms-and-definitions", "begriffe"]),
                 (r"^## Searching", ["searching", "suche"]),
                 (r"^## Sharing documents", ["sharing", "freigaben"]),
                 (r"^### Share Links", ["share-links"]),
                 (r"^### The consumption directory", ["consumption-directory", "eingangsordner"])],
    "permissions.md": [(r"^### Users and Groups", ["users-and-groups", "benutzer-und-gruppen"]),
                       (r"^## Permissions", ["permissions", "rechte"])],
    "mail.md": [(r"^#### Processed Mail", ["processed-mail", "verarbeitete-mails"])],
    "workflows.md": [(r"^## Workflows", ["workflows"]),
                     (r"^#### Workflow placeholders", ["workflow-placeholders", "platzhalter"])],
    "custom-fields.md": [],
    "storage-paths.md": [(r"^## File name handling", ["file-name-handling", "dateinamen"])],
    "advanced.md": [(r"^## Barcodes", ["barcodes"])],
}

# sections to drop: (page, heading regex); drops until next heading of same or higher level
DROP = {
    "advanced.md": [r"^### Docker$", r"^## Celery Monitoring", r"^## Custom Container Initialization",
                    r"^## Installing third-party parser plugins", r"^## MySQL Caveats"],
    "usage.md": [r"^## Architecture"],
}

ARCH = """## Architecture

PaperRust is one program, `paperrust`:

- **`paperrust serve`** – web server: REST API, the (unchanged upstream) Angular web UI, login
  pages, file downloads, WebSocket status messages.
- **`paperrust serve --worker`** – additionally runs the task worker in the same process:
  consumption (OCR, parsers, barcodes, classifier), mail fetching, workflows, scheduled tasks
  (like Celery beat) and the consumption-directory watcher. `--worker-only` runs the worker without
  HTTP.
- **`paperrust manage <command>`** – the management commands (`migrate`, `document_exporter`, …),
  see [Administration](administration.md).
- **PostgreSQL** stores everything, including the task queue.
  No Redis, Celery, Python, Tika or Gotenberg is required.

In **mixed operation** PaperRust sits in front of an upstream paperless-ngx (Django + Celery) and
forwards everything it does not serve itself; see [Installation](installation.md#mixed-operation).
"""


def level(l):
    m = re.match(r"^(#{1,6}) ", l)
    return len(m.group(1)) if m else None


def drop_sections(lines, pats):
    out, i, fence = [], 0, False
    while i < len(lines):
        l = lines[i]
        if l.lstrip().startswith("```"):
            fence = not fence
        if not fence and any(re.match(p, l) for p in pats):
            lv = level(l)
            # remove preceding <a id> line(s)
            while out and (out[-1] == "" or out[-1].startswith("<a id=")):
                out.pop()
            out.append("")
            i += 1
            f2 = False
            while i < len(lines):
                if lines[i].lstrip().startswith("```"):
                    f2 = not f2
                lv2 = None if f2 else level(lines[i])
                if lv2 is not None and lv2 <= lv:
                    break
                if not f2 and lines[i].startswith("<a id=") and i + 2 < len(lines) and (level(lines[i + 2]) or 9) <= lv:
                    break
                i += 1
            continue
        out.append(l)
        i += 1
    return out


def insert(lines, page):
    out = []
    for l in lines:
        for pat, ids in ANCHORS.get(page, []):
            if re.match(pat, l):
                while out and out[-1] == "":
                    out.pop()
                out.append("")
                out.append("".join(f'<a id="{a}"></a>' for a in ids))
                out.append("")
        out.append(l)
        for pat, n in NOTES.get(page, []):
            if n and re.match(pat, l):
                out.append("")
                out.append(n.rstrip())
    return out


for page in list(HEADERS) + ["configuration.md", "ai-upstream.md"]:
    lines = open(os.path.join(P, page + ".body"), encoding="utf-8").read().split("\n")
    if lines and lines[0] == "---":
        end = lines.index("---", 1)
        lines = lines[end + 1:]
        while lines and (lines[0].strip() == "" or lines[0].startswith("# ")):
            lines.pop(0)
    lines = drop_sections(lines, DROP.get(page, []))
    if page == "usage.md":
        lines += [""] + ARCH.split("\n")
    lines = insert(lines, page)
    text = "\n".join(lines).strip() + "\n"
    text = re.sub(r"\n{3,}", "\n\n", text)
    if page == "configuration.md":
        hdr = open(os.path.join(P, "..", "config-header-en.md"), encoding="utf-8").read()
        open(os.path.join(OUT, "configuration.md"), "w", encoding="utf-8", newline="\n").write(hdr.rstrip() + "\n\n" + text)
    elif page == "ai-upstream.md":
        open(os.path.join(P, "ai-upstream.final.md"), "w", encoding="utf-8", newline="\n").write(text)
    else:
        open(os.path.join(OUT, page), "w", encoding="utf-8", newline="\n").write(HEADERS[page] + text)
    print(page, text.count("\n"))
