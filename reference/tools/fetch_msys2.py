"""Downloads MSYS2 UCRT64 packages from the official repo (repo.msys2.org), checks the SHA-256 against
the repo database ucrt64.db and extracts only */bin/*.dll and share/misc/magic.mgc to <target>.
Call: python -I fetch_msys2.py <download-dir> <target-dir> <package>...
"""

import hashlib
import io
import sys
import tarfile
import urllib.request
from pathlib import Path

import zstandard

REPO = "https://repo.msys2.org/mingw/ucrt64/"


def zst_tar(data: bytes) -> tarfile.TarFile:
    return tarfile.open(fileobj=io.BytesIO(zstandard.ZstdDecompressor().stream_reader(io.BytesIO(data)).read()))


def main(dl: str, dest: str, *pkgs: str) -> None:
    dl_dir, dest_dir = Path(dl), Path(dest)
    dest_dir.mkdir(parents=True, exist_ok=True)
    db = zst_tar((dl_dir / "ucrt64.db").read_bytes())
    index = {}
    for m in db.getmembers():
        if m.name.endswith("/desc"):
            f, cur = {}, None
            for line in db.extractfile(m).read().decode().split("\n"):
                if line.startswith("%"):
                    cur = line.strip("%")
                    f[cur] = []
                elif line and cur:
                    f[cur].append(line)
            index[f["NAME"][0]] = f
    for name in pkgs:
        full = f"mingw-w64-ucrt-x86_64-{name}"
        f = index[full]
        fn, want = f["FILENAME"][0], f["SHA256SUM"][0]
        path = dl_dir / fn
        if not path.exists():
            urllib.request.urlretrieve(REPO + fn, path)
        got = hashlib.sha256(path.read_bytes()).hexdigest()
        if got != want:
            raise SystemExit(f"SHA-256 falsch für {fn}: {got} != {want}")
        print(f"OK {want}  {fn}")
        t = zst_tar(path.read_bytes())
        for m in t.getmembers():
            n = m.name
            if m.isfile() and ((n.startswith("ucrt64/bin/") and n.endswith(".dll")) or n == "ucrt64/share/misc/magic.mgc"):
                (dest_dir / Path(n).name).write_bytes(t.extractfile(m).read())
                print("   ->", Path(n).name)


if __name__ == "__main__":
    main(*sys.argv[1:])
