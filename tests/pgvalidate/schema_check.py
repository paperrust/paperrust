"""Phase 6, cross check: tables/columns that the Rust code addresses, against
reference/schema/postgres.sql (schema v3.4.2 after `manage.py migrate`).

  python -I tests/pgvalidate/schema_check.py > results/pgvalidate/schema_check.json

Heuristic (static, without build):
 1. Schema: all `CREATE TABLE public.<t> (...)` with columns.
 2. Rust: all string literals (also r#"…"#) from crates/*/src/**.rs.
    a) Tables: identifiers with a known app prefix (documents_, auth_, guardian_, django_,
       paperless_mail_, paperless_, auditlog_, authtoken_, account_, socialaccount_, mfa_,
       paperless_ai_, paperrust_) in FROM/JOIN/INTO/UPDATE/TABLE position or as a whole
       literal (sea-query `Alias::new("documents_tag")`).
    b) Columns in raw SQL: `alias.column` / `"alias"."column"`, alias resolved via
       `FROM|JOIN <table> [AS] <alias>` in the same literal or the same file.
    c) sea-query: identifier literals in `Alias::new("x")`, `Expr::col("x")`, `.column("x")`,
       `col("x")` – must be a table or a column of any table, otherwise candidate
       (output alias, function name …) for manual review.
Output: JSON with unknown_tables, unknown_columns (resolved), unresolved_identifiers.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PREFIX = re.compile(r"^(documents|auth|guardian|django|paperless_mail|paperless|auditlog|authtoken|account|socialaccount|mfa|paperless_ai|paperrust)_[a-z0-9_]+$")


def load_schema():
    txt = open(os.path.join(ROOT, "reference", "schema", "postgres.sql"), encoding="utf-8").read()
    tables = {}
    for m in re.finditer(r"CREATE TABLE public\.(\w+) \((.*?)\n\);", txt, re.S):
        cols = set()
        for line in m.group(2).split("\n"):
            line = line.strip()
            cm = re.match(r'"?(\w+)"?\s', line)
            if cm and not re.match(r"(CONSTRAINT|PRIMARY KEY|UNIQUE|CHECK)\b", line):
                cols.add(cm.group(1))
        tables[m.group(1)] = cols
    views = set(re.findall(r"CREATE (?:MATERIALIZED )?VIEW public\.(\w+)", txt))
    return tables, views


STR = re.compile(r'r(#+)"(.*?)"\1|"((?:[^"\\]|\\.)*)"', re.S)


def literals(src):
    for m in STR.finditer(src):
        s = m.group(2) if m.group(2) is not None else m.group(3)
        yield m.start(), s


def main():
    tables, views = load_schema()
    allcols = set().union(*tables.values())
    unknown_tables, unknown_cols, unresolved = {}, {}, {}
    used_tables = {}
    files = []
    for dp, _, fns in os.walk(os.path.join(ROOT, "crates")):
        if os.sep + "src" not in dp and not dp.endswith("src"):
            continue
        files += [os.path.join(dp, f) for f in fns if f.endswith(".rs")]
    for fp in sorted(files):
        rel = os.path.relpath(fp, ROOT).replace("\\", "/")
        src = open(fp, encoding="utf-8").read()
        # Roughly separate test modules (#[cfg(test)] to end of file), mark separately
        cut = src.find("#[cfg(test)]")
        lits = list(literals(src))
        sql_lits = []
        for pos, s in lits:
            in_test = cut != -1 and pos > cut
            line = src.count("\n", 0, pos) + 1
            loc = f"{rel}:{line}" + (" (test)" if in_test else "")
            # a) whole literals as table name
            if PREFIX.match(s):
                if s in tables or s in views:
                    used_tables.setdefault(s, set()).add(rel)
                elif re.search(r"(Alias::new|from|table|join|into|TABLE|FROM|JOIN)\W*$", src[max(0, pos - 40):pos]):
                    unknown_tables.setdefault(s, []).append(loc)
            # SQL-Text
            if re.search(r"\b(SELECT|FROM|JOIN|UPDATE|INSERT|DELETE)\b", s):
                aliases = {}
                sql_lits.append((loc, s, aliases))
                for tm in re.finditer(r'\b(?:FROM|JOIN|INTO|UPDATE|TABLE)\s+(?:ONLY\s+)?"?(\w+)"?(?:\s+(?:AS\s+)?"?(\w+)"?)?', s, re.I):
                    t, al = tm.group(1), tm.group(2)
                    if t.lower() in ("select", "lateral", "unnest", "jsonb_array_elements", "generate_series"):
                        continue
                    if t in tables or t in views:
                        used_tables.setdefault(t, set()).add(rel)
                        aliases[t] = t
                        if al and al.upper() not in ("WHERE", "ON", "LEFT", "INNER", "JOIN", "GROUP", "ORDER", "LIMIT", "SET", "VALUES", "USING", "CROSS", "RIGHT", "FULL", "UNION", "AND", "OR", "WITH", "RETURNING", "OFFSET", "HAVING", "FOR", "SELECT", "NATURAL", "EXCEPT"):
                            aliases[al] = t
                    elif PREFIX.match(t):
                        unknown_tables.setdefault(t, []).append(loc)
            # c) sea-query identifiers
        for m in re.finditer(r'(Alias::new|Expr::col|\.column|\bcol)\(\s*"([a-z_][a-z0-9_]*)"\s*\)', src):
            ident = m.group(2)
            pos = m.start()
            in_test = cut != -1 and pos > cut
            if ident in tables or ident in allcols or ident in views:
                continue
            line = src.count("\n", 0, pos) + 1
            unresolved.setdefault(ident, []).append(f"{rel}:{line}" + (" (test)" if in_test else ""))
        # b) columns with resolved alias
        for loc, s, aliases in sql_lits:
            for cm in re.finditer(r'"?\b([a-z_][a-z0-9_]*)"?\."?([a-z_][a-z0-9_]*)\b"?', s):
                al, col = cm.group(1), cm.group(2)
                t = aliases.get(al)
                if t is None or col in ("*",):
                    continue
                if col not in tables.get(t, set()):
                    unknown_cols.setdefault(f"{t}.{col}", []).append(loc)
        # sea-query pairs (Alias::new("table"), Alias::new("column"))
        for m in re.finditer(r'\(\s*Alias::new\("(\w+)"\)\s*,\s*Alias::new\("(\w+)"\)\s*\)', src):
            t, col = m.group(1), m.group(2)
            if t in tables and col not in tables[t]:
                line = src.count("\n", 0, m.start()) + 1
                unknown_cols.setdefault(f"{t}.{col}", []).append(f"{rel}:{line}")
    out = {
        "schema_tables": len(tables),
        "tables_used": {t: sorted(v) for t, v in sorted(used_tables.items())},
        "schema_tables_unused_by_rust": sorted(set(tables) - set(used_tables)),
        "unknown_tables": unknown_tables,
        "unknown_columns": unknown_cols,
        "unresolved_identifiers": unresolved,
    }
    json.dump(out, sys.stdout, indent=1, ensure_ascii=False)


if __name__ == "__main__":
    main()
