#!/usr/bin/env python3
"""Messdurchlauf für ein System (ngxa = paperless-ngx 3.4.2 minimal, ngxb = ngx mit Tika+Gotenberg, rs = PaperRust), gegliedert in Szenarien.

Aufruf:
  python3 bench.py run <runde> <ngxa|ngxb|rs> [--smoke]   Szenarien 1–4 und 6 (frische Volumes)
  python3 bench.py big <ngxa|ngxb|rs> <anzahl> [--smoke]  Szenario 5: Archiv auf <anzahl>+230 Dokumente vergrößern
Ergebnis: ~/prbench/results/run_<runde>_<system>.json bzw. big_<system>.json (+ samples_*.jsonl)

Szenarien (Reihenfolge innerhalb eines Laufs):
  S1 Leerlauf      Erststart (leere DB), 3 Neustarts, danach 10 min nichts tun (RAM, CPU, Prozesse)
  S4 Schwere Last  alle 230 Testdokumente auf einmal in den Eingangsordner; währenddessen fragt ein
                   "Nutzer" jede Sekunde Liste/Suche ab; danach Massenbearbeitung und "Alle auswählen"
  S6 Platte        Volumes nach dem Import, RAM im Leerlauf mit Dokumenten
  S2 Alltag        1 simulierter Nutzer, 15 min, mit Denkpausen (2–8 s)
  S3 Kleines Büro  derselbe Ablauf mit 5 und mit 10 gleichzeitigen Nutzern, je 5 min
Container laufen auf festen Kernen (--cpuset-cpus 0-3), das Messprogramm auf anderen (taskset 4,5).
"""
import http.client
import json
import math
import os
import random
import secrets
import shutil
import statistics
import subprocess
import sys
import threading
import time
import urllib.parse

BASE = os.path.expanduser("~/prbench")
TOOLS = os.path.join(BASE, "tools")
RESULTS = os.path.join(BASE, "results")
CONSUME = os.path.join(BASE, "consume")
STAGING = os.path.join(BASE, "staging")
DOCS = os.path.join(BASE, "docs")
LOCK = os.path.expanduser("~/jobs/paperrust_bench.lock")
PORT = 18000
NET = "prbench_net"
TZ = "Europe/Zurich"
ADMIN = "bench"
SECRET = "prbench-local-secret-key-not-for-production-0123456789"
IMAGES = {"ngx": "ghcr.io/paperless-ngx/paperless-ngx:3.4.2", "rs": "prbench_paperrust:local",
          "db": "docker.io/library/postgres:17", "broker": "docker.io/valkey/valkey:9-alpine",
          "gotenberg": "docker.io/gotenberg/gotenberg:8.38", "tika": "docker.io/apache/tika:3.3.1.0"}
SYSTEM_IMAGES = {"ngxa": ["ngx", "broker"], "ngxb": ["ngx", "broker", "gotenberg", "tika"],
                 "ngxc": ["ngx", "broker", "gotenberg", "tika"], "rs": ["rs"]}
NGX = ("ngxa", "ngxb", "ngxc")
WORDS = ["rechnung", "heizöl", "zahnreinigung", "versicherung", "mahnung", "angebot", "gartenpflege",
         "internet", "inspektion", "treppenhaus", "visitenkarten", "überweisung", "jahresabrechnung",
         "renovierung", "fahrradreparatur", "lizenz", "zürich", "bern", "basel", "zwetschgenröster"]


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def docker(*args, check=True):
    r = subprocess.run(["sudo", "-n", "docker", *args], capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"docker {' '.join(args[:3])}: {r.stderr.strip()[:500]}")
    return r.stdout.strip()


def admin_pw():
    p = os.path.join(BASE, "state", "admin_pw")
    if not os.path.exists(p):
        with open(p, "w") as f:
            f.write(secrets.token_urlsafe(18))
        os.chmod(p, 0o600)
    return open(p).read().strip()


def mem_avail():
    for l in open("/proc/meminfo"):
        if l.startswith("MemAvailable:"):
            return int(l.split()[1]) // 1024
    return 0


def foreign_mpc():
    import glob
    return [p for p in glob.glob(os.path.expanduser("~/jobs/mpc_*.lock")) if "mpc_luecke_" not in p]


