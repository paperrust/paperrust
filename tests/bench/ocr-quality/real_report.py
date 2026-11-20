"""Evaluation of real scans (group `real`) – numbers ONLY, no content.

    python tests/bench/ocr-quality/real_report.py <real-out-dir>

`<real-out-dir>` = tests/bench/ocr-quality/real/out (git-ignored), containing one folder per PDF with
`pages.jsonl`, `<engine>/page-NN.txt`, `img/page-NN.jpg`, `ingest.jsonl` (from `ocr_quality pages/ingest`).

Output:
- Markdown on stdout: pages, time/page per engine, recognized characters/words, mean
  word confidence (only Tesseract provides one), agreement of the engines (CER Windows against
  Tesseract), times of the ingestion path; with `<stem>.gt.txt` next to the PDF additionally CER/WER against the
  reference. No text excerpts – the result may go into RESULTS.md.
- `<real-out-dir>/<pdf>/side-by-side.html`: page image | Tesseract | Windows OCR for judging by
  eye (stays git-ignored, contains the recognized text).
"""

from __future__ import annotations

import html
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from score import lev, measure, norm  # noqa: E402

pct = lambda v: f"{100 * v:.1f} %"


def report(d: Path, real_dir: Path) -> None:
    rows = [json.loads(l) for l in (d / "pages.jsonl").read_text(encoding="utf-8").splitlines()]
    engines = sorted({r["engine"] for r in rows}, key=lambda e: (e != "tesseract", e))
    pages = sorted({r["page"] for r in rows})
    text = {(e, p): (d / e / f"page-{p:02d}.txt").read_text(encoding="utf-8") for e in engines for p in pages}
    print(f"#### {d.name}: {len(pages)} Seiten\n")
    print("| Engine | ms/Seite (Mittel) | ms/Seite (max) | Zeichen | Wörter | mittl. Wortkonfidenz |")
    print("|---|---|---|---|---|---|")
    for e in engines:
        rs = [r for r in rows if r["engine"] == e]
        confs = [r["mean_conf"] for r in rs if r.get("mean_conf") is not None]
        conf = f"{sum(confs) / len(confs):.1f}" if confs else "– (liefert keine)"
        print(
            f"| {e} | {sum(r['ms'] for r in rs) / len(rs):.0f} | {max(r['ms'] for r in rs):.0f} | "
            f"{sum(r['chars'] for r in rs)} | {sum(len(norm(text[(e, r['page'])]).split()) for r in rs)} | {conf} |"
        )
    if "tesseract" in engines and "windows" in engines:
        ce = cn = we = wn = 0
        per = []
        f1s = []
        for p in pages:
            m = measure(text[("tesseract", p)], text[("windows", p)])
            ce, cn, we, wn = ce + m["ce"], cn + m["cn"], we + m["we"], wn + m["wn"]
            per.append(m["ce"] / max(m["cn"], 1))
            f1s.append(m["f1"])
        print(
            f"\nÜbereinstimmung Windows gegen Tesseract (Tesseract als Bezug): CER {pct(ce / max(cn, 1))}, "
            f"WER {pct(we / max(wn, 1))}; je Seite CER min {pct(min(per))} / Median {pct(sorted(per)[len(per) // 2])} / max {pct(max(per))}; "
            f"Wort-F1 (reihenfolgeunabhängig) Mittel {pct(sum(f1s) / len(f1s))}, min {pct(min(f1s))}."
        )
    gt = real_dir / f"{d.name}.gt.txt"
    if gt.is_file():
        ref = gt.read_text(encoding="utf-8")
        for e in engines:
            hyp = "\n".join(text[(e, p)] for p in pages)
            m = measure(ref, hyp)
            print(f"\nGegen Referenz ({e}): CER {pct(m['ce'] / max(m['cn'], 1))}, WER {pct(m['we'] / max(m['wn'], 1))}, Wort-F1 {pct(m['f1'])}")
    ing = d / "ingest.jsonl"
    if ing.exists():
        print("\nEinleseweg (`parse_document`, deu+eng, Standard-Konfiguration inkl. rotate_pages):\n")
        print("| Engine | Jobs | Seiten | Zeit s | s/Seite | Zeichen Dokumenttext | Zeichen Textebene |")
        print("|---|---|---|---|---|---|---|")
        for l in ing.read_text(encoding="utf-8").splitlines():
            t = json.loads(l)
            n = t["pages"] or len(pages)
            print(f"| {t['engine']} | {t['jobs']} | {n} | {t['ms'] / 1000:.1f} | {t['ms'] / 1000 / n:.2f} | {t['chars']} | {t['layer_chars']} |")
    print()

    # Side-by-side view (local only, git-ignored)
    parts = [
        "<!doctype html><meta charset=utf-8><title>OCR side by side</title><style>"
        "body{font:14px system-ui;margin:16px;background:#f6f6f6}"
        ".p{display:grid;grid-template-columns:1fr 1fr 1fr;gap:12px;margin-bottom:28px}"
        "img{width:100%;border:1px solid #ccc;background:#fff}"
        "pre{white-space:pre-wrap;background:#fff;border:1px solid #ccc;padding:8px;margin:0;font:12px/1.4 ui-monospace,Consolas,monospace;max-height:1400px;overflow:auto}"
        "h2{font-size:15px}</style>",
        f"<h1>{html.escape(d.name)}</h1><p>links Seitenbild, Mitte Tesseract, rechts Windows OCR. Übereinstimmung je Seite als CER (Windows gegen Tesseract).</p>",
    ]
    for p in pages:
        info = []
        for e in engines:
            r = next(r for r in rows if r["engine"] == e and r["page"] == p)
            info.append(f"{e} {r['ms']:.0f} ms")
        agree = ""
        if ("tesseract", p) in text and ("windows", p) in text:
            a, b = norm(text[("tesseract", p)]), norm(text[("windows", p)])
            agree = f" · CER W/T {pct(lev(a, b) / max(len(a), 1))}"
        parts.append(f"<h2>Seite {p} · {' · '.join(info)}{agree}</h2><div class=p>")
        parts.append(f"<img src='img/page-{p:02d}.jpg' alt='Seite {p}'>")
        for e in ("tesseract", "windows"):
            parts.append(f"<pre>{html.escape(text.get((e, p), '(nicht gemessen)'))}</pre>")
        parts.append("</div>")
    (d / "side-by-side.html").write_text("".join(parts), encoding="utf-8")


def main() -> None:
    out = Path(sys.argv[1])
    real_dir = out.parent
    for d in sorted(p for p in out.iterdir() if (p / "pages.jsonl").exists()):
        report(d, real_dir)


if __name__ == "__main__":
    main()
