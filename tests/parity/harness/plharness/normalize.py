"""Normalisation of volatile values - and ONLY of volatile values.

Applied identically to both responses before diffing:

* datetimes (ISO 8601 strings, full string match) at or after the cut-off -> ``<ts>``
  (values written during the run: last_login, modified, added of new objects ...).
  Older datetimes are compared exactly.
* write cases only: values under keys ``task_id`` and top-level UUID strings (the task
  id returned by post_document/bulk_edit-like routes) -> ``<task-id>``
* CSRF tokens: keys ``csrf_token``/``csrfmiddlewaretoken`` in JSON, the hidden form field
  in HTML bodies -> ``<csrf>``
* headers: only a whitelist is compared (see COMPARED_HEADERS); Server/Date/Set-Cookie
  values etc. are never compared. Set-Cookie is reduced to the sorted cookie names.

Anything else must be identical or be covered by a named accepted-deviation rule.
"""

from __future__ import annotations

import datetime as _dt
import re
from typing import Any

ISO_DT_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(:\d{2}(\.\d{1,6})?)?(Z|[+-]\d{2}:?\d{2})?$"
)
UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I)
HTML_CSRF_RE = re.compile(r'(name=["\']csrfmiddlewaretoken["\']\s+value=["\'])[^"\']+')
HTML_CSRF_RE2 = re.compile(r'(value=["\'])[^"\']+(["\']\s+name=["\']csrfmiddlewaretoken)')

CSRF_KEYS = {"csrf_token", "csrfmiddlewaretoken", "csrftoken"}
TASK_ID_KEYS = {"task_id"}

COMPARED_HEADERS = [
    "content-type",
    "content-disposition",
    "content-language",
    "location",
    "allow",
    "vary",
    "www-authenticate",
    "x-api-version",
    "x-version",
    "cache-control",
    "etag",
    "last-modified",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
    "cross-origin-opener-policy",
]


def _parse_dt(s: str) -> _dt.datetime | None:
    try:
        dt = _dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return None
    if dt.tzinfo is None:
        # naive datetimes are rare in DRF output; treat them as UTC for the cut-off test
        dt = dt.replace(tzinfo=_dt.timezone.utc)
    return dt


# Lists that upstream produces from Python sets: order depends on PYTHONHASHSEED
# (random per Django process) and is therefore not part of the behavior -> compare sorted.
# Source: paperless/serialisers.py UserSerializer.get_inherited_permissions (set), UiSettingsView permissions.
UNORDERED_KEYS = {"inherited_permissions", "permissions"}

# POST /api/documents/selection_data/: upstream annotates without ORDER BY (Django >= 3.1 ignores
# Meta.ordering with GROUP BY) -> order of the HashAggregate plan, different between two Django runs
# (docs/contract/pl-documents.md "documents.selection_data"); PaperRust delivers fixed by
# id. Therefore compared sorted by id - only at the top level (POST response), not under
# "selection_data" of the list (there the upstream order is deterministic and is checked).
SELECTION_POST_KEYS = (
    "selected_correspondents",
    "selected_tags",
    "selected_document_types",
    "selected_storage_paths",
    "selected_custom_fields",
)


# Label lists with ``ordering=[-]document_count`` (tags, correspondents, types, storage paths,
# custom fields): upstream ``OrderingFilter`` -> ``order_by("document_count")`` without a
# tie-break key, the order of equally large entries is that of the PostgreSQL plan (not
# part of the behavior; it changes e.g. with the query shape of the count, PERF-SCALE Q2/L5).
# Therefore within a run of equal ``document_count`` it is compared sorted by ``id``; the
# numbers themselves and the order of different numbers remain checked exactly.
TIE_ORDERINGS = {"document_count", "-document_count"}


def sort_ties_by_id(results: list, key: str) -> list:
    out: list = []
    run: list = []
    for x in results:
        if run and x.get(key) != run[-1].get(key):
            out += sorted(run, key=lambda y: y["id"])
            run = []
        run.append(x)
    return out + sorted(run, key=lambda y: y["id"])


class Normalizer:
    def __init__(self, cutoff: _dt.datetime, write: bool = False) -> None:
        self.cutoff = cutoff
        self.write = write
        self.applied: dict[str, int] = {}

    def _count(self, what: str) -> None:
        self.applied[what] = self.applied.get(what, 0) + 1

    def value(self, v: Any, key: str | None = None, top: bool = False) -> Any:
        if isinstance(v, dict):
            return {k: self.value(x, key=k) for k, x in v.items()}
        if isinstance(v, list):
            out = [self.value(x, key=key) for x in v]
            if key in UNORDERED_KEYS and all(isinstance(x, str) for x in out):
                self._count("unordered-set-list")
                return sorted(out)
            return out
        if isinstance(v, str):
            if key in CSRF_KEYS:
                self._count("csrf")
                return "<csrf>"
            if self.write and (key in TASK_ID_KEYS or top) and UUID_RE.match(v):
                self._count("task-id")
                return "<task-id>"
            if ISO_DT_RE.match(v):
                dt = _parse_dt(v)
                if dt is not None and dt >= self.cutoff:
                    self._count("timestamp-after-cutoff")
                    return "<ts>"
        return v

    def json(self, doc: Any, ordering: str | None = None) -> Any:
        out = self.value(doc, top=True)
        if (
            ordering in TIE_ORDERINGS
            and isinstance(out, dict)
            and isinstance(out.get("results"), list)
            and all(isinstance(x, dict) and isinstance(x.get("id"), int) and "document_count" in x
                    for x in out["results"])
        ):
            out["results"] = sort_ties_by_id(out["results"], "document_count")
            self._count("document-count-ties-by-id")
        if isinstance(out, dict) and all(isinstance(out.get(k), list) for k in SELECTION_POST_KEYS):
            for k in SELECTION_POST_KEYS:
                if all(isinstance(x, dict) and isinstance(x.get("id"), int) for x in out[k]):
                    out[k] = sorted(out[k], key=lambda x: x["id"])
            self._count("selection-data-post-by-id")
        return out

    def text(self, s: str) -> str:
        s2 = HTML_CSRF_RE.sub(r"\1<csrf>", s)
        s2 = HTML_CSRF_RE2.sub(r"\1<csrf>\2", s2)
        if s2 != s:
            self._count("csrf")
        return s2

    def headers(self, headers: list[tuple[str, str]], compared: list[str] | None = None) -> dict[str, str]:
        names = compared or COMPARED_HEADERS
        out: dict[str, str] = {}
        for k, v in headers:
            lk = k.lower()
            if lk in names:
                out[lk] = (out[lk] + ", " + v) if lk in out else v
        cookies = sorted(
            {v.split("=", 1)[0].strip() for k, v in headers if k.lower() == "set-cookie"}
        )
        if cookies:
            out["set-cookie(names)"] = ",".join(cookies)
        if "vary" in out:  # order of Vary tokens is not significant
            out["vary"] = ", ".join(sorted(t.strip().lower() for t in out["vary"].split(",")))
        return out