def sh_out(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout.strip()


def conditions():
    return {"time": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "free_m": sh_out("free -m"), "uptime": sh_out("uptime"),
            "cpu_governor": sh_out("cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"), "thermo": sh_out("cat /sys/class/thermal/thermal_zone0/temp"),
            "locks": sh_out("ls ~/jobs/*.lock 2>/dev/null"),
            "containers": sh_out("sudo -n docker ps --format '{{.Names}}'").split("\n")}


def oom_count():
    return int(sh_out("sudo -n dmesg 2>/dev/null | grep -ci 'out of memory\\|oom-kill' || true") or 0)


# ------------------------------------------------------------------ Container definitions
def specs(system, cpus, pw):
    p = f"prbench_{system}_"
    threads = "4" if cpus == "0-3" else "2"
    db = ["run", "-d", "--name", p + "db", "--network", NET, "--cpuset-cpus", cpus,
          "-v", p + "pgdata:/var/lib/postgresql/data",
          "-e", "POSTGRES_DB=paperless", "-e", "POSTGRES_USER=paperless", "-e", "POSTGRES_PASSWORD=paperless",
          IMAGES["db"]]
    vols = ["-v", p + "data:/usr/src/paperless/data", "-v", p + "media:/usr/src/paperless/media",
            "-v", CONSUME + ":/usr/src/paperless/consume"]
    if system in NGX:
        # ngxa = minimal (web server container + Valkey), ngxb = complete (additionally Tika + Gotenberg),
        # both after upstream docker/compose/docker-compose.postgres(-tika).yml
        extras = [["run", "-d", "--name", p + "broker", "--network", NET, "--cpuset-cpus", cpus,
                   "-v", p + "redis:/data", IMAGES["broker"]]]
        names = [p + "web", p + "broker"]
        env = {"PAPERLESS_REDIS": f"redis://{p}broker:6379", "PAPERLESS_DBHOST": p + "db",
               "PAPERLESS_DBENGINE": "postgresql", "PAPERLESS_SECRET_KEY": SECRET, "PAPERLESS_TIME_ZONE": TZ,
               "PAPERLESS_OCR_LANGUAGE": "deu+eng", "PAPERLESS_ADMIN_USER": ADMIN, "PAPERLESS_ADMIN_PASSWORD": pw,
               "PAPERLESS_THREADS_PER_WORKER": threads}
        if system in ("ngxb", "ngxc"):
            extras.append(["run", "-d", "--name", p + "gotenberg", "--network", NET, "--cpuset-cpus", cpus,
                           IMAGES["gotenberg"], "gotenberg", "--chromium-disable-javascript=true",
                           "--chromium-allow-list=^file:///tmp/.*"])
            extras.append(["run", "-d", "--name", p + "tika", "--network", NET, "--cpuset-cpus", cpus,
                           IMAGES["tika"]])
            names += [p + "gotenberg", p + "tika"]
            env.update({"PAPERLESS_TIKA_ENABLED": "1",
                        "PAPERLESS_TIKA_GOTENBERG_ENDPOINT": f"http://{p}gotenberg:3000",
                        "PAPERLESS_TIKA_ENDPOINT": f"http://{p}tika:9998"})
        if system == "ngxc":
            # like ngxb, but without cleaning/straightening (same work as PaperRust)
            env.update({"PAPERLESS_OCR_CLEAN": "none", "PAPERLESS_OCR_DESKEW": "false"})
        app = ["run", "-d", "--name", p + "web", "--network", NET, "--cpuset-cpus", cpus,
               "-p", f"127.0.0.1:{PORT}:8000", *vols]
        for k, v in env.items():
            app += ["-e", f"{k}={v}"]
        app.append(IMAGES["ngx"])
        return {"db": db, "extras": extras, "app": app, "names": names + [p + "db"],
                "appname": p + "web", "dbname": p + "db",
                "volumes": [p + "pgdata", p + "data", p + "media", p + "redis"]}
    env = {"PAPERRUST_DBENGINE": "postgresql", "PAPERRUST_DBHOST": p + "db", "PAPERRUST_DBNAME": "paperless",
           "PAPERRUST_DBUSER": "paperless", "PAPERRUST_DBPASS": "paperless", "PAPERRUST_SECRET_KEY": SECRET,
           "PAPERRUST_TIME_ZONE": TZ, "PAPERRUST_OCR_LANGUAGE": "deu+eng", "PAPERRUST_ADMIN_USER": ADMIN,
           "PAPERRUST_ADMIN_PASSWORD": pw, "PAPERRUST_THREADS_PER_WORKER": threads}
    app = ["run", "-d", "--name", p + "app", "--network", NET, "--cpuset-cpus", cpus, "--stop-timeout", "45",
           "-p", f"127.0.0.1:{PORT}:8000", *vols]
    for k, v in env.items():
        app += ["-e", f"{k}={v}"]
    app.append(IMAGES["rs"])
    return {"db": db, "extras": [], "app": app, "names": [p + "app", p + "db"], "appname": p + "app",
            "dbname": p + "db", "volumes": [p + "pgdata", p + "data", p + "media"]}


# ------------------------------------------------------------------ HTTP
class Api:
    def __init__(self):
        self.token = None

    def req(self, method, path, body=None, headers=None, timeout=300, conn=None):
        h = {"Accept": "application/json; version=9"}
        h.update(headers or {})
        if self.token:
            h["Authorization"] = f"Token {self.token}"
        own = conn is None
        c = conn or http.client.HTTPConnection("127.0.0.1", PORT, timeout=timeout)
        try:
            c.request(method, path, body=body, headers=h)
            r = c.getresponse()
            data = r.read()
            return r.status, data
        finally:
            if own:
                c.close()

    def json(self, method, path, obj=None, conn=None):
        body = json.dumps(obj).encode() if obj is not None else None
        st, data = self.req(method, path, body, {"Content-Type": "application/json"}, conn=conn)
        try:
            return st, json.loads(data or b"null")
        except ValueError:
            return st, None

    def count(self):
        try:
            st, d = self.json("GET", "/api/documents/?page_size=1")
        except (OSError, http.client.HTTPException):
            return -1
        return d.get("count", -1) if st == 200 and isinstance(d, dict) else -1

    def all_ids(self):
        ids, page = [], 1
        while True:
            st, d = self.json("GET", f"/api/documents/?page={page}&page_size=1000&fields=id&ordering=id")
            if st != 200 or not isinstance(d, dict):
                break
            ids += [x["id"] for x in d.get("results", [])]
            if not d.get("next"):
                break
            page += 1
        return ids


def multipart(fields, files):
    b = "----prbench" + secrets.token_hex(8)
    out = bytearray()
    for k, v in fields:
        out += f"--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n".encode()
    for k, name, data in files:
        out += (f"--{b}\r\nContent-Disposition: form-data; name=\"{k}\"; filename=\"{name}\"\r\n"
                f"Content-Type: application/pdf\r\n\r\n").encode() + data + b"\r\n"
    out += f"--{b}--\r\n".encode()
    return bytes(out), f"multipart/form-data; boundary={b}"


def wait_http(api, path, ok, timeout=900):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            st, _ = api.req("GET", path, timeout=5)
            if st in ok:
                return time.time() - t0
        except (OSError, http.client.HTTPException):
            pass
        time.sleep(0.25)
    raise RuntimeError(f"keine Antwort von {path} nach {timeout}s")


def get_token(api, pw, timeout=1800):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            body = urllib.parse.urlencode({"username": ADMIN, "password": pw}).encode()
            st, data = api.req("POST", "/api/token/", body, {"Content-Type": "application/x-www-form-urlencoded"},
                               timeout=10)
            if st == 200:
                api.token = json.loads(data)["token"]
                return
        except (OSError, http.client.HTTPException, ValueError):
            pass
        time.sleep(0.5)
    raise RuntimeError("kein Token erhalten")


# ------------------------------------------------------------------ Statistics
def pct(s, q):
    if not s:
        return None
    s = sorted(s)
    return s[min(len(s) - 1, int(math.ceil(q * len(s))) - 1)]


def lat_stats(recs):
    """recs: list of (latency_s, status)."""
    lat = [r[0] * 1000 for r in recs]
    err = sum(1 for r in recs if r[1] < 200 or r[1] >= 400)
    if not lat:
        return {"n": 0}
    return {"n": len(lat), "median_ms": round(statistics.median(lat), 1), "p95_ms": round(pct(lat, 0.95), 1),
            "p99_ms": round(pct(lat, 0.99), 1), "max_ms": round(max(lat), 1), "errors": err,
            "error_rate": round(err / len(lat), 4)}


# ------------------------------------------------------------------ Memory
def mem_once(names):
    r = subprocess.run(["sudo", "-n", "python3", os.path.join(TOOLS, "memsample.py"), "--once", *names],
                       capture_output=True, text=True)
    return json.loads(r.stdout)


def load_samples(path, t_from, t_to):
    out = []
    with open(path) as f:
        for line in f:
            try:
                s = json.loads(line)
            except ValueError:
                continue
            if t_from <= s["t"] <= t_to:
                out.append(s)
    return out


def window(path, t_from, t_to, appnames, dbname):
    """RAM (median/max, PSS and anon) and CPU seconds for a time window from the samples."""
    S = load_samples(path, t_from, t_to)
    if len(S) < 2:
        return {"samples": len(S)}

    def app(s, k):
        return sum(s["c"].get(a, {}).get(k, 0) for a in appnames)

    def db(s, k):
        return s["c"].get(dbname, {}).get(k, 0)
    MiB = 2 ** 20
    res = {"samples": len(S), "seconds": round(S[-1]["t"] - S[0]["t"], 1)}
    for k in ("pss", "anon"):
        a = [app(s, k) / MiB for s in S]
        d = [db(s, k) / MiB for s in S]
        res[f"app_{k}_median_mib"] = round(statistics.median(a), 1)
        res[f"app_{k}_max_mib"] = round(max(a), 1)
        res[f"db_{k}_median_mib"] = round(statistics.median(d), 1)
        res[f"db_{k}_max_mib"] = round(max(d), 1)
    res["cpu_s"] = {n: round((S[-1]["c"].get(n, {}).get("cpu_us", 0) - S[0]["c"].get(n, {}).get("cpu_us", 0)) / 1e6, 2)
                    for n in appnames + [dbname]}
    res["app_cpu_s"] = round(sum(res["cpu_s"][n] for n in appnames), 2)
    res["db_cpu_s"] = res["cpu_s"][dbname]
    # CPU peak (cores) over 10 s windows
    peak = 0.0
    j = 0
    for i in range(len(S)):
        while S[i]["t"] - S[j]["t"] > 10:
            j += 1
        dt = S[i]["t"] - S[j]["t"]
        if dt >= 5:
            used = sum(S[i]["c"].get(n, {}).get("cpu_us", 0) - S[j]["c"].get(n, {}).get("cpu_us", 0)
                       for n in appnames + [dbname]) / 1e6
            peak = max(peak, used / dt)
    res["cpu_peak_cores_10s"] = round(peak, 2)
    res["min_avail_mb"] = min(s["avail_mb"] for s in S)
    res["max_nproc_app"] = max(sum(s["c"].get(a, {}).get("nproc", 0) for a in appnames) for s in S)
    return res


def activity(path, t_from, t_to, names, bucket=30):
    """CPU seconds per 30 s section (shows background activity such as schedules)."""
    S = load_samples(path, t_from, t_to)
    out, j = [], 0
    for i in range(1, len(S)):
        if S[i]["t"] - S[j]["t"] >= bucket:
            out.append(round(sum(S[i]["c"].get(n, {}).get("cpu_us", 0) - S[j]["c"].get(n, {}).get("cpu_us", 0)
                                 for n in names) / 1e6, 3))
            j = i
    return out


def vol_sizes(vols):
    out = {}
    for v in vols:
        mp = docker("volume", "inspect", "-f", "{{.Mountpoint}}", v, check=False)
        if mp:
            r = subprocess.run(["sudo", "-n", "du", "-sb", mp], capture_output=True, text=True)
            out[v] = int(r.stdout.split()[0]) if r.stdout else None
            sub = subprocess.run(["sudo", "-n", "du", "-sb", "--max-depth=1", mp], capture_output=True, text=True)
            out[v + "/detail"] = {l.split("\t")[1].replace(mp, "") or "/": int(l.split("\t")[0])
                                  for l in sub.stdout.splitlines() if "\t" in l}
    return out


def processes(name):
    return docker("top", name, "-o", "pid,rss,args", check=False).splitlines()[:40]


# ------------------------------------------------------------------ Ingestion via the inbox folder
def drop_files(files):
    os.makedirs(STAGING, exist_ok=True)
    for f in files:
        dst = os.path.join(STAGING, os.path.basename(f))
        shutil.copy(f, dst)
        os.chmod(dst, 0o666)
    t0 = time.time()
    for f in files:
        os.rename(os.path.join(STAGING, os.path.basename(f)), os.path.join(CONSUME, os.path.basename(f)))
    return t0


def wait_count(api, expected, t0, abort_file, stall=1200, total=6 * 3600):
    timeline, last, last_change = [], -1, time.time()
    while True:
        time.sleep(2)
        c = api.count()
        now = time.time()
        if c != last:
            timeline.append((round(now - t0, 1), c))
            last, last_change = c, now
        if c >= expected:
            break
        if os.path.exists(abort_file):
            raise RuntimeError("Abbruch: RAM knapp")
        if now - last_change > stall or now - t0 > total:
            log(f"Einlesen hängt bei {c}/{expected}")
            break
    return {"seconds": round(time.time() - t0, 2), "final_count": last, "expected": expected,
            "timeline": timeline, "left_in_consume": len(os.listdir(CONSUME))}


def per_type_times(api):
    """Processing time per document kind from the 'added' timestamps (worker works one after the other)."""
    rows, page = [], 1
    while True:
        st, d = api.json("GET", f"/api/documents/?page={page}&page_size=1000&fields=id,original_file_name,added")
        if st != 200 or not isinstance(d, dict):
            return {"error": f"http {st}"}
        rows += d.get("results", [])
        if not d.get("next"):
            break
        page += 1
    from datetime import datetime
    pts = []
    for r in rows:
        try:
            pts.append((datetime.fromisoformat(r["added"].replace("Z", "+00:00")).timestamp(),
                        (r.get("original_file_name") or "?")[:1]))
        except (KeyError, ValueError, AttributeError, TypeError):
            pass
    pts.sort()
    sums = {}
    for (t_prev, _), (t, kind) in zip(pts, pts[1:]):
        sums.setdefault(kind, []).append(t - t_prev)
    return {k: {"n": len(v), "sum_s": round(sum(v), 1), "median_s": round(statistics.median(v), 2)}
            for k, v in sums.items()} | {"rows": len(rows)}


def tasks_summary(api):
    try:
        st, d = api.json("GET", "/api/tasks/?page_size=2000")
    except (OSError, http.client.HTTPException):
        return {"error": "keine Antwort"}
    items = d if isinstance(d, list) else (d or {}).get("results", []) if isinstance(d, dict) else []
    summ, fails = {}, []
    for t in items:
        s = t.get("status")
        summ[s] = summ.get(s, 0) + 1
        if s == "FAILURE":
            fails.append({"file": t.get("task_file_name"), "result": str(t.get("result"))[:200]})
    return {"http": st, "by_status": summ, "failures": fails[:20]}


# ------------------------------------------------------------------ User simulation
class Recorder:
    def __init__(self):
        self.lock = threading.Lock()
        self.recs = []  # (t, scenario, action, key, latency_s, status)

    def add(self, *r):
        with self.lock:
            self.recs.append(r)


class Uploads:
    """Unique documents for uploading (extra/e00000…e00999)."""

    def __init__(self):
        self.lock = threading.Lock()
        self.i = 0
        self.dir = os.path.join(DOCS, "extra")

    def next(self):
        with self.lock:
            i = self.i
            self.i += 1
        return os.path.join(self.dir, f"e{i % 1000:05d}.pdf")


class User:
    def __init__(self, api, rec, scen, seed, uploads, ctx):
        self.api, self.rec, self.scen, self.uploads, self.ctx = api, rec, scen, uploads, ctx
        self.rng = random.Random(seed)
        self.conn = None

    def call(self, action, key, method, path, body=None, ctype=None):
        if self.conn is None:
            self.conn = http.client.HTTPConnection("127.0.0.1", PORT, timeout=300)
        t = time.perf_counter()
        try:
            st, data = self.api.req(method, path, body, {"Content-Type": ctype} if ctype else None, conn=self.conn)
        except (OSError, http.client.HTTPException):
            self.conn.close()
            self.conn = None
            st, data = -1, b""
        self.rec.add(time.time(), self.scen, action, key, time.perf_counter() - t, st)
        try:
            return st, json.loads(data) if data and data[:1] in (b"{", b"[") else None
        except ValueError:
            return st, None

    def ids(self):
        return self.ctx["ids"]

    def session_start(self):
        self.call("start", "ui_settings", "GET", "/api/ui_settings/")
        for ep in ("tags", "correspondents", "document_types", "storage_paths", "custom_fields"):
            self.call("start", ep, "GET", f"/api/{ep}/?page=1&page_size=100000")
        self.dashboard()

    def dashboard(self):
        self.call("dashboard", "saved_views", "GET", "/api/saved_views/?page=1&page_size=100000")
        self.call("dashboard", "statistics", "GET", "/api/statistics/")
        self.call("dashboard", "recent", "GET",
                  "/api/documents/?page=1&page_size=10&ordering=-created&truncate_content=true")
        self.call("dashboard", "tasks", "GET", "/api/tasks/?acknowledged=false")

    def doc_list(self):
        n = max(1, self.ctx.get("count", 230))
        page = self.rng.randint(1, max(1, math.ceil(n / 50)))
        self.call("list", "documents_page", "GET",
                  f"/api/documents/?page={page}&page_size=50&ordering=-created&truncate_content=true")

    def doc_filter(self):
        r = self.rng.random()
        if r < 0.3 and self.ctx.get("tag"):
            q = f"tags__id__all={self.ctx['tag']}"
            key = "filter_tag"
        elif r < 0.6:
            q = "title_content__icontains=" + urllib.parse.quote(self.rng.choice(WORDS[:16]))
            key = "filter_text"
        elif r < 0.85:
            y = self.rng.choice([2023, 2024, 2025, 2026])
            q = f"created__date__gte={y}-01-01&created__date__lte={y}-12-31"
            key = "filter_date"
        else:
            q = "document_type__isnull=1&ordering=title"
            key = "filter_type"
        self.call("filter", key, "GET", f"/api/documents/?page=1&page_size=50&{q}&truncate_content=true")

    def search(self):
        w = self.rng.choice(WORDS)
        self.call("search", "autocomplete", "GET", f"/api/search/autocomplete/?term={urllib.parse.quote(w[:4])}&limit=10")
        self.call("search", "fulltext", "GET",
                  f"/api/documents/?query={urllib.parse.quote(w)}&page=1&page_size=50&truncate_content=true")

    def open_doc(self):
        ids = self.ids()
        if not ids:
            return
        i = self.rng.choice(ids)
        self.call("open", "detail", "GET", f"/api/documents/{i}/")
        self.call("open", "metadata", "GET", f"/api/documents/{i}/metadata/")
        self.call("open", "notes", "GET", f"/api/documents/{i}/notes/")
        self.call("open", "suggestions", "GET", f"/api/documents/{i}/suggestions/")
        self.call("open", "thumb", "GET", f"/api/documents/{i}/thumb/")
        self.call("open", "preview", "GET", f"/api/documents/{i}/preview/")

    def edit(self):
        ids = self.ids()
        if not ids:
            return
        i = self.rng.choice(ids)
        st, d = self.call("edit", "detail", "GET", f"/api/documents/{i}/")
        tags = list(d.get("tags", [])) if isinstance(d, dict) else []
        t = self.ctx.get("tag")
        if t:
            tags = [x for x in tags if x != t] if t in tags else tags + [t]
        body = json.dumps({"title": f"Geändert {self.rng.randint(1, 99999)}", "tags": tags}).encode()
        self.call("edit", "patch", "PATCH", f"/api/documents/{i}/", body, "application/json")

    def upload(self):
        for _ in range(self.rng.randint(1, 3)):
            p = self.uploads.next()
            body, ct = multipart([("title", "Upload " + os.path.basename(p))],
                                 [("document", os.path.basename(p), open(p, "rb").read())])
            self.call("upload", "post_document", "POST", "/api/documents/post_document/", body, ct)

    def run(self, until, think=(2.0, 8.0)):
        self.session_start()
        acts = [(self.dashboard, 1), (self.doc_list, 3), (self.doc_filter, 2), (self.search, 2),
                (self.open_doc, 3), (self.edit, 1), (self.upload, 0.4)]
        total = sum(w for _, w in acts)
        while time.time() < until:
            time.sleep(self.rng.uniform(*think))
            if time.time() >= until:
                break
            r = self.rng.uniform(0, total)
            for fn, w in acts:
                r -= w
                if r <= 0:
                    fn()
                    break
        if self.conn:
            self.conn.close()


def refresh_ctx(api, ctx, stop):
    while not stop.is_set():
        ids = api.all_ids()
        if ids:
            ctx["ids"], ctx["count"] = ids, len(ids)
        stop.wait(30)


def simulate(api, rec, scen, users, seconds, uploads, ctx, seed):
    until = time.time() + seconds
    th = [threading.Thread(target=User(api, rec, scen, seed * 100 + u, uploads, ctx).run, args=(until,))
          for u in range(users)]
    for i, t in enumerate(th):
        t.start()
        time.sleep(0.5)  # staggered start
    for t in th:
        t.join()


def summarize_recs(recs, scen):
    R = [r for r in recs if r[1] == scen]
    out = {"all": lat_stats([(r[4], r[5]) for r in R])}
    keys = sorted({(r[2], r[3]) for r in R})
    out["by_endpoint"] = {f"{a}/{k}": lat_stats([(r[4], r[5]) for r in R if r[2] == a and r[3] == k])
                          for a, k in keys}
    out["status_codes"] = {}
    for r in R:
        out["status_codes"][str(r[5])] = out["status_codes"].get(str(r[5]), 0) + 1
    # Reading requests without upload (upload is deliberately slow/asynchronous)
    out["reads"] = lat_stats([(r[4], r[5]) for r in R if r[2] not in ("upload", "edit")])
    return out


# ------------------------------------------------------------------ Load measurements
class Probe(threading.Thread):
    """A user who alternately queries list and search every second (usability under load)."""

    def __init__(self, api, rec, scen):
        super().__init__(daemon=True)
        self.api, self.rec, self.scen = api, rec, scen
        self.stop = threading.Event()

    def run(self):
        c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=300)
        i = 0
        while not self.stop.is_set():
            if i % 2 == 0:
                key, path = "list", "/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true"
            else:
                key, path = "search", "/api/documents/?query=rechnung&page=1&page_size=25&truncate_content=true"
            t = time.perf_counter()
            try:
                st, _ = self.api.req("GET", path, conn=c)
            except (OSError, http.client.HTTPException):
                c.close()
                c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=300)
                st = -1
            dt = time.perf_counter() - t
            self.rec.add(time.time(), self.scen, "probe", key, dt, st)
            i += 1
            self.stop.wait(max(0.0, 1.0 - dt))
        c.close()


