"""Summarise a W2 report: per case outcome + compact diffs.  python w2_summary.py [report.json] [--routes]"""
import json
import sys
from collections import defaultdict
from pathlib import Path

args = [a for a in sys.argv[1:] if not a.startswith("--")]
d = Path(__file__).resolve().parents[2] / "results" / "parity" / "w2"
p = Path(args[0]) if args else max(d.glob("20*.json"))
rep = json.loads(p.read_text(encoding="utf-8"))
print(p.name, rep["summary"])
if "--routes" in sys.argv:
    per = defaultdict(lambda: defaultdict(int))
    for r in rep["results"]:
        per[r["route"]][r["outcome"]] += 1
    for route in sorted(per):
        print(f"  {route:40s} {dict(per[route])}")
    sys.exit(0)
for r in rep["results"]:
    if r["outcome"] in ("identical",):
        continue
    print(f"== {r['id']}  [{r['outcome']}] {r.get('reason', '')}")
    for s in r.get("steps", []):
        a, b = s.get("a", {}), s.get("b", {})
        if a.get("status") != b.get("status"):
            print(f"   step {s['method']} {s['url']}: A {a.get('status')} B {b.get('status')}")
    for x in r.get("diffs", [])[:12]:
        print(f"   {x.get('kind')} {x.get('path')}: A={str(x.get('a'))[:160]!s} | B={str(x.get('b'))[:160]!s}")
    if len(r.get("diffs", [])) > 12:
        print(f"   ... {len(r['diffs']) - 12} more")
