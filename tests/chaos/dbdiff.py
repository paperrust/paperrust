"""Row-by-row comparison of a table between two databases (aid for finding analysis, C6).

  python tests/chaos/dbdiff.py <table> [<table> …] [--a pl_chaos_a] [--b pl_chaos_r] [--port 54400]
Shows per table the rows that occur only in A or only in B (as text), at most 8 per side.
"""
import argparse
import subprocess
from pathlib import Path

PSQL = str(Path(__file__).resolve().parent.parent.parent / "reference/pg/pgsql/bin/psql.exe")


def rows(db, port, t):
    r = subprocess.run([PSQL, "-At", "-h", "127.0.0.1", "-p", str(port), "-U", "paperless", "-d", db, "-c",
                        f'SELECT x::text FROM "{t}" x ORDER BY 1'], capture_output=True, text=True, encoding="utf-8",
                       errors="replace")
    return r.stdout.splitlines()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("tables", nargs="+")
    p.add_argument("--a", default="pl_chaos_a")
    p.add_argument("--b", default="pl_chaos_r")
    p.add_argument("--port", default=54400)
    p.add_argument("-n", type=int, default=8)
    a = p.parse_args()
    for t in a.tables:
        ra, rb = rows(a.a, a.port, t), rows(a.b, a.port, t)
        sa, sb = set(ra), set(rb)
        print(f"== {t}: {len(ra)} / {len(rb)} Zeilen, nur A {len(sa - sb)}, nur B {len(sb - sa)}")
        for x in sorted(sa - sb)[: a.n]:
            print("  A:", x[:400])
        for x in sorted(sb - sa)[: a.n]:
            print("  B:", x[:400])


if __name__ == "__main__":
    main()