def timed(api, n, fn):
    lat = []
    for i in range(n):
        t = time.perf_counter()
        st = fn(i)
        lat.append((time.perf_counter() - t, st))
    return lat_stats(lat) | {"lat_ms": [round(x[0] * 1000, 1) for x in lat]}


def bulk_and_select(api, ids, reps):
    st, d = api.json("POST", "/api/tags/", {"name": "prbench-bulk", "matching_algorithm": 0})
    tag = d.get("id") if isinstance(d, dict) else None
    res = {"tag_create_http": st, "docs": len(ids)}

    def bulk(i):
        return api.json("POST", "/api/documents/bulk_edit/",
                        {"documents": ids, "method": "add_tag" if i % 2 == 0 else "remove_tag",
                         "parameters": {"tag": tag}})[0]
    res["bulk_edit_tag_all"] = timed(api, reps, bulk)

    # "Select all" like the UI: list (field "all" with all IDs) + selection_data for all
    def sel_list(i):
        return api.req("GET", "/api/documents/?page=1&page_size=50&ordering=-created&truncate_content=true")[0]

    def sel_data(i):
        return api.json("POST", "/api/documents/selection_data/", {"documents": ids})[0]
    res["select_all_list"] = timed(api, reps, sel_list)
    res["select_all_selection_data"] = timed(api, reps, sel_data)
    st, d = api.json("GET", "/api/documents/?page=1&page_size=25")
    res["list_has_all_field"] = isinstance(d, dict) and isinstance(d.get("all"), list)
    return res, tag


