"""Upstream side of the pl-pdfops parity (paperless-ngx v3.4.2, without DB).

Invocation (from the repo root directory, Git Bash):

    source reference/env.sh
    reference/venv/Scripts/python.exe tests/parity/pl-pdfops/gen_reference.py

1. Creates helper PDFs under samples/ (inherited attributes, bookmarks + XMP, PDF 1.4,
   encryptions R2/R3/R4/R6 with pikepdf).
2. Runs every operation from cases.json with the unchanged upstream code
   (`documents.pdf_ops`, `documents.converters`, `documents.barcodes`) and writes
   out/upstream/results.json + output PDFs.

Barcodes: `scan_pdf` runs unchanged; only `pdf2image.convert_from_path` (needs poppler,
not present in the Windows reference) is replaced by pypdfium2 (same DPI, RGB PPM in
work_dir like pdf2image). Rust rasterizes with the same pdfium.dll (pypdfium2_raw), so the
comparison only measures zxing-cpp against rxing. `Tag.objects.get_or_create` is replaced by a dummy
(returns the tag name instead of the id).
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import sys
import tempfile
import uuid
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "reference_settings")
for p in (REPO / "reference" / "overlay", REPO / "upstream" / "src"):
    if str(p) not in sys.path:
        sys.path.insert(0, str(p))

import django  # noqa: E402

django.setup()

import pikepdf  # noqa: E402
import pypdfium2 as pdfium  # noqa: E402
from PIL import Image  # noqa: E402

from documents import barcodes as bc_mod  # noqa: E402
from documents import converters  # noqa: E402
from documents import pdf_ops  # noqa: E402

S = REPO / "upstream" / "src" / "documents" / "tests" / "samples"
G = HERE / "samples"
OUT = HERE / "out" / "upstream"


def resolve(rel: str) -> Path:
    if rel.startswith("S/"):
        return S / rel[2:]
    if rel.startswith("G/"):
        return G / rel[2:]
    return REPO / rel


def norm_msg(msg: str) -> str:
    msg = msg.replace("\\", "/")
    for base in (str(REPO).replace("\\", "/") + "/",):
        msg = msg.replace(base, "")
    return msg


def err(e: BaseException) -> dict:
    return {"ok": False, "error_kind": type(e).__name__, "error": norm_msg(str(e))}


# ----------------------------------------------------------------------------------------------
# Helper PDFs
# ----------------------------------------------------------------------------------------------


def make_samples() -> None:
    G.mkdir(parents=True, exist_ok=True)
    odd = S / "double-sided-odd.pdf"
    even = S / "double-sided-even.pdf"

    # 4 pages, info + XMP + bookmark per page
    with pikepdf.new() as pdf:
        for src in (odd, even):
            with pikepdf.open(src) as s:
                pdf.pages.extend(s.pages)
        while len(pdf.pages) > 4:
            del pdf.pages[-1]
        pdf.docinfo["/Title"] = "Outline sample"
        pdf.docinfo["/Author"] = "PaperRust parity"
        with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
            meta["dc:title"] = "Outline sample"
        with pdf.open_outline() as outline:
            for i in range(len(pdf.pages)):
                outline.root.append(pikepdf.OutlineItem(f"Page {i + 1}", i))
        pdf.save(G / "outline.pdf")

    # Inherited /Rotate, /MediaBox, /Resources in the page tree
    with pikepdf.open(odd) as pdf:
        root = pdf.Root.Pages
        first = pdf.pages[0].obj
        root.MediaBox = first.MediaBox
        root.Rotate = 90
        res = first.Resources
        for p in pdf.pages:
            if "/MediaBox" in p.obj:
                del p.obj["/MediaBox"]
            if "/Rotate" in p.obj:
                del p.obj["/Rotate"]
        root.Resources = res
        del first["/Resources"]
        pdf.pages[1].obj.Rotate = 180
        pdf.save(G / "inherited.pdf")

    with pikepdf.open(even) as pdf:
        pdf.save(G / "v14.pdf", force_version="1.4")

    with pikepdf.open(G / "outline.pdf") as pdf:
        pdf.save(G / "owner-only-r4.pdf", encryption=pikepdf.Encryption(owner="owner", user="", R=4, aes=True))
        pdf.save(G / "user-r2.pdf", encryption=pikepdf.Encryption(owner="owner", user="secret", R=2, aes=False, metadata=False))
        pdf.save(G / "user-r4.pdf", encryption=pikepdf.Encryption(owner="owner", user="secret", R=4, aes=True))
        pdf.save(G / "user-r6.pdf", encryption=pikepdf.Encryption(owner="owner", user="secret", R=6))
        pdf.save(G / "user-r6-unicode.pdf", encryption=pikepdf.Encryption(owner="owner", user="Pässwört", R=6))


# ----------------------------------------------------------------------------------------------
# PDF operations
# ----------------------------------------------------------------------------------------------


def run_op(case: dict) -> dict:
    cid = case["id"]
    op = case["op"]
    try:
        if op == "rotate":
            dst = OUT / f"{cid}.pdf"
            pdf_ops.rotate_pdf(resolve(case["src"]), dst, case["degrees"])
            return {"ok": True, "outputs": [dst.name]}
        if op == "remove_pages":
            dst = OUT / f"{cid}.pdf"
            pdf_ops.remove_pages(resolve(case["src"]), dst, case["pages"])
            return {"ok": True, "outputs": [dst.name]}
        if op == "build":
            outputs = []
            for idx, specs in enumerate(case["outputs"]):
                path = OUT / f"{cid}_{idx}.pdf"
                outputs.append(
                    ([pdf_ops.PageSpec(s["page"], s.get("rotate", 0)) for s in specs], (lambda p=path: p)),
                )
            written = pdf_ops.build_pdfs(resolve(case["src"]), outputs)
            return {"ok": True, "outputs": [p.name for p in written]}
        if op == "merge":
            dst = OUT / f"{cid}.pdf"
            skipped = []
            with pdf_ops.PdfMerger() as merger:
                for i, src in enumerate(case["srcs"]):
                    try:
                        merger.add(resolve(src))
                    except Exception as e:  # bulk_edit.merge: document is skipped
                        skipped.append({"index": i, "error_kind": type(e).__name__})
                merger.save(dst)
            return {"ok": True, "outputs": [dst.name], "skipped": skipped}
        if op == "needs_decrypt":
            return {"ok": True, "result": pdf_ops.needs_decrypt(resolve(case["src"]))}
        if op == "decrypt":
            dst = OUT / f"{cid}.pdf"
            called = []

            def make():
                called.append(1)
                return dst

            pdf_ops.decrypt_pdf(resolve(case["src"]), make, case["password"])
            return {"ok": True, "outputs": [dst.name]}
        if op == "validate":
            n = pdf_ops.validate_page_operations(case["operations"], single_output=case["single_output"])
            return {"ok": True, "result": n}
        if op == "tiff_to_pdf":
            work = Path(tempfile.mkdtemp(prefix="plpdfops-tiff-"))
            src = resolve(case["src"])
            tmp_src = work / "in" / src.name
            tmp_src.parent.mkdir()
            shutil.copy2(src, tmp_src)
            pdf = converters.convert_from_tiff_to_pdf(tmp_src, work)
            dst = OUT / f"{cid}.pdf"
            shutil.copy(pdf, dst)
            return {"ok": True, "outputs": [dst.name], "name": pdf.name}
        raise ValueError(f"unknown op {op}")
    except Exception as e:
        return err(e)


# ----------------------------------------------------------------------------------------------
# Barcodes
# ----------------------------------------------------------------------------------------------


def fake_convert_from_path(pdf_path, dpi, output_folder, first_page, last_page, **_kw):
    doc = pdfium.PdfDocument(str(pdf_path))
    try:
        page = doc[first_page - 1]
        img = page.render(scale=dpi / 72).to_pil().convert("RGB")
    finally:
        doc.close()
    path = Path(output_folder) / f"{uuid.uuid4()}-{first_page:02d}.ppm"
    img.save(path)
    im = Image.open(path)
    im.load()
    return [im]


bc_mod.convert_from_path = fake_convert_from_path


class FakeTagManager:
    def get_or_create(self, name__iexact, defaults):
        return SimpleNamespace(pk=defaults["name"], __str__=lambda: defaults["name"]), True


bc_mod.Tag = SimpleNamespace(objects=FakeTagManager())


def barcode_settings(**over) -> SimpleNamespace:
    base = {
        "barcodes_enabled": False,
        "barcode_enable_tiff_support": True,
        "barcode_string": "PATCHT",
        "barcode_retain_split_pages": False,
        "barcode_enable_asn": False,
        "barcode_asn_prefix": "ASN",
        "barcode_upscale": 0.0,
        "barcode_dpi": 300,
        "barcode_max_pages": 0,
        "barcode_enable_tag": False,
        "barcode_tag_mapping": {"TAG:(.*)": "\\g<1>"},
        "barcode_tag_split": False,
        "barcode_store_values": True,
    }
    base.update(over)
    return SimpleNamespace(**base)


class Capture(logging.Handler):
    def __init__(self):
        super().__init__(logging.DEBUG)
        self.records: list[str] = []

    def emit(self, record):
        if record.levelno >= logging.WARNING:
            self.records.append(record.getMessage())


def scan(path: Path, settings, work: Path):
    pdf_path = path
    if path.suffix.lower() in (".tif", ".tiff"):
        pdf_path = converters.convert_from_tiff_to_pdf(path, work)
    codes = bc_mod.scan_pdf(pdf_path, settings, work)
    return pdf_path, codes


def run_barcodes(cases: dict) -> dict:
    out: dict = {"files": {}, "variants": {}}
    for rel in cases["barcode_files"]:
        path = resolve(rel)
        work = Path(tempfile.mkdtemp(prefix="plpdfops-bc-"))
        tmp_src = work / "in" / path.name
        tmp_src.parent.mkdir()
        shutil.copy2(path, tmp_src)
        entry: dict = {}
        try:
            pdf_path, codes = scan(tmp_src, barcode_settings(), work)
            entry["ok"] = True
            entry["barcodes"] = [[c.page, c.value, c.format] for c in codes]
        except Exception as e:
            entry.update(err(e))
            out["files"][rel] = entry
            continue
        entry["configs"] = {}
        for cfg in cases["configs"]:
            over = {k: v for k, v in cfg.items() if k != "id"}
            settings = barcode_settings(**over)
            plugin = bc_mod.BarcodePlugin.__new__(bc_mod.BarcodePlugin)
            plugin.settings = settings
            plugin.barcodes = [bc_mod.Barcode(c.page, c.value, settings, c.format) for c in codes]
            plugin.detect = lambda: None
            plugin.pdf_file = pdf_path
            plugin.input_doc = SimpleNamespace(original_file=tmp_src, mime_type="application/pdf")
            split_dir = Path(tempfile.mkdtemp(prefix="plpdfops-split-"))
            plugin.temp_dir = SimpleNamespace(name=str(split_dir))
            cap = Capture()
            logging.getLogger("paperless.barcodes").addHandler(cap)
            try:
                seps = plugin.get_separation_pages() if settings.barcodes_enabled else {}
                res = {
                    "separators": sorted([[k, v] for k, v in seps.items()]),
                    "asn": plugin.asn if settings.barcode_enable_asn else None,
                    "tags": plugin.tags if settings.barcode_enable_tag else [],
                    "stored": [x.stored() for x in plugin.barcodes],
                }
                if seps:
                    parts = plugin.separate_pages(seps)
                    names = []
                    for p in parts:
                        tgt = OUT / f"bc_{Path(rel).stem}_{cfg['id']}_{p.name}"
                        shutil.copy(p, tgt)
                        names.append(tgt.name)
                    res["parts"] = names
                res["warnings"] = [norm_msg(m) for m in cap.records]
            except Exception as e:
                res = err(e)
            finally:
                logging.getLogger("paperless.barcodes").removeHandler(cap)
            entry["configs"][cfg["id"]] = res
        out["files"][rel] = entry
    for v in cases["scan_variants"]:
        work = Path(tempfile.mkdtemp(prefix="plpdfops-bcv-"))
        settings = barcode_settings(barcode_dpi=v["dpi"], barcode_upscale=v["upscale"], barcode_max_pages=v.get("max_pages", 0))
        try:
            _, codes = scan(resolve(v["file"]), settings, work)
            out["variants"][v["id"]] = {"ok": True, "barcodes": [[c.page, c.value, c.format] for c in codes]}
        except Exception as e:
            out["variants"][v["id"]] = err(e)
    return out


def main() -> None:
    cases = json.loads((HERE / "cases.json").read_text(encoding="utf-8"))
    make_samples()
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    results = {"ops": {}, "barcodes": {}}
    for case in cases["ops"]:
        results["ops"][case["id"]] = run_op(case)
        print(case["id"], results["ops"][case["id"]].get("ok"), results["ops"][case["id"]].get("error", ""))
    only_ops = "--ops-only" in sys.argv
    if not only_ops:
        results["barcodes"] = run_barcodes(cases)
    (OUT / "results.json").write_text(json.dumps(results, indent=1, ensure_ascii=False), encoding="utf-8")
    print("written", OUT / "results.json")


if __name__ == "__main__":
    main()
