"""Comparison helper for the pl-cli parity tests (stdlib + psycopg from reference/venv).

  python compare.py tree A B [--zip]      file trees/ZIPs: same names, JSON (CRLF-normalized) and bytes equal
  python compare.py db DB_A DB_B [--ignore table.col ...]   all tables row by row (sorted by PK)

Exit 0 = equal, 1 = differences (are printed).
"""

import hashlib
import io
import json
import os
import sys
import zipfile


def files_of(root):
    out = {}
    for d, _, fs in os.walk(root):
        for f in fs:
            p = os.path.join(d, f)
            rel = os.path.relpath(p, root).replace("\\", "/")
            with open(p, "rb") as fh:
                out[rel] = fh.read()
    return out


def zip_files(path):
    out = {}
    with zipfile.ZipFile(path) as z:
        names = [i.filename for i in z.infolist()]
        for i in z.infolist():
            out[i.filename] = b"" if i.is_dir() else z.read(i)
    return out, names


def norm(name, data):
    if name.endswith(".json"):
        return data.replace(b"\r\n", b"\n")
    return data


def tree(a, b, is_zip):
    if is_zip:
        fa, na = zip_files(a)
        fb, nb = zip_files(b)
        if [n for n in na if not n.endswith("/")] != [n for n in nb if not n.endswith("/")]:
            # Order of the entries: only report
            print("INFO zip entry order differs")
        da = sorted(n for n in na if n.endswith("/"))
        db = sorted(n for n in nb if n.endswith("/"))
        if da != db:
            print("DIFF zip directory entries", da[:5], db[:5])
    else:
        fa, fb = files_of(a), files_of(b)
    bad = 0
    for n in sorted(set(fa) | set(fb)):
        if n not in fa or n not in fb:
            print("ONLY", "A" if n in fa else "B", n)
            bad += 1
            continue
        if norm(n, fa[n]) != norm(n, fb[n]):
            print("DIFF", n, len(fa[n]), len(fb[n]))
            bad += 1
    print(f"{len(fa)} vs {len(fb)} files, {bad} differences")
    return bad == 0


def db(a, b, ignore):
    import psycopg

    def dump(name):
        conn = psycopg.connect(host="127.0.0.1", port=54329, user="paperless", password="paperless", dbname=name)
        cur = conn.cursor()
        cur.execute(
            "SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE' "
            "AND table_name NOT LIKE 'paperrust%' ORDER BY 1"
        )
        tables = [r[0] for r in cur.fetchall()]
        out = {}
        for t in tables:
            cur.execute(
                "SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name=%s ORDER BY ordinal_position",
                (t,),
            )
            cols = [r[0] for r in cur.fetchall() if f"{t}.{r[0]}" not in ignore]
            sel = ", ".join(f'"{c}"::text' for c in cols)
            if not cols:
                continue
            order = ", ".join(str(i + 1) for i in range(len(cols)))
            cur.execute(f'SELECT {sel} FROM "{t}" ORDER BY {order}')
            out[t] = (cols, cur.fetchall())
        conn.close()
        return out

    da, dbb = dump(a), dump(b)
    bad = 0
    for t in sorted(set(da) | set(dbb)):
        if t not in da or t not in dbb:
            print("ONLY", t)
            bad += 1
            continue
        ca, ra = da[t]
        cb, rb = dbb[t]
        if ra != rb:
            bad += 1
            print(f"DIFF {t}: {len(ra)} vs {len(rb)} rows")
            sa, sb = set(ra), set(rb)
            for r in list(sa - sb)[:3]:
                print("   A:", dict(zip(ca, r)))
            for r in list(sb - sa)[:3]:
                print("   B:", dict(zip(cb, r)))
    print(f"{len(da)} tables, {bad} differing")
    return bad == 0


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "tree":
        ok = tree(sys.argv[2], sys.argv[3], "--zip" in sys.argv)
    else:
        ign = set()
        if "--ignore" in sys.argv:
            ign = set(sys.argv[sys.argv.index("--ignore") + 1 :])
        ok = db(sys.argv[2], sys.argv[3], ign)
    sys.exit(0 if ok else 1)
