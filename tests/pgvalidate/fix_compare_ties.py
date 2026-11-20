"""O2 (pgvalidate.md section 9): compare responses of two runs tie-aware.

  python -I tests/pgvalidate/fix_compare_ties.py <bodies-dir> <label-a> <label-b> [sortkey=created]

For lists (`count`/`results`): equal if `count` is equal, the sequence of the sort keys
on the page is equal and the document ids differ only within the tie group at the
page edge (first/last key of the page); all other groups must have the same
id set and equal ids equal bodies. Output: same / tie / DIFF per file.
"""
import json
import os
import re
import sys
from collections import defaultdict


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    base, la, lb = sys.argv[1], sys.argv[2], sys.argv[3]
    key = sys.argv[4] if len(sys.argv) > 4 else "created"
    da, db = os.path.join(base, la), os.path.join(base, lb)
    tally = defaultdict(int)
    for f in sorted(os.listdir(da)):
        pb = os.path.join(db, f)
        if not os.path.exists(pb):
            continue
        a = json.load(open(os.path.join(da, f), encoding="utf-8"))
        b = json.load(open(pb, encoding="utf-8"))
        verdict, why = compare(a, b, key)
        tally[verdict] += 1
        print(f"{verdict:5} {f}{(': ' + why) if why else ''}")
    print(f"== {la} vs {lb}: " + ", ".join(f"{k} {v}" for k, v in sorted(tally.items())))


def norm(v):
    if isinstance(v, str):
        return re.sub(r"127\.0\.0\.1:\d+", "127.0.0.1:PORT", v)
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items()}
    return v


def strip_links(v):
    return {k: x for k, x in v.items() if k not in ("next", "previous")}


def compare(a, b, key):
    a, b = norm(a), norm(b)
    if a == b:
        return "same", ""
    if not (isinstance(a, dict) and isinstance(b, dict) and "results" in a and "results" in b):
        return "DIFF", "kein Listenkörper"
    if a.get("count") != b.get("count"):
        return "DIFF", f"count {a.get('count')} != {b.get('count')}"
    ra, rb = a["results"], b["results"]
    ka, kb = [r.get(key) for r in ra], [r.get(key) for r in rb]
    if ka != kb:
        return "DIFF", f"Folge {key} verschieden"
    edge = {ka[0], ka[-1]} if ka else set()
    ga, gb = defaultdict(set), defaultdict(set)
    for r in ra:
        ga[r.get(key)].add(r["id"])
    for r in rb:
        gb[r.get(key)].add(r["id"])
    for k in ga:
        if k not in edge and ga[k] != gb[k]:
            return "DIFF", f"Gruppe {key}={k} innen verschieden"
    by_a = {r["id"]: r for r in ra}
    for r in rb:
        if r["id"] in by_a and by_a[r["id"]] != r:
            return "DIFF", f"Dokument {r['id']} verschieden"
    rest_a, rest_b = strip_links(a), strip_links(b)
    rest_a.pop("results"), rest_b.pop("results")
    if rest_a != rest_b:
        return "DIFF", "übrige Felder verschieden"
    return "tie", f"nur Gleichstand {key} am Seitenrand"


if __name__ == "__main__":
    main()
