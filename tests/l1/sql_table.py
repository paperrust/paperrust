"""Summarize the SQL measurement (docquery_diff PL_L1_TIMING).
python sql_table.py before.csv [after.csv]
before: columns legacy_* = before; after (optional, with index): ast_* = after. Without after: legacy vs ast from one file.
"""
import csv
import sys


def load(p):
    out = {}
    with open(p, encoding='utf-8') as f:
        for r in csv.DictReader(f):
            out[(r['user'], r['query'])] = r
    return out


b = load(sys.argv[1])
a = load(sys.argv[2]) if len(sys.argv) > 2 else b
print('| Nutzer | Anfrage | Seite vorher | Seite nachher | COUNT vorher | COUNT nachher |')
print('|---|---|---:|---:|---:|---:|')
tot = [0.0, 0.0, 0.0, 0.0]
for k in b:
    if k not in a:
        continue
    r0, r1 = b[k], a[k]
    v = [float(r0['legacy_page_ms']), float(r1['ast_page_ms']), float(r0['legacy_count_ms']), float(r1['ast_count_ms'])]
    for i in range(4):
        tot[i] += v[i]
    q = k[1].replace('ordering=', '').replace('&', ' ')
    print(f"| {k[0]} | `{q}` | {v[0]:.0f} | {v[1]:.0f} | {v[2]:.0f} | {v[3]:.0f} |")
print(f"| **Summe** | | **{tot[0]:.0f}** | **{tot[1]:.0f}** | **{tot[2]:.0f}** | **{tot[3]:.0f}** |")
