"""Phase 6 – fixes: median per (user, request, label) from runs.jsonl as a Markdown table.

  python -I tests/pgvalidate/fix_table.py results/pgvalidate/fix/runs.jsonl [before-label] [after-label]
"""
import json
import statistics
import sys
from collections import defaultdict


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    path = sys.argv[1]
    before = sys.argv[2] if len(sys.argv) > 2 else "rs-before"
    after = sys.argv[3] if len(sys.argv) > 3 else "rs-after"
    ms = defaultdict(list)
    status = {}
    order = []
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        k = (r["user"], r["name"])
        if k not in order:
            order.append(k)
        ms[(k, r["label"])].append(r["ms"])
        status[(k, r["label"])] = r["status"]

    def med(k, label):
        v = ms.get((k, label))
        return (statistics.median(v), len(v)) if v else (None, 0)

    print("| Anfrage | Nutzer | Django | Rust vorher | Rust nachher | Faktor vorher→nachher | n |")
    print("|---|---|---:|---:|---:|---:|---|")
    for k in order:
        d, nd = med(k, "django")
        b, nb = med(k, before)
        a, na = med(k, after)
        f = f"{b / a:.1f}×" if a and b else ""
        fmt = lambda x: f"{x:,.0f}".replace(",", " ") if x is not None else "–"
        print(f"| {k[1]} | {k[0]} | {fmt(d)} | {fmt(b)} | {fmt(a)} | {f} | {nd}/{nb}/{na} |")


if __name__ == "__main__":
    main()
