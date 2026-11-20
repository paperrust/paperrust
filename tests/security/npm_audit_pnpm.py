#!/usr/bin/env python3
"""`npm audit` für ein pnpm-Projekt (frontend-light hat pnpm-lock.yaml, kein package-lock.json).

Liest alle Paketversionen aus pnpm-lock.yaml (Abschnitt `packages:`) und fragt dieselbe
Schnittstelle wie `npm audit` ab (POST registry.npmjs.org/-/npm/v1/security/advisories/bulk).
Gesendet werden nur Paketnamen und Versionen.

    python tests/security/npm_audit_pnpm.py frontend-light/pnpm-lock.yaml [--json out.json]
Exit 0 = keine Meldung ab "moderate", 1 = Meldungen.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.request
from collections import defaultdict
from pathlib import Path

URL = "https://registry.npmjs.org/-/npm/v1/security/advisories/bulk"
ORDER = ["info", "low", "moderate", "high", "critical"]


def packages(lock: Path) -> dict[str, set[str]]:
    out: dict[str, set[str]] = defaultdict(set)
    in_pk = False
    for line in lock.read_text(encoding="utf-8").splitlines():
        if line.startswith("packages:"):
            in_pk = True
            continue
        if in_pk and line and not line.startswith(" "):
            break
        m = re.match(r"^  '?(@?[^@'\s][^@'\s]*)@([^('\s:]+)", line) if in_pk else None
        if m:
            out[m.group(1)].add(m.group(2))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("lock")
    ap.add_argument("--json")
    a = ap.parse_args()
    pk = packages(Path(a.lock))
    body = json.dumps({k: sorted(v) for k, v in pk.items()}).encode()
    req = urllib.request.Request(URL, data=body, method="POST",
                                 headers={"content-type": "application/json", "user-agent": "npm-audit-client"})
    with urllib.request.urlopen(req, timeout=60) as r:
        adv = json.loads(r.read())
    if a.json:
        Path(a.json).write_text(json.dumps({"packages": len(pk), "advisories": adv}, indent=1), encoding="utf-8")
    n = 0
    print(f"{len(pk)} Pakete, {sum(len(v) for v in pk.values())} Versionen geprüft")
    for name, items in sorted(adv.items()):
        for it in items:
            sev = it.get("severity", "?")
            hit = sorted(v for v in pk[name])
            print(f"{sev:9} {name} {','.join(hit)}  betroffen {it.get('vulnerable_versions')}  {it.get('title')}  {it.get('url')}")
            if ORDER.index(sev) >= ORDER.index("moderate") if sev in ORDER else True:
                n += 1
    return 1 if n else 0


if __name__ == "__main__":
    sys.exit(main())
