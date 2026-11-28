#!/usr/bin/env python3
"""PaperRust Last- und Gleichzeitigkeits-Suite (nur Python-Standardbibliothek).

Szenarien (siehe tests/stress/README.md):
  s1  Massenänderungen während der Worker einliest (bulk_edit add/remove/modify_tags, set_correspondent,
      set_permissions, delete/restore; seriell schnell hintereinander und 2-8 parallel)
  s2  viele Nutzer (gemischter Ablauf)
  s3  gleichzeitiges Bearbeiten desselben Dokuments (+ Workflow "Geändert", Version hochladen)
  s4  parallele Uploads, Duplikate, große und kaputte Datei
  s5  Neustart/Abbruch während der Verarbeitung (nur mit --kill-cmd/--start-cmd bzw. lokal)
  s6  Dauerlauf mit Speicher-/Handle-/DB-Verbindungs-Beobachtung
  s7  Datenkonsistenz (läuft nach jedem Szenario automatisch, einzeln aufrufbar)
  s8  abbrechende Clients: PATCH/bulk_edit mitten in der Bearbeitung hart abgebrochen (BEFUNDE B10);
      Gespeichertes muss trotzdem nachgearbeitet sein (Workflow „Geändert“, Suchindex)

Bewertung: jeder 5xx, jede Zeitüberschreitung/Verbindungsfehler und jede Inkonsistenz ist ein Fehler;
4xx nur, wenn der Aufruf ihn fachlich nicht erwarten lässt. Ergebnis: <out>/summary.json und
<out>/summary.md, Server-Log-Auszüge je Szenario.

Aufruf (lokal, Server aus tests/stress/server.sh):
  python tests/stress/stress.py --local s1 s4 s3 s5 s2
Entfernt (nur API-Prüfungen, kein Neustart, keine Datei-/DB-Prüfung):
  python tests/stress/stress.py --url https://paperrust.example.test --user admin --password ... s1 s2
Nur Testkonten verwenden: die Suite ändert, löscht und legt Dokumente an.
"""
from __future__ import annotations

import argparse
import base64
import collections
import http.client
import json
import os
import random
import re
import shlex
import ssl
import struct
import subprocess
import sys
import threading
import time
import traceback
import urllib.parse
import uuid
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent

# ----------------------------------------------------------------------------- Recording


class Recorder:
    """Every request: scenario, operation, status (-1 = exception/timeout), duration, error text."""

    def __init__(self):
        self.lock = threading.Lock()
        self.rows: list[dict] = []
        self.findings: list[dict] = []  # Inconsistencies and other errors (not HTTP)
        self.scenario = "-"

    def add(self, op, status, dt, ok, detail=""):
        with self.lock:
            self.rows.append({"t": time.time(), "scen": self.scenario, "op": op, "status": status,
                              "ms": round(dt * 1000, 1), "ok": ok, "detail": detail[:400]})

    def finding(self, kind, msg, **extra):
        with self.lock:
            self.findings.append({"scen": self.scenario, "kind": kind, "msg": msg, **extra})
        log(f"  !! {kind}: {msg}")

    def summary(self, scen):
        rows = [r for r in self.rows if r["scen"] == scen]
        by_op = collections.defaultdict(list)
        for r in rows:
            by_op[r["op"]].append(r)
        out = {"requests": len(rows), "errors": sum(1 for r in rows if not r["ok"]), "ops": {}}
        for op, rs in sorted(by_op.items()):
            lat = sorted(r["ms"] for r in rs)
            codes = collections.Counter(str(r["status"]) for r in rs)
            errs = [r for r in rs if not r["ok"]]
            out["ops"][op] = {
                "n": len(rs), "errors": len(errs), "codes": dict(sorted(codes.items())),
                "p50_ms": lat[len(lat) // 2], "p95_ms": lat[min(len(lat) - 1, int(len(lat) * 0.95))],
                "max_ms": lat[-1],
                "error_examples": [{"status": e["status"], "ms": e["ms"], "detail": e["detail"]} for e in errs[:5]],
            }
        out["findings"] = [f for f in self.findings if f["scen"] == scen]
        return out


REC = Recorder()
_log_lock = threading.Lock()


def log(*a):
    with _log_lock:
        print(time.strftime("%H:%M:%S"), *a, flush=True)


# ----------------------------------------------------------------------------- HTTP-Client


class Api:
    """One client per thread (own keep-alive connection)."""

    def __init__(self, base, user, password, token=None, timeout=60.0):
        u = urllib.parse.urlsplit(base)
        self.https = u.scheme == "https"
        self.host = u.hostname
        self.port = u.port or (443 if self.https else 80)
        self.prefix = u.path.rstrip("/")
        self.timeout = timeout
        if token:
            self.auth = "Token " + token
        else:
            self.auth = "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
        self.conn = None
        self.base, self.user, self.password, self.token = base, user, password, token

    def clone(self, timeout=None):
        return Api(self.base, self.user, self.password, self.token, timeout or self.timeout)

    def _connect(self):
        if self.https:
            return http.client.HTTPSConnection(self.host, self.port, timeout=self.timeout,
                                               context=ssl.create_default_context())
        return http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)

    def raw(self, method, path, body=None, headers=None):
        h = {"Authorization": self.auth, "Accept": "application/json; version=9"}
        if headers:
            h.update(headers)
        for attempt in (0, 1):
            if self.conn is None:
                self.conn = self._connect()
            try:
                self.conn.request(method, self.prefix + path, body=body, headers=h)
                r = self.conn.getresponse()
                data = r.read()
                if r.getheader("Connection", "").lower() == "close":
                    self.conn.close()
                    self.conn = None
                return r.status, data
            except (http.client.RemoteDisconnected, BrokenPipeError, ConnectionResetError, ConnectionAbortedError) as e:
                # Keep-alive connection closed by the server: reconnect once (no error)
                self.close()
                if attempt == 1:
                    raise e
            except Exception:
                self.close()
                raise
        raise RuntimeError("unreachable")

    def close(self):
        if self.conn:
            try:
                self.conn.close()
            except Exception:
                pass
        self.conn = None

    def call(self, op, method, path, payload=None, ok=(200, 201, 204), files=None, fields=None, raw_body=None,
             content_type=None):
        """Request + recording. Returns (status, json|bytes|None)."""
        headers = {}
        body = None
        if files is not None or fields is not None:
            body, ct = multipart(fields or {}, files or [])
            headers["Content-Type"] = ct
        elif payload is not None:
            body = json.dumps(payload).encode()
            headers["Content-Type"] = "application/json"
        elif raw_body is not None:
            body = raw_body
            headers["Content-Type"] = content_type or "application/octet-stream"
        t = time.perf_counter()
        try:
            st, data = self.raw(method, path, body, headers)
        except Exception as e:  # Timeout, connection abort
            dt = time.perf_counter() - t
            REC.add(op, -1, dt, False, f"{type(e).__name__}: {e} [{method} {path}]")
            return -1, None
        dt = time.perf_counter() - t
        good = st in ok
        detail = ""
        if not good:
            detail = f"{method} {path} -> {data[:300].decode('utf-8', 'replace')}"
        REC.add(op, st, dt, good, detail)
        ctype_json = data[:1] in (b"{", b"[", b'"') or data.strip() in (b"true", b"false", b"null")
        if ctype_json:
            try:
                return st, json.loads(data)
            except ValueError:
                pass
        return st, data

    # convenient helpers (not recorded as an error if ok matches)
    def get(self, op, path, ok=(200,)):
        return self.call(op, "GET", path, ok=ok)

    def post(self, op, path, payload, ok=(200, 201)):
        return self.call(op, "POST", path, payload, ok=ok)


