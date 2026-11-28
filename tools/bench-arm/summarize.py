#!/usr/bin/env python3
"""Fasst die Läufe zusammen: python3 summarize.py <ordner mit run_*.json/big_*.json> [meta.json] > tabellen.md

Schreibt <ordner>/rohdaten.json (alle Einzelwerte + Metadaten) und gibt je Szenario eine Markdown-Tabelle
aus: Median über die Runden, in Klammern Spannweite (min–max), Faktor = ngx-Wert / PaperRust-Wert
(bei "mehr ist besser" umgekehrt), jeweils gegen ngx vollständig (B) und ngx minimal (A).
"""
import glob
import json
import subprocess
import os
import statistics
import sys

SYSTEMS = ["ngxb", "ngxc", "ngxa", "rs"]
LABEL = {"ngxb": "ngx vollständig (B)", "ngxc": "ngx gleiche Arbeit (C)", "ngxa": "ngx minimal (A)",
         "rs": "PaperRust"}
SHORT = {"ngxb": "B", "ngxc": "C", "ngxa": "A"}


def load(folder):
    R = {s: [] for s in SYSTEMS}
    B = {}
    for p in sorted(glob.glob(os.path.join(folder, "run_*.json"))):
        r = json.load(open(p, encoding="utf-8"))
        if r.get("smoke") or r["system"] not in R:
            continue
        log = p.replace(".json", "_applog.txt")
        if os.path.exists(log):  # Breakdown of the S7 times from the app log
            bd = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                 "breakdown.py"), p, log], capture_output=True, text=True).stdout
            try:
                r["s7_breakdown"] = json.loads(bd)
            except ValueError:
                pass
        R[r["system"]].append(r)
    for p in sorted(glob.glob(os.path.join(folder, "big_*.json"))):
        r = json.load(open(p, encoding="utf-8"))
        if not r.get("smoke"):
            B[r["system"]] = r
    return R, B


def vals(rs, fn):
    out = []
    for r in rs:
        try:
            x = fn(r)
        except (KeyError, TypeError, ZeroDivisionError, IndexError, AttributeError, statistics.StatisticsError):
            x = None
        if isinstance(x, list):
            out += [y for y in x if isinstance(y, (int, float))]
        elif isinstance(x, (int, float)) and not isinstance(x, bool):
            out.append(x)
    return out


def fmt(v, d):
    if not v:
        return "–"
    m = statistics.median(v)
    if len(v) == 1 or min(v) == max(v):
        return f"{m:.{d}f}"
    return f"{m:.{d}f} ({min(v):.{d}f}–{max(v):.{d}f})"


def factor(ref, rs, better):
    if not ref or not rs:
        return "–"
    a, b = statistics.median(ref), statistics.median(rs)
    if a <= 0 or b <= 0:
        return "–"
    f = a / b if better == "low" else b / a
    if 0.9 <= f <= 1.1:
        return f"≈ gleich ({f:.2f}×)"
    return f"{f:.1f}× besser" if f > 1 else f"{1 / f:.1f}× schlechter"


def table(R, rows):
    present = [s for s in SYSTEMS if R[s]]
    refs = [s for s in present if s != "rs"] if "rs" in present else []
    head = "| Messgröße | " + " | ".join([LABEL[s] for s in present] + [f"PaperRust vs. {SHORT[s]}" for s in refs]) + " |"
    out = [head, "|---" * (len(present) + len(refs) + 1) + "|"]
    for label, fn, d, better in rows:
        v = {s: vals(R[s], fn) for s in SYSTEMS}
        if not any(v.values()):
            continue
        cells = [fmt(v[s], d) for s in present] + [factor(v[s], v["rs"], better) for s in refs]
        out.append(f"| {label} | " + " | ".join(cells) + " |")
    return "\n".join(out)


def app_cpu(w):
    return w["app_cpu_s"]


def nwin(key, field):
    return lambda r: r[key]["window"][field]


def vol(r, suffixes):
    return sum(v for k, v in r["s6"]["volumes_bytes"].items() if "/" not in k and k.endswith(suffixes)) / 2 ** 20