def micro(api, ids, n, conc):
    """Single endpoints under continuous fire (without pauses), for the scaling measurement."""
    q = urllib.parse.quote
    eps = {
        "list_page1": ["/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true"],
        "list_deep_page": ["/api/documents/?page=%d&page_size=25&ordering=-created&truncate_content=true"
                           % max(1, len(ids) // 50)],
        "search_common": [f"/api/documents/?query={q('rechnung')}&page=1&page_size=25&truncate_content=true"],
        "search_rare": [f"/api/documents/?query={q('zwetschgenröster')}&page=1&page_size=25&truncate_content=true"],
        "filter_text": [f"/api/documents/?title_content__icontains={q('heizöl')}&page=1&page_size=25&truncate_content=true"],
        "filter_date": ["/api/documents/?created__date__gte=2025-01-01&created__date__lte=2025-12-31&page=1&page_size=25&truncate_content=true"],
        "detail": [f"/api/documents/{i}/" for i in ids[:500]],
        "thumb": [f"/api/documents/{i}/thumb/" for i in ids[:500]],
    }
    res = {"hits": {}}
    for k in ("search_common", "search_rare", "filter_text", "filter_date"):
        st, d = api.json("GET", eps[k][0])
        res["hits"][k] = d.get("count") if isinstance(d, dict) else f"http {st}"
    for name, paths in eps.items():
        res[name] = {}
        for c in conc:
            run_load(api, paths, 10, c)
            r = run_load(api, paths, n if c == 1 else max(n, 30 * c), c)
            res[name][f"c{c}"] = r
            log(f"  {name} c{c}: median {r['median_ms']} ms, p95 {r['p95_ms']} ms, codes {r['codes']}")
    return res


def run_load(api, paths, n, conc):
    recs = []
    lock = threading.Lock()
    idx = [0]

    def worker():
        c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=300)
        while True:
            with lock:
                i = idx[0]
                idx[0] += 1
            if i >= n:
                break
            t = time.perf_counter()
            try:
                st, _ = api.req("GET", paths[i % len(paths)], conn=c)
            except (OSError, http.client.HTTPException):
                c.close()
                c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=300)
                st = -1
            with lock:
                recs.append((time.perf_counter() - t, st))
        c.close()
    t0 = time.perf_counter()
    th = [threading.Thread(target=worker) for _ in range(conc)]
    for t in th:
        t.start()
    for t in th:
        t.join()
    s = lat_stats(recs)
    s["rps"] = round(len(recs) / (time.perf_counter() - t0), 2)
    s["codes"] = {}
    for _, st in recs:
        s["codes"][str(st)] = s["codes"].get(str(st), 0) + 1
    return s



