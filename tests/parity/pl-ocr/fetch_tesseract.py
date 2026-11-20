"""Fetches Tesseract (+ eng/osd language data) as MSYS2 UCRT64 packages from the official repo
(repo.msys2.org) to tests/parity/pl-ocr/.tools/tesseract (git-ignored).

Every package file is checked against the SHA-256 from the repo database ucrt64.db (like
reference/tools/fetch_msys2.py). Dependencies are resolved from %DEPENDS%.

Invocation (Git Bash, from the repo root directory):
    reference/venv/Scripts/python.exe -I tests/parity/pl-ocr/fetch_tesseract.py [--dry-run]
"""

import hashlib
import io
import re
import sys
import tarfile
import urllib.request
from pathlib import Path

import zstandard

REPO = "https://repo.msys2.org/mingw/ucrt64/"
HERE = Path(__file__).resolve().parent
TOOLS = HERE / ".tools"
DL = TOOLS / "downloads"
DEST = TOOLS / "tesseract"
ROOTS = ["tesseract-ocr", "tesseract-data-eng", "tesseract-data-osd", "tesseract-data-deu"]
PREFIX = "mingw-w64-ucrt-x86_64-"
# Dependencies not needed at runtime (only for training tools/scripts).
SKIP = {"python", "icu", "pango", "cairo", "fontconfig", "freetype", "glib2", "harfbuzz"}


def zst_tar(data: bytes) -> tarfile.TarFile:
    raw = zstandard.ZstdDecompressor().stream_reader(io.BytesIO(data)).read()
    return tarfile.open(fileobj=io.BytesIO(raw))


def load_index(db_path: Path) -> dict:
    db = zst_tar(db_path.read_bytes())
    index = {}
    provides = {}
    for m in db.getmembers():
        if not m.name.endswith("/desc"):
            continue
        f, cur = {}, None
        for line in db.extractfile(m).read().decode().split("\n"):
            if line.startswith("%"):
                cur = line.strip("%")
                f[cur] = []
            elif line and cur:
                f[cur].append(line)
        index[f["NAME"][0]] = f
        for p in f.get("PROVIDES", []):
            provides[re.split(r"[<>=]", p)[0]] = f["NAME"][0]
    return index, provides


def closure(index, provides, roots):
    todo = [PREFIX + r for r in roots]
    seen = []
    while todo:
        n = todo.pop()
        n = n if n in index else provides.get(n, n)
        if n in seen or n not in index:
            continue
        short = n.removeprefix(PREFIX)
        if short in SKIP:
            continue
        seen.append(n)
        for d in index[n].get("DEPENDS", []):
            todo.append(re.split(r"[<>=]", d)[0])
    return seen


def main(dry: bool) -> None:
    DL.mkdir(parents=True, exist_ok=True)
    db = DL / "ucrt64.db"
    if not db.exists():
        urllib.request.urlretrieve(REPO + "ucrt64.db", db)
    index, provides = load_index(db)
    pkgs = closure(index, provides, ROOTS)
    total = sum(int(index[p]["CSIZE"][0]) for p in pkgs)
    for p in pkgs:
        print(f"{int(index[p]['CSIZE'][0]) / 1e6:8.2f} MB  {index[p]['FILENAME'][0]}")
    print(f"total {total / 1e6:.1f} MB in {len(pkgs)} packages")
    if dry:
        return
    (DEST / "bin").mkdir(parents=True, exist_ok=True)
    (DEST / "tessdata").mkdir(parents=True, exist_ok=True)
    for p in pkgs:
        fn, want = index[p]["FILENAME"][0], index[p]["SHA256SUM"][0]
        path = DL / fn
        if not path.exists():
            urllib.request.urlretrieve(REPO + fn, path)
        got = hashlib.sha256(path.read_bytes()).hexdigest()
        if got != want:
            raise SystemExit(f"SHA-256 mismatch for {fn}: {got} != {want}")
        print(f"OK {want}  {fn}")
        t = zst_tar(path.read_bytes())
        for m in t.getmembers():
            n = m.name
            if not m.isfile():
                continue
            if n.startswith("ucrt64/bin/") and (n.endswith(".dll") or n.endswith("/tesseract.exe")):
                (DEST / "bin" / Path(n).name).write_bytes(t.extractfile(m).read())
            elif n.startswith("ucrt64/share/tessdata/") and n.count("/") == 3:
                (DEST / "tessdata" / Path(n).name).write_bytes(t.extractfile(m).read())


if __name__ == "__main__":
    main("--dry-run" in sys.argv[1:])
