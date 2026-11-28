#!/usr/bin/env python3
"""Speicher-/CPU-Abtaster für Docker-Container (läuft als root, liest nur).

Je Container und Abtastung:
  pss      Summe von Pss aus /proc/<pid>/smaps_rollup aller Prozesse der Container-cgroup
           (geteilte Seiten anteilig, ohne Seitencache) – Hauptwert.
  anon     memory.stat "anon" der cgroup (eigener Arbeitsspeicher ohne Dateicache)
  current  memory.current der cgroup (inkl. Seitencache, nur zur Info)
  cpu_us   cpu.stat usage_usec (kumuliert)
Schutz für den geteilten Rechner: fällt MemAvailable unter 900 MB, wird <out>.abort geschrieben.

Aufruf: memsample.py <out.jsonl> <intervall_s> <container> [<container> ...]
        (läuft bis <out.jsonl>.stop existiert) oder: memsample.py --once <container> ...
"""
import json
import os
import subprocess
import sys
import time


def cg_dir(name):
    cid = subprocess.run(["docker", "inspect", "-f", "{{.Id}}", name], capture_output=True, text=True).stdout.strip()
    if not cid:
        return None
    return f"/sys/fs/cgroup/system.slice/docker-{cid}.scope"


def read(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return ""


def sample(dirs):
    out = {}
    for name, d in dirs.items():
        if not d or not os.path.isdir(d):
            continue
        pss = 0
        pids = read(d + "/cgroup.procs").split()
        for pid in pids:
            for line in read(f"/proc/{pid}/smaps_rollup").splitlines():
                if line.startswith("Pss:"):
                    pss += int(line.split()[1]) * 1024
                    break
        stat = dict(l.split() for l in read(d + "/memory.stat").splitlines() if l)
        cpu = dict(l.split() for l in read(d + "/cpu.stat").splitlines() if l)
        out[name] = {"pss": pss, "anon": int(stat.get("anon", 0)), "file": int(stat.get("file", 0)),
                     "current": int(read(d + "/memory.current") or 0), "cpu_us": int(cpu.get("usage_usec", 0)),
                     "nproc": len(pids)}
    return out


def mem_avail_mb():
    for l in read("/proc/meminfo").splitlines():
        if l.startswith("MemAvailable:"):
            return int(l.split()[1]) // 1024
    return 0


def main():
    if sys.argv[1] == "--once":
        names = sys.argv[2:]
        print(json.dumps({"t": time.time(), "avail_mb": mem_avail_mb(), "c": sample({n: cg_dir(n) for n in names})}))
        return
    out, interval, names = sys.argv[1], float(sys.argv[2]), sys.argv[3:]
    dirs = {n: cg_dir(n) for n in names}
    with open(out, "a") as f:
        while not os.path.exists(out + ".stop"):
            avail = mem_avail_mb()
            f.write(json.dumps({"t": time.time(), "avail_mb": avail, "c": sample(dirs)}) + "\n")
            f.flush()
            if avail < 900:
                open(out + ".abort", "w").write(f"MemAvailable {avail} MB\n")
            time.sleep(interval)


if __name__ == "__main__":
    main()
