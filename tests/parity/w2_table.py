"""Route table over several W2 reports: route -> identical / accepted / different (+ cases).

    python tests/parity/w2_table.py results/parity/w2/<a>.json [<b>.json ...]
A route counts as "identisch" when every compared case is identical or only accepted.
"""
import json
import sys
from collections import defaultdict

per = defaultdict(lambda: defaultdict(list))
for p in sys.argv[1:]:
    for r in json.load(open(p, encoding="utf-8"))["results"]:
        per[(r["crate"], r["route"])][r["outcome"]].append(r["id"])
print("| crate | id | Fälle | Ergebnis |")
print("|---|---|---|---|")
for (crate, route), o in sorted(per.items()):
    n = sum(len(v) for k, v in o.items() if k != "skipped")
    bad = o.get("different", []) + o.get("error", [])
    if bad:
        res = "abweichend: " + ", ".join(sorted(set(x.split("@")[0] for x in bad)))
    elif n:
        res = "identisch" + (f" ({len(o['accepted'])} akzeptiert)" if o.get("accepted") else "")
    else:
        res = "nur übersprungen"
    sk = len(o.get("skipped", []))
    print(f"| {crate} | `{route}` | {n}{f' (+{sk} übersprungen)' if sk else ''} | {res} |")