def multipart(fields, files):
    b = "----stress" + uuid.uuid4().hex
    out = bytearray()
    for k, v in fields.items():
        out += f"--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n".encode()
    for name, filename, data, ctype in files:
        out += (f"--{b}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                f"Content-Type: {ctype}\r\n\r\n").encode()
        out += data + b"\r\n"
    out += f"--{b}--\r\n".encode()
    return bytes(out), f"multipart/form-data; boundary={b}"


# ----------------------------------------------------------------------------- Test files

WORDS = ("rechnung vertrag versicherung steuer kündigung mahnung angebot lieferschein quittung bank konto "
         "miete strom heizöl wasser zahnarzt krankenkasse rente gehalt arbeitgeber finanzamt bescheid "
         "invoice contract insurance receipt statement payment order delivery tax salary pension").split()


def pdf_bytes(lines_per_page, pad=0):
    """Minimal text PDF (Helvetica), one page per entry in lines_per_page. pad = filler bytes (large file)."""
    objs = {1: "<< /Type /Catalog /Pages 2 0 R >>", 3: "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
    kids = []
    n = 4
    for lines in lines_per_page:
        esc = [ln.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)") for ln in lines]
        s = "BT /F1 14 Tf 60 760 Td 18 TL " + " ".join(f"({t}) '" for t in esc) + " ET"
        s = s.encode("latin-1", "replace").decode("latin-1")
        objs[n] = f"<< /Length {len(s.encode('latin-1'))} >>\nstream\n{s}\nendstream"
        objs[n + 1] = (f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents {n} 0 R "
                       f"/Resources << /Font << /F1 3 0 R >> >> >>")
        kids.append(f"{n + 1} 0 R")
        n += 2
    objs[2] = f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {len(kids)} >>"
    if pad:
        # unreferenced, compression-resistant data stream (large file)
        blob = os.urandom(pad)
        objs[n] = None
        pad_obj = (n, blob)
    else:
        pad_obj = None
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offs = {}
    for i in sorted(objs):
        offs[i] = len(out)
        if objs[i] is None and pad_obj:
            out += f"{i} 0 obj\n<< /Length {len(pad_obj[1])} >>\nstream\n".encode() + pad_obj[1] + b"\nendstream\nendobj\n"
        else:
            out += f"{i} 0 obj\n{objs[i]}\nendobj\n".encode("latin-1")
    size = max(objs) + 1
    x = len(out)
    out += f"xref\n0 {size}\n0000000000 65535 f \n".encode()
    for i in range(1, size):
        out += f"{offs.get(i, 0):010d} 00000 {'n' if i in offs else 'f'} \n".encode()
    out += f"trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    return bytes(out)


def png_bytes(w, h, seed):
    rng = random.Random(seed)
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        for x in range(w):
            v = 255 if ((x // 20 + y // 20 + seed) % 7) else rng.randrange(0, 80)
            rows.append(v)

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(rows))) + chunk(b"IEND", b"")


class DocFactory:
    """Unique test documents; every text PDF carries a unique search word (token)."""

    def __init__(self, run_id, seed=4711):
        self.run_id = run_id
        self.rng = random.Random(seed)
        self.n = 0
        self.lock = threading.Lock()

    def next(self, kind="pdf"):
        with self.lock:
            self.n += 1
            n = self.n
            token = f"stk{self.run_id}x{n:05d}"
            words = [self.rng.choice(WORDS) for _ in range(40)]
            pages = self.rng.choice((1, 1, 1, 2, 3))
        name = f"stress_{self.run_id}_{n:05d}"
        if kind == "png":
            return {"name": name + ".png", "token": None, "data": png_bytes(320, 240, n), "ctype": "image/png",
                    "title": name}
        lines = [[f"Stress Dokument {n} {token}", f"Lauf {self.run_id} Seite {p + 1}"]
                 + [" ".join(words[i:i + 8]) for i in range(0, 40, 8)] for p in range(pages)]
        return {"name": name + ".pdf", "token": token, "data": pdf_bytes(lines), "ctype": "application/pdf",
                "title": name}


# ----------------------------------------------------------------------------- Server environment


class Env:
    def __init__(self, args):
        self.args = args
        self.url = args.url
        self.local = args.local
        self.pid = None
        self.log_dir = Path(args.log_dir) if args.log_dir else None
        self.media = Path(args.media) if args.media else None
        self.psql = args.psql
        self.dsn = args.dsn
        self.kill_cmd = args.kill_cmd
        self.start_cmd = args.start_cmd
        if self.local:
            vals = local_env()
            self.url = self.url or vals.get("ST_URL")
            self.log_dir = self.log_dir or Path(vals["ST_LOG"])
            self.media = self.media or Path(vals["ST_MEDIA"])
            self.psql = self.psql or vals.get("ST_PSQL")
            self.dsn = self.dsn or vals.get("ST_DSN")
            srv = f"bash {shlex.quote(str(HERE / 'server.sh'))}"
            self.kill_cmd = self.kill_cmd or f"{srv} kill"
            self.start_cmd = self.start_cmd or f"{srv} start"
        self.refresh_pid()
        self.t_start = int(time.time()) - 5
        self.has_db = bool(self.psql and self.dsn) and self.sql("SELECT 1") is not None

    def refresh_pid(self):
        if self.args.pid:
            self.pid = int(self.args.pid)
        elif self.local:
            p = local_env().get("ST_PID")
            self.pid = int(p) if p and p.isdigit() else None

    # Database (read only)
    def sql(self, q):
        if not (self.psql and self.dsn):
            return None
        r = subprocess.run([self.psql, "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-d", self.dsn, "-c", q],
                           capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
        if r.returncode != 0:
            REC.finding("tool", f"psql fehlgeschlagen: {r.stderr.strip()[:200]}")
            return None
        return [ln.split("\t") for ln in r.stdout.splitlines() if ln]

    def log_files(self):
        if not self.log_dir:
            return []
        return [p for p in (self.log_dir / "paperrust.log", self.log_dir / "rust.err.log", self.log_dir / "rust.out.log")
                if p.exists()]

    def proc_stats(self):
        """RSS, private bytes, handles, threads of the server process (Windows: PowerShell)."""
        if not self.pid:
            return None
        if os.name == "nt":
            ps = (f"$p=Get-Process -Id {self.pid} -ErrorAction SilentlyContinue; if($p){{"
                  f"'{{0}} {{1}} {{2}} {{3}}' -f $p.WorkingSet64,$p.PrivateMemorySize64,$p.HandleCount,$p.Threads.Count}}")
            r = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", ps],
                               capture_output=True, text=True, timeout=60)
            parts = r.stdout.split()
            if len(parts) == 4:
                ws, priv, h, th = map(int, parts)
                return {"rss_mb": round(ws / 2**20, 1), "private_mb": round(priv / 2**20, 1), "handles": h, "threads": th}
            return None
        try:
            st = Path(f"/proc/{self.pid}/status").read_text()
            rss = int(re.search(r"VmRSS:\s+(\d+)", st).group(1)) / 1024
            fds = len(os.listdir(f"/proc/{self.pid}/fd"))
            th = int(re.search(r"Threads:\s+(\d+)", st).group(1))
            return {"rss_mb": round(rss, 1), "private_mb": None, "handles": fds, "threads": th}
        except OSError:
            return None

    def db_conns(self):
        rows = self.sql("SELECT state, count(*) FROM pg_stat_activity WHERE datname = current_database() "
                        "AND pid <> pg_backend_pid() GROUP BY 1")
        if rows is None:
            return None
        return {(r[0] or "none"): int(r[1]) for r in rows}


class LockWatch(threading.Thread):
    """Every 2 s: sessions that wait > 5 s for a lock, including the blocking session (pg_blocking_pids).
    If a Rust process keeps a transaction open and waits itself (e.g. for a second connection),
    PostgreSQL detects no deadlock – here you can see it."""
    Q = ("SELECT w.pid, round(extract(epoch FROM now() - w.query_start))::int, left(regexp_replace(w.query, '[[:space:]]+', ' ', 'g'), 220), "
         "b.pid, b.state, round(extract(epoch FROM now() - b.state_change))::int, "
         "left(regexp_replace(b.query, '[[:space:]]+', ' ', 'g'), 220), "
         "(SELECT string_agg(DISTINCT l.relation::regclass::text, ',') FROM pg_locks l JOIN pg_class c ON c.oid = l.relation "
         " WHERE l.pid = b.pid AND l.mode IN ('RowExclusiveLock','ShareRowExclusiveLock','ExclusiveLock') AND c.relkind = 'r') "
         "FROM pg_stat_activity w CROSS JOIN LATERAL unnest(pg_blocking_pids(w.pid)) bp(pid) "
         "JOIN pg_stat_activity b ON b.pid = bp.pid "
         "WHERE w.datname = current_database() AND w.wait_event_type = 'Lock' AND now() - w.query_start > interval '5 seconds'")

    def __init__(self, env):
        super().__init__(daemon=True)
        self.env = env
        self.seen = {}
        self.lock = threading.Lock()
        self.start()

    def run(self):
        while True:
            rows = self.env.sql(self.Q) or []
            with self.lock:
                for r in rows:
                    key = (r[0], r[2], r[3])
                    if key not in self.seen:
                        self.seen[key] = {"waiter_pid": r[0], "waiter_query": r[2], "blocker_pid": r[3],
                                          "blocker_state": r[4], "blocker_query": r[6], "first_wait_s": int(r[1]),
                                          "blocker_written_tables": r[7] if len(r) > 7 else ""}
                        log(f"  !! Sperre: pid {r[0]} wartet {r[1]}s auf pid {r[3]} ({r[4]}): {r[6][:120]} | wartend: {r[2][:120]}")
                    self.seen[key]["max_wait_s"] = int(r[1])
            time.sleep(2)

    def take(self):
        with self.lock:
            out = list(self.seen.values())
            self.seen = {}
        for o in out:
            if o.get("max_wait_s", 0) >= 10:
                REC.finding("lock", f"DB-Sperre {o['max_wait_s']}s: wartend '{o['waiter_query'][:120]}', "
                                    f"blockiert von pid {o['blocker_pid']} ({o['blocker_state']}) '{o['blocker_query'][:120]}'")
        return out


def local_env():
    r = subprocess.run(["bash", str(HERE / "server.sh"), "env"], capture_output=True, text=True, timeout=60)
    vals = {}
    for ln in r.stdout.splitlines():
        if "=" in ln:
            k, v = ln.split("=", 1)
            vals[k.strip()] = v.strip()
    return vals


class LogWatch:
    """Remembers the size of the server logs and afterwards returns the new WARN/ERROR lines."""
    NOISE = re.compile(r"--deskew is not implemented|--clean/--clean-final|beat entry skipped|Thumbnail font not available|"
                       r"Office to PDF conversion not available|Error while getting DPI|rust route active|"
                       r"listening on a non-loopback|worker task handlers")
    INTEREST = re.compile(r"\b(ERROR|WARN|WARNING|CRITICAL)\b|panick|deadlock|timed out|PoolTimedOut|"
                          r"status=5\d\d|outcome=\"(?!ok)")

    def __init__(self, env: Env):
        self.env = env
        self.start = {p: p.stat().st_size for p in env.log_files()}

    def excerpt(self, limit=150):
        lines = []
        for p in self.env.log_files():
            off = self.start.get(p, 0)
            try:
                with open(p, "rb") as f:
                    if f.seek(0, 2) < off:
                        off = 0  # Log started anew (restart)
                    f.seek(off)
                    data = f.read().decode("utf-8", "replace")
            except OSError:
                continue
            for ln in data.splitlines():
                if self.INTEREST.search(ln) and not self.NOISE.search(ln):
                    lines.append(f"[{p.name}] {ln[:600]}")
        groups = collections.Counter(re.sub(r"\d+", "#", re.sub(r"^\S+ \S+ |^\[[^\]]*\] ", "", ln.split("] ", 1)[-1]))[:160]
                                     for ln in lines)
        return {"count": len(lines), "groups": groups.most_common(25), "lines": lines[:limit]}


# ----------------------------------------------------------------------------- Helpers


def all_ids(api, op="ids"):
    st, d = api.get(op, "/api/documents/?page_size=1&fields=id")
    if st == 200 and isinstance(d, dict) and isinstance(d.get("all"), list):
        return d["all"], d.get("count")
    return [], None


UPLOADS: dict = {}  # task_id -> unique search word (None for images/special files)


def upload(api, doc, op="upload", ok=(200,), extra=None):
    fields = {"title": doc["title"]}
    if extra:
        fields.update(extra)
    st, d = api.call(op, "POST", "/api/documents/post_document/",
                     files=[("document", doc["name"], doc["data"], doc["ctype"])], fields=fields, ok=ok)
    if st == 200 and isinstance(d, str):
        UPLOADS[d] = doc.get("token")
        return d
    return None


def tasks_by_id(api, ids=None):
    """All PaperlessTask rows (newest first) as dict task_id -> row."""
    out = {}
    page = 1
    while True:
        st, d = api.get("tasks", f"/api/tasks/?page_size=500&page={page}&ordering=-date_created")
        if st != 200:
            break
        rows = d.get("results", d) if isinstance(d, dict) else d
        if not isinstance(rows, list):
            break
        for r in rows:
            out[r.get("task_id")] = r
        if not (isinstance(d, dict) and d.get("next")) or (ids and all(i in out for i in ids)):
            break
        page += 1
    return out


IGNORE_TASKS: set = set()  # do not wait for tasks that were already open before the run (e.g. test data)
def task_doc(row):
    """Document id of an ingestion task (API v9: related_document; newer: result_data.document_id)."""
    row = row or {}
    rd = row.get("result_data")
    if isinstance(rd, dict) and rd.get("document_id"):
        return int(rd["document_id"])
    rel = row.get("related_document")
    try:
        return int(rel) if rel not in (None, "") else None
    except (TypeError, ValueError):
        return None


TERMINAL = {"success", "failure", "revoked", "SUCCESS", "FAILURE", "REVOKED"}


def wait_tasks(api, task_ids, timeout, label="tasks"):
    """Waits until all task_ids are terminal. Returns dict task_id -> row; reports hanging ones."""
    deadline = time.time() + timeout
    rows = {}
    last = -1
    while time.time() < deadline:
        rows = tasks_by_id(api, task_ids)
        done = [t for t in task_ids if rows.get(t, {}).get("status") in TERMINAL]
        if len(done) != last:
            log(f"  {label}: {len(done)}/{len(task_ids)} fertig")
            last = len(done)
        if len(done) == len(task_ids):
            return rows
        time.sleep(3)
    missing = [t for t in task_ids if rows.get(t, {}).get("status") not in TERMINAL]
    REC.finding("hang", f"{len(missing)} von {len(task_ids)} Aufgaben nach {timeout}s nicht fertig",
                examples=[{"task": t, "status": rows.get(t, {}).get("status")} for t in missing[:5]])
    return rows


def wait_idle(api, env, timeout=600):
    """Waits until no PaperlessTask is pending/started anymore (and the internal queue is empty, if DB)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        rows = tasks_by_id(api)
        busy = [r for r in rows.values() if r.get("status") not in TERMINAL and r.get("task_id") not in IGNORE_TASKS]
        q = env.sql("SELECT count(*) FROM paperrust_task_queue WHERE state IN ('queued','running') "
                    f"AND created_at > to_timestamp({env.t_start})") if env.has_db else None
        qn = int(q[0][0]) if q else 0
        if not busy and qn == 0:
            return True
        time.sleep(3)
    q = env.sql("SELECT task_name, count(*) FROM paperrust_task_queue WHERE state IN ('queued','running') GROUP BY 1") if env.has_db else None
    REC.finding("hang", f"Warteschlange nach {timeout}s nicht leer: {dict((r[0], int(r[1])) for r in q or [])}")
    return False


def ensure_tag(api, name):
    st, d = api.get("tag_lookup", f"/api/tags/?name__iexact={urllib.parse.quote(name)}")
    if st == 200 and d.get("results"):
        return d["results"][0]["id"]
    st, d = api.post("tag_create", "/api/tags/", {"name": name, "matching_algorithm": 0})
    return d.get("id") if isinstance(d, dict) else None


def ensure_corr(api, name):
    st, d = api.get("corr_lookup", f"/api/correspondents/?name__iexact={urllib.parse.quote(name)}")
    if st == 200 and d.get("results"):
        return d["results"][0]["id"]
    st, d = api.post("corr_create", "/api/correspondents/", {"name": name, "matching_algorithm": 0})
    return d.get("id") if isinstance(d, dict) else None


def bulk(api, ids, method, params, op=None):
    st, d = api.call(op or f"bulk_{method}", "POST", "/api/documents/bulk_edit/",
                     {"documents": ids, "method": method, "parameters": params})
    return st


# ----------------------------------------------------------------------------- S1 bulk changes


def s1_bulk(api, env, a, fac):
    """Import of a.docs files; during and afterwards bulk_edit in all variants."""
    t_tag = ensure_tag(api, "stress-bulk")
    t_tag2 = ensure_tag(api, "stress-bulk-2")
    corr_a = ensure_corr(api, "stress-corr-a")
    corr_b = ensure_corr(api, "stress-corr-b")
    files = [fac.next("png" if i % 40 == 39 else "pdf") for i in range(a.docs)]
    log(f"S1: {len(files)} Dateien hochladen (4 parallel), währenddessen bulk_edit")
    task_ids = []
    upload_done = threading.Event()

    def uploader():
        def one(doc):
            c = api.clone()
            tid = upload(c, doc)
            c.close()
            return tid
        with ThreadPoolExecutor(4) as ex:
            for tid in ex.map(one, files):
                if tid:
                    task_ids.append(tid)
        upload_done.set()

    up = threading.Thread(target=uploader)
    up.start()

    ops_cycle = [
        ("add_tag", lambda: {"tag": t_tag}),
        ("remove_tag", lambda: {"tag": t_tag}),
        ("modify_tags", lambda: {"add_tags": [t_tag2], "remove_tags": [t_tag]}),
        ("modify_tags", lambda: {"add_tags": [t_tag], "remove_tags": [t_tag2]}),
        ("set_correspondent", lambda: {"correspondent": corr_a}),
        ("set_correspondent", lambda: {"correspondent": corr_b}),
        ("set_permissions", lambda: {"set_permissions": {"view": {"users": [3], "groups": []},
                                                         "change": {"users": [], "groups": []}}, "merge": True}),
    ]

    def bulk_round(parallel, label, reps):
        """reps passes of ops_cycle over all documents, per pass 'parallel' clients at the same time."""
        clients = [api.clone(timeout=a.timeout) for _ in range(parallel)]
        for rep in range(reps):
            ids, _ = all_ids(api)
            for k, (m, pf) in enumerate(ops_cycle):
                if parallel == 1:
                    bulk(clients[0], ids, m, pf(), op=f"{label}:{m}")
                else:
                    # same method from several clients at the same time (on the same documents)
                    ths = [threading.Thread(target=bulk, args=(clients[j], ids, m, pf(), f"{label}:{m}"))
                           for j in range(parallel)]
                    [t.start() for t in ths]
                    [t.join() for t in ths]
        for c in clients:
            c.close()

    # Phase A: during the import – serially in quick succession and with 2/4/8 in parallel
    phase = 0
    while not upload_done.is_set() or phase < 2:
        par = (1, 2, 4, 8)[phase % 4]
        bulk_round(par, f"during_p{par}", 1)
        phase += 1
        if upload_done.is_set() and phase >= 4:
            break
    log(f"S1: Upload fertig ({len(task_ids)} Aufgaben), bulk weiter während der Worker verarbeitet")
    # continue bulk as long as the worker is still ingesting
    rows = {}
    deadline = time.time() + a.ingest_timeout
    k = 0
    while time.time() < deadline:
        rows = tasks_by_id(api, task_ids)
        left = [t for t in task_ids if rows.get(t, {}).get("status") not in TERMINAL]
        if not left:
            break
        par = (1, 4, 2, 8)[k % 4]
        bulk_round(par, f"ingest_p{par}", 1)
        k += 1
        time.sleep(a.bulk_pause)
        if k % 4 == 0:
            log(f"  Import: noch {len(left)} offen")
    rows = wait_tasks(api, task_ids, 60, "Import")
    fails = [r for r in rows.values() if r.get("task_id") in task_ids and r.get("status") not in ("success", "SUCCESS")]
    for r in fails[:10]:
        REC.finding("ingest", f"Import-Aufgabe {r.get('status')}: {str(r.get('result_data') or r.get('result'))[:200]}")

    # Phase B: directly after the import – like the arm64 test (add/remove alternating, 30x serial)
    log("S1: direkt nach dem Import: 'Tag an alle' add/remove abwechselnd 30x seriell")
    ids, _ = all_ids(api)
    c = api.clone(timeout=a.timeout)
    for i in range(30):
        bulk(c, ids, "add_tag" if i % 2 == 0 else "remove_tag", {"tag": t_tag}, op="after:tag_all_alternating")
    # then in parallel 2..8 (same and mixed methods)
    for par in (2, 4, 8):
        bulk_round(par, f"after_p{par}", 2)

    # delete + restore (subset, also in parallel to a bulk add_tag)
    victims = ids[: min(20, len(ids))]
    th = threading.Thread(target=bulk, args=(api.clone(), ids, "add_tag", {"tag": t_tag2}, "after:add_tag_during_delete"))
    th.start()
    st = bulk(c, victims, "delete", {}, op="after:delete")
    th.join()
    if st == 200:
        st2, _ = c.post("trash_restore", "/api/trash/", {"documents": victims, "action": "restore"})
    # Check final state: add_tag to all → every document has the tag
    wait_idle(api, env, 300)
    ids, count = all_ids(api)
    bulk(c, ids, "add_tag", {"tag": t_tag}, op="final:add_tag")
    st, d = c.get("verify", f"/api/documents/?tags__id__all={t_tag}&page_size=1")
    if st == 200 and d.get("count") != count:
        REC.finding("consistency", f"nach add_tag an alle haben nur {d.get('count')} von {count} Dokumenten den Tag")
    c.close()
    up.join()
    return {"uploaded": len(task_ids), "ingest_failures": len(fails)}


# ----------------------------------------------------------------------------- S2 many users


def user_loop(api, a, until, rng, fac, own_docs):
    ids_cache = {"ids": [], "t": 0}

    def ids():
        if time.time() - ids_cache["t"] > 20 or not ids_cache["ids"]:
            ids_cache["ids"], _ = all_ids(api, "u:ids")
            ids_cache["t"] = time.time()
        return ids_cache["ids"] or [1]

    q = urllib.parse.quote
    acts = [
        (20, lambda: api.get("u:list", "/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true")),
        (8, lambda: api.get("u:list_deep", f"/api/documents/?page={rng.randint(1, 8)}&page_size=25&truncate_content=true",
                            ok=(200, 404))),
        (10, lambda: api.get("u:filter", f"/api/documents/?tags__id__in={rng.randint(1, 20)}&correspondent__id__in="
                             f"{rng.randint(1, 12)}&page_size=25&truncate_content=true")),
        (10, lambda: api.get("u:search", f"/api/documents/?query={q(rng.choice(WORDS))}&page_size=25&truncate_content=true")),
        (4, lambda: api.get("u:global_search", f"/api/search/?query={q(rng.choice(WORDS)[:4])}")),
        (4, lambda: api.get("u:autocomplete", f"/api/search/autocomplete/?term={q(rng.choice(WORDS)[:3])}")),
        (15, lambda: api.get("u:detail", f"/api/documents/{rng.choice(ids())}/")),
        (12, lambda: api.get("u:thumb", f"/api/documents/{rng.choice(ids())}/thumb/")),
        (3, lambda: api.get("u:preview", f"/api/documents/{rng.choice(ids())}/preview/")),
        (3, lambda: api.get("u:metadata", f"/api/documents/{rng.choice(ids())}/metadata/")),
        (3, lambda: api.get("u:notes_list", f"/api/documents/{rng.choice(ids())}/notes/")),
        (3, lambda: api.get("u:stats", "/api/statistics/")),
        (3, lambda: api.get("u:tags", "/api/tags/?page_size=100")),
        (2, lambda: api.post("u:selection_data", "/api/documents/selection_data/", {"documents": ids()[:100]})),
        (5, lambda: patch_title(api, rng, own_docs or ids())),
        (3, lambda: patch_tags(api, rng, own_docs or ids())),
        (3, lambda: add_note(api, rng, own_docs or ids())),
        (1, lambda: own_docs.append(upload(api, fac.next(), op="u:upload")) if a.user_uploads else None),
    ]
    total = sum(w for w, _ in acts)
    while time.time() < until:
        r = rng.uniform(0, total)
        for w, fn in acts:
            r -= w
            if r <= 0:
                try:
                    fn()
                except Exception as e:
                    REC.add("u:client_exception", -1, 0, False, f"{type(e).__name__}: {e}")
                break
        time.sleep(rng.uniform(0.05, a.think))
    api.close()


def patch_title(api, rng, ids):
    i = rng.choice([x for x in ids if isinstance(x, int)] or [1])
    api.call("u:patch_title", "PATCH", f"/api/documents/{i}/", {"title": f"stress title {rng.randint(0, 10**6)}"})


def patch_tags(api, rng, ids):
    i = rng.choice([x for x in ids if isinstance(x, int)] or [1])
    api.call("u:patch_tags", "PATCH", f"/api/documents/{i}/", {"tags": rng.sample(range(1, 21), rng.randint(0, 3))})


def add_note(api, rng, ids):
    i = rng.choice([x for x in ids if isinstance(x, int)] or [1])
    api.call("u:note_add", "POST", f"/api/documents/{i}/notes/", {"note": f"stress note {rng.random()}"})


def s2_users(api, env, a, fac):
    log(f"S2: {a.users} parallele Nutzer, {a.duration}s")
    until = time.time() + a.duration
    ths = []
    for u in range(a.users):
        ths.append(threading.Thread(target=user_loop, args=(api.clone(), a, until, random.Random(1000 + u), fac, [])))
        ths[-1].start()
        time.sleep(0.2)
    for t in ths:
        t.join()
    return {}


# ----------------------------------------------------------------------------- S3 the same document


def s3_same_doc(api, env, a, fac):
    wf_tag = ensure_tag(api, "stress-wf-updated")
    st, wf = api.post("wf_create", "/api/workflows/", {
        "name": f"stress updated {fac.run_id}", "order": 99, "enabled": True,
        "triggers": [{"type": 3, "sources": [1, 2, 3, 4], "matching_algorithm": 0, "match": "", "is_insensitive": True}],
        "actions": [{"type": 1, "assign_tags": [wf_tag]}],
    })
    wf_id = wf.get("id") if isinstance(wf, dict) else None
    # Target: an own, freshly ingested document
    doc = fac.next()
    tid = upload(api, doc, op="s3:upload")
    rows = wait_tasks(api, [tid], a.ingest_timeout, "S3-Dokument") if tid else {}
    did = task_doc(rows.get(tid)) if tid else None
    if not did:
        ids, _ = all_ids(api)
        did = ids[0]
    log(f"S3: Dokument {did}: {a.same_doc_clients} Clients PATCH gleichzeitig, Workflow 'Geändert', Version hochladen")
    titles = set()
    lock = threading.Lock()
    until = time.time() + a.same_doc_seconds

    def editor(k):
        c = api.clone()
        rng = random.Random(k)
        while time.time() < until:
            t = f"s3 title {k} {rng.randint(0, 10**6)}"
            r = rng.random()
            if r < 0.4:
                with lock:
                    titles.add(t)  # before sending: even an aborted request may have taken effect
                c.call("s3:patch_title", "PATCH", f"/api/documents/{did}/", {"title": t})
            elif r < 0.7:
                c.call("s3:patch_tags", "PATCH", f"/api/documents/{did}/", {"tags": rng.sample(range(1, 21), 2)})
            elif r < 0.8:
                with lock:
                    titles.add(t)
                c.call("s3:put_full", "PATCH", f"/api/documents/{did}/",
                       {"title": t, "correspondent": rng.randint(1, 12), "document_type": rng.randint(1, 8)})
            elif r < 0.9:
                c.call("s3:note", "POST", f"/api/documents/{did}/notes/", {"note": f"n{k}"})
            else:
                c.get("s3:get", f"/api/documents/{did}/")
        c.close()

    vtasks = []

    def versioner():
        c = api.clone()
        for i in range(a.versions):
            v = fac.next()
            st, d = c.call("s3:update_version", "POST", f"/api/documents/{did}/update_version/",
                           files=[("document", v["name"], v["data"], v["ctype"])], fields={"version_label": f"v{i}"})
            if st == 200 and isinstance(d, str):
                vtasks.append(d)
            time.sleep(a.same_doc_seconds / max(1, a.versions + 1))
        c.close()

    ths = [threading.Thread(target=editor, args=(k,)) for k in range(a.same_doc_clients)] + [threading.Thread(target=versioner)]
    [t.start() for t in ths]
    [t.join() for t in ths]
    vrows = wait_tasks(api, vtasks, a.ingest_timeout, "Versionen") if vtasks else {}
    for t in vtasks:
        if vrows.get(t, {}).get("status") not in ("success", "SUCCESS"):
            REC.finding("version", f"Versions-Aufgabe {vrows.get(t, {}).get('status')}: "
                                   f"{str(vrows.get(t, {}).get('result_data'))[:200]}")
    wait_idle(api, env, 300)
    st, d = api.get("s3:final_get", f"/api/documents/{did}/")
    if st == 200:
        if titles and d.get("title") not in titles:
            REC.finding("consistency", f"Titel nach parallelen PATCH unbekannt: {d.get('title')!r}")
        if wf_tag not in d.get("tags", []):
            REC.finding("workflow", f"Workflow 'Geändert' hat Tag {wf_tag} nicht gesetzt (tags={d.get('tags')})")
        if vtasks and len(d.get("versions") or []) < 1:
            REC.finding("version", f"keine Versionen am Dokument sichtbar (versions={d.get('versions')})")
    if wf_id:
        api.call("wf_delete", "DELETE", f"/api/workflows/{wf_id}/")
    return {"document": did, "versions_uploaded": len(vtasks)}


# ----------------------------------------------------------------------------- S4 Uploads


def s4_uploads(api, env, a, fac):
    log(f"S4: {a.parallel_uploads} parallele Uploads + Duplikate + große + kaputte Datei")
    docs = [fac.next("png" if i % 10 == 9 else "pdf") for i in range(a.parallel_uploads)]
    dup = docs[0]
    big_lines = [[f"Stress grosse Datei {fac.run_id}", "padding"]]
    big = {"name": f"stress_{fac.run_id}_big.pdf", "token": None, "title": f"stress_{fac.run_id}_big",
           "data": pdf_bytes(big_lines, pad=a.big_mb * 2**20), "ctype": "application/pdf"}
    broken = {"name": f"stress_{fac.run_id}_broken.pdf", "title": f"stress_{fac.run_id}_broken", "token": None,
              "data": b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R\n" + os.urandom(4000), "ctype": "application/pdf"}
    truncated = fac.next()
    truncated = dict(truncated, data=truncated["data"][: len(truncated["data"]) // 2], name=truncated["name"].replace(".pdf", "_trunc.pdf"))
    empty = {"name": f"stress_{fac.run_id}_empty.pdf", "title": f"stress_{fac.run_id}_empty", "token": None, "data": b"",
             "ctype": "application/pdf"}
    exe_like = {"name": f"stress_{fac.run_id}_bad.exe", "title": f"stress_{fac.run_id}_bad", "token": None,
                "data": b"MZ" + os.urandom(1000), "ctype": "application/octet-stream"}
    batch = [(d, "ok") for d in docs] + [(dup, "duplicate"), (dict(dup), "duplicate"), (big, "big"), (broken, "broken"),
                                          (truncated, "broken"), (empty, "empty"), (exe_like, "unsupported")]

    def one(item):
        d, kind = item
        c = api.clone(timeout=max(a.timeout, 300))
        ok = (200,) if kind in ("ok", "duplicate", "big", "broken") else (200, 400, 415)
        tid = upload(c, d, op=f"s4:upload_{kind}", ok=ok)
        c.close()
        return tid, kind, d
    with ThreadPoolExecutor(a.parallel_uploads) as ex:
        res = list(ex.map(one, batch))
    tids = [t for t, _, _ in res if t]
    rows = wait_tasks(api, tids, a.ingest_timeout, "S4")
    summary = collections.Counter()
    for tid, kind, d in res:
        if not tid:
            summary[f"{kind}:rejected"] += 1
            continue
        r = rows.get(tid, {})
        stt = str(r.get("status")).lower()
        summary[f"{kind}:{stt}"] += 1
        msg = str((r.get("result_data") or {}).get("error_message") or r.get("result") or "")
        if kind == "ok" and stt != "success":
            REC.finding("ingest", f"gültige Datei {d['name']} -> {stt}: {msg[:200]}")
        if kind in ("broken", "empty", "unsupported") and stt == "failure" and not msg:
            REC.finding("ingest", f"kaputte Datei {d['name']} fehlgeschlagen ohne Grund")
        if kind == "big" and stt != "success":
            REC.finding("ingest", f"große Datei ({a.big_mb} MB) -> {stt}: {msg[:200]}")
    dup_ok = summary.get("duplicate:success", 0) + summary.get("ok:success", 0)
    log(f"  S4 Ergebnis: {dict(summary)}")
    return {"outcomes": dict(summary), "dup_successes_incl_original": dup_ok}


# ----------------------------------------------------------------------------- S5 restart


def s5_restart(api, env, a, fac):
    if not (env.kill_cmd and env.start_cmd):
        log("S5: übersprungen (kein --kill-cmd/--start-cmd)")
        return {"skipped": True}
    files = [fac.next() for _ in range(a.restart_docs)]
    log(f"S5: {len(files)} Dateien hochladen, Server während der Verarbeitung hart beenden, neu starten")
    tids = []
    with ThreadPoolExecutor(4) as ex:
        for t in ex.map(lambda d: upload(api.clone(), d, op="s5:upload"), files):
            if t:
                tids.append(t)
    # wait until some are finished, then abort hard
    deadline = time.time() + a.ingest_timeout
    while time.time() < deadline:
        rows = tasks_by_id(api, tids)
        done = sum(1 for t in tids if rows.get(t, {}).get("status") in TERMINAL)
        if done >= max(2, len(tids) // 5):
            break
        time.sleep(1)
    api.close()
    log(f"  hart beenden nach {done} fertigen Aufgaben")
    subprocess.run(env.kill_cmd, shell=True, timeout=120)
    time.sleep(2)
    # Query while the server is gone: expected connection error, do not count as an error
    t0 = time.time()
    r = subprocess.run(env.start_cmd, shell=True, timeout=600, capture_output=True, text=True)
    log(f"  Neustart: {r.stdout.strip()[-200:]} ({time.time() - t0:.0f}s)")
    env.refresh_pid()
    rows = wait_tasks(api, tids, a.ingest_timeout, "nach Neustart")
    res = collections.Counter(str(rows.get(t, {}).get("status")).lower() for t in tids)
    for t in tids:
        r = rows.get(t, {})
        if str(r.get("status")).lower() != "success":
            REC.finding("restart", f"Aufgabe nach Neustart {r.get('status')}: {str(r.get('result_data'))[:200]}",
                        task=t)
    # every file exactly once as a document
    for d in files:
        st, j = api.get("s5:verify", f"/api/documents/?content__icontains={d['token']}&page_size=5&fields=id")
        n = j.get("count") if st == 200 else None
        if n != 1:
            REC.finding("restart", f"Datei {d['name']}: {n} Dokumente (erwartet 1)")
    return {"tasks": dict(res)}


# ----------------------------------------------------------------------------- S6 soak test


def s6_soak(api, env, a, fac):
    secs = int(a.soak_minutes * 60)
    log(f"S6: Dauerlauf {a.soak_minutes} min, {a.soak_users} Nutzer, Proben alle {a.sample_every}s")
    samples = []
    until = time.time() + secs
    ths = [threading.Thread(target=user_loop, args=(api.clone(), a, until, random.Random(5000 + u), fac, []))
           for u in range(a.soak_users)]
    [t.start() for t in ths]
    # additionally every 60 s a bulk add/remove over all documents and an upload
    c = api.clone()
    tag = ensure_tag(api, "stress-soak")
    k = 0
    next_bulk = time.time() + 30
    while time.time() < until:
        s = env.proc_stats() or {}
        s.update({"t": round(time.time() - (until - secs)), "db": env.db_conns()})
        samples.append(s)
        log(f"  Probe: {s}")
        end = min(until, time.time() + a.sample_every)
        while time.time() < end:
            if time.time() >= next_bulk:
                ids, _ = all_ids(c, "soak:ids")
                bulk(c, ids, "add_tag" if k % 2 == 0 else "remove_tag", {"tag": tag}, op="soak:bulk_tag_all")
                upload(c, fac.next(), op="soak:upload")
                k += 1
                next_bulk = time.time() + 60
            time.sleep(1)
    [t.join() for t in ths]
    c.close()
    wait_idle(api, env, 300)
    s = env.proc_stats() or {}
    s.update({"t": "end", "db": env.db_conns()})
    samples.append(s)
    trend = None
    pts = [x for x in samples if isinstance(x.get("t"), int) and x.get("private_mb")]
    if len(pts) >= 6:
        # Slope of the second half (MB/min) – steady growth after the warm-up phase?
        half = pts[len(pts) // 2:]
        xs = [p["t"] / 60 for p in half]
        ys = [p["private_mb"] for p in half]
        mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
        den = sum((x - mx) ** 2 for x in xs) or 1
        trend = round(sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / den, 2)
        hx = [p["handles"] for p in half]
        if trend > a.leak_mb_per_min:
            REC.finding("leak", f"privater Speicher wächst in der 2. Hälfte um {trend} MB/min "
                                f"({half[0]['private_mb']} → {half[-1]['private_mb']} MB)")
        if hx[-1] - hx[0] > 500:
            REC.finding("leak", f"Handles wachsen in der 2. Hälfte {hx[0]} → {hx[-1]}")
    return {"samples": samples, "private_mb_slope_2nd_half_mb_per_min": trend}


# ----------------------------------------------------------------------------- S7 consistency


def s7_consistency(api, env, a, fac, full_files=False):
    log("S7: Konsistenz")
    wait_idle(api, env, a.idle_timeout)
    res = {}
    ids, count = all_ids(api, "c:ids")
    res["documents"] = count
    if count is not None and count != len(ids):
        REC.finding("consistency", f"count {count} != len(all) {len(ids)}")
    # Detail, thumbnail and original per document (parallel 4)
    bad = collections.Counter()

    def check(i):
        c = api.clone()
        st, d = c.get("c:detail", f"/api/documents/{i}/")
        st2, _ = c.get("c:thumb", f"/api/documents/{i}/thumb/")
        st3 = 200
        if full_files:
            st3, _ = c.get("c:download", f"/api/documents/{i}/download/")
        c.close()
        return i, st, st2, st3
    with ThreadPoolExecutor(4) as ex:
        for i, st, st2, st3 in ex.map(check, ids):
            if st != 200:
                bad["detail"] += 1
            if st2 != 200:
                bad["thumb"] += 1
                REC.finding("consistency", f"Dokument {i}: Vorschaubild HTTP {st2}")
            if st3 != 200:
                bad["download"] += 1
                REC.finding("consistency", f"Dokument {i}: Original HTTP {st3}")
    res["api_bad"] = dict(bad)
    # Search index: find every successfully ingested text PDF of this run via its unique word
    # (mapping task -> document via result_data.document_id; titles may be changed by scenarios)
    trows = tasks_by_id(api, list(UPLOADS))
    pairs = []
    for tid, token in list(UPLOADS.items()):
        r = trows.get(tid) or {}
        did = task_doc(r)
        if token and did and str(r.get("status")).lower() == "success":
            pairs.append((did, token))
    if not UPLOADS:
        # standalone s7 run: all documents with a suite search word in the content
        st, d = api.get("c:own_docs", "/api/documents/?content__icontains=stk&page_size=100000&fields=id,content")
        for doc in (d.get("results", []) if st == 200 else []):
            m = re.search(r"stk\d+x\d{5}", doc.get("content") or "")
            if m:
                pairs.append((doc["id"], m.group(0)))
    expected = collections.Counter(t for _, t in pairs)
    own, miss = 0, 0
    for did, token in pairs:
        own += 1
        st, j = api.get("c:search_token", f"/api/documents/?query={token}&page_size=5&fields=id", ok=(200,))
        if st != 200:
            continue
        found = [x["id"] for x in j.get("results", [])]
        if did not in found:
            st2, dd = api.get("c:detail", f"/api/documents/{did}/?fields=id,content", ok=(200, 404))
            if st2 == 200 and token in (dd.get("content") or ""):
                miss += 1
                if miss <= 10:
                    REC.finding("index", f"Dokument {did} über sein Suchwort {token} nicht gefunden (Treffer {found})")
        elif len(found) > max(1, expected.get(token, 1)):
            # s4 deliberately uploads the same file several times (duplicates allowed): as many hits as
            # successful uploads with this search word are correct, more would be ingested twice
            REC.finding("consistency", f"Suchwort {token} trifft mehrere Dokumente {found} (doppelt eingelesen?)")
    res["own_docs"] = own
    res["index_missing"] = miss
    # Files and DB (local only)
    if env.media and env.has_db:
        rows = env.sql("SELECT id, coalesce(filename,''), coalesce(archive_filename,''), mime_type, "
                       "(deleted_at IS NOT NULL)::int FROM documents_document")
        media = env.media / "documents"
        ref_o, ref_a, ref_t = set(), set(), set()
        missing = collections.Counter()
        for did, fn, an, mt, deleted in rows:
            if fn:
                ref_o.add(os.path.normcase(str(media / "originals" / fn)))
                if not (media / "originals" / fn).exists():
                    missing["original"] += 1
                    REC.finding("files", f"Dokument {did}: Original fehlt ({fn})")
            else:
                missing["no_filename"] += 1
                REC.finding("files", f"Dokument {did}: kein Dateiname in der DB")
            if an:
                ref_a.add(os.path.normcase(str(media / "archive" / an)))
                if not (media / "archive" / an).exists():
                    missing["archive"] += 1
                    REC.finding("files", f"Dokument {did}: Archivdatei fehlt ({an})")
            th = [media / "thumbnails" / f"{int(did):07d}.{e}" for e in ("webp", "png")]
            ref_t.update(os.path.normcase(str(p)) for p in th)
            if not any(p.exists() for p in th):
                missing["thumbnail"] += 1
                REC.finding("files", f"Dokument {did}: Vorschaubild fehlt")
        orphans = []
        for sub, ref in (("originals", ref_o), ("archive", ref_a), ("thumbnails", ref_t)):
            base = media / sub
            if base.exists():
                for p in base.rglob("*"):
                    if p.is_file() and os.path.normcase(str(p)) not in ref:
                        orphans.append(str(p.relative_to(env.media)))
        if orphans:
            REC.finding("files", f"{len(orphans)} verwaiste Dateien", examples=orphans[:10])
        res["db_documents_incl_trash"] = len(rows)
        res["files_missing"] = dict(missing)
        res["orphans"] = len(orphans)
        # Queue and tasks
        q = env.sql("SELECT state, count(*) FROM paperrust_task_queue GROUP BY 1")
        res["queue_states"] = {r[0]: int(r[1]) for r in q or []}
        stuck = env.sql("SELECT task_id, task_name, state, attempts FROM paperrust_task_queue "
                        f"WHERE state IN ('queued','running') AND created_at > to_timestamp({env.t_start}) LIMIT 20")
        if stuck:
            REC.finding("queue", f"{len(stuck)} Einträge in paperrust_task_queue nicht abgeschlossen", examples=stuck[:5])
        t = env.sql("SELECT status, count(*) FROM documents_paperlesstask GROUP BY 1")
        res["paperlesstask_states"] = {r[0]: int(r[1]) for r in t or []}
        t = env.sql("SELECT task_id, task_type, status FROM documents_paperlesstask "
                    "WHERE lower(status) IN ('pending','started') AND date_created < now() - interval '2 minutes' "
                    f"AND date_created > to_timestamp({env.t_start})")
        if t:
            REC.finding("queue", f"{len(t)} PaperlessTask hängen (pending/started > 2 min)", examples=t[:5])
        t = env.sql("SELECT task_id, task_type FROM documents_paperlesstask WHERE lower(status) = 'failure' "
                    "AND (result_data IS NULL OR result_data::text IN ('null', '{}', '\"\"')) "
                    f"AND date_created > to_timestamp({env.t_start})")
        if t:
            REC.finding("queue", f"{len(t)} fehlgeschlagene Aufgaben ohne Grund", examples=t[:5])
        # Counter caches (trigger-maintained) against real count
        cnt = env.sql("SELECT count(*) FROM documents_document WHERE deleted_at IS NULL")
        res["db_live_documents"] = int(cnt[0][0]) if cnt else None
    return res


# ----------------------------------------------------------------------------- Procedure

# ----------------------------------------------------------------------------- S8 aborting clients (B10)


def abort_request(api, method, path, payload, delay):
    """Sends a request on its own connection and closes it hard (RST) after `delay` s without reading the
    response – like a client with a time limit, a closed tab or a proxy abort.
    Returns True if the response had already arrived before the abort (then it was not a real abort)."""
    import socket
    body = json.dumps(payload).encode()
    crlf = chr(13) + chr(10)
    head = crlf.join([
        f"{method} {api.prefix}{path} HTTP/1.1", f"Host: {api.host}:{api.port}", f"Authorization: {api.auth}",
        "Accept: application/json; version=9", "Content-Type: application/json", f"Content-Length: {len(body)}",
        "Connection: close", "", ""]).encode()
    t = time.perf_counter()
    s = socket.create_connection((api.host, api.port), timeout=10)
    answered = False
    try:
        s.sendall(head + body)
        if delay > 0:
            s.settimeout(delay)
            try:
                answered = bool(s.recv(1, socket.MSG_PEEK))
            except (socket.timeout, OSError):
                answered = False
        s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))  # RST instead of FIN
    finally:
        s.close()
    REC.add(f"s8:abort_{method.lower()}", 0 if not answered else 200, time.perf_counter() - t, True,
            "answered before abort" if answered else "aborted")
    return answered


def s8_abort(api, env, a, fac):
    """FINDINGS B10: clients abort writing requests in the middle of processing. What was saved
    must still be followed up completely: workflow "Document changed" (sets a
    tag), search index (new title findable via full text), follow-up of the bulk change."""
    wf_tag = ensure_tag(api, "stress-abort-wf")
    t_bulk = ensure_tag(api, "stress-abort-bulk")
    st, wf = api.post("wf_create", "/api/workflows/", {
        "name": f"stress abort {fac.run_id}", "order": 98, "enabled": True,
        "triggers": [{"type": 3, "sources": [1, 2, 3, 4], "matching_algorithm": 0, "match": "", "is_insensitive": True}],
        "actions": [{"type": 1, "assign_tags": [wf_tag]}],
    })
    wf_id = wf.get("id") if isinstance(wf, dict) else None
    n = max(4, a.abort_docs)
    docs = [fac.next() for _ in range(n)]
    tids = [t for t in (upload(api, d, op="s8:upload") for d in docs) if t]
    rows = wait_tasks(api, tids, a.ingest_timeout, "S8-Dokumente")
    dids = [d for d in (task_doc(rows.get(t)) for t in tids) if d]
    if len(dids) < 4:
        REC.finding("suite", f"S8: nur {len(dids)} Dokumente eingelesen")
        return {}
    wait_idle(api, env, 300)
    delays = [0.0, 0.005, 0.02, 0.05, 0.1, 0.2, 0.4]
    expect_title = {}
    rng = random.Random(fac.run_id)
    log(f"S8: {a.abort_rounds} Runden abgebrochene PATCH/bulk_edit auf {len(dids)} Dokumente (Workflow {wf_id})")
    answered = aborted = 0
    for rnd in range(a.abort_rounds):
        half = len(dids) // 2
        patch_ids, bulk_ids = dids[:half], dids[half:]
        if rnd % 2:
            patch_ids, bulk_ids = bulk_ids, patch_ids
        jobs = []
        for did in patch_ids:
            tok = f"abrt{fac.run_id}r{rnd}d{did}x"
            expect_title.setdefault(did, set()).add(tok)
            # Clear tags: the workflow must set the tag again after saving
            jobs.append(("PATCH", f"/api/documents/{did}/", {"title": tok, "tags": []}))
        # Bulk change: workflow tag gone, bulk tag added – the follow-up (bulk_update_documents →
        # workflow "Changed") must set the workflow tag again
        jobs.append(("POST", "/api/documents/bulk_edit/",
                     {"documents": bulk_ids, "method": "modify_tags",
                      "parameters": {"add_tags": [t_bulk], "remove_tags": [wf_tag]}}))
        with ThreadPoolExecutor(max_workers=len(jobs)) as ex:
            res = list(ex.map(lambda j: abort_request(api, j[0], j[1], j[2], rng.choice(delays)), jobs))
        answered += sum(res)
        aborted += len(res) - sum(res)
        time.sleep(0.5)
    wait_idle(api, env, a.idle_timeout)
    saved = 0
    missing_wf, missing_ix = [], []
    for did in dids:
        st, d = api.get("s8:final_get", f"/api/documents/{did}/")
        if st != 200 or not isinstance(d, dict):
            continue
        title = d.get("title") or ""
        if title in expect_title.get(did, set()):
            saved += 1
            hit = []
            for _ in range(10):  # Index tasks may run on briefly
                st2, r = api.get("s8:search", f"/api/documents/?query={urllib.parse.quote(title)}&fields=id&page_size=5")
                hit = [x.get("id") for x in (r.get("results", []) if st2 == 200 and isinstance(r, dict) else [])]
                if did in hit:
                    break
                time.sleep(3)
            if did not in hit:
                missing_ix.append({"doc": did, "title": title, "hits": hit})
        if wf_tag not in (d.get("tags") or []):
            missing_wf.append({"doc": did, "tags": d.get("tags"), "title": title})
    if missing_wf:
        REC.finding("workflow", f"S8: Workflow 'Geändert' nach abgebrochener Anfrage nicht ausgeführt "
                                f"({len(missing_wf)} Dokumente)", examples=missing_wf[:5])
    if missing_ix:
        REC.finding("index", f"S8: gespeicherter Titel nicht im Suchindex ({len(missing_ix)} Dokumente)",
                    examples=missing_ix[:5])
    if aborted == 0:
        REC.finding("suite", "S8: keine Anfrage wurde wirklich vor der Antwort abgebrochen (Test ohne Aussage)")
    if wf_id:
        api.call("wf_delete", "DELETE", f"/api/workflows/{wf_id}/")
    return {"documents": len(dids), "aborted": aborted, "answered_before_abort": answered,
            "titles_saved": saved, "missing_workflow": len(missing_wf), "missing_index": len(missing_ix)}


SCEN = {"s1": s1_bulk, "s2": s2_users, "s3": s3_same_doc, "s4": s4_uploads, "s5": s5_restart, "s6": s6_soak,
        "s7": s7_consistency, "s8": s8_abort}
TITLES = {"s1": "Massenänderungen während der Verarbeitung", "s2": "Viele Nutzer", "s3": "Gleichzeitiges Bearbeiten",
          "s4": "Parallele Uploads", "s5": "Neustart/Abbruch", "s6": "Dauerlauf", "s7": "Konsistenz",
          "s8": "Abbrechende Clients"}


def main():
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")  # Windows console/redirection: cp1252
        except (AttributeError, ValueError):
            pass
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("scenarios", nargs="*", default=["s1", "s4", "s3", "s5", "s2"])
    p.add_argument("--local", action="store_true", help="Server aus tests/stress/server.sh (Logs, DB, Medien, Neustart)")
    p.add_argument("--url")
    p.add_argument("--user", default="admin")
    p.add_argument("--password", default=os.environ.get("ST_PASSWORD", "admin"))
    p.add_argument("--token", default=os.environ.get("ST_TOKEN"))
    p.add_argument("--out")
    p.add_argument("--repeat", type=int, default=1, help="jedes Szenario so oft wiederholen")
    p.add_argument("--docs", type=int, default=200)
    p.add_argument("--users", type=int, default=20)
    p.add_argument("--duration", type=int, default=300)
    p.add_argument("--think", type=float, default=1.0)
    p.add_argument("--user-uploads", action="store_true", default=True)
    p.add_argument("--same-doc-clients", type=int, default=8)
    p.add_argument("--same-doc-seconds", type=int, default=60)
    p.add_argument("--versions", type=int, default=3)
    p.add_argument("--parallel-uploads", type=int, default=20)
    p.add_argument("--big-mb", type=int, default=60)
    p.add_argument("--restart-docs", type=int, default=30)
    p.add_argument("--soak-minutes", type=float, default=30)
    p.add_argument("--soak-users", type=int, default=10)
    p.add_argument("--sample-every", type=int, default=30)
    p.add_argument("--leak-mb-per-min", type=float, default=2.0)
    p.add_argument("--timeout", type=float, default=60.0, help="Zeitüberschreitung je Anfrage (s)")
    p.add_argument("--ingest-timeout", type=int, default=3600)
    p.add_argument("--idle-timeout", type=int, default=600, help="S7: höchstens so lange auf leere Warteschlange warten")
    p.add_argument("--bulk-pause", type=float, default=3.0, help="S1: Pause zwischen bulk-Runden während des Einlesens")
    p.add_argument("--keep-workflows", action="store_true",
                   help="vorhandene Workflows nicht abschalten (lokal werden sie sonst für den Lauf deaktiviert: "
                        "die Testdaten senden je Änderung Webhooks/Mails und fluten die Warteschlange)")
    p.add_argument("--full-files", action="store_true", help="S7 lädt jedes Original herunter")
    p.add_argument("--pid"), p.add_argument("--log-dir"), p.add_argument("--media")
    p.add_argument("--psql"), p.add_argument("--dsn")
    p.add_argument("--kill-cmd"), p.add_argument("--start-cmd")
    p.add_argument("--no-consistency", action="store_true")
    p.add_argument("--abort-docs", type=int, default=12, help="S8: Anzahl Dokumente")
    p.add_argument("--abort-rounds", type=int, default=20, help="S8: Runden abgebrochener Anfragen")
    a = p.parse_args()
    if not a.url and not a.local:
        p.error("--url oder --local angeben")
    env = Env(a)
    if a.local and not a.token and a.password == "admin":
        # Basic auth costs one PBKDF2 check per request (debug build ~140 ms) and distorts the load;
        # use the test token of the reference admin locally (reference/users.json, test value only).
        try:
            u = json.loads((REPO / "reference" / "users.json").read_text(encoding="utf-8"))["users"]
            a.token = next(x["token"] for x in u if x.get("username") == a.user)
        except (OSError, ValueError, KeyError, StopIteration):
            pass
    lockwatch = LockWatch(env) if env.has_db else None
    disabled_wf = []
    run_id = time.strftime("%m%d%H%M")
    out = Path(a.out or (Path(os.environ.get("TEMP", "/tmp")) / "paperrust-stress-results" / run_id))
    out.mkdir(parents=True, exist_ok=True)
    api = Api(env.url, a.user, a.password, a.token, a.timeout)
    fac = DocFactory(run_id)
    st, _ = api.get("ping", "/api/documents/?page_size=1")
    if st != 200:
        sys.exit(f"Server {env.url} nicht erreichbar/angemeldet (HTTP {st})")
    IGNORE_TASKS.update(t for t, r in tasks_by_id(api).items() if r.get("status") not in TERMINAL)
    if a.local and not a.keep_workflows:
        st, d = api.get("wf_list", "/api/workflows/?page_size=1000")
        for w in (d.get("results", []) if st == 200 else []):
            if w.get("enabled"):
                api.call("wf_disable", "PATCH", f"/api/workflows/{w['id']}/", {"enabled": False})
                disabled_wf.append(w["id"])
        log(f"Workflows für den Lauf abgeschaltet: {disabled_wf}")
    log(f"Ziel {env.url}, Lauf {run_id}, Ausgabe {out}, PID {env.pid}")
    results = {"run_id": run_id, "url": env.url, "args": {k: v for k, v in vars(a).items() if k not in ("password", "token")},
               "scenarios": []}
    seen = collections.Counter()
    for name in a.scenarios:
        for rep in range(a.repeat):
            seen[name] += 1
            total = a.repeat * a.scenarios.count(name)
            label = name if total == 1 else f"{name}#{seen[name]}"
            REC.scenario = label
            watch = LogWatch(env)
            mem0 = env.proc_stats()
            t0 = time.time()
            log(f"=== {label} {TITLES.get(name, '')}")
            try:
                extra = SCEN[name](api, env, a, fac)
            except Exception as e:  # Do not swallow errors of the suite itself
                REC.finding("suite", f"{type(e).__name__}: {e}", trace=traceback.format_exc()[-1500:])
                extra = {}
            if name not in ("s7",) and not a.no_consistency:
                try:
                    extra["consistency"] = s7_consistency(api, env, a, fac)
                except Exception as e:
                    REC.finding("suite", f"Konsistenz: {type(e).__name__}: {e}", trace=traceback.format_exc()[-1500:])
            dur = time.time() - t0
            if lockwatch:
                extra["lock_waits"] = lockwatch.take()
            summ = REC.summary(label)
            summ.update({"name": label, "title": TITLES.get(name), "seconds": round(dur), "extra": extra,
                         "proc_before": mem0, "proc_after": env.proc_stats(), "server_log": watch.excerpt()})
            results["scenarios"].append(summ)
            log(f"=== {label}: {summ['requests']} Anfragen, {summ['errors']} HTTP-Fehler, "
                f"{len(summ['findings'])} Befunde, {dur:.0f}s, Log-Warnungen {summ['server_log']['count']}")
            (out / "summary.json").write_text(json.dumps(results, indent=1, ensure_ascii=False, default=str), encoding="utf-8")
            (out / "summary.md").write_text(render_md(results), encoding="utf-8")
    for wid in disabled_wf:
        api.call("wf_enable", "PATCH", f"/api/workflows/{wid}/", {"enabled": True})
    (out / "requests.jsonl").write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in REC.rows), encoding="utf-8")
    md = render_md(results)
    (out / "summary.md").write_text(md, encoding="utf-8")
    print(md)
    bad = sum(s["errors"] + len(s["findings"]) for s in results["scenarios"])
    sys.exit(1 if bad else 0)


def render_md(res):
    L = [f"# Stress-Lauf {res['run_id']} gegen {res['url']}", ""]
    L.append("| Szenario | Dauer | Anfragen | HTTP-Fehler | Befunde | Log-Warnungen |")
    L.append("|---|---|---|---|---|---|")
    for s in res["scenarios"]:
        L.append(f"| {s['name']} {s['title']} | {s['seconds']} s | {s['requests']} | {s['errors']} | "
                 f"{len(s['findings'])} | {s['server_log']['count']} |")
    for s in res["scenarios"]:
        L += ["", f"## {s['name']} {s['title']}", ""]
        L.append("| Operation | n | Fehler | Codes | p50 ms | p95 ms | max ms |")
        L.append("|---|---|---|---|---|---|---|")
        for op, o in s["ops"].items():
            L.append(f"| {op} | {o['n']} | {o['errors']} | {o['codes']} | {o['p50_ms']} | {o['p95_ms']} | {o['max_ms']} |")
        for op, o in s["ops"].items():
            for e in o["error_examples"]:
                L.append(f"- `{op}` HTTP {e['status']} ({e['ms']} ms): {e['detail'][:250]}")
        for f in s["findings"]:
            L.append(f"- **{f['kind']}**: {f['msg']}")
        if s["server_log"]["groups"]:
            L += ["", "Server-Log (gruppiert):", "```"]
            L += [f"{n:5d}x {g}" for g, n in s["server_log"]["groups"]]
            L.append("```")
        ex = {k: v for k, v in s["extra"].items() if k != "samples"}
        L += ["", "```json", json.dumps(ex, ensure_ascii=False, default=str)[:3000], "```"]
    return "\n".join(L)


if __name__ == "__main__":
    main()
