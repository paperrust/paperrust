"""Median per (user, request) for legacy/ast from several runs.jsonl; hash equality of the responses.
python l1_table.py <label=runs.jsonl> ...   (e.g. noidx=bench_noidx/runs.jsonl idx=bench_idx/runs.jsonl)
"""
import json
import statistics
import sys
from collections import defaultdict

sets = []
for arg in sys.argv[1:]:
    name, path = arg.split('=', 1)
    d = defaultdict(list)
    sha = defaultdict(set)
    for line in open(path, encoding='utf-8'):
        r = json.loads(line)
        d[(r['label'], r['user'], r['name'])].append(r['ms'])
        sha[(r['user'], r['name'])].add(r['sha'])
    sets.append((name, d, sha))

keys = sorted({(u, n) for _, d, _ in sets for (_, u, n) in d})
cols = [f"{s}/{lab}" for s, _, _ in sets for lab in ('legacy', 'ast')]
print('| Nutzer | Anfrage | ' + ' | '.join(cols) + ' | Antwort gleich |')
print('|---|---|' + '---:|' * len(cols) + '---|')
tot = defaultdict(float)
for u, n in keys:
    vals = []
    same = True
    for s, d, sha in sets:
        for lab in ('legacy', 'ast'):
            v = d.get((lab, u, n))
            m = statistics.median(v) if v else None
            if m is not None:
                tot[(s, lab, u)] += m
            vals.append('–' if m is None else f"{m:,.0f}".replace(',', ' '))
        if len(sha.get((u, n), ())) > 1:
            same = False
    print(f"| {u} | {n} | " + ' | '.join(vals) + f" | {'ja' if same else 'NEIN'} |")
print()
for u in sorted({u for u, _ in keys}):
    print(f"Summe {u}: " + ', '.join(f"{s}/{lab} {tot[(s, lab, u)]/1000:.1f} s" for s, _, _ in sets for lab in ('legacy', 'ast')))
