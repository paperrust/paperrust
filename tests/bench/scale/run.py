"""Load measurement of PaperRust on the scale data set (docs/PERF-SCALE.md).

Server is started by tests/bench/scale/serve.ps1 (detached); this script only measures.
stdlib + psycopg only (reference venv, for looking up IDs). Invocation (Git Bash):
  "$PY" -I tests/bench/scale/run.py --base http://127.0.0.1:58181 --dsn "<libpq dsn pl_scale_1m>" \
      --out results/bench/scale/<date> [--only REGEX] [--users admin,restricted] [--n1 12] [--dur8 12]
  Scenarios with write access (reversible, need `serve --worker`): --scenarios bulk,rename,ingest
  Profile mode (server with PAPERRUST_LOG=warn,paperrust::access=info,sqlx::query=debug):
      --profile-log <serve.log>  → 1 call per endpoint, afterwards queries/request + SQL time from the log

Per endpoint and user: c=1 (up to --n1 calls or 40 s) and c=8 (--dur8 seconds, keep-alive per thread):
p50/p95/max in ms, throughput req/s, response size. Result: results.json + summary.md.
"""

from __future__ import annotations

import argparse
import http.client
import json
import re
import statistics
import sys
import threading
import time
import urllib.parse
import uuid as uuidlib
from pathlib import Path

import psycopg

REQ_TIMEOUT = 120.0
DSN = ""


def cancel_running(older_than_s: float = 5) -> int:
    """Cancel queries of the measurement instance whose client gave up due to a timeout."""
    with psycopg.connect(DSN, autocommit=True) as conn, conn.cursor() as cur:
        cur.execute("SELECT count(pg_cancel_backend(pid)) FROM pg_stat_activity WHERE datname = current_database() "
                    "AND pid <> pg_backend_pid() AND state = 'active' AND now() - query_start > %s * interval '1 second' "
                    "AND query NOT LIKE '%%paperrust_task_queue%%' AND query NOT LIKE '%%paperrust_sync_sequence%%'", (older_than_s,))
        return cur.fetchone()[0]


ADMIN_TOKEN = "5ca1e0000000000000000000000000000000ad01"
RESTRICTED_TOKEN = "5ca1e0000000000000000000000000000000ee02"


class Client:
    def __init__(self, base: str, token: str, timeout: float = 600, version: int = 10):
        timeout = REQ_TIMEOUT if timeout == 600 else timeout
        u = urllib.parse.urlparse(base)
        self.version = version
        self.host, self.port, self.token, self.timeout = u.hostname, u.port, token, timeout
        self.c = None

    def req(self, method: str, path: str, body=None, headers=None):
        hdr = {"Authorization": f"Token {self.token}", "Accept": f"application/json; version={self.version}", "Accept-Encoding": "identity"}
        if headers:
            hdr.update(headers)
        data = None
        if body is not None:
            if isinstance(body, (bytes, bytearray)):
                data = body
            else:
                data = json.dumps(body).encode()
                hdr["Content-Type"] = "application/json"
        for attempt in (0, 1):
            if self.c is None:
                self.c = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
            try:
                t0 = time.perf_counter()
                self.c.request(method, path, body=data, headers=hdr)
                resp = self.c.getresponse()
                raw = resp.read()
                dt = (time.perf_counter() - t0) * 1000
                if resp.getheader("Connection", "").lower() == "close":
                    self.c.close()
                    self.c = None
                return resp.status, raw, dt
            except TimeoutError:
                self.c = None
                raise
            except (ConnectionError, http.client.HTTPException, OSError):
                self.c = None
                if attempt:
                    raise
        raise RuntimeError("unreachable")


def pct(v: list[float], p: float) -> float:
    if not v:
        return float("nan")
    s = sorted(v)
    k = (len(s) - 1) * p
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


