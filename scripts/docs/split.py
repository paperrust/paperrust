"""Split converted upstream pages into PaperRust user-doc pages and fix cross links.
python -I split.py <scratch_dir> <out_dir>
"""
import re
import sys
import os

S, OUT = sys.argv[1], sys.argv[2]


def read(n):
    return open(os.path.join(S, n), encoding="utf-8").read().split("\n")


def find(lines, pattern, start=0):
    rx = re.compile(pattern)
    for i in range(start, len(lines)):
        if rx.match(lines[i]):
            # include a preceding <a id> line (+ blank) if present
            j = i
            while j - 2 >= 0 and lines[j - 1] == "" and lines[j - 2].startswith("<a id="):
                j -= 2
            return j
    raise SystemExit(f"not found: {pattern}")


def shift(lines, by):
    out, fence = [], False
    for l in lines:
        if l.lstrip().startswith("```"):
            fence = not fence
        if not fence and re.match(r"^#{2,6} ", l) and by:
            l = l[by:] if by > 0 else ("#" * -by) + l
        out.append(l)
    return out


usage = read("usage.gh.md")
adv = read("advanced_usage.gh.md")

u_mail = find(usage, r"^### Incoming Email")
u_rest = find(usage, r"^### REST API")
u_ai = find(usage, r"^## AI Features")
u_share = find(usage, r"^## Sharing documents")
u_perm = find(usage, r"^## Permissions")
u_wf = find(usage, r"^## Workflows")
u_cf = find(usage, r"^## Custom Fields")
u_pdf = find(usage, r"^## PDF Actions")

a_ai = find(adv, r"^## AI features")
a_hooks = find(adv, r"^## Hooking into the consumption")
a_fn = find(adv, r"^## File name handling")
a_rec = find(adv, r"^## Automatic recovery of invalid PDFs")

pages = {
    "usage.md": usage[:u_mail] + usage[u_rest:u_ai] + usage[u_share:u_perm] + usage[u_pdf:],
    "mail.md": shift(usage[u_mail:u_rest], 1),
    "permissions.md": usage[u_perm:u_wf],
    "workflows.md": usage[u_wf:u_cf],
    "custom-fields.md": usage[u_cf:u_pdf],
    "advanced.md": adv[:a_ai] + adv[a_hooks:a_fn] + adv[a_rec:],
    "storage-paths.md": adv[a_fn:a_rec],
    "ai-upstream.md": usage[u_ai:u_share] + adv[a_ai:a_hooks],
    "configuration.md": read("configuration.gh.md"),
    "api.md": read("api.gh.md"),
}


def slug(h):
    t = re.sub(r"^#+ ", "", h).strip().lower()
    t = re.sub(r"[^\w\- ]", "", t)
    return t.replace(" ", "-")


anchors = {}
for p, ls in pages.items():
    ids = set()
    fence = False
    for l in ls:
        if l.lstrip().startswith("```"):
            fence = not fence
        if fence:
            continue
        for m in re.finditer(r'<a id="([^"]+)"></a>', l):
            ids.add(m.group(1))
        if re.match(r"^#{1,6} ", l):
            ids.add(slug(l))
    anchors[p] = ids

owner = {}
for p, ids in anchors.items():
    for a in ids:
        owner.setdefault(a, p)

SRC_PAGES = {"usage.md", "advanced.md"}


def fix_links(p, ls):
    def rep(m):
        tgt = m.group(1)
        mm = re.match(r"^(?:(usage\.md|advanced\.md|configuration\.md|api\.md))?#(.+)$", tgt)
        if not mm:
            return m.group(0)
        page, a = mm.group(1), mm.group(2)
        here = page is None
        if page is None:
            page = p
        if page in anchors and a in anchors[page]:
            new = "#" + a if page == p else f"{page}#{a}"
        elif a in owner:
            new = "#" + a if owner[a] == p else f"{owner[a]}#{a}"
        else:
            new = tgt if not here else "#" + a
        return f"]({new})"

    return [re.sub(r"\]\(([^)\s]*#[^)\s]*)\)", rep, l) for l in ls]


os.makedirs(OUT, exist_ok=True)
for p, ls in pages.items():
    ls = fix_links(p, ls)
    while ls and ls[0].strip() == "":
        ls.pop(0)
    open(os.path.join(OUT, p + ".body"), "w", encoding="utf-8", newline="\n").write("\n".join(ls).rstrip() + "\n")
    print(p, len(ls))
