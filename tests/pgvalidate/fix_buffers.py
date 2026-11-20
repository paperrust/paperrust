"""Phase 6 – fixes: per request DB time (ms), round trips and buffers (shared hit+read of the topmost
plan nodes, from auto_explain) from the capture.py JSONs of fix_dbtime.sh.

  python -I tests/pgvalidate/fix_buffers.py results/pgvalidate/fix/dbtime
"""
import json,re,glob,os,collections,sys
res=collections.defaultdict(list)
for f in sorted(glob.glob(sys.argv[1]+'/*.json')):
    lab=os.path.basename(f).split('-r')[0]
    d=json.load(open(f,encoding='utf-8'))
    for u,es in d['users'].items():
        for e in es:
            buf=0
            for q in e['queries']:
                p=q.get('plan') or ''
                m=re.search(r'Buffers: shared([^\n]*)',p)
                if m:
                    for k in ('hit','read'):
                        mm=re.search(k+r'=(\d+)',m.group(1))
                        if mm: buf+=int(mm.group(1))
            res[(e['name'],u,lab)].append((round(e['db_ms']),e['roundtrips'],buf))
for n,u in sorted(set((k[0],k[1]) for k in res)):
    print(f"{n:26} {u:5} before={res[(n,u,'before')]}  after={res[(n,u,'after')]}")
