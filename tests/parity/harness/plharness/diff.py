"""Deep comparison of two responses.

Diff kinds:
  status        HTTP status differs
  header        a compared header differs (path = header:<name>)
  body-kind     one side JSON, the other not
  body          non-JSON bodies differ (sha256/length)
  type          JSON types differ (int vs float vs bool vs str ...)
  value         scalar values differ
  missing       key present in A, missing in B
  extra         key present in B, missing in A
  key-order     same keys, different order (DRF serializer field order matters)
  length        list lengths differ (elements are still compared up to the shorter length)

Paths use ``$`` for the root, ``.key`` and ``[i]``.
"""

from __future__ import annotations

import hashlib
from dataclasses import asdict, dataclass
from typing import Any

MAX_DIFFS = 200


@dataclass
class Diff:
    kind: str
    path: str
    a: Any = None
    b: Any = None

    def to_dict(self) -> dict:
        d = asdict(self)
        for side in ("a", "b"):
            v = d[side]
            if isinstance(v, (dict, list)):
                d[side] = _short(v)
            elif isinstance(v, str) and len(v) > 300:
                d[side] = v[:300] + "...(truncated)"
        return d


def _short(v: Any) -> Any:
    if isinstance(v, dict):
        return {"<dict keys>": list(v.keys())[:30]}
    if isinstance(v, list):
        return {"<list len>": len(v)}
    return v


def _typename(v: Any) -> str:
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int"
    if isinstance(v, float):
        return "float"
    if isinstance(v, str):
        return "str"
    if isinstance(v, list):
        return "list"
    if isinstance(v, dict):
        return "dict"
    return type(v).__name__


def json_diff(a: Any, b: Any, path: str = "$", out: list[Diff] | None = None) -> list[Diff]:
    if out is None:
        out = []
    if len(out) >= MAX_DIFFS:
        return out
    ta, tb = _typename(a), _typename(b)
    if ta != tb:
        out.append(Diff("type", path, a, b))
        return out
    if ta == "dict":
        ka, kb = list(a.keys()), list(b.keys())
        sa, sb = set(ka), set(kb)
        for k in ka:
            if k not in sb:
                out.append(Diff("missing", f"{path}.{k}", a[k], None))
        for k in kb:
            if k not in sa:
                out.append(Diff("extra", f"{path}.{k}", None, b[k]))
        common_a = [k for k in ka if k in sb]
        common_b = [k for k in kb if k in sa]
        if common_a != common_b:
            out.append(Diff("key-order", path, common_a, common_b))
        for k in common_a:
            json_diff(a[k], b[k], f"{path}.{k}", out)
        return out
    if ta == "list":
        if len(a) != len(b):
            out.append(Diff("length", path, len(a), len(b)))
        for i in range(min(len(a), len(b))):
            json_diff(a[i], b[i], f"{path}[{i}]", out)
        return out
    if a != b:
        out.append(Diff("value", path, a, b))
    return out


def body_digest(body: bytes) -> dict:
    return {"len": len(body), "sha256": hashlib.sha256(body).hexdigest()[:16]}


def compare(
    a_status: int,
    b_status: int,
    a_headers: dict[str, str],
    b_headers: dict[str, str],
    a_body: Any,
    b_body: Any,
    a_is_json: bool,
    b_is_json: bool,
    mode: str = "full",
) -> list[Diff]:
    """mode: full | status | status+headers | json-only (ignore headers)."""
    out: list[Diff] = []
    if a_status != b_status:
        out.append(Diff("status", "$status", a_status, b_status))
    if mode == "status":
        return out
    if mode in ("full", "status+headers"):
        for name in sorted(set(a_headers) | set(b_headers)):
            if a_headers.get(name) != b_headers.get(name):
                out.append(Diff("header", f"header:{name}", a_headers.get(name), b_headers.get(name)))
    if mode == "status+headers":
        return out
    if a_is_json != b_is_json:
        out.append(Diff("body-kind", "$body", "json" if a_is_json else "raw", "json" if b_is_json else "raw"))
        return out
    if a_is_json:
        json_diff(a_body, b_body, "$", out)
    else:
        if a_body != b_body:
            da = body_digest(a_body if isinstance(a_body, bytes) else str(a_body).encode())
            db = body_digest(b_body if isinstance(b_body, bytes) else str(b_body).encode())
            out.append(Diff("body", "$body", da, db))
    return out
