"""Phase 6: send frontend-typical requests one after the other and collect the SQL queries per request from
the server log (Rust: sqlx::query=debug + auto_explain notices; Django: own
SQL logger from pgv_settings.py).

  python -I tests/pgvalidate/capture.py --kind rs --base http://127.0.0.1:8090 \
      --log results/pgvalidate/logs/rs-capture.log --out results/pgvalidate/capture/rs.json

The requests run strictly one after the other; after each response there is a short wait and the log part
that grew since the request is evaluated. Result (JSON): per user and request status,
wall-clock time, queries (SQL, rows, duration, for Rust optionally plan with parameters).
"""
import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pgv_requests import REQUESTS, USERS  # noqa: E402

TS = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+Z\s+(\w+)\s+([\w:]+): ?(.*)$")


def rust_unescape(s):
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            n = s[i + 1]
            if n == "n":
                out.append("\n"); i += 2; continue
            if n == "t":
                out.append("\t"); i += 2; continue
            if n == "r":
                out.append("\r"); i += 2; continue
            if n in "\"\\'":
                out.append(n); i += 2; continue
            if n == "u" and s[i + 2:i + 3] == "{":
                j = s.index("}", i)
                out.append(chr(int(s[i + 3:j], 16))); i = j + 1; continue
        out.append(c); i += 1
    return "".join(out)


def parse_rust(text):
    """Splits the log excerpt into records (lines from a timestamp)."""
    recs, cur = [], None
    for line in text.splitlines():
        m = TS.match(line)
        if m:
            cur = {"level": m.group(1), "target": m.group(2), "msg": m.group(3)}
            recs.append(cur)
        elif cur is not None:
            cur["msg"] += "\n" + line
    queries, pending_plans, access = [], [], []
    for r in recs:
        if r["target"] == "sqlx::postgres::notice" and "plan:" in r["msg"]:
            pending_plans.append(r["msg"])
        elif r["target"] == "sqlx::query":
            m = re.search(r'db\.statement="((?:[^"\\]|\\.)*)"', r["msg"], re.S)
            sql = rust_unescape(m.group(1)).strip() if m else r["msg"]
            rr = re.search(r"rows_returned=(\d+)", r["msg"])
            el = re.search(r"elapsed_secs=([\d.e-]+)", r["msg"])
            q = {"sql": sql, "rows": int(rr.group(1)) if rr else None,
                 "ms": round(float(el.group(1)) * 1000, 3) if el else None}
            # auto_explain reports the plan before the query finishes
            for p in pending_plans:
                pt = p.split("Query Text:", 1)[-1]
                if pt.strip()[:60] in sql or sql[:60] in pt:
                    q["plan"] = p
            pending_plans = []
            queries.append(q)
        elif r["target"].endswith("::access"):
            access.append(r["msg"])
    return queries, access


def parse_django(text):
    queries = []
    for line in text.splitlines():
        if line.startswith("DJSQL\t"):
            _, dur, sql = line.split("\t", 2)
            queries.append({"sql": sql.replace("\\n", "\n"), "ms": round(float(dur) * 1000, 3)})
    return queries, []


def call(base, method, path, token, body=None, timeout=900):
    data = None
    headers = {"Authorization": "Token " + token, "Accept": "application/json; version=10"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read(); status = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read(); status = e.code
    except Exception as e:  # timeout or similar
        return 0, None, (time.perf_counter() - t0) * 1000, repr(e)
    ms = (time.perf_counter() - t0) * 1000
    try:
        js = json.loads(raw)
    except Exception:
        js = None
    return status, js, ms, len(raw)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", choices=["rs", "dj"], required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--users", default="admin,alice,bob")
    ap.add_argument("--only", default="")
    ap.add_argument("--skip", default="")
    ap.add_argument("--repeat", type=int, default=1, help="Wiederholungen (Zeitmessung, Median)")
    ap.add_argument("--settle", type=float, default=0.4)
    ap.add_argument("--timeout", type=float, default=900)
    a = ap.parse_args()
    only = set(filter(None, a.only.split(",")))
    skip = set(filter(None, a.skip.split(",")))
    parse = parse_rust if a.kind == "rs" else parse_django
    result = {"kind": a.kind, "base": a.base, "users": {}}
    for user in a.users.split(","):
        tok = USERS[user]
        st, js, _, _ = call(a.base, "GET", "/api/documents/?page=1&page_size=50&ordering=-created&fields=id", tok)
        ids = [d["id"] for d in (js or {}).get("results", [])]
        doc = ids[0] if ids else 1
        out = []
        for name, method, path, body in REQUESTS + (extra_requests() if only else []):
            if (only and name not in only) or name in skip:
                continue
            p = path.replace("{DOC}", str(doc))
            b = {"documents": ids} if body == "SELECTION" else body
            runs = []
            for rep in range(a.repeat):
                off = os.path.getsize(a.log)
                status, js, ms, size = call(a.base, method, p, tok, b, a.timeout)
                time.sleep(a.settle)
                with open(a.log, "rb") as f:
                    f.seek(off)
                    text = f.read().decode("utf-8", "replace")
                qs, acc = parse(text)
                runs.append({"status": status, "ms": round(ms, 1), "size": size, "queries": qs,
                             "access": acc})
                print(f"{a.kind} {user:5} {name:28} {status} {ms:9.1f} ms  q={len(qs)}", flush=True)
                if status == 0:
                    break
            walls = sorted(r["ms"] for r in runs)
            entry = {"name": name, "method": method, "path": p, "status": runs[-1]["status"],
                     "ms_median": walls[len(walls) // 2], "ms_all": walls,
                     "roundtrips": len(runs[-1]["queries"]),
                     "db_ms": round(sum(q["ms"] or 0 for q in runs[-1]["queries"]), 1),
                     "queries": runs[-1]["queries"], "access": runs[-1]["access"]}
            out.append(entry)
            result["users"][user] = out
            with open(a.out, "w", encoding="utf-8") as f:
                json.dump(result, f, ensure_ascii=False, indent=1)


def extra_requests():
    """Additional requests from fix_bench.EXTRA (e.g. O2 text searches), only useful when chosen via --only."""
    try:
        from fix_bench import EXTRA
    except ImportError:
        return []
    return list(EXTRA)


if __name__ == "__main__":
    main()
