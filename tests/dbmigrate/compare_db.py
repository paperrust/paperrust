"""Compares two PostgreSQL databases row by row (test tool, read only).

    python compare_db.py <dsn_expected> <dsn_actual> [--allow-extra-ct app.model,...]

Expected = e.g. paperless-ngx 3.4.2 has restructured a 2.20 DB itself (`manage.py migrate`), actual =
PaperRust has restructured the same DB. Every table of both sides, every column as text, sorted by
primary key. Tables with rows that only arise during the restructuring (IDs depend on the
insertion order) are compared via natural keys:
  django_content_type (app_label, model), auth_permission (type, codename),
  auth_user_user_permissions / auth_group_permissions (user or group, permission),
  documents_uisettings (user), django_migrations (app, name; without `applied`).
`--allow-extra-ct`: ContentTypes (with their default permissions) that may only be in "expected" –
Django creates them after `migrate` via post_migrate, PaperRust on first start (`manage migrate`).
Outputs only table/column names, keys and counts, no contents. Exit 1 on deviation.
"""

import sys

import psycopg

NATURAL = {
    "django_content_type": "SELECT app_label || '.' || model, '' FROM django_content_type",
    "auth_permission": "SELECT c.app_label || '.' || c.model || ':' || p.codename, p.name FROM auth_permission p JOIN django_content_type c ON c.id = p.content_type_id",
    "auth_user_user_permissions": "SELECT u.username || '|' || c.app_label || '.' || c.model || ':' || p.codename, '' FROM auth_user_user_permissions x JOIN auth_user u ON u.id = x.user_id JOIN auth_permission p ON p.id = x.permission_id JOIN django_content_type c ON c.id = p.content_type_id",
    "auth_group_permissions": "SELECT g.name || '|' || c.app_label || '.' || c.model || ':' || p.codename, '' FROM auth_group_permissions x JOIN auth_group g ON g.id = x.group_id JOIN auth_permission p ON p.id = x.permission_id JOIN django_content_type c ON c.id = p.content_type_id",
    "documents_uisettings": "SELECT user_id::text, settings::text FROM documents_uisettings",
    "django_migrations": "SELECT app || '.' || name, '' FROM django_migrations",
}


def tables(cur):
    cur.execute(
        "SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE' "
        "AND table_name NOT LIKE 'paperrust%' ORDER BY 1",
    )
    return [r[0] for r in cur.fetchall()]


def rows(cur, t):
    if t in NATURAL:
        cur.execute(NATURAL[t])
        return {k: v for k, v in cur.fetchall()}
    cur.execute(
        "SELECT a.attname FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) "
        "WHERE i.indisprimary AND i.indrelid = %s::regclass",
        [f'public."{t}"'],
    )
    pk = [r[0] for r in cur.fetchall()]
    cur.execute(
        "SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name=%s ORDER BY ordinal_position",
        [t],
    )
    cols = [r[0] for r in cur.fetchall()]
    sel = ", ".join(f'"{c}"::text' for c in cols)
    cur.execute(f'SELECT {sel} FROM public."{t}"')
    out = {}
    for r in cur.fetchall():
        d = dict(zip(cols, r))
        out["|".join(str(d[c]) for c in pk)] = d
    return out


def main():
    a_dsn, b_dsn = sys.argv[1], sys.argv[2]
    allow = set()
    if "--allow-extra-ct" in sys.argv:
        allow = set(sys.argv[sys.argv.index("--allow-extra-ct") + 1].split(","))
    bad = 0
    with psycopg.connect(a_dsn) as a, psycopg.connect(b_dsn) as b:
        ca, cb = a.cursor(), b.cursor()
        ta, tb = tables(ca), tables(cb)
        if ta != tb:
            print(f"Tabellen verschieden: nur erwartet {sorted(set(ta) - set(tb))}, nur ist {sorted(set(tb) - set(ta))}")
            bad += 1
        for t in [x for x in ta if x in tb]:
            ra, rb = rows(ca, t), rows(cb, t)
            only_a = [k for k in ra if k not in rb]
            only_b = [k for k in rb if k not in ra]
            if t in ("django_content_type", "auth_permission"):
                only_a = [k for k in only_a if k not in allow and k.split(":")[0] not in allow]
            diff_cols = {}
            for k in ra.keys() & rb.keys():
                va, vb = ra[k], rb[k]
                if isinstance(va, dict):
                    for c in va:
                        if va[c] != vb.get(c):
                            diff_cols.setdefault(c, []).append(k)
                elif va != vb:
                    diff_cols.setdefault("(Wert)", []).append(k)
            if only_a or only_b or diff_cols:
                bad += 1
                print(f"{t}: {len(ra)} erwartet / {len(rb)} ist")
                if only_a:
                    print(f"   nur erwartet: {only_a[:10]}")
                if only_b:
                    print(f"   nur ist: {only_b[:10]}")
                for c, ks in diff_cols.items():
                    print(f"   Spalte {c} verschieden bei Schlüssel {ks[:10]}")
        print(f"{len(ta)} Tabellen verglichen, {bad} mit Abweichungen")
    sys.exit(1 if bad else 0)


main()
