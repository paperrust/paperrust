#!/usr/bin/env python3
"""PaperRust Ausfall- und Wiederherstellungs-Suite (tests/chaos, nur Python-Standardbibliothek).

Läuft gegen den Wegwerf-Server aus tests/chaos/server.sh, der an der EIGENEN PostgreSQL-Instanz aus
tests/chaos/pg.sh hängt (Port 54400). Die geteilte Referenz-DB (54329) wird nie angefasst.

Szenarien (Details: tests/chaos/README.md):
  c1  Datenbank weg während Einlesen / Massenänderung / Bedienung, nach 10 s bzw. 2 min zurück
  c2  Datenbank-Neustart mitten in langer Aufgabe (Export, Archiv neu erzeugen, Index neu, Massenänderung)
  c3  Platte voll / Schreibfehler (Medien-, Vorschau-, Index-, Export-Ordner nicht beschreibbar)
  c4  Netz weg: Mail-Abruf (IMAP-Stub), Webhook, KI-Dienst (Mock), Remote-OCR (Mock)
  c5  harter Server-Abbruch in jeder Phase des Einlesens (zeitgesteuert über viele Punkte)
  c6  Sicherung und Wiederherstellung (Export alle Arten -> frische DB -> Import -> Vergleich; pl-guard)
  c7  Konsistenz (läuft nach jedem Szenario): Dateien, Prüfsummen, Index, Warteschlange, Sanity-Checker

Aufruf (Git-Bash im Repo):
  bash tests/chaos/pg.sh init && bash tests/chaos/server.sh reset && bash tests/chaos/server.sh start
  python tests/chaos/chaos.py c1 c2 c5 c7
Ergebnis: <out>/summary.json + summary.md (Standard %TEMP%/paperrust-chaos-results/<lauf>).
Exit 1 bei jedem Befund.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import random
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(REPO / "tests" / "stress"))
import stress as S  # noqa: E402  (Api, test files, task helpers, recorder)

REC = S.REC
log = S.log
TOKEN_ADMIN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"
BASH = shutil.which("bash") or r"C:\Program Files\Git\bin\bash.exe"
TERMINAL = S.TERMINAL


# ----------------------------------------------------------------------------- Environment


def sh(script, *args, timeout=900, env=None, check=False):
    e = dict(os.environ)
    if env:
        e.update(env)
    r = subprocess.run([BASH, str(HERE / script), *args], capture_output=True, text=True, encoding="utf-8",
                       errors="replace", timeout=timeout, env=e)
    out = (r.stdout + r.stderr).strip()
    if check and r.returncode != 0:
        raise RuntimeError(f"{script} {' '.join(args)} fehlgeschlagen: {out[-600:]}")
    return r.returncode, out


class Ctx:
    def __init__(self, a):
        self.a = a
        self.reload()
        self.api = S.Api(self.url, "admin", "admin", TOKEN_ADMIN, timeout=a.timeout)
        self.t_start = int(time.time()) - 5

    def reload(self):
        _, out = sh("server.sh", "env")
        vals = dict(ln.split("=", 1) for ln in out.splitlines() if "=" in ln and not ln.startswith("["))
        self.url = vals["CH_URL"]
        self.pid = vals.get("CH_PID")
        self.media = Path(vals["CH_MEDIA"])
        self.data = Path(vals["CH_DATA"])
        self.consume = Path(vals["CH_CONSUME"])
        self.work = Path(vals["CH_WORK"])
        self.logdir = Path(vals["CH_LOG"])
        self.psql = vals["CH_PSQL"]
        self.dsn = vals["CH_DSN"]

    def sql(self, q, quiet=False):
        r = subprocess.run([self.psql, "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-d", self.dsn, "-c", q],
                           capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
        if r.returncode != 0:
            if not quiet:
                REC.finding("tool", f"psql fehlgeschlagen: {r.stderr.strip()[:200]}")
            return None
        return [ln.split("\t") for ln in r.stdout.splitlines() if ln]

    def one(self, q):
        r = self.sql(q)
        return r[0][0] if r else None

    # Control server / DB
    def server(self, *cmd, env=None):
        rc, out = sh("server.sh", *cmd, env=env)
        log(f"  [server {' '.join(cmd)}] {out.splitlines()[-1] if out else ''}")
        self.reload()
        self.api.close()
        return rc, out

    def pg(self, *cmd):
        rc, out = sh("pg.sh", *cmd)
        log(f"  [pg {' '.join(cmd)}] {out.splitlines()[-1] if out else ''}")
        return rc, out

    def manage(self, *cmd, timeout=1800):
        return sh("server.sh", "manage", *cmd, timeout=timeout)

    def log_tail_since(self, t0, pattern=None, limit=60):
        """WARN/ERROR lines from the server log since t0 (epoch seconds), grouped."""
        out = collections.Counter()
        for f in (self.logdir / "paperrust.log", self.logdir / "rust.err.log", self.logdir / "rust.out.log"):
            if not f.exists():
                continue
            try:
                with open(f, encoding="utf-8", errors="replace") as fh:
                    for ln in fh:
                        if (" WARN " in ln or " ERROR " in ln or "[WARNING]" in ln or "[ERROR]" in ln) and (
                                pattern is None or re.search(pattern, ln)):
                            ts = ln[:19]
                            try:
                                tt = time.mktime(time.strptime(ts.replace("T", " "), "%Y-%m-%d %H:%M:%S"))
                                # Log timestamps are UTC (tracing) or local (paperless.log)
                                if ln[:30].find("Z") > 0:
                                    tt -= time.timezone - (3600 if time.daylight and time.localtime().tm_isdst else 0)
                                if tt < t0 - 2:
                                    continue
                            except ValueError:
                                pass
                            key = re.sub(r"\d{4}-\d\d-\d\dT[\d:.]+Z?|\[\d{4}-[^\]]+\]", "", ln).strip()
                            key = re.sub(r"\b[0-9a-f]{8}-[0-9a-f-]{27}\b", "<uuid>", key)
                            key = re.sub(r"\b\d+\b", "N", key)[:260]
                            out[key] += 1
            except OSError:
                pass
        return out.most_common(limit)


def wait_http(ctx, timeout=300, path="/api/documents/?page_size=1"):
    """Waits until the API returns 200 again. Returns seconds until then or None."""
    t0 = time.time()
    c = ctx.api.clone(timeout=20)
    while time.time() - t0 < timeout:
        try:
            st, _ = c.raw("GET", path)
            if st == 200:
                c.close()
                return time.time() - t0
        except Exception:
            c.close()
        time.sleep(0.5)
    c.close()
    return None


def probe(ctx, op, method, path, payload=None, files=None, fields=None, timeout=60):
    """Single request during an outage: status, duration, body (for assessing the message)."""
    c = ctx.api.clone(timeout=timeout)
    t = time.perf_counter()
    body = None
    headers = {}
    if files is not None or fields is not None:
        body, ct = S.multipart(fields or {}, files or [])
        headers["Content-Type"] = ct
    elif payload is not None:
        body = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    try:
        st, data = c.raw(method, path, body, headers)
        txt = data[:300].decode("utf-8", "replace")
    except Exception as e:
        st, txt = -1, f"{type(e).__name__}: {e}"
    dt = time.perf_counter() - t
    c.close()
    REC.add(op, st, dt, True, f"{method} {path} -> {txt}")
    return st, dt, txt


def judge_outage_reply(op, st, dt, txt, max_secs=15):
    """Assessment of a response while the DB is gone: fast, 503, understandable message."""
    ok = True
    if st == -1:
        REC.finding("message", f"{op}: keine Antwort ({txt[:120]}) nach {dt:.1f}s")
        ok = False
    elif st >= 500 and st != 503:
        REC.finding("message", f"{op}: HTTP {st} statt 503 'Datenbank nicht erreichbar' ({txt[:120]!r})")
        ok = False
    elif st == 503 and not re.search(r"database|Datenbank", txt, re.I):
        REC.finding("message", f"{op}: 503 ohne verständliche Meldung ({txt[:120]!r})")
        ok = False
    if dt > max_secs:
        REC.finding("message", f"{op}: Antwort erst nach {dt:.1f}s (Nutzer wartet; erwartet <= {max_secs}s)")
        ok = False
    return ok


# ----------------------------------------------------------------------------- Test files

RUN = time.strftime("%H%M%S")
FAC = S.DocFactory("c" + RUN, seed=int(RUN))
UPLOADS: dict = {}  # task_id -> doc


def upload(ctx, doc, op="upload", ok=(200,), extra=None):
    fields = {"title": doc["title"]}
    if extra:
        fields.update(extra)
    st, d = ctx.api.clone().call(op, "POST", "/api/documents/post_document/",
                                 files=[("document", doc["name"], doc["data"], doc["ctype"])], fields=fields, ok=ok)
    if st == 200 and isinstance(d, str):
        UPLOADS[d] = doc
        S.UPLOADS[d] = doc.get("token")
        return d
    return None


def docs_with_token(ctx, token):
    """Document ids (incl. trash) with the search word in the content – directly from the DB."""
    rows = ctx.sql(f"SELECT id FROM documents_document WHERE content LIKE '%{token}%' ORDER BY id")
    return [int(r[0]) for r in rows or []]


def wait_tasks(ctx, tids, timeout, label):
    return S.wait_tasks(ctx.api, tids, timeout, label)


def check_once(ctx, docs, label, allow_missing=()):
    """Every file exactly once as a document (via its unique search word)."""
    lost, dup = [], []
    for d in docs:
        if not d.get("token"):
            continue
        ids = docs_with_token(ctx, d["token"])
        if len(ids) == 0 and d["token"] not in allow_missing:
            lost.append(d["name"])
        elif len(ids) > 1:
            dup.append((d["name"], ids))
    if lost:
        REC.finding("lost", f"{label}: {len(lost)} Dateien nicht als Dokument angekommen", examples=lost[:10])
    if dup:
        REC.finding("duplicate", f"{label}: {len(dup)} Dateien doppelt eingelesen", examples=dup[:10])
    return lost, dup


def task_failures(ctx, tids, label, allowed=None):
    rows = S.tasks_by_id(ctx.api, tids)
    res = collections.Counter()
    for t in tids:
        r = rows.get(t, {})
        s = str(r.get("status")).lower()
        res[s] += 1
        if s != "success":
            msg = str(r.get("result") or r.get("result_data"))[:240]
            if allowed and re.search(allowed, msg or ""):
                continue
            REC.finding("task", f"{label}: Aufgabe {s}: {msg}", task=t)
    return dict(res)


# ----------------------------------------------------------------------------- c7 consistency


def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def md5(p):
    h = hashlib.md5()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def wait_idle(ctx, timeout=900):
    deadline = time.time() + timeout
    while time.time() < deadline:
        q = ctx.sql("SELECT count(*) FROM paperrust_task_queue WHERE state IN ('queued','running') "
                    "AND (eta IS NULL OR eta < now() + interval '30 seconds')", quiet=True)
        if q and int(q[0][0]) == 0:
            return True
        time.sleep(3)
    q = ctx.sql("SELECT task_name, state, count(*) FROM paperrust_task_queue WHERE state IN ('queued','running') GROUP BY 1,2")
    REC.finding("hang", f"Warteschlange nach {timeout}s nicht leer: {q}")
    return False


def sanity_ok(rc, out):
    """Sanity checker: ok without lines of level ERROR/WARNING (INFO such as "no OCR data" for images
    without text is a hint, not a problem)."""
    if rc != 0:
        return False
    if "No issues detected" in out:
        return True
    return not re.search(r"^(ERROR|WARNING)\s*\|", out, re.M)


def c7_consistency(ctx, label="c7", sanity=True):
    log(f"C7 [{label}]: Konsistenz")
    wait_idle(ctx, ctx.a.idle_timeout)
    res = {}
    rows = ctx.sql("SELECT id, coalesce(filename,''), coalesce(archive_filename,''), coalesce(checksum,''), "
                   "coalesce(archive_checksum,''), mime_type FROM documents_document") or []
    media = ctx.media / "documents"
    ref = set()
    bad = collections.Counter()
    for did, fn, an, ck, ack, mt in rows:
        if not fn:
            bad["no_filename"] += 1
            REC.finding("files", f"{label}: Dokument {did}: kein Dateiname")
            continue
        p = media / "originals" / fn
        ref.add(os.path.normcase(str(p)))
        if not p.exists():
            bad["original_missing"] += 1
            REC.finding("files", f"{label}: Dokument {did}: Original fehlt ({fn})")
        elif ck and len(ck) == 32 and md5(p) != ck or ck and len(ck) == 64 and sha256(p) != ck:
            bad["checksum"] += 1
            REC.finding("files", f"{label}: Dokument {did}: Prüfsumme Original stimmt nicht")
        if an:
            pa = media / "archive" / an
            ref.add(os.path.normcase(str(pa)))
            if not pa.exists():
                bad["archive_missing"] += 1
                REC.finding("files", f"{label}: Dokument {did}: Archivdatei fehlt ({an})")
            elif ack and len(ack) == 32 and md5(pa) != ack or ack and len(ack) == 64 and sha256(pa) != ack:
                bad["archive_checksum"] += 1
                REC.finding("files", f"{label}: Dokument {did}: Prüfsumme Archiv stimmt nicht")
        th = [media / "thumbnails" / f"{int(did):07d}.{e}" for e in ("webp", "png")]
        ref.update(os.path.normcase(str(t)) for t in th)
        if not any(t.exists() for t in th):
            bad["thumbnail_missing"] += 1
            REC.finding("files", f"{label}: Dokument {did}: Vorschaubild fehlt")
    orphans = []
    for sub in ("originals", "archive", "thumbnails"):
        base = media / sub
        if base.exists():
            for p in base.rglob("*"):
                if p.is_file() and os.path.normcase(str(p)) not in ref:
                    orphans.append(str(p.relative_to(ctx.media)))
    if orphans:
        REC.finding("files", f"{label}: {len(orphans)} verwaiste Dateien", examples=orphans[:10])
    # Leftovers in spool / working folder
    scratch = ctx.data.parent / "scratch"
    left = [p for p in scratch.rglob("*") if p.is_file()] if scratch.exists() else []
    # The worker cleans up the working folder of a hard-terminated process after timeout + 10 min
    # (pl-consumer sweep_stale_scratch); finding only if something older than 45 min remains.
    stale = [str(p.relative_to(scratch)) for p in left if time.time() - p.stat().st_mtime > 45 * 60]
    if stale:
        REC.finding("files", f"{label}: {len(stale)} Dateien bleiben im Arbeitsordner (scratch) länger als 45 min liegen",
                    examples=stale[:5])
    left = [str(p.relative_to(scratch)) for p in left]
    # Search index: every document of this run via its search word
    miss = 0
    for tid, d in list(UPLOADS.items()):
        tok = d.get("token")
        if not tok:
            continue
        ids = docs_with_token(ctx, tok)
        if len(ids) != 1:
            continue
        st, j = ctx.api.get("c7:search", f"/api/documents/?query={tok}&page_size=5&fields=id")
        if st == 200 and ids[0] not in [x["id"] for x in j.get("results", [])]:
            deleted = ctx.one(f"SELECT (deleted_at IS NOT NULL)::int FROM documents_document WHERE id={ids[0]}")
            if deleted == "0":
                miss += 1
                if miss <= 10:
                    REC.finding("index", f"{label}: Dokument {ids[0]} über {tok} nicht im Suchindex")
    # Queue / tasks
    stuck = ctx.sql("SELECT task_id, task_name, state, attempts FROM paperrust_task_queue "
                    "WHERE state IN ('queued','running') AND (eta IS NULL OR eta < now())")
    if stuck:
        REC.finding("queue", f"{label}: {len(stuck)} Einträge in paperrust_task_queue offen", examples=stuck[:5])
    hang = ctx.sql("SELECT task_id, task_type, status FROM documents_paperlesstask WHERE lower(status) IN ('pending','started') "
                   f"AND date_created < now() - interval '2 minutes' AND date_created > to_timestamp({ctx.t_start})")
    if hang:
        REC.finding("queue", f"{label}: {len(hang)} PaperlessTask hängen (PENDING/STARTED > 2 min)", examples=hang[:5])
    res.update({"documents_incl_trash": len(rows), "bad": dict(bad), "orphans": len(orphans),
                "scratch_left": len(left), "index_missing": miss})
    if sanity:
        rc, out = ctx.manage("document_sanity_checker")
        tail = [ln for ln in out.splitlines() if "WARN pl_core::config" not in ln][-8:]
        res["sanity_checker"] = {"rc": rc, "tail": tail}
        if not sanity_ok(rc, out):
            REC.finding("sanity", f"{label}: document_sanity_checker meldet Probleme (rc {rc})", examples=tail)
    log(f"  -> {res}")
    return res


# ----------------------------------------------------------------------------- c1 database gone


def outage_probes(ctx, ids, tag_id, label):
    """Operation while the DB is gone (list, detail, change, bulk change, upload, UI)."""
    out = []
    did = ids[0]
    checks = [
        ("liste", "GET", f"/api/documents/?page_size=25", None),
        ("detail", "GET", f"/api/documents/{did}/", None),
        ("patch", "PATCH", f"/api/documents/{did}/", {"title": f"chaos-{label}-patch"}),
        ("bulk", "POST", "/api/documents/bulk_edit/", {"documents": ids[:20], "method": "add_tag",
                                                     "parameters": {"tag": tag_id}}),
        ("status", "GET", "/api/status/", None),
        ("tasks", "GET", "/api/tasks/?page_size=5", None),
    ]
    for name, m, p, body in checks:
        st, dt, txt = probe(ctx, f"{label}:{name}", m, p, payload=body)
        good = judge_outage_reply(f"{label}:{name}", st, dt, txt)
        out.append({"op": name, "status": st, "secs": round(dt, 1), "ok": good, "body": txt[:160]})
    doc = FAC.next()
    st, dt, txt = probe(ctx, f"{label}:upload", "POST", "/api/documents/post_document/",
                        files=[("document", doc["name"], doc["data"], doc["ctype"])], fields={"title": doc["title"]})
    if st == 200:
        tid = txt.strip().strip('"')
        UPLOADS[tid] = doc
        out.append({"op": "upload", "status": st, "secs": round(dt, 1), "ok": True, "accepted": tid, "doc": doc})
    else:
        good = judge_outage_reply(f"{label}:upload", st, dt, txt)
        out.append({"op": "upload", "status": st, "secs": round(dt, 1), "ok": good, "body": txt[:160]})
    # UI (HTML) – must not hang
    st, dt, txt = probe(ctx, f"{label}:ui", "GET", "/rs/app/")
    out.append({"op": "ui", "status": st, "secs": round(dt, 1), "ok": dt < 15})
    return out


def c1_case(ctx, down_secs, label):
    log(f"C1 [{label}]: Datenbank {down_secs}s weg während Einlesen + Massenänderung + Bedienung")
    t0 = time.time()
    tag = S.ensure_tag(ctx.api, f"chaos-{label}-{RUN}")
    probe_tag = S.ensure_tag(ctx.api, f"chaos-{label}-probe-{RUN}")
    ids = sorted(S.all_ids(ctx.api)[0])
    docs = [FAC.next("png" if i % 9 == 8 else "pdf") for i in range(ctx.a.docs)]
    tids = [t for t in (upload(ctx, d, op=f"{label}:upload_before") for d in docs) if t]
    # wait until the first ones are processed (ingestion is certainly running)
    deadline = time.time() + 300
    while time.time() < deadline:
        rows = S.tasks_by_id(ctx.api, tids)
        if sum(1 for t in tids if rows.get(t, {}).get("status") in TERMINAL) >= 2:
            break
        time.sleep(0.5)
    # Bulk change in the background shortly before the outage (all documents)
    bulk_res = {}

    def bulk_bg():
        st, dt, txt = probe(ctx, f"{label}:bulk_during", "POST", "/api/documents/bulk_edit/",
                            payload={"documents": ids, "method": "add_tag", "parameters": {"tag": tag}}, timeout=120)
        bulk_res.update(status=st, secs=round(dt, 1), body=txt[:200])
    th = threading.Thread(target=bulk_bg)
    th.start()
    time.sleep(0.02)
    ctx.pg("kill")
    t_down = time.time()
    # Operation during the outage in its own thread: the DB comes back after exactly down_secs,
    # no matter how long the requests hang
    probes = []
    pt = threading.Thread(target=lambda: probes.extend(outage_probes(ctx, ids, probe_tag, label)))
    pt.start()
    rest = down_secs - (time.time() - t_down)
    if rest > 0:
        time.sleep(rest)
    ctx.pg("start")
    pt.join(300)
    th.join(150)
    rec = wait_http(ctx, 300)
    log(f"  API wieder da nach {rec if rec is None else round(rec, 1)}s")
    if rec is None or rec > 30:
        REC.finding("recovery", f"{label}: API nach DB-Rückkehr erst nach {rec}s wieder bereit")
    accepted = [p for p in probes if p.get("accepted")]
    all_tids = tids + [p["accepted"] for p in accepted]
    all_docs = docs + [p["doc"] for p in accepted]
    rows = wait_tasks(ctx, all_tids, ctx.a.ingest_timeout, f"{label}: Einlesen")
    tasks = task_failures(ctx, all_tids, label)
    lost, dup = check_once(ctx, all_docs, label)
    # Bulk change: all or nothing
    have = int(ctx.one(f"SELECT count(*) FROM documents_document_tags WHERE tag_id={tag} AND document_id IN "
                       f"({','.join(map(str, ids))})") or 0)
    bulk_res["tagged"] = f"{have}/{len(ids)}"
    if have not in (0, len(ids)):
        REC.finding("partial", f"{label}: Massenänderung nur teilweise ausgeführt ({have}/{len(ids)})")
    if have == 0 and bulk_res.get("status") not in (200, 503, None):
        REC.finding("message", f"{label}: Massenänderung beim Ausfall abgebrochen, Meldung HTTP {bulk_res.get('status')} "
                               f"{bulk_res.get('body', '')[:80]!r} statt 503 „Datenbank nicht erreichbar“")
    if bulk_res.get("status") == 200 and have != len(ids):
        REC.finding("lost", f"{label}: Massenänderung mit 200 bestätigt, aber nur {have}/{len(ids)} Dokumente getaggt")
    # Follow-up tasks (bulk_update_documents, index) must not get stuck
    res = {"down_secs": down_secs, "uploads": len(all_tids), "tasks": tasks, "lost": lost, "duplicates": dup,
           "probes": probes, "bulk": bulk_res, "recovery_secs": rec and round(rec, 1)}
    res["consistency"] = c7_consistency(ctx, label)
    res["log"] = ctx.log_tail_since(t0, limit=25)
    return res


class workflows_off:
    """Switch off test data workflows (webhook to 127.0.0.1:9, mail) for the duration (like tests/stress)."""

    def __init__(self, ctx):
        self.ctx = ctx

    def __enter__(self):
        r = self.ctx.sql("SELECT id FROM documents_workflow WHERE enabled")
        self.ids = [x[0] for x in r or []]
        if self.ids:
            self.ctx.sql(f"UPDATE documents_workflow SET enabled=false WHERE id IN ({','.join(self.ids)})")
        return self

    def __exit__(self, *a):
        if self.ids:
            self.ctx.sql(f"UPDATE documents_workflow SET enabled=true WHERE id IN ({','.join(self.ids)})")


def c1_db_outage(ctx):
    with workflows_off(ctx):
        return _c1(ctx)


def _c1(ctx):
    out = {}
    for secs, label in ((10, "c1-10s"), (ctx.a.long_outage, f"c1-{ctx.a.long_outage}s")):
        out[label] = c1_case(ctx, secs, label)
    return out



# ----------------------------------------------------------------------------- c5 hard abort per phase


class LogTail:
    """New lines from paperrust.log (from now on)."""

    def __init__(self, path):
        self.path = path
        self.pos = path.stat().st_size if path.exists() else 0

    def lines(self):
        if not self.path.exists():
            return []
        with open(self.path, "rb") as f:
            f.seek(self.pos)
            data = f.read()
        cut = data.rfind(b"\n") + 1
        self.pos += cut
        return data[:cut].decode("utf-8", "replace").splitlines()


def hard_kill(ctx):
    pid = ctx.pid
    if pid:
        subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
    pidf = ctx.work / "run" / "rust.pid"
    try:
        pidf.unlink()
    except OSError:
        pass


# (name, pattern in the log – {stem} = file name without extension, delay after the hit in s)
PHASES = [
    ("direkt nach Upload (Aufgabe noch nicht begonnen)", None, 0.0),
    ("Vorprüfung/Beginn", r"Consuming {stem}", 0.0),
    ("während Texterkennung (Parsing)", r"Parsing {stem}", 0.0),
    ("während Vorschaubild", r"Generating thumbnail for {stem}", 0.0),
    ("Beginn Speichern (Transaktion offen, vor Datei ablegen)", r"Saving record to database", 0.0),
    ("in der Transaktion (Zuordnung, Dateien abgelegt?)", r"Assigning|Tagging|matched on document .*{stem}", 0.0),
    ("nach Commit, vor Löschen des Originals", r"Deleting original file .*{stem}", 0.0),
    ("nach 'consumption finished' (Suchindex/Nacharbeit)", r"Document \d+ consumption finished", 0.0),
    ("kurz nach Abschluss (Index schreiben)", r"Document \d+ consumption finished", 0.25),
]


def c5_kill_phases(ctx):
    with workflows_off(ctx):
        return _c5(ctx)


def _c5(ctx):
    out = []
    t0 = time.time()
    for name, pat, delay in PHASES:
        for rep in range(ctx.a.kill_reps):
            doc = FAC.next()
            stem = doc["name"].rsplit(".", 1)[0]
            tail = LogTail(ctx.logdir / "paperrust.log")
            tid = upload(ctx, doc, op="c5:upload")
            hit = None
            if pat is None:
                hit = "sofort"
            else:
                rx = re.compile(pat.format(stem=re.escape(stem)))
                dl = time.time() + 120
                while time.time() < dl and not hit:
                    for ln in tail.lines():
                        if rx.search(ln):
                            hit = ln[:160]
                            break
                    else:
                        time.sleep(0.01)
            if delay:
                time.sleep(delay)
            hard_kill(ctx)
            log(f"C5 [{name}#{rep}] hart beendet bei: {hit}")
            rc, msg = ctx.server("start")
            if rc != 0:
                REC.finding("recovery", f"c5 {name}: Neustart fehlgeschlagen: {msg[-300:]}")
                continue
            rows = wait_tasks(ctx, [tid], ctx.a.ingest_timeout, f"c5 {name}") if tid else {}
            st = str(rows.get(tid, {}).get("status")).lower() if tid else "kein upload"
            ids = docs_with_token(ctx, doc["token"])
            r = {"phase": name, "rep": rep, "hit": hit, "task": st, "documents": ids}
            if st != "success":
                REC.finding("restart", f"c5 {name}: Aufgabe {st}: {str(rows.get(tid, {}).get('result'))[:200]}")
            if len(ids) != 1:
                REC.finding("lost" if not ids else "duplicate", f"c5 {name}: Datei {doc['name']} als {len(ids)} Dokumente")
            # after every abort: files/index of this document
            r["consistency"] = c7_consistency(ctx, f"c5 {name}", sanity=False)
            out.append(r)
    res = {"kills": out, "consistency": c7_consistency(ctx, "c5-ende"), "log": ctx.log_tail_since(t0, limit=25)}
    return res


# ----------------------------------------------------------------------------- c6 backup / restore

# Tables that are not compared (derived, transient or operating state) – with reason.
VOLATILE = {
    "django_session": "Sitzungen (upstream exportiert sie nicht)",
    "django_migrations": "Migrationsstand des Zielsystems",
    "paperrust_migrations": "Migrationsstand des Zielsystems",
    "django_content_type": "ids entstehen beim migrate neu (upstream: contenttypes werden ersetzt)",
    "auth_permission": "ids entstehen beim migrate neu (Zuordnung über codename)",
    "auth_group_permissions": "verweist auf neu nummerierte auth_permission (Inhalt per codename geprüft: perm:group)",
    "auth_user_user_permissions": "verweist auf neu nummerierte auth_permission (Inhalt per codename geprüft: perm:user)",
    "paperrust_task_queue": "Warteschlange (Betriebszustand)",
    "paperrust_task_group": "Warteschlange (Betriebszustand)",
    "documents_paperlesstask": "Aufgabenliste (upstream exportiert sie nicht)",
    "paperrust_beat_state": "Zeitplaner-Zustand",
    "paperrust_cache": "Cache",
    "paperrust_label_count": "abgeleiteter Zähler (Trigger)",
    "paperrust_label_count_delta": "abgeleiteter Zähler (Trigger)",
    "paperrust_search_dirty": "Suchindex-Merkliste",
    "paperrust_sync_log": "Sync-Protokoll (Gerätesync, abgeleitet)",
    "paperrust_sync_meta": "Sync-Zustand",
    "paperrust_sync_ident": "Sync-Zustand",
    "paperrust_sync_pending": "Sync-Zustand",
    "paperrust_sync_tombstone": "Sync-Zustand",
    "paperrust_integrity_state": "Prüflauf-Zustand",
    "paperrust_backup_filecache": "Sicherungs-Zustand",
    "paperrust_backup_object": "Sicherungs-Zustand",
    "paperrust_backup_run": "Sicherungs-Zustand",
    "paperrust_guard_state": "Schutz-Zustand",
    "paperrust_guard_event": "Schutz-Ereignisse",
    "paperrust_guard_finding": "Schutz-Befunde",
    "paperrust_grant_meta": "abgeleiteter Freigabe-Index (manage grants rebuild)",
    "paperrust_grant_perm": "abgeleiteter Freigabe-Index",
    "paperrust_grant_raw": "abgeleiteter Freigabe-Index",
    "paperrust_doc_date_state": "abgeleitet (Fristen-Erkennung)",
    "paperrust_timeline_state": "abgeleitet (Fristen-Erkennung)",
    "paperrust_ai_job": "KI-Aufgaben (Betriebszustand)",
    "paperrust_setting_history": "Änderungsverlauf der Einstellungen",
    "authtoken_token": "API-Token: upstream-Exporter sichert sie nicht (Zugänge nach Wiederherstellung neu anlegen)",
}


def db_tables(dsn, psql):
    r = subprocess.run([psql, "-At", "-d", dsn, "-c",
                        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace "
                        "WHERE n.nspname='public' AND c.relkind='r' ORDER BY 1"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    return [x for x in r.stdout.split() if x]


# Columns that are intentionally different after export/import (with reason, see BERICHT.md C6):
#  uuid/rev: sync identity and change counter of the PaperRust tables are created anew on import
#  modified (document): the upstream importer sets auto_now fields to "now" on import (bulk_create)
#  id (document-tag mapping): row ids of the join table without meaning
IGNORE_COLS = {
    "*paperrust_": {"uuid", "rev"},
    "paperrust_document": {"modified"},
    "paperrust_document_tag": {"id"},
    "auth_user": {"last_login"},
}
ROW_FILTER = {"auth_user": "username <> 'AnonymousUser'"}


def _ignored(t):
    out = set(IGNORE_COLS.get(t, set()))
    if t.startswith("paperrust_"):
        out |= IGNORE_COLS["*paperrust_"]
    return out


def db_fingerprint(dsn, psql):
    """Per table: rows and md5 over all rows (sorted). Timestamps to milliseconds (the
    upstream export format, DjangoJSONEncoder, truncates microseconds), columns from IGNORE_COLS left out;
    permissions additionally by codename (ids of auth_permission are created anew on migrate)."""
    cols = {}
    r = subprocess.run([psql, "-At", "-F", chr(9), "-d", dsn, "-c",
                        "SELECT c.table_name, c.column_name, c.data_type FROM information_schema.columns c "
                        "JOIN information_schema.tables t ON t.table_name=c.table_name AND t.table_schema=c.table_schema "
                        "WHERE c.table_schema='public' AND t.table_type='BASE TABLE' ORDER BY c.table_name, c.ordinal_position"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    for ln in r.stdout.splitlines():
        t, c, ty = ln.split(chr(9))
        cols.setdefault(t, []).append((c, ty))
    out = {}
    for t, cl in cols.items():
        ign = _ignored(t)
        exprs = []
        for c, ty in cl:
            if c in ign:
                continue
            q = '"' + c + '"'
            exprs.append(f"date_trunc('milliseconds', {q})" if ty.startswith("timestamp") else q)
        if not exprs:
            continue
        w = f" WHERE {ROW_FILTER[t]}" if t in ROW_FILTER else ""
        q = (f"SELECT count(*), coalesce(md5(string_agg(r, '|' ORDER BY r)), '-') FROM "
             f"(SELECT ROW({', '.join(exprs)})::text AS r FROM \"{t}\"{w}) x")
        r = subprocess.run([psql, "-At", "-F", chr(9), "-d", dsn, "-c", q], capture_output=True, text=True,
                           encoding="utf-8", errors="replace")
        if r.returncode == 0 and r.stdout.strip():
            n, h = r.stdout.strip().split(chr(9))
            out[t] = (int(n), h)
    for name, q in {
        "perm:group": "SELECT g.name||':'||p.codename FROM auth_group_permissions gp JOIN auth_group g ON g.id=gp.group_id "
                      "JOIN auth_permission p ON p.id=gp.permission_id ORDER BY 1",
        "perm:user": "SELECT u.username||':'||p.codename FROM auth_user_user_permissions up JOIN auth_user u ON u.id=up.user_id "
                     "JOIN auth_permission p ON p.id=up.permission_id ORDER BY 1",
    }.items():
        r = subprocess.run([psql, "-At", "-d", dsn, "-c", q], capture_output=True, text=True, encoding="utf-8", errors="replace")
        rows = r.stdout.split()
        out[name] = (len(rows), hashlib.md5(chr(10).join(rows).encode()).hexdigest())
    return out


def files_fingerprint(media):
    out = {}
    base = Path(media) / "documents"
    for sub in ("originals", "archive", "thumbnails"):
        d = base / sub
        if d.exists():
            for p in d.rglob("*"):
                if p.is_file():
                    out[f"{sub}/{p.relative_to(d).as_posix()}"] = sha256(p)
    return out


def compare(src_db, dst_db, src_files, dst_files, label):
    diffs = []
    for t in sorted(set(src_db) | set(dst_db)):
        if t in VOLATILE:
            continue
        a, b = src_db.get(t), dst_db.get(t)
        if a != b:
            diffs.append({"table": t, "source": a, "restored": b})
    missing = sorted(set(src_files) - set(dst_files))
    extra = sorted(set(dst_files) - set(src_files))
    changed = sorted(k for k in set(src_files) & set(dst_files) if src_files[k] != dst_files[k])
    if diffs:
        REC.finding("restore", f"{label}: {len(diffs)} Tabellen weichen ab",
                    examples=[f"{d['table']}: {d['source']} -> {d['restored']}" for d in diffs[:30]])
    if missing or changed or extra:
        REC.finding("restore", f"{label}: Dateien weichen ab (fehlend {len(missing)}, verändert {len(changed)}, "
                               f"zusätzlich {len(extra)})", examples={"missing": missing[:10], "changed": changed[:10],
                                                                       "extra": extra[:10]})
    return {"table_diffs": diffs, "files": {"source": len(src_files), "restored": len(dst_files),
                                            "missing": missing[:20], "changed": changed[:20], "extra": extra[:20]}}


def rmtree_force(p):
    """Delete folder, including read-only files (backup objects of pl-guard are read-only)."""
    import stat

    def onexc(func, path, exc):
        try:
            os.chmod(path, stat.S_IWRITE)
            func(path)
        except OSError:
            pass
    if Path(p).exists():
        shutil.rmtree(p, onexc=onexc)


def restore_env(db="pl_chaos_r"):
    return {"CH_DB": db}


def prepare_rich_data(ctx):
    """Contents that a backup must preserve: note, custom field value, permissions, workflow with
    else branch and AI condition, workflow history."""
    api = ctx.api
    did = sorted(S.all_ids(api)[0])[0]
    api.post("c6:note", f"/api/documents/{did}/notes/", {"note": f"Chaos-Notiz {RUN} äöü"})
    st, cf = api.post("c6:cf", "/api/custom_fields/", {"name": f"chaos-cf-{RUN}", "data_type": "string"})
    if isinstance(cf, dict) and cf.get("id"):
        api.call("c6:cf_set", "PATCH", f"/api/documents/{did}/",
                 {"custom_fields": [{"field": cf["id"], "value": f"wert-{RUN}"}]})
    api.call("c6:perm", "POST", "/api/documents/bulk_edit/",
             {"documents": [did], "method": "set_permissions",
              "parameters": {"set_permissions": {"view": {"users": [3], "groups": []}, "change": {"users": [], "groups": [1]}},
                             "merge": True}})
    wf = ctx.one("SELECT min(id) FROM documents_workflow")
    act = ctx.one("SELECT max(id) FROM documents_workflowaction")
    if wf and act:
        ctx.sql(f"INSERT INTO paperrust_workflow_else_action (workflow_id, workflowaction_id) VALUES ({wf}, {act}) "
                "ON CONFLICT DO NOTHING")
        ctx.sql(f"INSERT INTO paperrust_workflow_ext (workflow_id, ai_condition) VALUES ({wf}, "
                "'{\"question\": \"Ist das eine Rechnung?\", \"expect\": true}'::jsonb) ON CONFLICT (workflow_id) DO UPDATE "
                "SET ai_condition = excluded.ai_condition")
        ctx.sql(f"INSERT INTO paperrust_workflow_run (workflow_id, trigger_type, document_id, result, branch, message) "
                f"VALUES ({wf}, 1, {did}, 'ok', 'else', 'chaos {RUN}')")
    return {"document": did, "workflow": wf}


def run_export(ctx, kind, root):
    out = root / f"export-{kind}"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    args = ["document_exporter", str(out)]
    if not kind.startswith("dir"):
        args += ["-z", "--zip-compression", kind, "-zn", f"chaos-{kind}"]
    if kind == "dir-pass":
        args += ["--passphrase", "chaos-export-local-only"]
    if kind == "dir-split":
        args += ["--split-manifest"]
    t = time.time()
    rc, outp = ctx.manage(*args)
    secs = round(time.time() - t, 1)
    if rc != 0:
        REC.finding("export", f"Export {kind} fehlgeschlagen (rc {rc}): {outp[-300:]}")
        return None, secs
    path = out if kind.startswith("dir") else out / f"chaos-{kind}.zip"
    return path, secs


def restore_into_fresh(ctx, path, kind):
    env = restore_env()
    sh("server.sh", "reset-empty", env=env)
    rc, o = sh("server.sh", "manage", "migrate", "--no-input", env=env, timeout=1800)
    if rc != 0:
        REC.finding("restore", f"{kind}: migrate auf frischer DB fehlgeschlagen: {o[-300:]}")
        return None
    args = ["document_importer", str(path)]
    if kind == "dir-pass":
        args += ["--passphrase", "chaos-export-local-only"]
    t = time.time()
    rc, o = sh("server.sh", "manage", *args, env=env, timeout=3600)
    secs = round(time.time() - t, 1)
    if rc != 0:
        REC.finding("restore", f"{kind}: Import fehlgeschlagen (rc {rc}): {o[-400:]}")
        return None
    return secs


def target_ctx_vals():
    _, out = sh("server.sh", "env", env=restore_env())
    return dict(ln.split("=", 1) for ln in out.splitlines() if "=" in ln and not ln.startswith("["))


def c6_backup(ctx):
    res = {"prepared": prepare_rich_data(ctx)}
    wait_idle(ctx, 300)
    root = ctx.work.parent / "c6"
    root.mkdir(parents=True, exist_ok=True)
    src_db = db_fingerprint(ctx.dsn, ctx.psql)
    src_files = files_fingerprint(ctx.media)
    res["source"] = {"tables": len(src_db), "files": len(src_files)}
    tv = target_ctx_vals()
    res["exports"] = {}
    for kind in ctx.a.export_kinds.split(","):
        path, esecs = run_export(ctx, kind, root)
        entry = {"export_secs": esecs}
        if path:
            isecs = restore_into_fresh(ctx, path, kind)
            entry["import_secs"] = isecs
            if isecs is not None:
                dst_db = db_fingerprint(tv["CH_DSN"], ctx.psql)
                dst_files = files_fingerprint(tv["CH_MEDIA"])
                entry["compare"] = compare(src_db, dst_db, src_files, dst_files, f"Export/Import {kind}")
                rc, o = sh("server.sh", "manage", "document_sanity_checker", env=restore_env())
                entry["sanity"] = o.splitlines()[-1:] if o else None
                if not sanity_ok(rc, o):
                    REC.finding("sanity", f"{kind}: Sanity-Checker nach Import meldet Probleme", examples=o.splitlines()[-6:])
        res["exports"][kind] = entry
        log(f"C6 {kind}: export {esecs}s import {entry.get('import_secs')}s, "
            f"Abweichungen: {len((entry.get('compare') or {}).get('table_diffs', []))}")
    res["guard"] = c6_guard(ctx, root, src_db, src_files, tv)
    sh("server.sh", "drop", env=restore_env())
    rmtree_force(root)
    return res


def c6_guard(ctx, root, src_db, src_files, tv):
    target = root / "guard-target"
    rmtree_force(target)
    genv = {"PAPERRUST_BACKUP_TARGET": str(target), "PAPERRUST_BACKUP_PASSPHRASE": "chaos-backup-local-only"}
    out = {}
    for step in (["backup", "init"], ["backup", "run"], ["backup", "list"], ["backup", "check"]):
        rc, o = sh("server.sh", "manage", *step, env=genv, timeout=1800)
        out[" ".join(step)] = {"rc": rc, "out": [ln for ln in o.splitlines() if "pl_core::config" not in ln][-2:]}
        if rc != 0:
            REC.finding("guard", f"manage {' '.join(step)} fehlgeschlagen: {o[-300:]}")
            return out
    env = {**restore_env(), **genv}
    sh("server.sh", "reset-empty", env=env)
    rc, o = sh("server.sh", "manage", "migrate", "--no-input", env=env, timeout=1800)
    rc, o = sh("server.sh", "manage", "backup", "restore", "--database", "--media", tv["CH_MEDIA"], env=env, timeout=3600)
    out["restore"] = {"rc": rc, "out": o.splitlines()[-3:]}
    if rc != 0:
        REC.finding("guard", f"backup restore fehlgeschlagen: {o[-400:]}")
        return out
    dst_db = db_fingerprint(tv["CH_DSN"], ctx.psql)
    dst_files = files_fingerprint(tv["CH_MEDIA"])
    out["compare"] = compare(src_db, dst_db, src_files, dst_files, "pl-guard-Wiederherstellung")
    rc, o = sh("server.sh", "manage", "document_sanity_checker", env=restore_env())
    out["sanity"] = o.splitlines()[-1:]
    if not sanity_ok(rc, o):
        REC.finding("sanity", "pl-guard: Sanity-Checker nach Wiederherstellung meldet Probleme", examples=o.splitlines()[-6:])
    return out


# ----------------------------------------------------------------------------- c2 DB restart in a long task


def manage_popen(*args, env=None):
    e = dict(os.environ)
    if env:
        e.update(env)
    return subprocess.Popen([BASH, str(HERE / "server.sh"), "manage", *args], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", env=e)


def restart_db_during(ctx, proc, after, down=5.0):
    """Waits `after` s (or until the process ends), terminates the DB hard, restarts it after `down` s."""
    t0 = time.time()
    while time.time() - t0 < after and proc.poll() is None:
        time.sleep(0.05)
    still = proc.poll() is None
    ctx.pg("kill")
    time.sleep(down)
    ctx.pg("start")
    try:
        out, _ = proc.communicate(timeout=1800)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, _ = proc.communicate()
        REC.finding("hang", "Befehl hängt nach DB-Neustart (30 min)")
    return still, proc.returncode, out


def clean_tail(out, n=4):
    return [ln for ln in (out or "").splitlines() if "pl_core::config" not in ln and " INFO " not in ln][-n:]


def judge_cli_after_db_loss(label, still, rc, out):
    """If the command was still running during the outage, it must end with an error code and an understandable message."""
    if not still:
        log(f"  {label}: Befehl war schon fertig, bevor die DB weg war (kein Befund, zu kurz)")
        return
    if rc == 0:
        return  # survived the outage
    txt = (out or "")[-1500:]
    if not re.search(r"database|Datenbank|connection|Verbindung", txt, re.I):
        REC.finding("message", f"{label}: Abbruch ohne verständliche Meldung (rc {rc}): {clean_tail(out, 3)}")
    if re.search(r"panicked|RUST_BACKTRACE|stack backtrace", txt):
        REC.finding("crash", f"{label}: Absturz (panic) statt Fehlermeldung: {clean_tail(out, 3)}")


def c2_long_tasks(ctx):
    with workflows_off(ctx):
        return _c2(ctx)


def _c2(ctx):
    res = {}
    root = ctx.work.parent / "c2"
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    # (a) Export (folder and ZIP/lzma): DB gone after 1 s
    for kind in ("dir", "lzma"):
        out_dir = root / f"export-{kind}"
        out_dir.mkdir()
        args = ["document_exporter", str(out_dir)] + ([] if kind == "dir" else ["-z", "--zip-compression", "lzma", "-zn", "x"])
        p = manage_popen(*args)
        still, rc, out = restart_db_during(ctx, p, 1.0)
        wait_http(ctx, 120)
        judge_cli_after_db_loss(f"Export {kind}", still, rc, out)
        left = sorted(str(x.relative_to(out_dir)) for x in out_dir.rglob("*") if x.is_file())
        r = {"running_at_outage": still, "rc": rc, "tail": clean_tail(out), "files_left": len(left)}
        if rc != 0:
            # an aborted export must not look like a valid one
            if kind == "dir" and (out_dir / "manifest.json").exists():
                REC.finding("export", "Export dir: nach Abbruch liegt eine manifest.json (sieht vollständig aus)")
            if kind != "dir" and (out_dir / "x.zip").exists():
                REC.finding("export", "Export lzma: nach Abbruch liegt eine ZIP-Datei (sieht vollständig aus)")
            tmp = [x for x in left if x.endswith(".tmp") or ".tmp" in x]
            r["tmp_left"] = tmp
        # repetition must work
        rc2, out2 = ctx.manage(*args)
        r["rerun_rc"] = rc2
        if rc2 != 0:
            REC.finding("recovery", f"Export {kind}: Wiederholung nach DB-Neustart scheitert: {clean_tail(out2, 3)}")
        res[f"export_{kind}"] = r
    shutil.rmtree(root, ignore_errors=True)
    # (b) Regenerate archive (CLI, all documents): DB gone after 3 s
    p = manage_popen("document_archiver", "--overwrite")
    still, rc, out = restart_db_during(ctx, p, 3.0)
    wait_http(ctx, 120)
    judge_cli_after_db_loss("document_archiver", still, rc, out)
    res["archiver"] = {"running_at_outage": still, "rc": rc, "tail": clean_tail(out)}
    rc2, out2 = ctx.manage("document_archiver", "--overwrite")
    res["archiver"]["rerun_rc"] = rc2
    if rc2 != 0:
        REC.finding("recovery", f"document_archiver: Wiederholung scheitert: {clean_tail(out2, 3)}")
    res["archiver"]["consistency"] = c7_consistency(ctx, "c2-archiver")
    # (c) Rebuild search index: DB gone after 1 s
    p = manage_popen("document_index", "reindex")
    still, rc, out = restart_db_during(ctx, p, 1.0)
    wait_http(ctx, 120)
    judge_cli_after_db_loss("document_index reindex", still, rc, out)
    res["reindex"] = {"running_at_outage": still, "rc": rc, "tail": clean_tail(out)}
    rc2, out2 = ctx.manage("document_index", "reindex")
    res["reindex"]["rerun_rc"] = rc2
    if rc2 != 0:
        REC.finding("recovery", f"document_index reindex: Wiederholung scheitert: {clean_tail(out2, 3)}")
    # (d) Server tasks: "Reprocess" for 15 documents, DB restart in the middle
    ids = sorted(S.all_ids(ctx.api)[0])[:15]
    before = {r[0]: r[1] for r in ctx.sql(f"SELECT id::text, coalesce(archive_checksum,'') FROM documents_document "
                                          f"WHERE id IN ({','.join(map(str, ids))})") or []}
    t0 = time.time()
    st, _ = ctx.api.post("c2:reprocess", "/api/documents/bulk_edit/", {"documents": ids, "method": "reprocess", "parameters": {}})
    time.sleep(2.0)
    ctx.pg("kill")
    time.sleep(10)
    ctx.pg("start")
    wait_http(ctx, 120)
    wait_idle(ctx, ctx.a.idle_timeout)
    rows = ctx.sql("SELECT task_name, state, coalesce(error->>'error_message', error::text, '') FROM paperrust_task_queue "
                   f"WHERE created_at >= to_timestamp({t0 - 1}) ORDER BY id") or []
    failed = [r for r in rows if r[1] == "failed"]
    res["reprocess"] = {"status": st, "tasks": collections.Counter(r[1] for r in rows), "failed": failed[:5]}
    if failed:
        REC.finding("task", f"Neu verarbeiten: {len(failed)} Aufgaben nach DB-Neustart fehlgeschlagen (nicht wiederholt)",
                    examples=[f"{r[0]}: {r[2][:160]}" for r in failed[:5]])
    res["consistency"] = c7_consistency(ctx, "c2-ende")
    return res


# ----------------------------------------------------------------------------- c3 disk full / write error
# Under Windows without admin rights no small drive (VHD/RAM disk) can be mounted. Replacement:
# revoke the write permission of the own user on the affected folder via ACL (icacls /deny, only
# own test folders under %TEMP%) – the software then sees a write error at exactly the place
# where "There is not enough space on the disk" would come with a full disk. The
# message for real ENOSPC is checked by unit tests in pl-consumer.


class deny_write:
    def __init__(self, path):
        self.path = Path(path)
        self.user = os.environ.get("USERNAME", "")

    def __enter__(self):
        self.path.mkdir(parents=True, exist_ok=True)
        r = subprocess.run(["icacls", str(self.path), "/deny", f"{self.user}:(OI)(CI)(WD,AD,WA,WEA)"], capture_output=True,
                           text=True, encoding="utf-8", errors="replace")
        if r.returncode != 0:
            raise RuntimeError(f"icacls deny fehlgeschlagen: {r.stdout} {r.stderr}")
        return self

    def __exit__(self, *a):
        subprocess.run(["icacls", str(self.path), "/remove:d", self.user, "/T", "/C"], capture_output=True)


def wait_terminal(ctx, tids, timeout, label):
    rows = wait_tasks(ctx, tids, timeout, label) if tids else {}
    return {t: (str(rows.get(t, {}).get("status")).lower(), str(rows.get(t, {}).get("result"))[:240]) for t in tids}


def c3_disk_full(ctx):
    with workflows_off(ctx):
        return _c3(ctx)


def _c3(ctx):
    res = {}
    media = ctx.media / "documents"
    for label, target in (("Originale", media / "originals"), ("Archiv", media / "archive"),
                          ("Vorschaubilder", media / "thumbnails")):
        t0 = time.time()
        docs = [FAC.next() for _ in range(2)]
        with deny_write(target):
            tids = [t for t in (upload(ctx, d, op=f"c3:{label}") for d in docs) if t]
            # wait for the first attempt (failure or "is retried"), record the message
            dl = time.time() + 60
            during = {}
            while time.time() < dl:
                rows = S.tasks_by_id(ctx.api, tids)
                during = {t: (str(rows.get(t, {}).get("status")).lower(), str(rows.get(t, {}).get("result"))[:240]) for t in tids}
                if all(v[0] not in ("pending", "started", "none") for v in during.values()):
                    break
                q = ctx.sql("SELECT count(*) FROM paperrust_task_queue WHERE retries > 0 AND state='queued' "
                            f"AND task_id IN ({','.join(repr(t) for t in tids)})", quiet=True)
                if q and int(q[0][0]) == len(tids):
                    break
                time.sleep(1)
            code, _ = ctx.api.get(f"c3:{label}:liste", "/api/documents/?page_size=5")
        # Write permission back ("space free"): the documents must arrive without manual work
        st = wait_terminal(ctx, tids, 300, f"c3 {label}")
        r = {"during": during, "tasks": st, "api_during": code}
        msgs = ctx.sql("SELECT coalesce(error->>'error_message','') FROM paperrust_task_queue "
                       f"WHERE task_id IN ({','.join(repr(t) for t in tids)})", quiet=True) or []
        r["queue_messages"] = [m[0][:200] for m in msgs]
        logmsg = " ".join(k for k, _ in ctx.log_tail_since(t0, pattern=r"Could not write|disk full", limit=5))
        r["log_message"] = logmsg[:300]
        failed_first = any(s_ != "success" for s_, _ in during.values())
        if failed_first and not re.search(r"disk full|write permission", logmsg, re.I):
            REC.finding("message", f"c3 {label}: Schreibfehler ohne verständliche Meldung im Log/Aufgabe: {during}")
        for t, (s_, msg) in st.items():
            if s_ != "success" and not re.search(r"disk full|write permission|Speicherplatz|kept", msg, re.I):
                REC.finding("message", f"c3 {label}: Fehlschlag ohne verständlichen Grund: {msg}")
        # Recovery: write permission back -> new uploads work, consistency is right
        d2 = FAC.next()
        t2 = upload(ctx, d2, op=f"c3:{label}:danach")
        st2 = wait_terminal(ctx, [t2] if t2 else [], 300, f"c3 {label} danach")
        r["after"] = st2
        if not t2 or list(st2.values())[0][0] != "success":
            REC.finding("recovery", f"c3 {label}: nach Freigabe des Ordners scheitert ein neuer Upload: {st2}")
        lost, dup = check_once(ctx, docs, f"c3 {label}")
        r["lost_after_failure"] = len(lost)
        r["consistency"] = c7_consistency(ctx, f"c3 {label}", sanity=False)
        r["log"] = ctx.log_tail_since(t0, limit=8)
        res[label] = r
    # Search index folder not writable: document arrives, is the index caught up later?
    t0 = time.time()
    idx = ctx.data / "index"
    doc = FAC.next()
    with deny_write(idx):
        tid = upload(ctx, doc, op="c3:index")
        st = wait_terminal(ctx, [tid] if tid else [], 300, "c3 Index")
        time.sleep(5)
    res["Suchindex"] = {"tasks": st}
    time.sleep(20)  # Wait for the catching up of the index (list/retry)
    ids = docs_with_token(ctx, doc["token"])
    if ids:
        code, j = ctx.api.get("c3:index:search", f"/api/documents/?query={doc['token']}&page_size=5&fields=id")
        found = code == 200 and ids[0] in [x["id"] for x in j.get("results", [])]
        res["Suchindex"]["found_after"] = found
        if not found:
            REC.finding("index", "c3 Suchindex: Dokument nach Schreibfehler im Index auch 20 s nach Freigabe nicht auffindbar")
    res["Suchindex"]["log"] = ctx.log_tail_since(t0, limit=8)
    # Export to a non-writable target
    exp = ctx.work.parent / "c3-export"
    shutil.rmtree(exp, ignore_errors=True)
    with deny_write(exp):
        rc, out = ctx.manage("document_exporter", str(exp))
    res["Export"] = {"rc": rc, "tail": clean_tail(out, 3)}
    if rc == 0:
        REC.finding("export", "c3 Export: in nicht beschreibbares Ziel meldet Erfolg")
    elif re.search(r"panicked", out):
        REC.finding("crash", f"c3 Export: Absturz statt Meldung: {clean_tail(out, 3)}")
    elif not re.search(r"denied|Zugriff|permission|space|Speicherplatz", out, re.I):
        REC.finding("message", f"c3 Export: Meldung unverständlich: {clean_tail(out, 3)}")
    shutil.rmtree(exp, ignore_errors=True)
    res["consistency"] = c7_consistency(ctx, "c3-ende")
    return res


# ----------------------------------------------------------------------------- c4 network gone

import http.server  # noqa: E402

sys.path.insert(0, str(HERE))
import imap_stub  # noqa: E402


def task_doc_id(row):
    return S.task_doc(row)


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


class MockHttp:
    """Small HTTP receiver on a fixed port that only listens on demand (before: connection refused)."""

    def __init__(self, port, handler):
        self.port = port
        self.handler = handler
        self.hits = []
        self.srv = None

    def up(self):
        outer = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def _do(self):
                n = int(self.headers.get("content-length") or 0)
                body = self.rfile.read(n) if n else b""
                outer.hits.append((self.command, self.path, body[:2000]))
                code, headers, data = outer.handler(self.command, self.path, body)
                self.send_response(code)
                for k, v in headers.items():
                    self.send_header(k, v)
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            do_GET = do_POST = do_PUT = _do

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", self.port), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def down(self):
        if self.srv:
            self.srv.shutdown()
            self.srv.server_close()
            self.srv = None


def c4_network(ctx):
    imap = imap_stub.ImapStub()
    hook_port, ai_port, ocr_port = free_port(), free_port(), free_port()
    extra = ("PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS=true PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS=true "
             f"PAPERLESS_AI_ENABLED=true PAPERLESS_AI_LLM_BACKEND=openai-like PAPERLESS_AI_LLM_ENDPOINT=http://127.0.0.1:{ai_port}/v1 "
             "PAPERLESS_AI_LLM_MODEL=chaos-model PAPERLESS_AI_LLM_API_KEY=chaos-local PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS=true "
             "PAPERLESS_AI_LLM_REQUEST_TIMEOUT=10")
    ctx.server("stop")
    ctx.server("start", env={"CH_EXTRA_ENV": extra})
    res = {}
    try:
        with workflows_off(ctx):
            res["mail"] = c4_mail(ctx, imap)
        res["webhook"] = c4_webhook(ctx, hook_port)
        res["ai"] = c4_ai(ctx, ai_port)
    finally:
        imap.close()
    res["remote_ocr"] = c4_remote_ocr(ctx, ocr_port, extra)
    ctx.server("stop")
    ctx.server("start")
    res["consistency"] = c7_consistency(ctx, "c4-ende")
    return res


def mail_fetch(ctx):
    t = time.time()
    rc, out = ctx.manage("mail_fetcher", timeout=600)
    return rc, round(time.time() - t, 1), [ln for ln in out.splitlines() if "chaos" in ln.lower() or "ERROR" in ln][-6:]


def c4_mail(ctx, imap):
    api = ctx.api
    st, acc = api.post("c4:mail_account", "/api/mail_accounts/", {
        "name": f"chaos-{RUN}", "imap_server": "127.0.0.1", "imap_port": imap.port, "imap_security": 1,
        "username": imap.user, "password": imap.password, "character_set": "UTF-8", "account_type": 1})
    if not isinstance(acc, dict) or not acc.get("id"):
        REC.finding("tool", f"c4: Mailkonto nicht angelegt: {st} {str(acc)[:200]}")
        return {}
    st, rule = api.post("c4:mail_rule", "/api/mail_rules/", {
        "name": f"chaos-rule-{RUN}", "account": acc["id"], "folder": "INBOX", "maximum_age": 30, "action": 3,
        "attachment_type": 1, "consumption_scope": 1, "order": 0, "enabled": True,
        "assign_title_from": 1, "assign_correspondent_from": 1})
    if not isinstance(rule, dict) or not rule.get("id"):
        REC.finding("tool", f"c4: Mailregel nicht angelegt: {st} {str(rule)[:200]}")
        return {}
    res = {}

    def new_mails(n, tag):
        docs = []
        for _ in range(n):
            d = FAC.next()
            imap.append(imap_stub.mail_with_pdf(f"Chaos {tag} {d['title']}", d["data"], d["name"]))
            docs.append(d)
        return docs

    def settle(label, docs):
        wait_idle(ctx, 300)
        time.sleep(2)
        wait_idle(ctx, 300)
        lost, dup = check_once(ctx, docs, label)
        return {"lost": len(lost), "dup": len(dup)}

    # m1: server not reachable (port closed) -> understandable error, nothing ingested; afterwards normal
    docs = new_mails(2, "m1")
    imap.offline = True
    rc, secs, tail = mail_fetch(ctx)
    r = {"offline": {"rc": rc, "secs": secs, "log": tail}}
    if secs > 120:
        REC.finding("hang", f"c4 Mail offline: Abruf dauerte {secs}s")
    imap.offline = False
    rc, secs, tail = mail_fetch(ctx)
    r["online"] = {"rc": rc, "secs": secs}
    r.update(settle("c4 Mail m1", docs))
    r["flags"] = imap.flags()
    res["m1_offline_then_online"] = r
    # m2: connection drops while fetching (FETCH) -> next run catches up, nothing twice
    docs = new_mails(2, "m2")
    imap.drop_on, imap.drop_count = "FETCH", 1
    rc, secs, tail = mail_fetch(ctx)
    r = {"drop": {"rc": rc, "secs": secs, "log": tail}}
    imap.drop_on = None
    rc, secs, tail = mail_fetch(ctx)
    r["again"] = {"rc": rc, "secs": secs}
    r.update(settle("c4 Mail m2", docs))
    res["m2_drop_on_fetch"] = r
    # m3: connection drops at the mail action (mark as read) -> no duplicate, action later
    docs = new_mails(2, "m3")
    imap.drop_on, imap.drop_count = "STORE", 3
    rc, secs, tail = mail_fetch(ctx)
    wait_idle(ctx, 300)
    r = {"drop": {"rc": rc, "secs": secs, "log": tail}}
    imap.drop_on = None
    for _ in range(2):
        mail_fetch(ctx)
        wait_idle(ctx, 300)
    r.update(settle("c4 Mail m3", docs))
    unseen = [u for u, f in imap.flags().items() if "\\Seen" not in f]
    r["unseen_after"] = unseen
    refetched = imap.log.count("FETCH")
    r["note"] = (f"{len(unseen)} Mails bleiben ungelesen: die Mail-Aktion wird nach dem Abbruch nicht nachgeholt "
                 "(wie upstream: verarbeitete Mails werden übersprungen); kein Duplikat") if unseen else ""
    res["m3_drop_on_store"] = r
    # Clean up
    api.call("c4:rule_del", "DELETE", f"/api/mail_rules/{rule['id']}/", ok=(204,))
    api.call("c4:acc_del", "DELETE", f"/api/mail_accounts/{acc['id']}/", ok=(204,))
    res["imap_log_tail"] = imap.log[-12:]
    return res


def c4_webhook(ctx, port):
    """Workflow "Document changed → webhook" (test data workflow 2) to a receiver that does not listen
    for 3 s. Expectation: delivery after the retry."""
    hook = MockHttp(port, lambda m, p, b: (200, {}, b"ok"))
    wf = ctx.one("SELECT w.id FROM documents_workflow w JOIN documents_workflow_actions wa ON wa.workflow_id=w.id "
                 "JOIN documents_workflowaction a ON a.id=wa.workflowaction_id WHERE a.webhook_id IS NOT NULL LIMIT 1")
    wh = ctx.one(f"SELECT a.webhook_id FROM documents_workflow_actions wa JOIN documents_workflowaction a "
                 f"ON a.id=wa.workflowaction_id WHERE wa.workflow_id={wf} AND a.webhook_id IS NOT NULL LIMIT 1") if wf else None
    if not wh:
        REC.finding("tool", "c4 Webhook: kein Testdaten-Workflow mit Webhook gefunden")
        return {}
    old_url = ctx.one(f"SELECT url FROM documents_workflowactionwebhook WHERE id={wh}")
    ctx.sql(f"UPDATE documents_workflowactionwebhook SET url='http://127.0.0.1:{port}/hook' WHERE id={wh}")
    others = [r[0] for r in ctx.sql(f"SELECT id FROM documents_workflow WHERE enabled AND id<>{wf}") or []]
    if others:
        ctx.sql(f"UPDATE documents_workflow SET enabled=false WHERE id IN ({','.join(others)})")
    ctx.sql(f"UPDATE documents_workflow SET enabled=true WHERE id={wf}")
    trig = ctx.sql(f"SELECT t.type FROM documents_workflow_triggers wt JOIN documents_workflowtrigger t ON t.id=wt.workflowtrigger_id "
                   f"WHERE wt.workflow_id={wf}")
    res = {"workflow": wf, "trigger_types": trig}
    t0 = time.time()
    try:
        # a document that the trigger filters match (test data: document type 1, without tag 7)
        did = ctx.one("SELECT d.id FROM documents_document d WHERE d.deleted_at IS NULL AND d.document_type_id = 1 "
                      "AND NOT EXISTS (SELECT 1 FROM documents_document_tags t WHERE t.document_id = d.id AND t.tag_id = 7) "
                      "ORDER BY d.id LIMIT 1")
        ctx.api.call("c4:patch", "PATCH", f"/api/documents/{did}/", {"title": f"chaos webhook {RUN}"})
        time.sleep(3)
        hook.up()
        dl = time.time() + 90
        while time.time() < dl and not hook.hits:
            time.sleep(1)
        res["delivered_after_s"] = round(time.time() - t0, 1) if hook.hits else None
        rows = ctx.sql("SELECT state, retries, coalesce(error->>'error_type','') FROM paperrust_task_queue "
                       f"WHERE task_name LIKE '%send_webhook' AND created_at > to_timestamp({t0 - 1})")
        res["tasks"] = rows
        if not hook.hits:
            REC.finding("lost", "c4 Webhook: nach 3 s Ausfall des Empfängers nie zugestellt (kein Neuversuch)")
    finally:
        hook.down()
        ctx.sql(f"UPDATE documents_workflowactionwebhook SET url='{old_url}' WHERE id={wh}")
        if others:
            ctx.sql(f"UPDATE documents_workflow SET enabled=true WHERE id IN ({','.join(others)})")
    wait_idle(ctx, 120)
    return res


def c4_ai(ctx, port):
    """AI service not reachable: suggestions answer quickly with a clear message, classic suggestions work."""
    did = sorted(S.all_ids(ctx.api)[0])[0]
    t = time.time()
    st, d = ctx.api.get("c4:ai_suggestions", f"/api/documents/{did}/ai_suggestions/", ok=(200, 503))
    secs = round(time.time() - t, 1)
    res = {"ai_suggestions": {"status": st, "secs": secs, "body": str(d)[:200]}}
    if st >= 500 and st != 503:
        REC.finding("message", f"c4 KI: ai_suggestions bei nicht erreichbarem Dienst HTTP {st}: {str(d)[:150]}")
    elif st == 503 and "reach" not in str(d) and "timed out" not in str(d):
        REC.finding("message", f"c4 KI: 503 ohne klare Meldung: {str(d)[:150]}")
    if secs > 15:
        REC.finding("message", f"c4 KI: Antwort erst nach {secs}s")
    st, d = ctx.api.get("c4:suggestions", f"/api/documents/{did}/suggestions/")
    res["classic_suggestions"] = st
    if st != 200:
        REC.finding("message", f"c4 KI: klassische Vorschläge gehen nicht mehr (HTTP {st})")
    return res


def c4_remote_ocr(ctx, port, extra):
    """Remote OCR (Azure interface as mock) gone for 20 s during ingestion, then back: the document is
    ingested after the retry, not discarded."""
    pdf = S.pdf_bytes([["Remote OCR Ergebnis chaos"]])

    def azure(m, p, b):
        if m == "POST" and ":analyze" in p:
            return 202, {"Operation-Location": f"http://127.0.0.1:{port}/documentintelligence/documentModels/prebuilt-read/analyzeResults/r1?api-version=x"}, b""
        if m == "GET" and p.split("?")[0].endswith("/pdf"):
            return 200, {"content-type": "application/pdf"}, pdf
        if m == "GET" and "analyzeResults/r1" in p:
            return 200, {"content-type": "application/json"}, json.dumps(
                {"status": "succeeded", "analyzeResult": {"content": "Remote OCR Ergebnis chaos"}}).encode()
        return 404, {}, b"{}"
    mock = MockHttp(port, azure)
    env = extra + (f" PAPERLESS_REMOTE_OCR_ENGINE=azureai PAPERLESS_REMOTE_OCR_API_KEY=chaos-local "
                   f"PAPERLESS_REMOTE_OCR_ENDPOINT=http://127.0.0.1:{port} PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS=true "
                   "PAPERLESS_REMOTE_OCR_MODE=always")
    ctx.server("stop")
    ctx.server("start", env={"CH_EXTRA_ENV": env})
    res = {}
    try:
        with workflows_off(ctx):
            # image (needs text recognition); a text PDF would be read locally (without remote call)
            doc = FAC.next("png")
            t0 = time.time()
            tid = upload(ctx, doc, op="c4:remote_ocr")
            time.sleep(20)
            mock.up()
            rows = wait_tasks(ctx, [tid], 300, "c4 Remote-OCR") if tid else {}
            st = str(rows.get(tid, {}).get("status")).lower()
            did = task_doc_id(rows.get(tid, {}))
            res = {"task": st, "result": str(rows.get(tid, {}).get("result"))[:200], "mock_hits": len(mock.hits),
                   "secs": round(time.time() - t0, 1), "document": did,
                   "content_from_remote": bool(did and "Remote OCR Ergebnis" in (ctx.one(f"SELECT content FROM documents_document WHERE id={did}") or ""))}
            if not mock.hits:
                REC.finding("tool", "c4 Remote-OCR: Mock wurde nie aufgerufen (Remote-OCR nicht aktiv?)")
            if st != "success" or not did:
                REC.finding("lost", f"c4 Remote-OCR: nach 20 s Ausfall nicht eingelesen ({st}): {res['result']}")
    finally:
        mock.down()
    return res


# ----------------------------------------------------------------------------- Procedure

SCEN = {"c1": c1_db_outage, "c4": c4_network, "c3": c3_disk_full, "c2": c2_long_tasks, "c6": c6_backup, "c5": c5_kill_phases, "c7": lambda ctx: c7_consistency(ctx)}


def render_md(res):
    lines = [f"# Ausfall-Suite {res['run']}", "", f"Server {res['url']}, Binary {res.get('binary')}", ""]
    for name, r in res["scenarios"].items():
        lines += [f"## {name}", "", "```json", json.dumps(r.get("result"), ensure_ascii=False, indent=1, default=str)[:20000],
                  "```", ""]
        if r.get("findings"):
            lines += ["**Befunde:**", ""] + [f"- [{f['kind']}] {f['msg']}" for f in r["findings"]] + [""]
    return "\n".join(lines)


def main():
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("scenarios", nargs="*", default=["c1", "c7"])
    p.add_argument("--docs", type=int, default=12)
    p.add_argument("--long-outage", type=int, default=120)
    p.add_argument("--timeout", type=float, default=60.0)
    p.add_argument("--ingest-timeout", type=int, default=900)
    p.add_argument("--idle-timeout", type=int, default=900)
    p.add_argument("--export-kinds", default="dir,dir-pass,dir-split,stored,deflated,bzip2,lzma,zstd")
    p.add_argument("--kill-reps", type=int, default=1)
    p.add_argument("--out")
    a = p.parse_args()
    out = Path(a.out or Path(os.environ.get("TEMP", "/tmp")) / "paperrust-chaos-results" / time.strftime("%Y%m%d-%H%M%S"))
    out.mkdir(parents=True, exist_ok=True)
    ctx = Ctx(a)
    res = {"run": RUN, "url": ctx.url, "binary": str(REPO / "target/debug/paperrust.exe"), "scenarios": {}}
    for name in a.scenarios:
        fn = SCEN[name]
        REC.scenario = name
        t = time.time()
        try:
            r = fn(ctx)
        except Exception as e:
            REC.finding("crash", f"Szenario {name} abgebrochen: {type(e).__name__}: {e}", tb=traceback.format_exc()[-1500:])
            r = None
        res["scenarios"][name] = {"secs": round(time.time() - t), "result": r,
                                  "findings": [f for f in REC.findings if f["scen"] == name],
                                  "summary": REC.summary(name)}
        (out / "summary.json").write_text(json.dumps(res, ensure_ascii=False, indent=1, default=str), encoding="utf-8")
        (out / "summary.md").write_text(render_md(res), encoding="utf-8")
    log(f"Ergebnis: {out}")
    nf = len(REC.findings)
    log(f"{nf} Befunde")
    for f in REC.findings:
        log(f"  [{f['scen']}/{f['kind']}] {f['msg']}")
    sys.exit(1 if nf else 0)


if __name__ == "__main__":
    main()
