"""Scores the output of `ocr_quality images` and `ocr_quality ingest` against the reference text.

    python tests/bench/ocr-quality/score.py <corpus-dir> <run-dir>     (Markdown on stdout)

Normalization before the comparison (both sides equal): Unicode NFC, every whitespace run → one
space, edges removed. Case and punctuation count.

- CER = Levenshtein(characters) / characters of the reference
- WER = Levenshtein(words) / words of the reference
- word F1 = agreement of the word multisets (independent of reading order; separates
  recognition errors from differing order, e.g. for tables/columns)
Group sums: errors and reference lengths added over all images of the group (micro), time = sum
of the medians.
"""

from __future__ import annotations

import json
import statistics
import sys
import unicodedata
from collections import Counter
from pathlib import Path


def norm(s: str) -> str:
    return " ".join(unicodedata.normalize("NFC", s).split())


def lev(a, b) -> int:
    if len(a) < len(b):
        a, b = b, a
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def f1(ref: list[str], hyp: list[str]) -> float:
    if not ref and not hyp:
        return 1.0
    c = sum((Counter(ref) & Counter(hyp)).values())
    p = c / len(hyp) if hyp else 0.0
    r = c / len(ref) if ref else 0.0
    return 2 * p * r / (p + r) if p + r else 0.0


def measure(ref: str, hyp: str):
    ref, hyp = norm(ref), norm(hyp)
    return {
        "ce": lev(ref, hyp),
        "cn": len(ref),
        "we": lev(ref.split(), hyp.split()),
        "wn": len(ref.split()),
        "f1": f1(ref.split(), hyp.split()),
    }


pct = lambda v: f"{100 * v:.1f} %"


def main() -> None:
    corpus, run = Path(sys.argv[1]), Path(sys.argv[2])
    manifest = json.loads((corpus / "manifest.json").read_text(encoding="utf-8"))
    timings = {}
    tp = run / "timings.jsonl"
    for line in tp.read_text(encoding="utf-8").splitlines() if tp.exists() else []:
        t = json.loads(line)
        timings[(t["engine"], t["name"])] = t
    engines = sorted({e for e, _ in timings}, key=lambda e: (e != "tesseract", e))

    # --- Single images ---
    rows = []
    groups: dict = {}
    for it in manifest["images"]:
        ref = (corpus / it["gt"]).read_text(encoding="utf-8")
        row = {"it": it}
        for e in engines:
            p = run / e / f"{it['name']}.txt"
            m = measure(ref, p.read_text(encoding="utf-8") if p.exists() else "")
            m["ms"] = timings.get((e, it["name"]), {}).get("median_ms", float("nan"))
            row[e] = m
            keys = [it["group"]]
            if it["group"] == "scan":
                # Subgroups by strength (name ends in _mild/_strong/_fax)
                keys.append("scan-" + it["name"].rsplit("_", 1)[-1])
            for key in keys:
                g = groups.setdefault(key, {}).setdefault(e, {"ce": 0, "cn": 0, "we": 0, "wn": 0, "f1": [], "ms": 0.0, "n": 0})
                for k in ("ce", "cn", "we", "wn"):
                    g[k] += m[k]
                g["f1"].append(m["f1"])
                g["ms"] += m["ms"]
                g["n"] += 1
        rows.append(row)

    print("### Zusammenfassung je Gruppe (Einzelseiten, Erkennung allein)\n")
    print("| Gruppe | Seiten | Engine | CER | WER | Wort-F1 | ms/Seite (Median) |")
    print("|---|---|---|---|---|---|---|")
    for gname in ("clean", "scan", "scan-mild", "scan-strong", "scan-fax", "real"):
        for e in engines:
            g = groups.get(gname, {}).get(e)
            if not g:
                continue
            print(
                f"| {gname} | {g['n']} | {e} | {pct(g['ce'] / max(g['cn'], 1))} | {pct(g['we'] / max(g['wn'], 1))} | "
                f"{pct(statistics.mean(g['f1']))} | {g['ms'] / g['n']:.0f} |"
            )

    print("\n### Je Seite\n")
    print("| Bild | Gruppe | DPI | Format | " + " | ".join(f"{e} CER | {e} WER | {e} F1 | {e} ms" for e in engines) + " |")
    print("|" + "---|" * (4 + 4 * len(engines)))
    for r in rows:
        it = r["it"]
        cells = []
        for e in engines:
            m = r[e]
            cells += [pct(m["ce"] / max(m["cn"], 1)), pct(m["we"] / max(m["wn"], 1)), pct(m["f1"]), f"{m['ms']:.0f}"]
        fmt = "bitonal PNG" if it.get("params", {}).get("bilevel") else ("JPEG q%s" % it["params"]["jpeg"] if it.get("params", {}).get("jpeg") else "PNG")
        print(f"| {it['name']} | {it['group']} | {it['dpi'] or '–'} | {fmt} | " + " | ".join(cells) + " |")

    thr = run / "throughput.jsonl"
    if thr.exists():
        print("\n### Durchsatz (alle Einzelseiten, Threads wie die Seiten-Pipeline)\n")
        print("| Engine | Threads | Seiten | Wandzeit s | s/Seite |")
        print("|---|---|---|---|---|")
        for line in thr.read_text(encoding="utf-8").splitlines():
            t = json.loads(line)
            print(f"| {t['engine']} | {t['threads']} | {t['pages']} | {t['wall_ms'] / 1000:.1f} | {t['wall_ms'] / 1000 / t['pages']:.2f} |")

    ing = run / "ingest.jsonl"
    if ing.exists():
        print("\n### Echter Einleseweg (`parse_document`, mehrseitige Bild-PDFs, deu+eng, rotate_pages an)\n")
        print("| PDF | Seiten | Engine | Jobs | Zeit s | s/Seite | CER Text | WER Text | CER Textebene |")
        print("|---|---|---|---|---|---|---|---|---|")
        for line in ing.read_text(encoding="utf-8").splitlines():
            t = json.loads(line)
            name = Path(t["pdf"]).stem
            pd = next(p for p in manifest["pdfs"] if p["name"] == name)
            ref = (corpus / pd["gt"]).read_text(encoding="utf-8")
            d = run / "ingest" / name
            mt = measure(ref, (d / f"ingest-{t['engine']}.txt").read_text(encoding="utf-8"))
            ml = measure(ref, (d / f"ingest-{t['engine']}-layer.txt").read_text(encoding="utf-8"))
            n = t["pages"] or pd["pages"]
            print(
                f"| {name} | {n} | {t['engine']} | {t['jobs']} | {t['ms'] / 1000:.1f} | {t['ms'] / 1000 / n:.2f} | "
                f"{pct(mt['ce'] / mt['cn'])} | {pct(mt['we'] / mt['wn'])} | {pct(ml['ce'] / ml['cn'])} |"
            )


if __name__ == "__main__":
    main()