# ------------------------------------------------------------------ S7 ingestion in detail, OCR quality
import re  # noqa: E402

TOKEN = re.compile(r"[0-9a-zäöüß]{4,}")


def word_recall(expected_text, content):
    exp = set(TOKEN.findall((expected_text or "").lower()))
    got = set(TOKEN.findall((content or "").lower()))
    if not exp:
        return None
    return round(len(exp & got) / len(exp), 3)


def search_ids(api, word):
    try:
        st, d = api.json("GET", f"/api/documents/?query={urllib.parse.quote(word)}&page_size=5&fields=id")
    except (OSError, http.client.HTTPException):
        return []
    if st != 200 or not isinstance(d, dict):
        return []
    return [x["id"] for x in d.get("results", [])]


def failed_task_for(api, fname):
    t = tasks_summary(api)
    return any((f.get("file") or "") == fname for f in t.get("failures", []))


def s7_detail(api, samples, appnames, dbname, smoke, system=""):
    man = json.load(open(os.path.join(DOCS, "detail", "manifest.json"), encoding="utf-8"))
    names = sorted(man)
    if smoke:
        names = [n for n in names if n.endswith("_00.pdf") or n.endswith("_00.jpg") or n.endswith("_00.docx")]
    out = {"docs": {}}
    for name in names:
        e = man[name]
        if system == "ngxa" and e["kind"] == "d_office":
            # ngx without Tika/Gotenberg does not know DOCX (file would be ignored in the inbox folder)
            out["docs"][name] = {"kind": e["kind"], "searchable_s": None, "unsupported": True, "window": {}}
            continue
        word = e["words"][-1]  # with 8 pages: word on the last page
        n0 = api.count()
        t0 = drop_files([os.path.join(DOCS, "detail", name)])
        r = {"kind": e["kind"], "searchable_s": None, "in_list_s": None, "failed": False, "t_drop": round(t0, 3)}
        last_task_check = time.time()
        while time.time() - t0 < (900 if not smoke else 300):
            time.sleep(0.5)
            now = time.time()
            if r["in_list_s"] is None and api.count() > n0:
                r["in_list_s"] = round(now - t0, 2)
            ids = search_ids(api, word)
            if ids:
                r["searchable_s"] = round(time.time() - t0, 2)
                r["doc_id"] = ids[0]
                break
            if now - last_task_check > 10:
                last_task_check = now
                if failed_task_for(api, name):
                    r["failed"] = True
                    break
        r["window"] = window(samples, t0, time.time(), appnames, dbname)
        if r.get("doc_id"):
            st, d = api.json("GET", f"/api/documents/{r['doc_id']}/")
            content = d.get("content", "") if isinstance(d, dict) else ""
            r["word_recall"] = word_recall(e["text"], content)
            r["all_words_found"] = all(search_ids(api, w) for w in e["words"])
            r["content_chars"] = len(content)
            st, m = api.json("GET", f"/api/documents/{r['doc_id']}/metadata/")
            if isinstance(m, dict):
                r["has_archive"] = m.get("has_archive_version")
                r["archive_size"] = m.get("archive_size")
                r["original_size"] = m.get("original_size")
        elif not r["failed"]:
            r["timeout"] = True
            r["failed"] = failed_task_for(api, name)
        out["docs"][name] = r
        log(f"  S7 {name}: durchsuchbar nach {r['searchable_s']} s, Wortquote {r.get('word_recall')}, "
            f"Archiv {r.get('has_archive')}, fehlgeschlagen {r['failed']}")
        time.sleep(3)
    # Summary per kind
    groups = {}
    for name, r in out["docs"].items():
        g = r["kind"] if r["kind"] != "c_photo" else ("c_photo_png" if name.endswith(".png") else "c_photo_jpg")
        groups.setdefault(g, []).append(r)
    out["by_kind"] = {}
    for g, rs in groups.items():
        ok = [x["searchable_s"] for x in rs if x["searchable_s"] is not None]
        rec = [x["word_recall"] for x in rs if x.get("word_recall") is not None]
        out["by_kind"][g] = {"n": len(rs), "ok": len(ok),
                             "median_s": round(statistics.median(ok), 2) if ok else None,
                             "p95_s": round(pct(ok, 0.95), 2) if ok else None,
                             "max_s": round(max(ok), 2) if ok else None,
                             "word_recall_median": round(statistics.median(rec), 3) if rec else None,
                             "peak_app_pss_mib": max((x["window"].get("app_pss_max_mib", 0) for x in rs), default=0),
                             "peak_cpu_cores": max((x["window"].get("cpu_peak_cores_10s", 0) for x in rs), default=0)}
    return out


