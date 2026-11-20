"""Phase 6: output a captured Rust query as executable SQL (parameters from the
auto_explain line `Query Parameters:` substituted) – for before/after measurements.

  python -I tests/pgvalidate/replay.py results/pgvalidate/capture/rs-explain.json alice tags.listAll 1
"""
import json
import re
import sys


def inline(plan, sql):
    m = re.search(r"Query Parameters: (.*)", plan or "")
    if not m:
        return sql
    params = {}
    for pm in re.finditer(r"\$(\d+) = ('(?:[^']|'')*'|NULL)", m.group(1)):
        params[int(pm.group(1))] = pm.group(2)
    return re.sub(r"\$(\d+)\b", lambda x: params.get(int(x.group(1)), x.group(0)), sql)


def main():
    path, user, name, idx = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
    d = json.load(open(path, encoding="utf-8"))
    e = next(e for e in d["users"][user] if e["name"] == name)
    q = e["queries"][idx]
    sys.stdout.write(inline(q.get("plan"), q["sql"]).rstrip().rstrip(";") + "\n")


if __name__ == "__main__":
    main()