SCEN = {}
SCEN["S1 Leerlauf (leere Datenbank, 10 min nichts tun)"] = [
    ("Erststart bis Anmeldung möglich [s]", lambda r: r["first_start_s"], 1, "low"),
    ("Neustart bis Dokumentliste antwortet [s]", lambda r: r["s1"]["restart_s"], 2, "low"),
    ("RAM Dienste (ohne DB), Median [MiB]", lambda r: r["s1"]["idle"]["app_pss_median_mib"], 0, "low"),
    ("RAM Dienste (ohne DB), Maximum [MiB]", lambda r: r["s1"]["idle"]["app_pss_max_mib"], 0, "low"),
    ("RAM Datenbank, Median [MiB]", lambda r: r["s1"]["idle"]["db_pss_median_mib"], 0, "low"),
    ("CPU-Zeit Dienste in 10 min [CPU-s]", lambda r: r["s1"]["idle"]["app_cpu_s"], 2, "low"),
    ("CPU-Zeit Datenbank in 10 min [CPU-s]", lambda r: r["s1"]["idle"]["db_cpu_s"], 2, "low"),
    ("Prozesse der Dienste (ohne DB)", lambda r: r["s1"]["idle"]["max_nproc_app"], 0, "low"),
    ("Container (inkl. DB)", lambda r: len([k for k in r["s1"]["idle"]["cpu_s"]]), 0, "low"),
    ("30-s-Abschnitte mit Hintergrundarbeit > 0,5 CPU-s (von 20)",
     lambda r: sum(1 for x in r["s1"]["activity_app_cpu_s_per_30s"] if x > 0.5), 0, "low"),
]
def kind_of(name, r):
    k = r["s7"]["docs"][name]["kind"]
    return k if k != "c_photo" else ("c_photo_png" if name.endswith(".png") else "c_photo_jpg")


S7KINDS = [("a_text", "PDF mit Text"), ("b_scan1", "Scan 1 Seite"), ("b_scan8", "Scan 8 Seiten"),
           ("c_photo_jpg", "Foto JPG"), ("c_photo_png", "Bild PNG"), ("d_office", "Office DOCX")]
SCEN["S7 Einlesen je Art (ein Dokument, Ablegen bis durchsuchbar)"] = (
    [(f"{lab}: Median [s]", (lambda k: lambda r: r["s7"]["by_kind"][k]["median_s"])(k), 1, "low") for k, lab in S7KINDS] +
    [(f"{lab}: p95 [s]", (lambda k: lambda r: r["s7"]["by_kind"][k]["p95_s"])(k), 1, "low") for k, lab in S7KINDS] +
    [(f"{lab}: reine Verarbeitung laut Protokoll, Median [s]",
      (lambda k: lambda r: statistics.median(v["process"] for n, v in r["s7_breakdown"].items()
                                             if "process" in v and kind_of(n, r) == k))(k), 2, "low")
     for k, lab in S7KINDS] +
    [(f"{lab}: erfolgreich [Anzahl]", (lambda k: lambda r: r["s7"]["by_kind"][k]["ok"])(k), 0, "high") for k, lab in S7KINDS] +
    [(f"{lab}: Wortquote OCR/Text [%]", (lambda k: lambda r: 100 * r["s7"]["by_kind"][k]["word_recall_median"])(k), 1, "high") for k, lab in S7KINDS] +
    [("RAM-Spitze Dienste beim Einzeldokument [MiB]",
      lambda r: max(v["peak_app_pss_mib"] for v in r["s7"]["by_kind"].values()), 0, "low"),
     ("CPU-Spitze beim Einzeldokument [Kerne, 10-s-Mittel]",
      lambda r: max(v["peak_cpu_cores"] for v in r["s7"]["by_kind"].values()), 2, "low")])
