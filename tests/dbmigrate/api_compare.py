"""Compares two api_fetch.py results (source vs. target of the DB move).

Invocation: python -I api_compare.py <source.json> <target.json> [<report.json>]
Exit 0 = all non-backend-dependent responses identical.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from api_fetch import BACKEND_SPECIFIC, COLLATION  # noqa: E402


def first_diff(a, b, path="$"):
    if type(a) is not type(b):
        return f"{path}: Typ {type(a).__name__} ≠ {type(b).__name__}"
    if isinstance(a, dict):
        for k in list(a) + [k for k in b if k not in a]:
            if k not in a or k not in b:
                return f"{path}.{k}: nur in {'Quelle' if k in a else 'Ziel'}"
            d = first_diff(a[k], b[k], f"{path}.{k}")
            if d:
                return d
        if list(a) != list(b):
            return f"{path}: Schlüsselreihenfolge"
        return None
    if isinstance(a, list):
        if len(a) != len(b):
            return f"{path}: Länge {len(a)} ≠ {len(b)}"
        for i, (x, y) in enumerate(zip(a, b)):
            d = first_diff(x, y, f"{path}[{i}]")
            if d:
                return d
        return None
    return None if a == b else f"{path}: {str(a)[:80]!r} ≠ {str(b)[:80]!r}"


def order_free(v):
    """Make lists without fixed order comparable (objects with id by id, strings sorted)."""
    if isinstance(v, dict):
        return {k: order_free(x) for k, x in v.items()}
    if isinstance(v, list):
        v = [order_free(x) for x in v]
        if v and all(isinstance(x, dict) and "id" in x for x in v):
            return sorted(v, key=lambda x: x["id"])
        if all(isinstance(x, str) for x in v):
            return sorted(v)
        # e.g. custom_fields of a document: Meta.ordering = created, on a tie
        # the engine decides (MariaDB ≠ PostgreSQL) – count only as "order"
        if all(isinstance(x, dict) for x in v):
            return sorted(v, key=lambda x: json.dumps(x, sort_keys=True))
    return v


def env_free(v):
    """/api/config/ lists variables set via the environment – PAPERLESS_DB* differ by definition."""
    if isinstance(v, list) and v and isinstance(v[0], dict) and "externally_configured_variables" in v[0]:
        v = [dict(x, externally_configured_variables=sorted(e for e in x["externally_configured_variables"] if not e.startswith("PAPERLESS_DB"))) for x in v]
    return v


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    a = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    b = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    same, diff, expected, order_only = [], [], [], []
    for k in sorted(set(a) | set(b)):
        path = k.split(" ", 1)[1]
        d = "fehlt" if k not in a or k not in b else (None if a[k] == b[k] else first_diff(a[k], b[k]) or "verschieden")
        if d is None:
            same.append(k)
        elif order_free(env_free(a[k].get("json"))) == order_free(env_free(b[k].get("json"))) and a[k]["status"] == b[k]["status"] and "json" in a[k]:
            (same if env_free(a[k].get("json")) == env_free(b[k].get("json")) else order_only).append(k)
        elif path in BACKEND_SPECIFIC or path in COLLATION:
            expected.append((k, d))
        else:
            diff.append((k, d))
    print(f"identisch: {len(same)}, nur Reihenfolge: {len(order_only)}, verschieden: {len(diff)}, backend-abhängig verschieden: {len(expected)}")
    for k in order_only:
        print(f"  (Reihenfolge) {k}")
    for k, d in diff:
        print(f"  DIFF {k}: {d}")
    for k, d in expected:
        print(f"  (erwartet) {k}: {d}")
    if len(sys.argv) > 3:
        Path(sys.argv[3]).write_text(
            json.dumps({"identical": len(same), "order_only": order_only, "different": [list(x) for x in diff], "backend_specific": [list(x) for x in expected],
                        "statuses": sorted({v.get("status") for v in a.values()})}, ensure_ascii=False, indent=1),
            encoding="utf-8",
        )
    return 1 if diff else 0


if __name__ == "__main__":
    raise SystemExit(main())
