"""Phase 6 – fixes: compare response bodies of two runs (host in link normalized).

  python -I tests/pgvalidate/fix_compare.py <bodies-dir> <label-a> <label-b>

Output per request: equal / order only (lists equal as a set) / different (first paths).
"""
import json
import os
import re
import sys


def norm(v):
    if isinstance(v, str):
        # Normalize host:port of the links (Rust/Django run on different ports)
        return re.sub(r"127\.0\.0\.1:\d+", "127.0.0.1:PORT", v)
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items()}
    return v


def canon(v):
    """Lists as sorted sets (for the 'order only' comparison)."""
    if isinstance(v, list):
        return sorted((canon(x) for x in v), key=lambda x: json.dumps(x, sort_keys=True))
    if isinstance(v, dict):
        return {k: canon(x) for k, x in v.items()}
    return v


def diffs(a, b, path="", out=None):
    out = [] if out is None else out
    if len(out) > 8:
        return out
    if type(a) is not type(b):
        out.append(f"{path}: {str(a)[:80]} != {str(b)[:80]}")
    elif isinstance(a, dict):
        if list(a) != list(b):
            out.append(f"{path}: keys {list(a)} != {list(b)}")
        for k in a:
            if k in b:
                diffs(a[k], b[k], f"{path}.{k}", out)
    elif isinstance(a, list):
        if len(a) != len(b):
            out.append(f"{path}: len {len(a)} != {len(b)}")
        for i, (x, y) in enumerate(zip(a, b)):
            diffs(x, y, f"{path}[{i}]", out)
    elif a != b:
        out.append(f"{path}: {str(a)[:80]} != {str(b)[:80]}")
    return out


def main():
    root, la, lb = sys.argv[1:4]
    da, db = os.path.join(root, la), os.path.join(root, lb)
    same = order = diff = 0
    for f in sorted(os.listdir(da)):
        pb = os.path.join(db, f)
        if not os.path.exists(pb):
            continue
        a = norm(json.load(open(os.path.join(da, f), encoding="utf-8")))
        b = norm(json.load(open(pb, encoding="utf-8")))
        if a == b:
            same += 1
            print(f"same   {f}")
        elif canon(a) == canon(b):
            order += 1
            print(f"ORDER  {f}: " + "; ".join(diffs(a, b)[:3]))
        else:
            diff += 1
            print(f"DIFF   {f}: " + "; ".join(diffs(a, b)))
    print(f"== {la} vs {lb}: same {same}, order-only {order}, different {diff}")


if __name__ == "__main__":
    main()
