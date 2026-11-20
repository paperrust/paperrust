"""Convert upstream paperless-ngx mkdocs markdown to GitHub-flavoured markdown for PaperRust.

Usage: python -I mk2gh.py <in.md> <out.md> <page-kind> [rust_vars.txt]
- headings with {#id} -> <a id="id"></a> line + heading
- definition lists (": text" + 4-space continuation) -> plain paragraphs
- admonitions (!!! kind "title") -> GitHub alerts (> [!NOTE] ...)
- links to other upstream pages -> PaperRust page names
- product name in prose -> PaperRust (not in code, URLs, paths)
- images dropped (upstream screenshots carry paperless-ngx branding)
- optional: per-variable PaperRust status line (configuration page)
"""
import re
import sys

PAGE_MAP = {
    "usage.md": "usage.md",
    "advanced_usage.md": "advanced.md",
    "configuration.md": "configuration.md",
    "administration.md": "administration.md",
    "setup.md": "installation.md",
    "api.md": "api.md",
    "faq.md": "faq.md",
    "troubleshooting.md": "troubleshooting.md",
    "index.md": "README.md",
}
UPSTREAM_DOCS = "https://docs.paperless-ngx.com/"

ALERT = {
    "note": "NOTE", "info": "NOTE", "tip": "TIP", "hint": "TIP",
    "warning": "WARNING", "caution": "CAUTION", "danger": "CAUTION",
    "failure": "CAUTION", "important": "IMPORTANT", "example": "NOTE",
    "abstract": "NOTE", "question": "NOTE", "success": "TIP",
}


def map_link(target: str) -> str:
    if target.startswith(("http://", "https://", "mailto:", "#")):
        return target
    m = re.match(r"^([A-Za-z0-9_-]+\.md)(#.*)?$", target)
    if m:
        page, anchor = m.group(1), m.group(2) or ""
        if page in PAGE_MAP:
            return PAGE_MAP[page] + anchor
        # pages PaperRust does not carry (changelog, development, migration-v3)
        return UPSTREAM_DOCS + page[:-3] + "/" + anchor
    return target


NAME_RE = re.compile(r"\bPaperless-ngx\b|\bpaperless-ngx\b|\bPaperless\b(?![-_./:@])|\bpaperless\b(?![-_./:@])")


def rename_prose(seg: str) -> str:
    # do not touch link targets / URLs
    out, pos = [], 0
    for m in re.finditer(r"\]\([^)]*\)|https?://\S+|<[^>]+>", seg):
        out.append(NAME_RE.sub("PaperRust", seg[pos:m.start()]))
        out.append(m.group(0))
        pos = m.end()
    out.append(NAME_RE.sub("PaperRust", seg[pos:]))
    return "".join(out)


def fix_inline(line: str) -> str:
    # links
    line = re.sub(r"\]\(([^)\s]+)\)", lambda m: "](" + map_link(m.group(1)) + ")", line)
    # drop images
    line = re.sub(r"!\[[^\]]*\]\([^)]*\)(\{[^}]*\})?", "", line)
    # rename outside code spans
    parts = re.split(r"(`[^`]*`)", line)
    return "".join(p if p.startswith("`") else rename_prose(p) for p in parts)


def heading(line: str):
    m = re.match(r"^(#{1,6}) (.*?)\s*\{#([A-Za-z0-9_-]+)\}\s*$", line)
    if m:
        hashes, text, hid = m.groups()
        # [`X`](#X) -> `X`
        text = re.sub(r"^\[(.*)\]\(#[^)]*\)$", r"\1", text)
        tag = f'<a id="{hid}"></a>'
        if hid != hid.lower():
            tag += f'<a id="{hid.lower()}"></a>'
        return [tag, "", f"{hashes} {text}"], hid
    return None, None


def dedent_block(lines, i, n=4, lazy=False):
    """collect following lines that are blank or indented >= n (plus lazy paragraph
    continuation lines directly after a non-blank line); return (block, next_i)"""
    block = []
    while i < len(lines):
        l = lines[i]
        if l.strip() == "" or l.startswith(" " * n):
            block.append(l[n:] if l.startswith(" " * n) else "")
            i += 1
        elif lazy and block and block[-1].strip() != "" and not re.match(r"^(#|: |!!!|```)", l):
            block.append(l)
            i += 1
        elif lazy and not block and not re.match(r"^(#|: |!!!|```)", l):
            block.append(l)
            i += 1
        else:
            break
    while block and block[-1] == "":
        block.pop()
    return block, i