# ---------------------------------------------------------------------------------------------------
def discover(dsn: str) -> dict:
    """IDs for the endpoints from the DB (read only)."""
    c = {}
    with psycopg.connect(dsn) as conn, conn.cursor() as cur:
        q = lambda sql, *a: (cur.execute(sql, a), cur.fetchall())[1]
        c["restricted_id"] = q("SELECT id FROM auth_user WHERE username='restricted'")[0][0]
        tags = q("SELECT tag_id, count(*) FROM documents_document_tags WHERE tag_id<>1 GROUP BY 1 ORDER BY 2 DESC")
        c["tag_top"], c["tag_2"], c["tag_mid"], c["tag_rare"] = tags[0][0], tags[1][0], tags[len(tags) // 10][0], tags[len(tags) // 2][0]
        # Root with the most descendants (via the parent edge; treenode columns no longer exist)
        c["tag_root_nested"] = q(
            "WITH RECURSIVE d(root, id) AS (SELECT id, id FROM documents_tag WHERE tn_parent_id IS NULL "
            "UNION ALL SELECT d.root, t.id FROM documents_tag t JOIN d ON t.tn_parent_id = d.id) "
            "SELECT root FROM d GROUP BY root ORDER BY count(*) DESC, root LIMIT 1")[0][0]
        corr = q("SELECT correspondent_id, count(*) FROM documents_document WHERE correspondent_id IS NOT NULL GROUP BY 1 ORDER BY 2 DESC")
        c["corr_top"], c["corr_mid"] = corr[0][0], corr[len(corr) // 20][0]
        c["corr_10k"] = min(corr, key=lambda x: abs(x[1] - 10000))[0]
        c["corr_10k_n"] = min(corr, key=lambda x: abs(x[1] - 10000))[1]
        # Bulk edit: storage path 1 (files present) from a date on, ~10k documents
        n_sp1 = q("SELECT count(*) FROM documents_document WHERE storage_path_id=1 AND deleted_at IS NULL")[0][0]
        k = min(10000, max(1, n_sp1 // 5))
        c["bulk_from"] = str(q("SELECT created FROM documents_document WHERE storage_path_id=1 AND deleted_at IS NULL ORDER BY created DESC LIMIT 1 OFFSET %s", k - 1)[0][0])
        c["bulk_n"] = q("SELECT count(*) FROM documents_document WHERE storage_path_id=1 AND deleted_at IS NULL AND created >= %s", c["bulk_from"])[0][0]
        c["dt_top"] = q("SELECT document_type_id FROM documents_document WHERE document_type_id IS NOT NULL GROUP BY 1 ORDER BY count(*) DESC LIMIT 1")[0][0]
        cf = {t: q("SELECT id FROM documents_customfield WHERE data_type=%s ORDER BY id LIMIT 1", t)[0][0] for t in ("integer", "date", "select", "string", "monetary")}
        c["cf"] = cf
        c["cf_select_opt"] = q("SELECT extra_data->'select_options'->0->>'id' FROM documents_customfield WHERE id=%s", cf["select"])[0][0]
        c["doc_shared"] = q("SELECT id FROM documents_document WHERE owner_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 1 OFFSET 50")[0][0]
        c["doc_media"] = q("SELECT id FROM documents_document WHERE owner_id IS NULL AND deleted_at IS NULL AND storage_path_id IS DISTINCT FROM 1 AND filename NOT LIKE 'sp1/%%' ORDER BY id")[0][0]
        # File document: one with a real file (thumbnail present in the generator == media/rename set); owner NULL preferred
        c["doc_sp1"] = q("SELECT id FROM documents_document WHERE storage_path_id=1 AND owner_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 1")[0][0]
        c["first_ids_100"] = [r[0] for r in q("SELECT id FROM documents_document WHERE owner_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 100")]
        c["first_ids_10k"] = [r[0] for r in q("SELECT id FROM documents_document WHERE owner_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 10000")]
        try:
            c["uuid_tag_top"] = str(q("SELECT uuid FROM paperrust_sync_object WHERE kind=2 AND local_id=%s", c["tag_top"])[0][0])
            c["uuid_doc_shared"] = str(q("SELECT uuid FROM paperrust_sync_object WHERE kind=1 AND local_id=%s", c["doc_shared"])[0][0])
        except Exception:  # noqa: BLE001
            c["uuid_tag_top"] = c["uuid_doc_shared"] = None
        sv = {}
        for uname in ("admin", "restricted"):
            s = q("SELECT u.settings FROM documents_uisettings u JOIN auth_user a ON a.id=u.user_id WHERE a.username=%s", uname)
            ids = s[0][0]["saved_views"]["dashboard_views_visible_ids"] if s else []
            views = []
            for vid in ids:
                rules = q("SELECT rule_type, value FROM documents_savedviewfilterrule WHERE saved_view_id=%s ORDER BY id", vid)
                srt = q("SELECT sort_field, sort_reverse FROM documents_savedview WHERE id=%s", vid)
                views.append({"id": vid, "rules": rules, "sort": srt[0] if srt else ("created", True)})
            sv[uname] = views
        c["dashboard"] = sv
    return c


# Rule type → query parameter (excerpt from pl-sync-client/src/filter_rules.rs for the generated types)
RULE_PARAM = {6: ("tags__id__all", True), 22: ("tags__id__in", True), 17: ("tags__id__none", True), 26: ("correspondent__id__in", True),
              4: ("document_type__id", False), 9: ("created__date__gt", False), 19: ("title_content", False), 42: ("custom_field_query", False),
              33: ("owner__id__in", True), 5: ("is_in_inbox", False)}


def view_query(v: dict, page_size=10) -> str:
    params: dict[str, str] = {}
    for rt, val in v["rules"]:
        name, multi = RULE_PARAM[rt]
        params[name] = f"{params[name]},{val}" if multi and name in params else str(val)
    sf, rev = v["sort"]
    params.update(page="1", page_size=str(page_size), ordering=("-" if rev else "") + (sf or "created"), truncate_content="true")
    return "/api/documents/?" + urllib.parse.urlencode(params)


def endpoints(c: dict) -> list[tuple[str, str, str, object]]:
    # Filter/sorting without selection_data (isolates the filter cost); the Angular UI (API v10)
    # always sends include_selection_data=true → own ui_* endpoints
    D = "/api/documents/?page=1&page_size=25&truncate_content=true"
    U = D + "&include_selection_data=true"
    cfq = json.dumps(["AND", [[c["cf"]["integer"], "gt", 50000], [c["cf"]["date"], "gte", "2022-01-01"]]])
    cfq_sel = json.dumps([c["cf"]["select"], "exact", c["cf_select_opt"]])
    cfq_or = json.dumps(["OR", [[c["cf"]["string"], "icontains", "betrag"], [c["cf"]["monetary"], "gt", 10000]]])
    e = [
        ("ui_docs_default", "GET", f"{U}&ordering=-created", None),
        ("ui_docs_correspondent", "GET", f"{U}&ordering=-created&correspondent__id__in={c['corr_mid']}", None),
        ("ui_fts_query_word", "GET", f"{U}&query=rechnung", None),
        ("docs_default", "GET", f"{D}&ordering=-created", None),
        ("docs_default_v9_all_ids", "GET", f"{D}&ordering=-created&__v9", None),
        ("docs_sort_title", "GET", f"{D}&ordering=title", None),
        ("docs_sort_correspondent", "GET", f"{D}&ordering=correspondent__name", None),
        ("docs_sort_asn", "GET", f"{D}&ordering=-archive_serial_number", None),
        ("docs_sort_num_notes", "GET", f"{D}&ordering=-num_notes", None),
        ("docs_sort_custom_field", "GET", f"{D}&ordering=-custom_field_{c['cf']['integer']}", None),
        ("docs_sort_added", "GET", f"{D}&ordering=-added", None),
        ("docs_page_2000", "GET", f"/api/documents/?page=2000&page_size=25&truncate_content=true&ordering=-created", None),
        ("docs_fields_id_all", "GET", "/api/documents/?page=1&page_size=100000&fields=id", None),
        ("f_tag_any", "GET", f"{D}&ordering=-created&tags__id__in={c['tag_mid']},{c['tag_rare']}", None),
        ("f_tag_all", "GET", f"{D}&ordering=-created&tags__id__all={c['tag_top']},{c['tag_2']}", None),
        ("f_tag_none", "GET", f"{D}&ordering=-created&tags__id__none={c['tag_top']}", None),
        ("f_tag_nested_root", "GET", f"{D}&ordering=-created&tags__id__in={c['tag_root_nested']}", None),
        ("f_correspondent", "GET", f"{D}&ordering=-created&correspondent__id__in={c['corr_mid']}", None),
        ("f_created_range", "GET", f"{D}&ordering=-created&created__date__gte=2020-01-01&created__date__lte=2020-12-31", None),
        ("f_cfq_and", "GET", f"{D}&ordering=-created&custom_field_query={urllib.parse.quote(cfq)}", None),
        ("f_cfq_select", "GET", f"{D}&ordering=-created&custom_field_query={urllib.parse.quote(cfq_sel)}", None),
        ("f_cfq_or_icontains", "GET", f"{D}&ordering=-created&custom_field_query={urllib.parse.quote(cfq_or)}", None),
        ("f_cf_icontains", "GET", f"{D}&ordering=-created&custom_fields__icontains=kalo", None),
        ("f_owner_in", "GET", f"{D}&ordering=-created&owner__id__in={c['restricted_id']}", None),
        ("f_shared_by_me", "GET", f"{D}&ordering=-created&shared_by__id={c['restricted_id']}", None),
        ("f_title_content_common", "GET", f"{D}&ordering=-created&title_content=rechnung", None),
        ("f_title_content_rare", "GET", f"{D}&ordering=-created&title_content=zzqxnotfound", None),
        ("f_inbox", "GET", f"{D}&ordering=-created&is_in_inbox=1", None),
        ("f_combo", "GET", f"{D}&ordering=-created&correspondent__id__in={c['corr_top']}&tags__id__all={c['tag_top']}&created__date__gte=2018-01-01", None),
        ("fts_query_word", "GET", f"{D}&query=rechnung", None),
        ("fts_query_multi", "GET", f"{D}&query=kunde%20betrag%20zahlung", None),
        ("fts_query_rare", "GET", f"{D}&query=kalomiber", None),
        ("fts_more_like", "GET", f"{D}&more_like_id={c['doc_shared']}", None),
        ("search_autocomplete", "GET", "/api/search/autocomplete/?term=rech", None),
        ("search_global", "GET", "/api/search/?query=rech", None),
        ("selection_data_100", "POST", "/api/documents/selection_data/", {"documents": c["first_ids_100"]}),
        ("selection_data_10k", "POST", "/api/documents/selection_data/", {"documents": c["first_ids_10k"]}),
        ("selection_data_all", "POST", "/api/documents/selection_data/", {"all": True, "filters": {}}),
        ("statistics", "GET", "/api/statistics/", None),
        ("tags_page", "GET", "/api/tags/?page=1&page_size=25&ordering=name", None),
        ("tags_all", "GET", "/api/tags/?page=1&page_size=100000", None),
        ("correspondents_page", "GET", "/api/correspondents/?page=1&page_size=25&ordering=name", None),
        ("correspondents_all", "GET", "/api/correspondents/?page=1&page_size=100000", None),
        ("correspondents_by_count", "GET", "/api/correspondents/?page=1&page_size=25&ordering=-document_count", None),
        ("document_types_all", "GET", "/api/document_types/?page=1&page_size=100000", None),
        ("storage_paths_all", "GET", "/api/storage_paths/?page=1&page_size=100000", None),
        ("custom_fields_all", "GET", "/api/custom_fields/?page=1&page_size=100000", None),
        ("saved_views_all", "GET", "/api/saved_views/?page=1&page_size=100000", None),
        ("users_all", "GET", "/api/users/?page=1&page_size=100000", None),
        ("ui_settings", "GET", "/api/ui_settings/", None),
        ("doc_retrieve", "GET", f"/api/documents/{c['doc_shared']}/", None),
        ("doc_notes", "GET", f"/api/documents/{c['doc_shared']}/notes/", None),
        ("doc_suggestions", "GET", f"/api/documents/{c['doc_shared']}/suggestions/", None),
        ("doc_metadata", "GET", f"/api/documents/{c['doc_sp1']}/metadata/", None),
        ("doc_thumb", "GET", f"/api/documents/{c['doc_sp1']}/thumb/", None),
        ("doc_download", "GET", f"/api/documents/{c['doc_sp1']}/download/", None),
        ("tasks", "GET", "/api/tasks/", None),
        ("status", "GET", "/api/status/", None),
        ("v2_documents", "GET", "/rs/api/v2/documents?limit=50", None),
        ("v2_documents_count", "GET", "/rs/api/v2/documents?limit=50&count=true", None),
        ("v2_query_tag_date", "POST", "/rs/api/v2/documents/query",
         {"filter": {"all": [{"tags": {"any": [c["uuid_tag_top"]]}}, {"created": {"from": "2020-01-01", "to": "2020-12-31"}}]}, "sort": "-created", "limit": 50, "count": True}),
        ("v2_query_text", "POST", "/rs/api/v2/documents/query", {"filter": {"text": {"query": "rechnung"}}, "limit": 50}),
        ("v2_timeline", "GET", "/rs/api/v2/timeline", None),
        ("v2_upcoming", "GET", "/rs/api/v2/timeline/upcoming", None),
        ("v2_sync_changes_first", "GET", "/rs/api/v2/sync/changes?limit=500&embed=true", None),
        ("v2_sync_ids_documents", "GET", "/rs/api/v2/sync/ids?kind=document", None),
    ]
    return e


def measure(base, token, method, path, body, n1, dur8, conc=8) -> dict:
    ver = 10
    if path.endswith("&__v9"):
        path, ver = path[:-5], 9
    cl = Client(base, token, version=ver)
    st, raw, dt = cl.req(method, path, body)  # Warm up
    res = {"status": st, "bytes": len(raw), "warm_ms": round(dt, 1)}
    if st >= 400:
        res["error"] = raw[:300].decode("utf-8", "replace")
        return res
    lat = []
    t_end = time.time() + 40
    n_min = 2 if dt > 5000 else 3
    while len(lat) < n1 and (time.time() < t_end or len(lat) < n_min):
        st, raw, dt = cl.req(method, path, body)
        lat.append(dt)
    res.update(c1_n=len(lat), c1_p50=round(pct(lat, .5), 1), c1_p95=round(pct(lat, .95), 1), c1_max=round(max(lat), 1),
               c1_rps=round(1000 / statistics.mean(lat), 2))
    if dur8 <= 0 or pct(lat, .5) > 8000:
        res["c8_skipped"] = "c1 p50 > 8 s" if dur8 > 0 else "off"
        return res
    lat8, errs, lock = [], [0], threading.Lock()
    stop_at = time.time() + dur8

    def worker():
        c = Client(base, token, version=ver)
        while True:
            try:
                st, _, dt = c.req(method, path, body)
            except Exception:  # noqa: BLE001
                with lock:
                    errs[0] += 1
                if time.time() >= stop_at:
                    return
                continue
            with lock:
                lat8.append(dt)
                if st >= 400:
                    errs[0] += 1
            if time.time() >= stop_at:
                return

    t0 = time.time()
    th = [threading.Thread(target=worker) for _ in range(conc)]
    for t in th:
        t.start()
    for t in th:
        t.join()
    wall = time.time() - t0
    res.update(c8_n=len(lat8), c8_p50=round(pct(lat8, .5), 1), c8_p95=round(pct(lat8, .95), 1), c8_rps=round(len(lat8) / wall, 2), c8_err=errs[0])
    return res


# ---------------------------------------------------------------------------------------------------
def wait_tasks(cl: Client, task_ids: list[str], timeout=3600) -> float:
    t0 = time.time()
    pending = set(task_ids)
    while pending and time.time() - t0 < timeout:
        for tid in list(pending):
            st, raw, _ = cl.req("GET", f"/api/tasks/?task_id={tid}")
            if st == 200:
                arr = json.loads(raw)
                if isinstance(arr, dict):
                    arr = arr.get("results", [])
                if arr and str(arr[0].get("status")).upper() in ("SUCCESS", "FAILURE"):
                    pending.discard(tid)
        if pending:
            time.sleep(0.5)
    return time.time() - t0


def scenario_bulk(base, dsn, c) -> dict:
    cl = Client(base, ADMIN_TOKEN)
    flt = {"storage_path__id__in": "1", "created__date__gte": c["bulk_from"]}
    out = {"filter": flt, "expected": c["bulk_n"]}
    st, raw, dt = cl.req("POST", "/api/tags/", {"name": f"bench-bulk-{int(time.time())}", "matching_algorithm": 0})
    tag = json.loads(raw)["id"]
    # selection_data for the same selection (UI calls it when selecting): see endpoint selection_data_10k
    # (> 120 s or > 15 min with all+filters) – omitted here, otherwise it blocks the scenario.
    for method in ("add_tag", "remove_tag"):
        t0 = time.time()
        st, raw, dt = cl.req("POST", "/api/documents/bulk_edit/", {"all": True, "filters": flt, "method": method, "parameters": {"tag": tag}})
        out[f"{method}_status"] = st
        out[f"{method}_ms"] = round(dt, 1)
        # Wait for the index catch-up (task bulk_update_documents): queue empty
        out[f"{method}_queue_drain_s"] = round(wait_queue_empty(dsn), 1)
    cl.req("DELETE", f"/api/tags/{tag}/")
    return out


def wait_queue_empty(dsn, timeout=3600) -> float:
    t0 = time.time()
    with psycopg.connect(dsn, autocommit=True) as conn, conn.cursor() as cur:
        cur.execute("SELECT to_regclass('paperrust_task_queue') IS NOT NULL")
        if not cur.fetchone()[0]:
            return float("nan")
        while time.time() - t0 < timeout:
            cur.execute("SELECT count(*) FROM paperrust_task_queue WHERE state IN ('queued','running') AND eta <= now() + interval '5 seconds'")
            if cur.fetchone()[0] == 0:
                return time.time() - t0
            time.sleep(0.5)
    return float("nan")


def scenario_rename(base, dsn, window_s: float = 600, media_root: str = "") -> dict:
    """Rename storage path 1 (50,000 documents with files): PATCH response time + follow-up work
    (bulk_update_documents: file name per document anew, move file, index). If the follow-up work takes
    longer than `window_s`, the rate is measured and extrapolated, the task ended via
    terminate_requested and the initial state restored directly (path via SQL,
    moved files back to sp1/<year>/<pk>.pdf) – a reverse PATCH would take hours again."""
    import os
    cl = Client(base, ADMIN_TOKEN)
    out = {"window_s": window_s}
    with psycopg.connect(dsn, autocommit=True) as conn, conn.cursor() as cur:
        cur.execute("SELECT path FROM documents_storagepath WHERE id=1")
        orig = cur.fetchone()[0]
        cur.execute("SELECT count(*) FROM documents_document WHERE storage_path_id=1")
        out["docs"] = cur.fetchone()[0]
        t0 = time.time()
        st, raw, dt = cl.req("PATCH", "/api/storage_paths/1/", {"path": "renamed/{{ created_year }}/{{ correspondent }}/{{ title }}"})
        out["patch_status"], out["patch_ms"] = st, round(dt, 1)
        samples = []
        while True:
            cur.execute("SELECT count(*) FROM documents_document WHERE storage_path_id=1 AND filename LIKE 'renamed/%%'")
            n = cur.fetchone()[0]
            samples.append((round(time.time() - t0, 1), n))
            if n >= out["docs"] or time.time() - t0 > window_s:
                break
            time.sleep(5)
        el = time.time() - t0
        out["moved"], out["elapsed_s"] = n, round(el, 1)
        # Rate from the first moved document (start overhead separate)
        first = next((t for t, k in samples if k > 0), None)
        out["first_move_after_s"] = first
        if first is not None and el - first > 0:
            rate = n / (el - first)
            out["docs_per_s"] = round(rate, 2)
            out["ms_per_doc"] = round(1000 / rate, 1) if rate else None
            out["extrapolated_total_s"] = round(first + out["docs"] / rate, 0) if rate else None
        out["samples"] = samples[:: max(1, len(samples) // 20)]
        if n < out["docs"]:
            cur.execute("UPDATE paperrust_task_queue SET revoke_requested = true, terminate_requested = true "
                        "WHERE task_name = 'documents.tasks.bulk_update_documents' AND state IN ('queued','running')")
            for _ in range(120):
                cur.execute("SELECT count(*) FROM paperrust_task_queue WHERE task_name = 'documents.tasks.bulk_update_documents' AND state = 'running'")
                if cur.fetchone()[0] == 0:
                    break
                time.sleep(1)
        # Restore initial state (without signal: path via SQL, files back)
        t1 = time.time()
        cur.execute("UPDATE documents_storagepath SET path = %s WHERE id = 1", (orig,))
        cur.execute("SELECT id, filename, EXTRACT(YEAR FROM created)::int FROM documents_document "
                    "WHERE storage_path_id = 1 AND filename NOT LIKE 'sp1/%%'")
        rows = cur.fetchall()
        base_dir = os.path.join(media_root, "documents", "originals")
        fixed = 0
        for pk, fn, year in rows:
            target = f"sp1/{year}/{pk:07d}.pdf"
            src, dst = os.path.join(base_dir, fn), os.path.join(base_dir, target)
            if os.path.exists(src):
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                try:
                    os.replace(src, dst)
                except OSError as e:  # Task may still be moving (abort in progress)
                    out.setdefault("restore_errors", []).append(str(e)[:200])
            cur.execute("UPDATE documents_document SET filename = %s WHERE id = %s", (target, pk))
            fixed += 1
        out["restored_docs"], out["restore_s"] = fixed, round(time.time() - t1, 1)
    return out


def scenario_ingest(base, dsn, n=50) -> dict:
    cl = Client(base, ADMIN_TOKEN)
    out = {"n": n}
    tids = []
    t0 = time.time()
    for i in range(n):
        bnd = uuidlib.uuid4().hex
        txt = (f"Rechnung Nr. BENCH-{i:04d} {uuidlib.uuid4()}\nBetrag 123.45 CHF\nKundennummer 4711\n" * 20).encode()
        body = (f"--{bnd}\r\nContent-Disposition: form-data; name=\"document\"; filename=\"bench-{i:04d}.txt\"\r\n"
                f"Content-Type: text/plain\r\n\r\n").encode() + txt + f"\r\n--{bnd}--\r\n".encode()
        st, raw, dt = cl.req("POST", "/api/documents/post_document/", body, {"Content-Type": f"multipart/form-data; boundary={bnd}"})
        if st == 200:
            tids.append(json.loads(raw))
    out["upload_s"] = round(time.time() - t0, 2)
    out["accepted"] = len(tids)
    out["consumed_s"] = round(wait_tasks(cl, tids) + out["upload_s"], 2)
    out["per_doc_ms"] = round(out["consumed_s"] * 1000 / max(1, len(tids)), 1)
    # clean up: delete new documents + empty trash
    with psycopg.connect(dsn, autocommit=True) as conn, conn.cursor() as cur:
        cur.execute("SELECT id FROM documents_document WHERE original_filename LIKE 'bench-%%.txt'")
        ids = [r[0] for r in cur.fetchall()]
        st = []
        for t in tids:
            cur.execute("SELECT status FROM documents_paperlesstask WHERE task_id=%s", (t,))
            st.append(cur.fetchone())
    out["task_status"] = {s: sum(1 for x in st if x and x[0] == s) for s in {x[0] for x in st if x}}
    if ids:
        cl.req("POST", "/api/documents/bulk_edit/", {"documents": ids, "method": "delete", "parameters": {}})
        cl.req("POST", "/api/trash/", {"action": "empty", "documents": ids})
    return out


# ---------------------------------------------------------------------------------------------------
def parse_profile(log: Path, since: int) -> list[dict]:
    """Access lines + sqlx::query lines before them (sequential calls) → queries/request."""
    out, q_n, q_ms, worst = [], 0, 0.0, (0.0, "")
    with log.open(encoding="utf-8", errors="replace") as f:
        f.seek(since)
        for line in f:
            if "sqlx::query" in line:
                if "paperrust_sync_sequence" in line or "paperrust_task_queue" in line:
                    continue  # Background (sequencer, queue query)
                q_n += 1
                m = re.search(r"elapsed_secs=([\d.eE+-]+)", line)
                if m:
                    v = float(m.group(1)) * 1000
                    q_ms += v
                    if v > worst[0]:
                        sm = re.search(r'summary="([^"]*)"', line)
                        worst = (v, sm.group(1) if sm else "")
            elif "paperrust::access" in line:
                m = re.search(r"path=(\S+).*?status=(\d+) ms=(\d+)", line)
                if m:
                    out.append({"path": m.group(1), "status": int(m.group(2)), "ms": int(m.group(3)), "queries": q_n,
                                "sql_ms": round(q_ms, 1), "worst_sql_ms": round(worst[0], 1), "worst_sql": worst[1][:160]})
                q_n, q_ms, worst = 0, 0.0, (0.0, "")
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:58181")
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--skip", default="")
    ap.add_argument("--users", default="admin,restricted")
    ap.add_argument("--n1", type=int, default=12)
    ap.add_argument("--dur8", type=float, default=12)
    ap.add_argument("--scenarios", default="")
    ap.add_argument("--profile-log", default="")
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--resume", action="store_true", help="vorhandene results.json fortsetzen")
    ap.add_argument("--rename-window", type=float, default=600, help="max. Beobachtung Umbenennung (s)")
    ap.add_argument("--media-root", default="", help="PAPERLESS_MEDIA_ROOT der Messinstanz (Wiederherstellung)")
    a = ap.parse_args()
    global REQ_TIMEOUT, DSN
    REQ_TIMEOUT, DSN = a.timeout, a.dsn
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    ctx = discover(a.dsn)
    (out / "context.json").write_text(json.dumps({k: v for k, v in ctx.items() if not k.startswith("first_ids")}, indent=1, default=str), encoding="utf-8")
    eps = endpoints(ctx)
    for uname, views in ctx["dashboard"].items():
        for k, v in enumerate(views):
            eps.append((f"dashboard_{uname}_w{k}", "GET", view_query(v), None))
    if a.only:
        eps = [e for e in eps if re.search(a.only, e[0])]
    if a.skip:
        eps = [e for e in eps if not re.search(a.skip, e[0])]
    tokens = {"admin": ADMIN_TOKEN, "restricted": RESTRICTED_TOKEN}
    results: dict = {"context": {k: v for k, v in ctx.items() if not k.startswith("first_ids")}, "endpoints": {}, "scenarios": {}}
    resfile = out / ("profile.json" if a.profile_log else "results.json")
    if a.resume and resfile.exists():
        results["endpoints"] = json.loads(resfile.read_text(encoding="utf-8")).get("endpoints", {})

    if a.profile_log:
        log = Path(a.profile_log)
        prof = {}
        for uname in a.users.split(","):
            cl = Client(a.base, tokens[uname])
            for name, method, path, body in eps:
                if name.startswith("dashboard_") and not name.startswith(f"dashboard_{uname}"):
                    continue
                cl.version = 10
                if path.endswith("&__v9"):
                    path, cl.version = path[:-5], 9
                cl.req(method, path, body)  # warm
                time.sleep(0.3)
                pos = log.stat().st_size
                st, raw, dt = cl.req(method, path, body)
                time.sleep(0.6)
                rows = parse_profile(log, pos)
                prof[f"{uname}:{name}"] = {"status": st, "client_ms": round(dt, 1), "bytes": len(raw), "server": rows[-1] if rows else None}
                print(f"{uname:10s} {name:28s} {dt:8.1f} ms  q={rows[-1]['queries'] if rows else '?'}", flush=True)
        resfile.write_text(json.dumps(prof, indent=1), encoding="utf-8")
        return 0

    if a.scenarios:
        for s in a.scenarios.split(","):
            print(f"Szenario {s} …", flush=True)
            if s == "bulk":
                results["scenarios"][s] = scenario_bulk(a.base, a.dsn, ctx)
            elif s == "rename":
                results["scenarios"][s] = scenario_rename(a.base, a.dsn, a.rename_window, a.media_root)
            elif s == "ingest":
                results["scenarios"][s] = scenario_ingest(a.base, a.dsn)
            print(json.dumps(results["scenarios"][s]), flush=True)
        sf = out / "scenarios.json"
        old = json.loads(sf.read_text(encoding="utf-8")) if sf.exists() else {}
        old.update(results["scenarios"])
        sf.write_text(json.dumps(old, indent=1), encoding="utf-8")
        return 0

    for uname in a.users.split(","):
        for name, method, path, body in eps:
            if name.startswith("dashboard_") and not name.startswith(f"dashboard_{uname}"):
                continue
            key = f"{uname}:{name}"
            if key in results["endpoints"] and not results["endpoints"][key].get("error"):
                continue
            try:
                r = measure(a.base, tokens[uname], method, path, body, a.n1, a.dur8)
            except TimeoutError:
                r = {"error": f"timeout > {REQ_TIMEOUT:.0f} s", "timeout": True, "status": "timeout"}
                print(f"  abgebrochen: {cancel_running()} Abfrage(n)", flush=True)
            except Exception as e:  # noqa: BLE001
                r = {"error": repr(e)}
            r["path"] = path if len(path) < 300 else path[:300] + "…"
            results["endpoints"][f"{uname}:{name}"] = r
            print(f"{uname:10s} {name:28s} st={r.get('status')} c1 p50={r.get('c1_p50')} p95={r.get('c1_p95')} "
                  f"c8 p50={r.get('c8_p50')} rps={r.get('c8_rps')} bytes={r.get('bytes')}", flush=True)
            resfile.write_text(json.dumps(results, indent=1), encoding="utf-8")
    # summary.md
    lines = ["| Endpunkt | admin c1 p50 | p95 | c8 p50 | c8 req/s | restricted c1 p50 | p95 | c8 p50 | c8 req/s | Bytes (admin) |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    names = sorted({k.split(":", 1)[1] for k in results["endpoints"]}, key=lambda n: [e[0] for e in eps].index(n) if n in [e[0] for e in eps] else 999)
    for n in names:
        A, R = results["endpoints"].get(f"admin:{n}", {}), results["endpoints"].get(f"restricted:{n}", {})
        f = lambda d, k: ("err " + str(d.get("status"))) if d.get("error") else (d.get(k, "–") if d else "–")
        lines.append(f"| {n} | {f(A,'c1_p50')} | {f(A,'c1_p95')} | {f(A,'c8_p50')} | {f(A,'c8_rps')} | {f(R,'c1_p50')} | {f(R,'c1_p95')} | {f(R,'c8_p50')} | {f(R,'c8_rps')} | {A.get('bytes', '–')} |")
    (out / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