def scan_quality(api):
    """OCR quality of the 30 scans from S4: word ratio content against reference text."""
    man = json.load(open(os.path.join(DOCS, "scan_manifest.json"), encoding="utf-8"))
    rows, page = [], 1
    while True:
        st, d = api.json("GET", f"/api/documents/?page={page}&page_size=1000&fields=id,original_file_name")
        if st != 200 or not isinstance(d, dict):
            break
        rows += d.get("results", [])
        if not d.get("next"):
            break
        page += 1
    rec = {}
    for r in rows:
        fn = r.get("original_file_name")
        if fn in man:
            st, d = api.json("GET", f"/api/documents/{r['id']}/")
            if isinstance(d, dict):
                rec[fn] = word_recall(man[fn]["text"], d.get("content", ""))
    v = [x for x in rec.values() if x is not None]
    return {"n": len(v), "median": round(statistics.median(v), 3) if v else None,
            "min": min(v) if v else None, "per_doc": rec}


# ------------------------------------------------------------------ Procedure
def teardown(sp):
    for n in sp["names"]:
        docker("rm", "-f", n, check=False)
    for v in sp["volumes"]:
        docker("volume", "rm", v, check=False)


def start_system(sp, api, pw, res):
    teardown(sp)
    docker("network", "create", NET, check=False)
    docker(*sp["db"])
    t0 = time.time()

    def ready():
        return subprocess.run(["sudo", "-n", "docker", "exec", sp["dbname"], "pg_isready", "-U", "paperless",
                               "-d", "paperless", "-h", "127.0.0.1"], capture_output=True).returncode == 0
    while not ready():
        time.sleep(0.5)
    time.sleep(2)
    while not ready():
        time.sleep(0.5)
    res["db_ready_s"] = round(time.time() - t0, 1)
    for e in sp["extras"]:
        docker(*e)
    log("Erststart")
    t0 = time.time()
    docker(*sp["app"])
    res["first_start_http_s"] = round(wait_http(api, "/api/", {200, 302, 401, 403}, timeout=1800), 2)
    get_token(api, pw)
    res["first_start_s"] = round(time.time() - t0, 2)
    log(f"  Erststart: HTTP nach {res['first_start_http_s']} s, Anmeldung möglich nach {res['first_start_s']} s")