SCEN["S4 Schwere Last (230 Dokumente auf einmal, dabei Liste/Suche jede Sekunde)"] = [
    ("Import 230 Dokumente (200 Text + 30 Scans) [s]", lambda r: r["s4"]["import"]["seconds"], 0, "low"),
    ("  Durchsatz [Dok./min]", lambda r: r["s4"]["docs"] / r["s4"]["import"]["seconds"] * 60, 1, "high"),
    ("  davon Text-PDFs: Zeit je Dokument, Median [s]", lambda r: r["s4"]["per_type"]["t"]["median_s"], 2, "low"),
    ("  davon Scans: Zeit je Dokument, Median [s]", lambda r: r["s4"]["per_type"]["s"]["median_s"], 1, "low"),
    ("Bedienung während Import: Median [ms]", lambda r: r["s4"]["probe_during"]["all"]["median_ms"], 0, "low"),
    ("Bedienung während Import: p95 [ms]", lambda r: r["s4"]["probe_during"]["all"]["p95_ms"], 0, "low"),
    ("Bedienung während Import: Fehler [Anzahl]", lambda r: r["s4"]["probe_during"]["all"]["errors"], 0, "low"),
    ("Bedienung nach Import: Median [ms]", lambda r: r["s4"]["probe_after"]["all"]["median_ms"], 0, "low"),
    ("Bedienung nach Import: p95 [ms]", lambda r: r["s4"]["probe_after"]["all"]["p95_ms"], 0, "low"),
    ("RAM-Spitze Dienste [MiB]", lambda r: r["s4"]["import_window"]["app_pss_max_mib"], 0, "low"),
    ("RAM-Spitze Datenbank [MiB]", lambda r: r["s4"]["import_window"]["db_pss_max_mib"], 0, "low"),
    ("CPU-Spitze [Kerne, 10-s-Mittel, max. 4]", lambda r: r["s4"]["import_window"]["cpu_peak_cores_10s"], 2, "low"),
    ("CPU-Zeit Import, Dienste [CPU-s]", lambda r: r["s4"]["import_window"]["app_cpu_s"], 0, "low"),
    ("Kleinster freier RAM des Testgeräts [MB]", lambda r: r["s4"]["import_window"]["min_avail_mb"], 0, "high"),
    ("OCR-Wortquote der 30 Scans, Median [%]", lambda r: 100 * r["s4"]["scan_ocr_quality"]["median"], 1, "high"),
    ("Massenbearbeitung: Tag an alle, Median [ms]", lambda r: r["s4"]["bulk_select"]["bulk_edit_tag_all"]["median_ms"], 0, "low"),
    ("Massenbearbeitung: p95 [ms]", lambda r: r["s4"]["bulk_select"]["bulk_edit_tag_all"]["p95_ms"], 0, "low"),
    ("„Alle auswählen“ (selection_data), Median [ms]", lambda r: r["s4"]["bulk_select"]["select_all_selection_data"]["median_ms"], 0, "low"),
]
SCEN["S6 Platz auf der Platte (nach dem Import, 230+ Dokumente)"] = [
    ("Datenbank-Volume [MiB]", lambda r: vol(r, ("pgdata",)), 1, "low"),
    ("Daten (Index, Vorschaubilder …) + Medien [MiB]", lambda r: vol(r, ("_data", "_media")), 1, "low"),
    ("Valkey-Volume [MiB]", lambda r: vol(r, ("_redis",)), 2, "low"),
    ("RAM Leerlauf mit Dokumenten, Dienste [MiB]", lambda r: r["s6"]["idle_with_docs"]["app_pss_median_mib"], 0, "low"),
    ("RAM Leerlauf mit Dokumenten, Datenbank [MiB]", lambda r: r["s6"]["idle_with_docs"]["db_pss_median_mib"], 0, "low"),
]
for key, title in [("s2_1user", "S2 Alltag (1 Person, 15 min, mit Pausen)"),
                   ("s3_5users", "S3 Kleines Büro (5 Personen gleichzeitig, 5 min)"),
                   ("s3_10users", "S3 Kleines Büro (10 Personen gleichzeitig, 5 min)")]:
    SCEN[title] = [
        ("Anfragen gesamt", (lambda k: lambda r: r[k]["all"]["n"])(key), 0, "high"),
        ("Antwortzeit alle Anfragen, Median [ms]", (lambda k: lambda r: r[k]["all"]["median_ms"])(key), 0, "low"),
        ("Antwortzeit p95 [ms]", (lambda k: lambda r: r[k]["all"]["p95_ms"])(key), 0, "low"),
        ("Antwortzeit p99 [ms]", (lambda k: lambda r: r[k]["all"]["p99_ms"])(key), 0, "low"),
        ("Nur Lesen (ohne Hochladen/Ändern), Median [ms]", (lambda k: lambda r: r[k]["reads"]["median_ms"])(key), 0, "low"),
        ("Nur Lesen, p95 [ms]", (lambda k: lambda r: r[k]["reads"]["p95_ms"])(key), 0, "low"),
        ("Fehlerquote [%]", (lambda k: lambda r: 100 * r[k]["all"]["error_rate"])(key), 2, "low"),
        ("RAM Dienste, Median [MiB]", (lambda k: lambda r: r[k]["window"]["app_pss_median_mib"])(key), 0, "low"),
        ("RAM Dienste, Spitze [MiB]", (lambda k: lambda r: r[k]["window"]["app_pss_max_mib"])(key), 0, "low"),
        ("RAM Datenbank, Spitze [MiB]", (lambda k: lambda r: r[k]["window"]["db_pss_max_mib"])(key), 0, "low"),
        ("CPU-Zeit Dienste [CPU-s]", (lambda k: lambda r: r[k]["window"]["app_cpu_s"])(key), 0, "low"),
        ("CPU-Spitze [Kerne]", (lambda k: lambda r: r[k]["window"]["cpu_peak_cores_10s"])(key), 2, "low"),
    ]


