"""Compares out/upstream (gen_reference.py) with out/rust (cargo test --test parity).

Invocation: reference/venv/Scripts/python.exe tests/parity/pl-pdfops/compare.py

Per operation: result/error kind/error text, and per output PDF (read with pikepdf + pypdfium2,
i.e. independent of both implementations): page count, text per page, effective rotation,
MediaBox, PDF version, info keys/title, XMP present, bookmarks, encryption and a
raster deviation per page (30 dpi, grayscale, mean absolute difference).
Barcodes: per file the set (page, value, format) and per configuration separator pages, ASN, tags,
stored values and the partial PDFs (page count + text).
Writes out/compare.json; exit 1 on differences that are not in KNOWN.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pikepdf
import pypdfium2 as pdfium
from PIL import ImageChops
from PIL import ImageStat

HERE = Path(__file__).resolve().parent
UP = HERE / "out" / "upstream"
RS = HERE / "out" / "rust"

# Intentional deviations (docs/contract/pl-pdfops.md, Known differences): (area, key)
KNOWN = {
}


def norm_text(t: str) -> str:
    return " ".join(t.replace("\r", " ").replace("\n", " ").split())


def inspect(path: Path) -> dict:
    info: dict = {}
    with pikepdf.open(path) as pdf:
        info["version"] = pdf.pdf_version
        info["encrypted"] = pdf.is_encrypted
        info["info_keys"] = sorted(str(k) for k in pdf.docinfo.keys()) if "/Info" in pdf.trailer else []
        info["title"] = str(pdf.docinfo.get("/Title", "")) if "/Info" in pdf.trailer else ""
        info["xmp"] = "/Metadata" in pdf.Root
        with pdf.open_outline() as ol:
            info["outline"] = [it.title for it in ol.root]
        info["mediabox"] = [[round(float(x), 2) for x in p.mediabox] for p in pdf.pages]
    try:
        doc = pdfium.PdfDocument(str(path))
    except pdfium.PdfiumError as e:
        info["pdfium_error"] = str(e)
        info["pages"], info["texts"], info["rotation"], info["_renders"] = None, [], [], []
        return info
    try:
        info["pages"] = len(doc)
        texts, rots, renders = [], [], []
        for i in range(len(doc)):
            page = doc[i]
            tp = page.get_textpage()
            texts.append(norm_text(tp.get_text_bounded()))
            rots.append(page.get_rotation())
            renders.append(page.render(scale=30 / 72, grayscale=True).to_pil().convert("L"))
        info["texts"] = texts
        info["rotation"] = rots
        info["_renders"] = renders
    finally:
        doc.close()
    return info


def diff_pdf(case: str, a: Path, b: Path, diffs: list) -> dict:
    ia, ib = inspect(a), inspect(b)
    for key in ("pdfium_error", "pages", "texts", "rotation", "mediabox", "version", "encrypted", "info_keys", "title", "xmp", "outline"):
        if ia.get(key) != ib.get(key):
            diffs.append({"case": case, "file": a.name, "key": key, "upstream": ia.get(key), "rust": ib.get(key), "known": (case, key) in KNOWN})
    pix = []
    for ra, rb in zip(ia["_renders"], ib["_renders"]):
        if ra.size != rb.size:
            pix.append(None)
            continue
        pix.append(round(ImageStat.Stat(ImageChops.difference(ra, rb)).mean[0], 3))
    if any(p is None or p > 2.0 for p in pix):
        diffs.append({"case": case, "file": a.name, "key": "render", "upstream": "-", "rust": pix, "known": (case, "render") in KNOWN})
    return {"pages": ia["pages"], "render_diff": pix}


def base_msg(m: str) -> str:
    # Error texts contain temp paths: compare only the file name
    head, sep, tail = m.rpartition(": ")
    return (head.replace("\\", "/").rsplit("/", 1)[-1] + sep + tail) if sep else m


def main() -> int:
    up = json.loads((UP / "results.json").read_text(encoding="utf-8"))
    rs = json.loads((RS / "results.json").read_text(encoding="utf-8"))
    diffs: list = []
    summary: dict = {"ops": {}, "barcodes": {}}
    for cid, u in up["ops"].items():
        r = rs["ops"].get(cid)
        if r is None:
            diffs.append({"case": cid, "key": "missing"})
            continue
        for key in ("ok", "error_kind", "result", "skipped", "name"):
            if u.get(key) != r.get(key):
                diffs.append({"case": cid, "key": key, "upstream": u.get(key), "rust": r.get(key), "known": (cid, key) in KNOWN})
        if base_msg(u.get("error", "")) != base_msg(r.get("error", "")):
            diffs.append({"case": cid, "key": "error", "upstream": u.get("error"), "rust": r.get("error"), "known": (cid, "error") in KNOWN})
        if u.get("ok") and r.get("ok"):
            if u.get("outputs") != r.get("outputs"):
                diffs.append({"case": cid, "key": "outputs", "upstream": u.get("outputs"), "rust": r.get("outputs")})
            files = []
            for name in u.get("outputs", []):
                if (RS / name).exists():
                    files.append(diff_pdf(cid, UP / name, RS / name, diffs))
            summary["ops"][cid] = files
    ub, rb = up.get("barcodes", {}), rs.get("barcodes", {})
    for rel, u in ub.get("files", {}).items():
        r = rb.get("files", {}).get(rel)
        if r is None:
            continue
        key = Path(rel).name
        if u.get("ok") != r.get("ok") or base_msg(u.get("error", "")) != base_msg(r.get("error", "")):
            diffs.append({"case": key, "key": "scan_error", "upstream": u.get("error"), "rust": r.get("error"), "known": (key, "scan_error") in KNOWN})
            continue
        if not u.get("ok"):
            continue
        su = sorted({tuple(x) for x in u["barcodes"]})
        sr = sorted({tuple(x) for x in r["barcodes"]})
        summary["barcodes"][key] = {"upstream": len(u["barcodes"]), "rust": len(r["barcodes"])}
        if su != sr:
            diffs.append({"case": key, "key": "barcodes", "upstream": su, "rust": sr, "known": (key, "barcodes") in KNOWN})
        for cfg, cu in u.get("configs", {}).items():
            cr = r["configs"].get(cfg, {})
            for k in ("separators", "asn", "warnings"):
                if cu.get(k) != cr.get(k):
                    diffs.append({"case": f"{key}/{cfg}", "key": k, "upstream": cu.get(k), "rust": cr.get(k), "known": (key, "barcodes") in KNOWN})
            for k in ("tags", "stored"):
                a = sorted(json.dumps(x, sort_keys=True) for x in cu.get(k, []))
                b = sorted(json.dumps(x, sort_keys=True) for x in cr.get(k, []))
                if a != b:
                    diffs.append({"case": f"{key}/{cfg}", "key": k, "upstream": a, "rust": b, "known": (key, "barcodes") in KNOWN})
            if cu.get("parts") != cr.get("parts"):
                diffs.append({"case": f"{key}/{cfg}", "key": "parts", "upstream": cu.get("parts"), "rust": cr.get("parts"), "known": (key, "barcodes") in KNOWN})
            else:
                for name in cu.get("parts") or []:
                    diff_pdf(f"{key}/{cfg}", UP / name, RS / name, diffs)
    for vid, u in ub.get("variants", {}).items():
        r = rb.get("variants", {}).get(vid)
        if r is None:
            continue
        a = sorted({tuple(x) for x in u.get("barcodes", [])})
        b = sorted({tuple(x) for x in r.get("barcodes", [])})
        if a != b or u.get("ok") != r.get("ok"):
            diffs.append({"case": f"variant/{vid}", "key": "barcodes", "upstream": a, "rust": b, "known": (vid, "barcodes") in KNOWN})
    (HERE / "out" / "compare.json").write_text(json.dumps({"diffs": diffs, "summary": summary}, indent=1, ensure_ascii=False, default=str), encoding="utf-8")
    unknown = [d for d in diffs if not d.get("known")]
    for d in diffs:
        flag = "KNOWN " if d.get("known") else "DIFF  "
        print(flag, d.get("case"), d.get("file", ""), d.get("key"), "\n    upstream:", str(d.get("upstream"))[:300], "\n    rust:    ", str(d.get("rust"))[:300])
    print(f"{len(diffs)} Unterschiede, davon {len(unknown)} unbekannt")
    return 1 if unknown else 0


if __name__ == "__main__":
    sys.exit(main())
