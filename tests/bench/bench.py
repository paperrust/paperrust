"""PaperRust vs. Django: startup time, memory, throughput, response times.

Invocation (Git Bash, after `source reference/env.sh`; the script starts DB copy and server itself):
    "$PY" -I tests/bench/bench.py --rust <paperrust.exe> --out results/bench/<date>

Procedure per candidate (one after the other, never simultaneously):
  1. Start process, time until the first API response (GET /api/ui_settings/ with token) returns 200 = startup time.
  2. Warm up, then per endpoint N seconds with C parallel keep-alive clients (token auth).
  3. Memory (working set + private bytes) of all processes of the candidate: when idle after
     start and as maximum during load (sampling every 0.5 s).
Django: granian like reference/django-start.sh (workers via --dj-workers). Celery/Redis are NOT
counted (additionally needed in Docker) – noted in the report.
PaperRust: `paperrust serve` (Django off) and optionally `serve --worker` (incl. ingestion worker).
Only stdlib + PowerShell (memory). Result: results.json + summary.md (medians per candidate).

A/B comparison of several Rust builds (O1), interleaved, median over the rounds:
    "$PY" -I tests/bench/bench.py --only rust --no-rust-worker --rounds 3 \\
        --cand b0=<old.exe> --cand o1=<new.exe> --cand o1w4=<new.exe>@TOKIO_WORKER_THREADS=4 \\
        --out results/bench/<date>-o1
    `--startup-only N`: only N starts per candidate (time to the first API response, no load).
Data folder of the Rust candidates: results/bench/work (isolated from the reference; the index is there).
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
REF = REPO / "reference"

ENDPOINTS = [
    ("documents_list", "/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true"),
    ("document_retrieve", "/api/documents/1/"),
    ("tags_all", "/api/tags/?page=1&page_size=100000"),
    ("correspondents_all", "/api/correspondents/?page=1&page_size=100000"),
    ("statistics", "/api/statistics/"),
    ("saved_views", "/api/saved_views/?page=1&page_size=100000"),
    ("ui_settings", "/api/ui_settings/"),
]


def admin_token() -> str:
    users = json.loads((REF / "users.json").read_text(encoding="utf-8"))["users"]
    for u in users:
        if u.get("username") == "admin":
            return u["token"]
    raise SystemExit("kein admin in reference/users.json")


def proc_tree_mem(root_pid: int) -> tuple[float, float]:
    """(working set MB, private MB) of the process and all descendants."""
    ps = (
        "$all = Get-CimInstance Win32_Process | Select ProcessId,ParentProcessId;"
        f"$ids = @({root_pid}); $grow = $true;"
        "while ($grow) { $grow = $false; foreach ($p in $all) { if ($ids -contains $p.ParentProcessId -and -not ($ids -contains $p.ProcessId)) { $ids += $p.ProcessId; $grow = $true } } };"
        "$ws = 0; $pv = 0; foreach ($i in $ids) { $q = Get-Process -Id $i -ErrorAction SilentlyContinue; if ($q) { $ws += $q.WorkingSet64; $pv += $q.PrivateMemorySize64 } };"
        "Write-Output \"$ws $pv\""
    )
    out = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
                         capture_output=True, text=True, timeout=60).stdout.strip().split()
    return int(out[0]) / 2**20, int(out[1]) / 2**20


class Sampler(threading.Thread):
    def __init__(self, pid: int):
        super().__init__(daemon=True)
        self.pid, self.stop, self.max_ws, self.max_pv = pid, threading.Event(), 0.0, 0.0

    def run(self):
        while not self.stop.is_set():
            try:
                ws, pv = proc_tree_mem(self.pid)
                self.max_ws, self.max_pv = max(self.max_ws, ws), max(self.max_pv, pv)
            except Exception:
                pass
            self.stop.wait(0.5)


def get(port: int, path: str, token: str, conn: http.client.HTTPConnection | None = None):
    c = conn or http.client.HTTPConnection("127.0.0.1", port, timeout=120)
    c.request("GET", path, headers={"Authorization": f"Token {token}", "Accept": "application/json; version=9"})
    r = c.getresponse()
    body = r.read()
    return c, r.status, len(body)


def wait_ready(port: int, token: str, t0: float, limit: float = 180) -> float:
    while time.perf_counter() - t0 < limit:
        try:
            _, st, _ = get(port, "/api/ui_settings/", token)
            if st == 200:
                return time.perf_counter() - t0
        except OSError:
            pass
        time.sleep(0.05)
    raise RuntimeError(f"Port {port} nicht bereit nach {limit}s")


def load(port: int, path: str, token: str, conc: int, secs: float):
    lat: list[float] = []
    errs = [0]
    lock = threading.Lock()
    end = time.perf_counter() + secs

    def worker():
        conn = None
        mine = []
        while time.perf_counter() < end:
            t = time.perf_counter()
            try:
                conn, st, _ = get(port, path, token, conn)
                if st != 200:
                    errs[0] += 1
            except Exception:
                errs[0] += 1
                conn = None
                continue
            mine.append(time.perf_counter() - t)
        with lock:
            lat.extend(mine)

    ths = [threading.Thread(target=worker) for _ in range(conc)]
    for t in ths:
        t.start()
    for t in ths:
        t.join()
    lat.sort()
    n = len(lat)
    q = lambda p: lat[min(n - 1, int(p * n))] * 1000 if n else None  # noqa: E731
    return {"requests": n, "rps": round(n / secs, 1), "errors": errs[0],
            "p50_ms": q(0.5), "p95_ms": q(0.95), "p99_ms": q(0.99),
            "mean_ms": statistics.fmean(lat) * 1000 if n else None}


def stop_tree(pid: int):
    subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True)


def port_free(port: int) -> None:
    """Foreign server on the port (e.g. parallel measurement) -> abort instead of measuring it as well."""
    try:
        http.client.HTTPConnection("127.0.0.1", port, timeout=2).connect()
    except OSError:
        return
    raise SystemExit(f"Port {port} ist belegt - anderen --port waehlen")


def run_candidate(name, cmd, env, port, token, args, out_dir: Path):
    port_free(port)
    log = open(out_dir / f"{name}.log", "a", encoding="utf-8")
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, env=env, stdout=log, stderr=subprocess.STDOUT,
                         creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
    try:
        startup = wait_ready(port, token, t0)
        if p.poll() is not None:
            raise SystemExit(f"{name} beendet (Port {port} belegt?), siehe Log")
        for _ in range(3):  # Warm up
            for _, path in ENDPOINTS:
                get(port, path, token)
        time.sleep(3)
        idle_ws, idle_pv = proc_tree_mem(p.pid)
        res = {"startup_s": round(startup, 2), "idle_ws_mb": round(idle_ws, 1),
               "idle_private_mb": round(idle_pv, 1), "endpoints": {}}
        s = Sampler(p.pid)
        s.start()
        for ep, path in ENDPOINTS:
            res["endpoints"][ep] = load(port, path, token, args.conc, args.secs)
            print(f"  {name} {ep}: {res['endpoints'][ep]}", flush=True)
        s.stop.set()
        s.join()
        res["load_max_ws_mb"] = round(s.max_ws, 1)
        res["load_max_private_mb"] = round(s.max_pv, 1)
        return res
    finally:
        stop_tree(p.pid)
        log.close()
        time.sleep(2)


def startup_only(name, cmd, env, port, token, out_dir: Path) -> dict:
    """One start: time to the first 200 response, duration of the first requests, idle memory."""
    port_free(port)
    log = open(out_dir / f"{name}.startup.log", "a", encoding="utf-8")
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, env=env, stdout=log, stderr=subprocess.STDOUT,
                         creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
    try:
        startup = wait_ready(port, token, t0)
        if p.poll() is not None:
            raise SystemExit(f"{name} beendet (Port {port} belegt?), siehe Log")
        conn, firsts = None, []
        for _ in range(5):
            t = time.perf_counter()
            conn, _, _ = get(port, "/api/ui_settings/", token, conn)
            firsts.append(round((time.perf_counter() - t) * 1000, 1))
        time.sleep(2)
        ws, pv = proc_tree_mem(p.pid)
        return {"startup_s": round(startup, 3), "next_ms": firsts, "idle_ws_mb": round(ws, 1),
                "idle_private_mb": round(pv, 1)}
    finally:
        stop_tree(p.pid)
        log.close()
        time.sleep(1.5)


def parse_cand(spec: str) -> tuple[str, str, dict]:
    """`name=path[@ENV=VAL[@ENV=VAL…]]`."""
    name, rest = spec.split("=", 1)
    path, *envs = rest.split("@")
    return name, path, dict(e.split("=", 1) for e in envs)


def summary(results: dict) -> str:
    med = statistics.median
    lines = ["| Kandidat | Start s | WS Leerlauf MB | WS Last max MB | " +
             " | ".join(f"{ep} rps · p50/p95" for ep, _ in ENDPOINTS) + " |",
             "|---" * (4 + len(ENDPOINTS)) + "|"]
    for name, runs in results.items():
        if not runs:
            continue
        cell = [name, f"{med(r['startup_s'] for r in runs):.2f}", f"{med(r['idle_ws_mb'] for r in runs):.1f}",
                f"{med(r.get('load_max_ws_mb', 0) for r in runs):.1f}"]
        for ep, _ in ENDPOINTS:
            rs = [r["endpoints"][ep] for r in runs if "endpoints" in r]
            if rs:
                cell.append(f"{med(x['rps'] for x in rs):.0f} · {med(x['p50_ms'] for x in rs):.1f}/"
                            f"{med(x['p95_ms'] for x in rs):.1f}")
            else:
                cell.append("–")
        lines.append("| " + " | ".join(cell) + " |")
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust", help="ein Rust-Build (Kandidaten paperrust und paperrust_worker)")
    ap.add_argument("--cand", action="append", default=[], help="name=pfad[@ENV=WERT…] (mehrfach)")
    ap.add_argument("--no-rust-worker", action="store_true", help="ohne Kandidat paperrust_worker")
    ap.add_argument("--startup-only", type=int, default=0, help="nur N Starts je Kandidat")
    ap.add_argument("--port", type=int, default=8095, help="Port der Rust-Kandidaten (Worker: +1)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--db", default="pl_bench")
    ap.add_argument("--conc", type=int, default=8)
    ap.add_argument("--secs", type=float, default=15)
    ap.add_argument("--dj-workers", type=int, default=1)
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--only", choices=["rust", "django", "all"], default="all")
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    token = admin_token()

    base = dict(os.environ)
    base["PAPERLESS_DBNAME"] = args.db
    base["PGDATABASE"] = args.db
    base["PAPERLESS_REDIS_PREFIX"] = "bench"

    work = REPO / "results/bench/work"
    rust_env = dict(base, PAPERRUST_BIND=f"127.0.0.1:{args.port}",
                    PAPERRUST_ROUTES=str(REPO / "results/parity/routes-all.toml"),
                    PAPERRUST_BRANDING="off", PAPERRUST_LOG="warn",
                    PAPERLESS_DATA_DIR=str(work / "data"), PAPERLESS_MEDIA_ROOT=str(work / "media"),
                    PAPERLESS_CONSUMPTION_DIR=str(work / "consume"))
    rust_worker_env = dict(rust_env, PAPERRUST_BIND=f"127.0.0.1:{args.port + 1}")
    dj_env = dict(base, GRANIAN_HOST="127.0.0.1", GRANIAN_PORT="8097", GRANIAN_WORKERS=str(args.dj_workers))
    dj_cmd = [str(REF / "venv/Scripts/granian.exe"), "--interface", "asginl", "--ws", "--loop", "asyncio",
              "--working-dir", str(REPO / "upstream/src"), "paperless.asgi:application"]

    cands = []
    if args.only in ("rust", "all"):
        if args.rust:
            cands.append(("paperrust", [args.rust, "serve"], rust_env, args.port))
            if not args.no_rust_worker:
                cands.append(("paperrust_worker", [args.rust, "serve", "--worker"], rust_worker_env, args.port + 1))
        for spec in args.cand:
            name, path, extra = parse_cand(spec)
            cands.append((name, [path, "serve"], dict(rust_env, **extra), args.port))
    if args.only in ("django", "all"):
        cands += [("django", dj_cmd, dj_env, 8097)]
    if not cands:
        raise SystemExit("keine Kandidaten (--rust/--cand/--only)")

    results: dict[str, list] = {c[0]: [] for c in cands}
    for r in range(args.startup_only or args.rounds):  # interleaved: every round all candidates
        for name, cmd, env, port in cands:
            print(f"Runde {r + 1}: {name}", flush=True)
            if args.startup_only:
                res = startup_only(name, cmd, env, port, token, out)
                print(f"  {name}: {res}", flush=True)
            else:
                res = run_candidate(name, cmd, env, port, token, args, out)
            results[name].append(res)
    (out / "results.json").write_text(json.dumps({"args": vars(args), "results": results}, indent=2),
                                      encoding="utf-8")
    (out / "summary.md").write_text(summary(results), encoding="utf-8")
    print(summary(results))
    print("fertig:", out / "results.json")


if __name__ == "__main__":
    sys.exit(main())