def prepare(tag):
    os.makedirs(RESULTS, exist_ok=True)
    os.makedirs(CONSUME, exist_ok=True)
    # the ngx container takes over the inbox folder (UID 1000) – take it back and empty it
    subprocess.run(["sudo", "-n", "chown", f"{os.getuid()}:{os.getgid()}", CONSUME], check=False)
    subprocess.run(["sudo", "-n", "find", CONSUME, "-mindepth", "1", "-delete"], check=False)
    os.chmod(CONSUME, 0o777)
    samples = os.path.join(RESULTS, f"samples_{tag}.jsonl")
    for suffix in ("", ".stop", ".abort"):
        if os.path.exists(samples + suffix):
            os.remove(samples + suffix)
    with open(LOCK, "w") as f:
        f.write(f"{os.getpid()} {time.strftime('%Y-%m-%dT%H:%M:%S')} PAPERRUST BENCHMARK {tag}\n")
    while mem_avail() < 2000:
        log(f"warte auf RAM ({mem_avail()} MB)")
        time.sleep(30)
    return samples


def finish(sp, res, out, sampler, samples):
    if sampler:
        open(samples + ".stop", "w").close()
        try:
            sampler.wait(timeout=30)
        except subprocess.TimeoutExpired:
            pass
    try:
        info = json.loads(docker("inspect", sp["appname"], check=False) or "[]")
        if info:
            res["app_state"] = {k: info[0]["State"].get(k) for k in ("OOMKilled", "ExitCode", "Restarting")}
            res["app_restart_count"] = info[0].get("RestartCount")
        logs = subprocess.run(["sudo", "-n", "docker", "logs", "-t", sp["appname"]], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True).stdout.splitlines()
        # save the complete app log with timestamps (breakdown of the ingestion times)
        with open(out.replace(".json", "_applog.txt"), "w") as f:
            f.write(chr(10).join(logs) + chr(10))
        res["app_log_lines"] = len(logs)
        res["app_log_errors"] = [l[:300] for l in logs if " ERROR" in l or "Traceback" in l or "panicked" in l][:60]
    except Exception as e:  # noqa: BLE001
        res["finish_error"] = str(e)
    t = time.time()
    docker("stop", sp["appname"], check=False)
    res["final_stop_s"] = round(time.time() - t, 2)
    res["oom_dmesg_end"] = oom_count()
    teardown(sp)
    with open(out, "w") as f:
        json.dump(res, f, indent=1, ensure_ascii=False)
    if os.path.exists(LOCK):
        os.remove(LOCK)
    log("fertig", out)


def run(rnd, system, smoke, short=False):
    tag = f"{rnd}_{system}"
    samples = prepare(tag)
    cpus = "0-1" if foreign_mpc() else "0-3"
    pw = admin_pw()
    sp = specs(system, cpus, pw)
    appnames = [n for n in sp["names"] if n != sp["dbname"]]
    res = {"round": rnd, "system": system, "smoke": smoke, "short": short, "cpuset": cpus, "images": [IMAGES[x] for x in SYSTEM_IMAGES[system]],
           "conditions_start": conditions(), "oom_dmesg_start": oom_count()}
    out = os.path.join(RESULTS, f"run_{tag}.json")
    api = Api()
    sampler = None
    rec = Recorder()
    k = (lambda full, small: small if smoke else full)
    try:
        start_system(sp, api, pw, res)
        sampler = subprocess.Popen(["sudo", "-n", "python3", os.path.join(TOOLS, "memsample.py"), samples, "2",
                                    *sp["names"]])
        # ---------------- S1 idle
        time.sleep(15)
        res["s1"] = {"restart_s": [], "stop_s": []}
        for i in range(3):
            t = time.time()
            docker("stop", sp["appname"])
            res["s1"]["stop_s"].append(round(time.time() - t, 2))
            time.sleep(5)
            t = time.time()
            docker("start", sp["appname"])
            wait_http(api, "/api/documents/?page_size=1", {200}, timeout=900)
            res["s1"]["restart_s"].append(round(time.time() - t, 2))
            log(f"  Neustart {i + 1}: {res['s1']['restart_s'][-1]} s")
            time.sleep(10)
        res["s1"]["processes_start"] = {n: processes(n) for n in appnames}
        res["s1"]["conditions"] = conditions()
        t0 = time.time()
        log("S1 Leerlauf 10 min")
        time.sleep(k(60 if short else 600, 30))
        t1 = time.time()
        res["s1"]["idle"] = window(samples, t0, t1, appnames, sp["dbname"])
        res["s1"]["activity_app_cpu_s_per_30s"] = activity(samples, t0, t1, appnames)
        res["s1"]["activity_db_cpu_s_per_30s"] = activity(samples, t0, t1, [sp["dbname"]])
        res["s1"]["processes_end"] = {n: processes(n) for n in appnames}
        log(f"  Leerlauf: App {res['s1']['idle'].get('app_pss_median_mib')} MiB, CPU {res['s1']['idle'].get('app_cpu_s')} s")
        # ---------------- S7 ingestion in detail (one document after the other, quiet system)
        log("S7 Einlesen je Art")
        res["s7"] = s7_detail(api, samples, appnames, sp["dbname"], smoke, system)
        res["s7"]["tasks"] = tasks_summary(api)
        time.sleep(10)
        # ---------------- S4 heavy load: import + operation
        text = sorted(os.path.join(DOCS, "text", f) for f in os.listdir(os.path.join(DOCS, "text")))
        scan = sorted(os.path.join(DOCS, "scan", f) for f in os.listdir(os.path.join(DOCS, "scan")))
        if smoke:
            text, scan = text[:6], scan[:2]
        files = scan + text
        res["s4"] = {"docs": len(files), "conditions": conditions()}
        log(f"S4 Import {len(files)} Dokumente + Bedienung")
        probe = Probe(api, rec, "s4_during")
        probe.start()
        n_before = api.count()
        res["s4"]["docs_before"] = n_before
        t_imp = drop_files(files)
        try:
            res["s4"]["import"] = wait_count(api, n_before + len(files), t_imp, samples + ".abort")
        finally:
            probe.stop.set()
            probe.join()
        t_imp_end = time.time()
        log(f"  Import: {res['s4']['import']['seconds']} s, {res['s4']['import']['final_count']} Dokumente")
        res["s4"]["import_window"] = window(samples, t_imp, t_imp_end, appnames, sp["dbname"])
        res["s4"]["per_type"] = per_type_times(api)
        res["s4"]["scan_ocr_quality"] = scan_quality(api)
        log(f"  OCR-Wortquote Scans: {res['s4']['scan_ocr_quality'].get('median')}")
        time.sleep(20)
        res["s4"]["tasks"] = tasks_summary(api)
        probe = Probe(api, rec, "s4_after")
        probe.start()
        time.sleep(k(60, 10))
        probe.stop.set()
        probe.join()
        res["s4"]["probe_during"] = summarize_recs(rec.recs, "s4_during")
        res["s4"]["probe_after"] = summarize_recs(rec.recs, "s4_after")
        ids = api.all_ids()
        t_b = time.time()
        bs, tag_id = bulk_and_select(api, ids, k(30, 4))
        res["s4"]["bulk_select"] = bs
        res["s4"]["bulk_window"] = window(samples, t_b, time.time(), appnames, sp["dbname"])
        log(f"  Massenbearbeitung median {bs['bulk_edit_tag_all'].get('median_ms')} ms, "
            f"selection_data {bs['select_all_selection_data'].get('median_ms')} ms")
        # ---------------- S6 disk + idle with documents
        time.sleep(k(60, 5))
        t0 = time.time()
        time.sleep(k(60, 10))
        res["s6"] = {"volumes_bytes": vol_sizes(sp["volumes"]),
                     "idle_with_docs": window(samples, t0, time.time(), appnames, sp["dbname"])}
        # ---------------- S2 everyday, S3 office
        ctx = {"ids": ids, "count": len(ids), "tag": tag_id}
        stop = threading.Event()
        rt = threading.Thread(target=refresh_ctx, args=(api, ctx, stop), daemon=True)
        rt.start()
        uploads = Uploads()
        for scen, users, secs in ((() if short else (("s2_1user", 1, k(900, 40)), ("s3_5users", 5, k(300, 30)),
                                  ("s3_10users", 10, k(300, 30))))):
            log(f"{scen}: {users} Nutzer, {secs} s")
            n0 = api.count()
            t0 = time.time()
            simulate(api, rec, scen, users, secs, uploads, ctx, seed=int(rnd) if str(rnd).isdigit() else 7)
            t1 = time.time()
            res[scen] = summarize_recs(rec.recs, scen)
            res[scen]["window"] = window(samples, t0, t1, appnames, sp["dbname"])
            res[scen]["docs_before"], res[scen]["docs_after"] = n0, api.count()
            a = res[scen]["all"]
            log(f"  {scen}: n={a.get('n')} median {a.get('median_ms')} ms p95 {a.get('p95_ms')} ms "
                f"p99 {a.get('p99_ms')} ms Fehler {a.get('errors')}")
            time.sleep(k(30, 5))
        stop.set()
        res["uploads_used"] = uploads.i
        res["requests_raw"] = [(round(r[0], 2), r[1], r[2], r[3], round(r[4] * 1000, 1), r[5]) for r in rec.recs]
        res["conditions_end"] = conditions()
    except Exception as e:  # noqa: BLE001 – errors belong in the raw data
        res["error"] = f"{type(e).__name__}: {e}"
        log("FEHLER", res["error"])
        res["app_log_tail"] = subprocess.run(["sudo", "-n", "docker", "logs", "--tail", "60", sp["appname"]],
                                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                             text=True).stdout[-6000:]
    finally:
        finish(sp, res, out, sampler, samples)


