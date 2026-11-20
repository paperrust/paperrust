"""Generates reference/requirements-windows.txt from upstream/uv.lock (via `uv export`).

Approach: `uv export --frozen` yields the locked versions with environment markers
(upstream locks only for linux/darwin). We evaluate the markers as they apply in the
Docker image (linux, x86_64, CPython 3.12), take over exactly these versions
and replace/remove only what cannot be installed on Windows or is not needed.
Call: python -I make_requirements.py <export.txt> <output.txt>
"""

import sys

from packaging.markers import Marker
from packaging.requirements import Requirement

LINUX_ENV = {
    "sys_platform": "linux",
    "platform_system": "Linux",
    "platform_machine": "x86_64",
    "os_name": "posix",
    "python_version": "3.12",
    "python_full_version": "3.12.10",
    "implementation_name": "cpython",
    "platform_python_implementation": "CPython",
}

# Package -> reason (not installed)
EXCLUDE = {
    "torch": "nur paperless_ai (lokale Embeddings); ~1 GB, AI ist in der Referenz aus",
    "sentence-transformers": "zieht torch; nur paperless_ai",
    "transformers": "nur via sentence-transformers",
    "tokenizers": "nur via transformers",
    "safetensors": "nur via transformers",
    "llama-index-embeddings-huggingface": "braucht sentence-transformers; nur paperless_ai",
    "uvloop": "gibt es nicht für Windows (granian[uvloop]-Extra); granian läuft ohne",
    "psycopg-c": "C-Build unter Windows nicht möglich -> psycopg-binary gleicher Version",
}
# Replacement (additionally installed)
EXTRA = [
    ("psycopg-binary==3.3.4", "Windows-Wheel derselben psycopg-Version 3.3.4 statt psycopg-c"),
    ("pywin32==311", "Windows-Abhängigkeit von portalocker (concurrent-log-handler); nicht im Linux-Lock"),
]


def main(src: str, dst: str, extra: str = "yes") -> None:
    out = []
    seen = {}
    for raw in open(src, encoding="utf-8"):
        line = raw.strip()
        if not line or line.startswith("#") or line.startswith("-"):
            continue
        req = Requirement(line)
        if req.marker is not None and not req.marker.evaluate(LINUX_ENV):
            continue
        name = req.name.lower()
        if name in EXCLUDE:
            out.append(f"# ausgelassen: {line.split(';')[0].strip()}  ({EXCLUDE[name]})")
            continue
        spec = str(req.specifier)
        if req.url:
            raise SystemExit(f"unerwartete URL-Anforderung: {line}")
        ver = spec.removeprefix("==")
        if "+" in ver:  # torch==x+cpu etc.
            ver = ver.split("+")[0]
        if name in seen and seen[name] != ver:
            raise SystemExit(f"Konflikt {name}: {seen[name]} vs {ver}")
        seen[name] = ver
        out.append(f"{req.name}=={ver}")
    for req, why in (EXTRA if extra == "yes" else []):
        out.append(f"{req}  # {why}")
    header = [
        "# GENERIERT von reference/tools/make_requirements.py aus upstream/uv.lock (v3.4.2).",
        "# Versionen = Linux-x86_64/CPython-3.12-Auflösung des Locks (wie Docker-Image).",
        "# Installation: pip install --no-deps -r requirements-windows.txt",
    ]
    open(dst, "w", encoding="utf-8", newline="\n").write("\n".join(header + out) + "\n")
    print(f"{len(seen)} Pakete, {sum(1 for o in out if o.startswith('# ausgelassen'))} ausgelassen")


if __name__ == "__main__":
    main(*sys.argv[1:4])
