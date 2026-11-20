#!/usr/bin/env python3
"""Startkorpus für die Fuzz-Ziele aus vorhandenen Testdateien (upstream-Beispiele, Parität-Proben)
plus erzeugte Grenzfälle (ZIP mit ../-Namen, Abfragen, Vorlagen). Schreibt fuzz/corpus/<ziel>/."""

from __future__ import annotations

import hashlib
import io
import random
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
SRC_DIRS = [REPO / "upstream" / "src", REPO / "tests" / "parity", REPO / "crates"]
LIMIT = 300 * 1024
EXT = {
    "pdf_text": ["pdf"],
    "office": ["docx", "xlsx", "pptx", "odt", "ods", "odp", "odg", "doc", "xls", "ppt", "rtf"],
    "mail": ["eml", "msg"],
    "image": ["png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "bmp", "heic"],
}
MAX_PER_TARGET = {"pdf_text": 150}


def put(target: str, data: bytes) -> None:
    d = ROOT / "corpus" / target
    d.mkdir(parents=True, exist_ok=True)
    (d / hashlib.sha1(data).hexdigest()).write_bytes(data)


def files(exts: list[str]):
    for base in SRC_DIRS:
        for p in base.rglob("*"):
            if "node_modules" in p.parts or "target" in p.parts or ".work" in p.parts:
                continue
            if p.suffix.lower().lstrip(".") in exts and p.is_file() and p.stat().st_size <= LIMIT:
                yield p


def main() -> int:
    random.seed(4242)
    for target, exts in EXT.items():
        found = sorted(files(exts), key=lambda p: (p.stat().st_size, str(p)))
        cap = MAX_PER_TARGET.get(target)
        if cap and len(found) > cap:
            found = random.sample(found, cap)
        for p in found:
            put(target, p.read_bytes())
        print(target, len(found))
    # ZIP import: real export structure + edge cases (traversal, absolute paths, drive letter, duplicates)
    names = ["manifest.json", "metadata.json", "originals/a.pdf", "../escape.txt", "/abs/x.txt", "C:\\win\\x.txt",
             "a/../../b.txt", "..\\b.txt", "dir/", "x" * 300, "con.txt", "a:b.txt"]
    for method in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED, zipfile.ZIP_BZIP2, zipfile.ZIP_LZMA):
        for i in range(len(names)):
            b = io.BytesIO()
            with zipfile.ZipFile(b, "w", method) as z:
                for n in names[: i + 1]:
                    if n.endswith("/"):
                        z.writestr(zipfile.ZipInfo(n), b"")
                    else:
                        z.writestr(n, b'{"model": "documents.document", "pk": 1}' if n.endswith(".json") else b"data")
            put("zip_import", b.getvalue())
    # Document filter: real parameter combinations
    for q in ["title__icontains=abc", "created__date__gt=2020-01-01&created__date__lt=2021-02-30",
              "custom_field_query=%5B%22OR%22%2C%5B%5B1%2C%22exact%22%2C%22x%22%5D%5D%5D",
              "custom_field_query=[\"AND\",[[\"d\",\"range\",[\"2020-01-01\",\"2021-01-01\"]],[\"i\",\"gt\",5]]]",
              "tags__id__all=1,2,3&tags__id__none=4", "ordering=-custom_field_3,title", "search=foo bar \"baz qux\"",
              "created__year=2023&created__month=13", "added__gte=2024-01-01T00:00:00+02:00",
              "created__date__gte=today", "created=[-1 week to now]", "query=added:[-1 week to now]",
              "custom_fields__icontains=x&custom_field_query=[\"sel\",\"in\",[\"a1\"]]",
              "id__in=1,2,x&owner__id__none=1&shared_by__id=2", "title_content=%25'%20OR%201=1--",
              "mime_type=application/pdf&is_tagged=1&has_custom_fields=0", "ordering=" + "a," * 50]:
        put("docfilter", q.encode())
    # Templates
    for t in ["{{ title }}", "{{ correspondent }}/{{ created_year }}/{{ title }}", "{% for t in tag_list %}{{t}}{% endfor %}",
              "{{ custom_fields|get_cf_value('x', 'y') }}", "{{ created|datetime('%Y') }}", "{title}/{created}",
              "{% for i in range(999999999) %}x{% endfor %}", "{{ ''.__class__.__mro__ }}", "{{ self }}",
              "{{ ../../etc/passwd }}", "../../{{ title }}", "{{ title|slugify }}", "{% raw %}{{x}}{% endraw %}",
              "{{ document.owner.password }}", "{{ 'a' * 100000000 }}", "{% macro m() %}{{ m() }}{% endmacro %}{{ m() }}",
              "{{ doc_title }} {{ added_date }} {{ correspondent }}", "{doc_title}-{created_year}"]:
        put("templating", t.encode())
    return 0


if __name__ == "__main__":
    sys.exit(main())