def big(system, n, smoke):
    """S5: enlarge archive (230 test documents + n unique text PDFs), then measure list/search/filter."""
    tag = f"big_{system}"
    samples = prepare(tag)
    cpus = "0-1" if foreign_mpc() else "0-3"
    pw = admin_pw()
    sp = specs(system, cpus, pw)
    appnames = [x for x in sp["names"] if x != sp["dbname"]]
    res = {"system": system, "smoke": smoke, "cpuset": cpus, "extra_docs": n, "conditions_start": conditions(),
           "oom_dmesg_start": oom_count()}
    out = os.path.join(RESULTS, f"{tag}.json")
    api = Api()
    sampler = None
    try:
        start_system(sp, api, pw, res)
        sampler = subprocess.Popen(["sudo", "-n", "python3", os.path.join(TOOLS, "memsample.py"), samples, "5",
                                    *sp["names"]])
        text = sorted(os.path.join(DOCS, "text", f) for f in os.listdir(os.path.join(DOCS, "text")))
        scan = sorted(os.path.join(DOCS, "scan", f) for f in os.listdir(os.path.join(DOCS, "scan")))
        extra = [os.path.join(DOCS, "extra", f"e{i:05d}.pdf") for i in range(1000, 1000 + n)]
        stages = []
        base = scan + text
        if smoke:
            base = text[:5]
        t0 = drop_files(base)
        wait_count(api, len(base), t0, samples + ".abort")
        stages.append(("s230", []))
        # Stages: first half the amount, then the full amount (scaling visible)
        half = n // 2
        stages += [(f"s{len(base) + half}", extra[:half]), (f"s{len(base) + n}", extra[half:])]
        res["stages"] = {}
        expected = len(base)
        for name, files in stages:
            st = {}
            if files:
                expected += len(files)
                log(f"S5 Import bis {expected}")
                # refill in packages of 500 (do not flood the inbox folder with thousands of files)
                t0 = time.time()
                for i in range(0, len(files), 500):
                    drop_files(files[i:i + 500])
                    st.setdefault("chunks", []).append(wait_count(api, expected - len(files) + i + len(files[i:i + 500]),
                                                                   time.time(), samples + ".abort"))
                st["import_seconds"] = round(time.time() - t0, 1)
                st["import_window"] = window(samples, t0, time.time(), appnames, sp["dbname"])
            time.sleep(30)
            ids = api.all_ids()
            st["count"] = len(ids)
            st["conditions"] = conditions()
            t0 = time.time()
            st["micro"] = micro(api, ids, 100 if not smoke else 10, (1, 8))
            st["micro_window"] = window(samples, t0, time.time(), appnames, sp["dbname"])
            st["volumes_bytes"] = vol_sizes(sp["volumes"])
            res["stages"][name] = st
            with open(out, "w") as f:
                json.dump(res, f, indent=1, ensure_ascii=False)
    except Exception as e:  # noqa: BLE001
        res["error"] = f"{type(e).__name__}: {e}"
        log("FEHLER", res["error"])
    finally:
        finish(sp, res, out, sampler, samples)


if __name__ == "__main__":
    smoke = "--smoke" in sys.argv
    short = "--short" in sys.argv
    args = [a for a in sys.argv[1:] if a not in ("--smoke", "--short")]
    if args[0] == "run":
        run(args[1], args[2], smoke, short)
    elif args[0] == "big":
        big(args[1], int(args[2]), smoke)
    else:
        sys.exit(__doc__)
