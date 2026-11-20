#!/usr/bin/env python3
"""Schlüsselt die S7-Einlesezeit je Dokument aus dem App-Protokoll auf (docker logs -t).

Aufruf: python3 breakdown.py <run_X.json> [<run_X_applog.txt>]  -> JSON auf stdout
Spannen je Dokument (Sekunden):
  wait_detect   Ablage -> Eingangsordner meldet Datei (enthält CONSUMER_STABILITY_DELAY, Standard 5 s bei beiden)
  queue         gemeldet -> Verarbeitung beginnt ("Consuming …")
  process       Verarbeitung (Parsen/OCR, Vorschaubild, Speichern, Index) bis "consumption finished"
  to_search     fertig -> Messprogramm findet das Kennwort in der Suche (Abfrage alle ~0,5 s + Indexsicht)
"""
import json
import re
import sys
from datetime import datetime

TS = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(\.\d+)?Z ")


def ts(line):
    m = TS.match(line)
    if not m:
        return None
    frac = (m.group(2) or ".0")[:7]
    return datetime.fromisoformat(m.group(1) + frac + "+00:00").timestamp()


def main():
    run = json.load(open(sys.argv[1], encoding="utf-8"))
    logp = sys.argv[2] if len(sys.argv) > 2 else sys.argv[1].replace(".json", "_applog.txt")
    lines = [l for l in open(logp, encoding="utf-8", errors="replace") if "access:" not in l]
    out = {}
    for name, r in run.get("s7", {}).get("docs", {}).items():
        if r.get("searchable_s") is None:
            continue
        t_add = t_cons = t_fin = None
        for i, l in enumerate(lines):
            if t_add is None and name in l and "to the task queue" in l:
                t_add = ts(l)
            if t_cons is None and name in l and "Consuming" in l:
                t_cons = ts(l)
                for l2 in lines[i:]:
                    if "consumption finished" in l2:
                        t_fin = ts(l2)
                        break
                break
        d = {"searchable_s": r["searchable_s"]}
        t0 = r.get("t_drop")
        if t0 is None and t_fin is not None:
            t0 = None  # older runs without filing time: processing duration only
        if t0 and t_add:
            d["wait_detect"] = round(t_add - t0, 2)
        if t_add and t_cons:
            d["queue"] = round(t_cons - t_add, 2)
        if t_cons and t_fin:
            d["process"] = round(t_fin - t_cons, 2)
        if t0 and t_fin:
            d["to_search"] = round(t0 + r["searchable_s"] - t_fin, 2)
        out[name] = d
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
