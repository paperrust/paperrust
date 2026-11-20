"""Checks downloaded packages against the SHA-256 values from upstream/uv.lock.

Platform-independent files (py3-none-any, sdist) are in the lock with a hash and must
match. Windows wheels are not in the lock (upstream locks only linux/darwin);
for them the SHA-256 is logged (source: PyPI over HTTPS, version from the lock).
Call: python -I verify_wheels.py <uv.lock> <wheel-dir> <report.txt>
"""

import hashlib
import sys
import tomllib
from pathlib import Path


def main(lock: str, wheels: str, report: str) -> int:
    data = tomllib.loads(Path(lock).read_text(encoding="utf-8"))
    known = {}
    for pkg in data.get("package", []):
        for entry in [pkg.get("sdist")] + list(pkg.get("wheels", [])):
            if entry and "url" in entry and "hash" in entry:
                known[entry["url"].rsplit("/", 1)[-1]] = entry["hash"].removeprefix("sha256:")
    lines, bad = [], 0
    ok = other = 0
    for f in sorted(Path(wheels).iterdir()):
        digest = hashlib.sha256(f.read_bytes()).hexdigest()
        if f.name in known:
            if known[f.name] == digest:
                ok += 1
                lines.append(f"LOCK-OK   {digest}  {f.name}")
            else:
                bad += 1
                lines.append(f"MISMATCH  {digest}  {f.name} (lock: {known[f.name]})")
        else:
            other += 1
            lines.append(f"PYPI-ONLY {digest}  {f.name}")
    head = [
        "# Herkunft der Python-Pakete der Referenz (generiert von tools/verify_wheels.py)",
        f"# LOCK-OK={ok} (Hash identisch mit upstream/uv.lock), PYPI-ONLY={other} "
        f"(Windows-Wheel, nicht im Lock; Version aus Lock, Quelle pypi.org), MISMATCH={bad}",
    ]
    Path(report).write_text("\n".join(head + lines) + "\n", encoding="utf-8", newline="\n")
    print(head[1])
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:4]))
