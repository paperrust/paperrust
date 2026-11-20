"""Phase 6: evaluation of the captures (capture.py) – round trips, DB time, plan findings.

  python -I tests/pgvalidate/analyze.py results/pgvalidate/capture/rs-explain.json \
      [--dj results/pgvalidate/capture/dj-sql.json] [--rs-time …rs-time.json --dj-time …dj-time.json] \
      --explain-dir results/pgvalidate/explain --md results/pgvalidate/summary.md

Plan heuristic (auto_explain text):
  SEQ   seq scan on a large table (> 10,000 rows in pl_pgvalidate)
  LOOP  SubPlan/nested loop with loops > 50 over a large table (correlated subquery)
  SORT  sort without index over > 10,000 rows or external merge
  N+1   same SQL shape > 3× within one request
"""
import argparse
import json
import os
import re
from collections import Counter

BIG = ["documents_document", "documents_document_tags", "documents_customfieldinstance",
       "documents_note", "auditlog_logentry", "guardian_userobjectpermission",
       "guardian_groupobjectpermission", "documents_paperlesstask"]


def shape(sql):
    s = re.sub(r"\$\d+", "?", sql)
    s = re.sub(r"'[^']*'", "?", s)
    s = re.sub(r"\b\d+\b", "?", s)
    s = re.sub(r"(\?\s*,\s*)+\?", "?", s)
    return re.sub(r"\s+", " ", s).strip()


def flags(plan):
    out = set()
    if not plan:
        return out
    for m in re.finditer(r"Seq Scan on (\w+)[^\n]*actual time=[\d.]+\.\.[\d.]+ rows=([\d.]+) loops=(\d+)", plan):
        t, rows, loops = m.group(1), float(m.group(2)), int(m.group(3))
        if t in BIG:
            out.add(f"SEQ:{t}" + (f"x{loops}" if loops > 1 else ""))
    for m in re.finditer(r"(SubPlan \d+|Nested Loop)[\s\S]{0,400}?loops=(\d+)", plan):
        if int(m.group(2)) > 50:
            out.add("LOOP")
            break
    for m in re.finditer(r"Sort  \(cost=[^\n]*rows=([\d.]+) loops=(\d+)\)\n\s*Sort Key: ([^\n]+)\n\s*Sort Method: ([^\n]+)", plan):
        if float(m.group(1)) * int(m.group(2)) > 10000 or "external" in m.group(4):
            out.add(f"SORT:{m.group(3)[:40]}")
    return out


def plan_ms(plan):
    m = re.search(r"duration: ([\d.]+) ms", plan or "")
    return float(m.group(1)) if m else None


def load(p):
    return json.load(open(p, encoding="utf-8")) if p and os.path.exists(p) else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rs")
    ap.add_argument("--dj")
    ap.add_argument("--rs-time")
    ap.add_argument("--dj-time")
    ap.add_argument("--explain-dir")
    ap.add_argument("--md")
    a = ap.parse_args()
    rs, dj, rst, djt = load(a.rs), load(a.dj), load(a.rs_time), load(a.dj_time)

    def by_name(doc, user):
        return {e["name"]: e for e in (doc or {}).get("users", {}).get(user, [])} if doc else {}

    lines = []
    if a.explain_dir:
        os.makedirs(a.explain_dir, exist_ok=True)
    for user, entries in rs["users"].items():
        djn, rstn, djtn = by_name(dj, user), by_name(rst, user), by_name(djt, user)
        lines.append(f"\n### {user}\n")
        lines.append("| Anfrage | Status | RT Rust | RT Django | DB-ms Rust (explain) | Wand ms Rust (warm, min aus 2) | Wand ms Django (warm, min aus 2) | langsamste Rust-Abfrage ms | Plan-Befunde |")
        lines.append("|---|---|---|---|---|---|---|---|---|")
        for e in entries:
            qs = e["queries"]
            fl = set()
            slow = max(qs, key=lambda q: q["ms"] or 0) if qs else None
            for i, q in enumerate(qs):
                fl |= flags(q.get("plan"))
                if a.explain_dir and q.get("plan"):
                    fn = os.path.join(a.explain_dir, f"{user}__{e['name']}__{i:02d}.txt")
                    with open(fn, "w", encoding="utf-8") as f:
                        f.write(f"-- {e['method']} {e['path']}\n-- rows={q.get('rows')} ms={q.get('ms')}\n")
                        f.write(q["plan"])
            c = Counter(shape(q["sql"]) for q in qs)
            n1 = [n for n in c.values() if n > 3]
            if n1:
                fl.add(f"N+1:{max(n1)}x")
            d = djn.get(e["name"])
            rt_dj = d["roundtrips"] if d else ""
            wr = min(rstn[e["name"]]["ms_all"]) if e["name"] in rstn else ""
            wd = min(djtn[e["name"]]["ms_all"]) if e["name"] in djtn else ""
            lines.append(f"| {e['name']} | {e['status']} | {e['roundtrips']} | {rt_dj} | {e['db_ms']} | {wr} | {wd} | "
                         f"{round(slow['ms'], 1) if slow else ''} | {', '.join(sorted(fl))} |")
    md = "\n".join(lines)
    if a.md:
        open(a.md, "w", encoding="utf-8").write(md)
    print(md)


if __name__ == "__main__":
    main()