def endpoint_table(R, key):
    keys = set()
    for s in SYSTEMS:
        for r in R[s]:
            keys |= set(r.get(key, {}).get("by_endpoint", {}).keys())
    rows = []
    for k in sorted(keys):
        rows.append((f"{k} Median [ms]", (lambda k: lambda r: r[key]["by_endpoint"][k]["median_ms"])(k), 0, "low"))
        rows.append((f"{k} p95 [ms]", (lambda k: lambda r: r[key]["by_endpoint"][k]["p95_ms"])(k), 0, "low"))
        rows.append((f"{k} Fehler", (lambda k: lambda r: r[key]["by_endpoint"][k]["errors"])(k), 0, "low"))
    return table(R, rows)


def big_table(B):
    present = [s for s in SYSTEMS if s in B]
    if not present:
        return ""
    stages = sorted({st for s in present for st in B[s].get("stages", {})}, key=lambda x: int(x[1:]))
    out = []
    for st in stages:
        rows = []
        R = {s: ([{"m": B[s]["stages"][st]}] if s in B and st in B[s].get("stages", {}) else []) for s in SYSTEMS}
        rows.append(("Dokumente", lambda r: r["m"]["count"], 0, "high"))
        rows.append(("Import dieser Stufe [s]", lambda r: r["m"]["import_seconds"], 0, "low"))
        for ep in ("list_page1", "list_deep_page", "search_common", "search_rare", "filter_text", "filter_date",
                   "detail", "thumb"):
            for c in ("c1", "c8"):
                rows.append((f"{ep} {c[1]} gleichz., Median [ms]",
                             (lambda ep, c: lambda r: r["m"]["micro"][ep][c]["median_ms"])(ep, c), 1, "low"))
                rows.append((f"{ep} {c[1]} gleichz., p95 [ms]",
                             (lambda ep, c: lambda r: r["m"]["micro"][ep][c]["p95_ms"])(ep, c), 1, "low"))
        rows.append(("DB-Volume [MiB]", lambda r: sum(v for k, v in r["m"]["volumes_bytes"].items()
                                                     if "/" not in k and k.endswith("pgdata")) / 2 ** 20, 0, "low"))
        rows.append(("Daten+Medien [MiB]", lambda r: sum(v for k, v in r["m"]["volumes_bytes"].items()
                                                        if "/" not in k and k.endswith(("_data", "_media"))) / 2 ** 20, 0, "low"))
        rows.append(("RAM Dienste bei Abfragen, Spitze [MiB]", lambda r: r["m"]["micro_window"]["app_pss_max_mib"], 0, "low"))
        out.append(f"\n#### Stufe {st}\n\n" + table(R, rows))
    return "\n".join(out)


def main():
    folder = sys.argv[1]
    meta = json.load(open(sys.argv[2], encoding="utf-8")) if len(sys.argv) > 2 else {}
    R, B = load(folder)
    parts = []
    for title, rows in SCEN.items():
        parts.append(f"\n### {title}\n\n" + table(R, rows))
    parts.append("\n### S2 Alltag je Endpunkt\n\n" + endpoint_table(R, "s2_1user"))
    parts.append("\n### S5 Größeres Archiv\n" + big_table(B))
    print("\n".join(parts))
    for s in SYSTEMS:
        for r in R[s]:
            print(f"\n{s} Runde {r['round']}: Fehler={r.get('error')} cpuset={r.get('cpuset')} "
                  f"Aufgaben S4={r.get('s4', {}).get('tasks', {}).get('by_status')} "
                  f"OOM={r.get('app_state', {}).get('OOMKilled')}")
    slim = {s: [{k: v for k, v in r.items() if k != "requests_raw"} for r in R[s]] for s in SYSTEMS}
    raw = {"meta": meta, "runs": slim, "big": B,
           "requests_raw": {f"{s}_{r['round']}": r.get("requests_raw") for s in SYSTEMS for r in R[s]}}
    with open(os.path.join(folder, "rohdaten.json"), "w", encoding="utf-8") as f:
        json.dump(raw, f, ensure_ascii=False)


if __name__ == "__main__":
    main()