def convert(lines):
    out = []
    i = 0
    in_fence = False
    while i < len(lines):
        l = lines[i]
        if re.match(r"^\s*```", l):
            in_fence = not in_fence
            out.append(l)
            i += 1
            continue
        if in_fence:
            out.append(l)
            i += 1
            continue
        m = re.match(r'^(!!!|\?\?\?\+?) (\w+)(?: "([^"]*)")?\s*$', l)
        if m:
            kind, title = m.group(2).lower(), m.group(3)
            body, i = dedent_block(lines, i + 1)
            body = convert(body)
            out.append(f"> [!{ALERT.get(kind, 'NOTE')}]")
            if title:
                out.append(f"> **{fix_inline(title)}**")
                out.append(">")
            # strip leading blank
            while body and body[0].strip() == "":
                body.pop(0)
            for b in body:
                out.append("> " + b if b.strip() else ">")
            out.append("")
            continue
        if l.startswith(": "):
            body, i = dedent_block(lines, i + 1, lazy=True)
            out.extend(convert([l[2:]] + body))
            out.append("")
            continue
        h, _ = heading(l)
        if h:
            out.extend(fix_inline(x) if x.startswith("#") else x for x in h)
            i += 1
            continue
        out.append(fix_inline(l))
        i += 1
    return out


def main():
    src, dst = sys.argv[1], sys.argv[2]
    kind = sys.argv[3] if len(sys.argv) > 3 else ""
    rust_vars = set()
    if len(sys.argv) > 4:
        rust_vars = set(open(sys.argv[4], encoding="utf-8").read().split())
    lines = open(src, encoding="utf-8").read().split("\n")
    # drop the first H1 (caller writes its own header)
    if lines and lines[0].startswith("# "):
        lines = lines[1:]
    res = convert(lines)
    if kind == "configuration":
        final = []
        for idx, l in enumerate(res):
            final.append(l)
            m = re.match(r"^#### `(PAPERLESS_[A-Z0-9_]+)", l)
            if m:
                var = m.group(1)
                final.append("")
                final.append(status_line(var, var in rust_vars))
        res = final
    open(dst, "w", encoding="utf-8", newline="\n").write("\n".join(res).rstrip() + "\n")


OVERRIDES = {
    # var prefix / name -> status text
    "PAPERLESS_TIKA_": "not needed: PaperRust extracts text from Office/mail files itself (no Tika, no Gotenberg). See [Known differences](#paperrust-differences).",
    "PAPERLESS_WEBSERVER_WORKERS": "not used: PaperRust is a single multi-threaded process (Tokio uses all CPU cores; `TOKIO_WORKER_THREADS` limits it).",
    "PAPERLESS_BIND_ADDR": "not used by PaperRust itself: the listen address is `PAPERRUST_BIND` (default `127.0.0.1:8080`). Only relevant for the upstream web server.",
    "PAPERLESS_PORT": "not used by PaperRust itself: the listen address is `PAPERRUST_BIND`. Only relevant for the upstream web server.",
    "PAPERLESS_DBENGINE": "PaperRust supports **only `postgresql`**. SQLite/MariaDB installations move with `paperrust-migrate-db` first, see [Switching from paperless-ngx](migration.md).",
    "PAPERLESS_DB_READ_CACHE_": "not read by PaperRust (no cachalot read cache in the Rust code); in mixed operation it still affects Django. Remember `invalidate_cachalot` after a database move.",
    "PAPERLESS_OCR_CLEAN": "read, but not carried out yet: PaperRust has no unpaper; it logs a warning instead (docs/contract/pl-ocr.md, known difference 2).",
    "PAPERLESS_OCR_DESKEW": "read, but not carried out yet: no deskewing (Leptonica); PaperRust logs a warning (docs/contract/pl-ocr.md, known difference 2).",
    "PAPERLESS_OCR_OUTPUT_TYPE": "read. Without Ghostscript, PDF/A output only stamps the PDF/A metadata; the file is not rewritten or optimised (docs/contract/pl-ocr.md, known difference 3).",
    "PAPERLESS_ENABLE_GPG_DECRYPTOR": "not implemented: encrypted mails are processed unchanged (docs/contract/pl-mail-worker.md, known difference 2).",
    "PAPERLESS_EMAIL_GNUPG_HOME": "not used: GPG decryption is not implemented in PaperRust.",
    "PAPERLESS_ENABLE_FLOWER": "not applicable: PaperRust has no Celery and therefore no Flower.",
}


def status_line(var, read):
    for k, v in OVERRIDES.items():
        if var == k or (k.endswith("_") and var.startswith(k)):
            return f"*PaperRust: {v}*"
    if read:
        return "*PaperRust: read by PaperRust with the same meaning.*"
    return ("*PaperRust: not read by the Rust code (source search, 2026-11-09). It still affects the "
            "upstream Django/Celery processes in mixed operation; without Django it has no effect.*")


if __name__ == "__main__":
    main()
