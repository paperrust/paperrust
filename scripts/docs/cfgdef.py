import re,sys,json
t=open(sys.argv[1],encoding='utf-8').read().split('\n')
start=next(i for i,l in enumerate(t) if l.startswith('# Reference'))
rs=[i for i,l in enumerate(t) if l=='## Required services']; end=rs[1]
out=[];cur=None;pend=[];sec=None
for l in t[start:end]:
    m=re.findall(r'<a id="([^"]+)"></a>',l)
    if m: pend=m; continue
    h=re.match(r'^(#{2,3}) (.*)',l)
    if h: out.append({'type':'sec','lvl':len(h.group(1)),'title':h.group(2),'ids':pend});pend=[];cur=None;continue
    if l.startswith('#### '):
        cur={'type':'var','head':l[5:].strip(),'ids':pend,'status':'','text':[]};pend=[];out.append(cur);continue
    if cur:
        if l.startswith('*PaperRust:'): cur['status']=l[len('*PaperRust: '):].rstrip('*')
        else: cur['text'].append(l)
for o in out:
    if o['type']=='var':
        txt=' '.join(x.strip() for x in o['text'] if x.strip())
        d=re.findall(r'(Defaults? (?:to|is)[^.]*(?:\.[0-9][^.]*)?\.?)',txt)
        o['default']=d
        o['text']=txt
json.dump(out,open('cfg.json','w',encoding='utf-8'),ensure_ascii=False,indent=0)
for o in out:
    if o['type']=='var': print(o['ids'][0] if o['ids'] else o['head'],'||',' ## '.join(o['default'])[:200])
