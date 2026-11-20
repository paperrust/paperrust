#!/usr/bin/env python3
"""Inhaltlicher Vergleich zweier document_exporter-Manifeste (docs/DATAMODEL-V2.md §3.3).

Datensätze werden über (model, pk) zugeordnet; Reihenfolge zählt nur mit --ordered. Felder lassen
sich ausnehmen (--ignore model.feld, z. B. documents.document.modified: auto_now beim Import, bei
upstream ebenso). Reihenfolge ist z. B. nach einem Datenbank-Umzug für Modelle ohne Meta.ordering
(guardian, CustomFieldInstance) die physische Zeilenfolge – auch upstream exportiert sie so.

Aufruf: manifest_eq.py A/manifest.json B/manifest.json [--ordered] [--any-order model,…] [--ignore model.feld ...]
  --any-order: mit --ordered für diese Modelle die Reihenfolge nicht prüfen (z. B. guardian, deren
               Reihenfolge upstream von der Join-Planung abhängt)
Exit 0 = gleich, 1 = verschieden (mit den ersten Unterschieden).
"""
import json
import sys


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    a_path, b_path, rest = argv[0], argv[1], argv[2:]
    ordered = "--ordered" in rest
    ignore = set()
    any_order = set()
    for i, x in enumerate(rest):
        if x == "--ignore":
            ignore.update(rest[i + 1].split(","))
        if x == "--any-order":
            any_order.update(rest[i + 1].split(","))
    a = json.load(open(a_path, encoding="utf-8"))
    b = json.load(open(b_path, encoding="utf-8"))

    def key(r):
        return (r["model"], str(r.get("pk")))

    def strip(r):
        f = {k: v for k, v in r.get("fields", {}).items() if f"{r['model']}.{k}" not in ignore}
        return {**r, "fields": f}

    diffs = []
    if ordered and [key(r) for r in a if r["model"] not in any_order] != [key(r) for r in b if r["model"] not in any_order]:
        diffs.append("record order differs")
    am = {key(r): strip(r) for r in a}
    bm = {key(r): strip(r) for r in b}
    for k in am.keys() - bm.keys():
        diffs.append(f"only in A: {k}")
    for k in bm.keys() - am.keys():
        diffs.append(f"only in B: {k}")
    for k in sorted(am.keys() & bm.keys()):
        if am[k] != bm[k]:
            fa, fb = am[k].get("fields", {}), bm[k].get("fields", {})
            fields = sorted(f for f in set(fa) | set(fb) if fa.get(f) != fb.get(f)) or ["(record)"]
            diffs.append(f"{k}: {', '.join(fields)}")
    print(f"{len(a)} vs {len(b)} records, {len(diffs)} differences")
    for d in diffs[:15]:
        print("  " + d)
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
