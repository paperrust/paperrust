"""Read-only timing legacy vs ast on pl_scale_1m (psql, one connection, interleaved, minimum of 3)."""
import glob
import os
import re
import subprocess
import sys

PSQL = sys.argv[1]
S = os.path.dirname(os.path.abspath(__file__))
env = dict(os.environ, PGPASSWORD="paperless")


def t(sql):
    out = subprocess.run([PSQL, "-h", "127.0.0.1", "-p", "54329", "-U", "paperless", "-d", "pl_scale_1m", "-Atc",
                          "EXPLAIN (ANALYZE, TIMING OFF) " + sql], capture_output=True, text=True, env=env).stdout
    m = re.search(r"Execution Time: ([\d.]+) ms", out)
    return float(m.group(1)) if m else float("nan")


print("| Nutzer | Anfrage | Seite legacy | Seite ast | COUNT legacy | COUNT ast |")
print("|---|---|---:|---:|---:|---:|")
for f in sorted(glob.glob(os.path.join(S, "dump1m_*.sql"))):
    lines = open(f, encoding="utf-8").read().splitlines()
    sql = {l.split(": ", 1)[0]: l.split(": ", 1)[1].rstrip(";") for l in lines if ": " in l}
    if "LEGACY" not in sql or "AST" not in sql:
        continue
    res = {k: [float("inf")] * 2 for k in ("LEGACY", "AST")}
    for _ in range(3):
        for k in ("LEGACY", "AST"):
            res[k][0] = min(res[k][0], t(sql[k] + " LIMIT 50"))
            res[k][1] = min(res[k][1], t("SELECT count(*) FROM (" + sql[k] + ") c"))
    name = os.path.basename(f)[7:-4]
    print(f"| {name.split('_', 1)[0]} | `{name.split('_', 1)[1] or 'default'}` | {res['LEGACY'][0]:.0f} | {res['AST'][0]:.0f} | {res['LEGACY'][1]:.0f} | {res['AST'][1]:.0f} |")
