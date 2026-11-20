"""Creates samples/ and reference.json with upstream results (paperless-ngx v3.4.2) for pl-ocr.

Invocation (from the repo root directory, Git Bash):

    source reference/env.sh
    reference/venv/Scripts/python.exe tests/parity/pl-ocr/gen_reference.py

No DB access: `OcrConfig()` is replaced by a namespace object (same fields),
`ocrmypdf.ocr`, `_convert_image_to_pdfa`, `_convert_pdf_to_pdfa` and `extract_text` are replaced by dummies as in
upstream `test_parse_modes.py`. Really running: `construct_ocrmypdf_parameters`
(incl. Pillow for DPI/alpha), `parse()` control logic, `should_produce_archive`, `post_process_text`,
`read_file_handle_unicode_errors`, OCRmyPDF `merge_sidecars`.
"""

from __future__ import annotations

import io
import json
import logging
import os
import shutil
import struct
import sys
import tempfile
import zlib
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "reference_settings")
for p in (REPO / "reference" / "overlay", REPO / "upstream" / "src"):
    if str(p) not in sys.path:
        sys.path.insert(0, str(p))

import django  # noqa: E402

django.setup()

import ocrmypdf  # noqa: E402
from django.test import override_settings  # noqa: E402
from ocrmypdf import exceptions as oexc  # noqa: E402
from PIL import Image  # noqa: E402
from PIL import TiffImagePlugin  # noqa: E402

from documents import consumer  # noqa: E402
from paperless.parsers import utils as putils  # noqa: E402
from paperless.parsers.tesseract import RasterisedDocumentParser  # noqa: E402

SAMPLES = HERE / "samples"
UPSTREAM_SAMPLES = REPO / "upstream" / "src" / "paperless" / "tests" / "samples" / "tesseract"
TMP = Path(tempfile.mkdtemp(prefix="plocr-ref-"))
log = logging.getLogger("plocr")


def norm(v):
    if isinstance(v, Path):
        v = str(v)
    if isinstance(v, str):
        v = v.replace(str(TMP), "<TMP>").replace("\\", "/")
        for d in (str(SAMPLES), str(UPSTREAM_SAMPLES)):
            v = v.replace(d.replace("\\", "/"), "<SAMPLES>")
        return v
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [norm(x) for x in v]
    return v


DEFAULT_CFG = dict(
    output_type="pdfa",
    pages=None,
    language="eng",
    mode="auto",
    archive_file_generation="auto",
    image_dpi=None,
    clean="clean",
    deskew=True,
    rotate=True,
    rotate_threshold=12.0,
    max_image_pixel=None,
    color_conversion_strategy="RGB",
    user_args=None,
)


def make_parser(cfg: dict) -> RasterisedDocumentParser:
    p = RasterisedDocumentParser.__new__(RasterisedDocumentParser)
    p.tempdir = TMP
    p.settings = SimpleNamespace(**{**DEFAULT_CFG, **cfg})
    p.archive_path = None
    p.text = None
    p.date = None
    p.log = log
    return p


# ---------------------------------------------------------------------------------------------
# Sample images
# ---------------------------------------------------------------------------------------------


def png_chunk(ty: bytes, body: bytes) -> bytes:
    return struct.pack(">I", len(body)) + ty + body + struct.pack(">I", zlib.crc32(ty + body) & 0xFFFFFFFF)


def png_with_phys(path: Path, unit: int, ppu: int) -> None:
    buf = io.BytesIO()
    Image.new("RGB", (120, 60), "white").save(buf, "PNG")
    data = buf.getvalue()
    i = data.index(b"IDAT") - 4
    data = data[:i] + png_chunk(b"pHYs", struct.pack(">IIB", ppu, ppu, unit)) + data[i:]
    path.write_bytes(data)


def jpeg_jfif_unit(path: Path, unit: int, density: int) -> None:
    buf = io.BytesIO()
    Image.new("RGB", (90, 40), "white").save(buf, "JPEG", dpi=(density, density))
    data = bytearray(buf.getvalue())
    j = data.index(b"JFIF\0")
    data[j + 7] = unit
    path.write_bytes(bytes(data))


def make_samples() -> list[str]:
    SAMPLES.mkdir(exist_ok=True)
    out = []

    def add(name):
        out.append(name)
        return SAMPLES / name

    Image.new("RGB", (200, 100), "white").save(add("png-300dpi.png"), dpi=(300, 300))
    Image.new("RGB", (200, 100), "white").save(add("png-nodpi.png"))
    Image.new("RGBA", (200, 100), (255, 255, 255, 128)).save(add("png-rgba-150dpi.png"), dpi=(150, 150))
    Image.new("LA", (200, 100)).save(add("png-la.png"))
    Image.new("P", (200, 100)).save(add("png-palette.png"), transparency=0)
    Image.new("I;16", (200, 100)).save(add("png-gray16.png"), dpi=(72.4, 72.4))
    png_with_phys(add("png-phys-unit0.png"), 0, 5000)
    png_with_phys(add("png-phys-meter-half.png"), 1, 5925)  # 150.495 dpi
    Image.new("RGB", (90, 40), "white").save(add("jpeg-200dpi.jpg"), dpi=(200, 200))
    jpeg_jfif_unit(add("jpeg-unit2.jpg"), 2, 100)
    jpeg_jfif_unit(add("jpeg-unit0.jpg"), 0, 1)
    exif = Image.Exif()
    exif[0x011A] = 240.0
    exif[0x011B] = 240.0
    exif[0x0128] = 2
    im = Image.new("RGB", (90, 40), "white")
    buf = io.BytesIO()
    im.save(buf, "JPEG", exif=exif.tobytes())
    data = bytearray(buf.getvalue())
    if b"JFIF\0" in data:
        j = data.index(b"JFIF\0")
        data[j + 7] = 0
    add("jpeg-exif-240.jpg").write_bytes(bytes(data))
    exif2 = Image.Exif()
    exif2[0x0112] = 6
    buf = io.BytesIO()
    im.save(buf, "JPEG", exif=exif2.tobytes())
    data = bytearray(buf.getvalue())
    if b"JFIF\0" in data:
        j = data.index(b"JFIF\0")
        data[j + 7] = 0
    add("jpeg-exif-nores.jpg").write_bytes(bytes(data))
    Image.new("L", (90, 40), 255).save(add("jpeg-gray.jpg"))
    Image.new("RGB", (200, 100), "white").save(add("tiff-dpi.tif"), dpi=(300, 300))
    Image.new("RGB", (200, 100), "white").save(add("tiff-nores.tif"))
    info = TiffImagePlugin.ImageFileDirectory_v2()
    info[282] = 118.0
    info[283] = 118.0
    info[296] = 3
    Image.new("RGB", (200, 100), "white").save(add("tiff-cm.tif"), tiffinfo=info)
    info = TiffImagePlugin.ImageFileDirectory_v2()
    info[282] = 300.0
    info[283] = 300.0
    info[296] = 1
    Image.new("RGB", (200, 100), "white").save(add("tiff-unit1.tif"), tiffinfo=info)
    Image.new("RGBA", (200, 100)).save(add("tiff-rgba.tif"), dpi=(200, 200))
    Image.new("LA", (200, 100)).save(add("tiff-la.tif"))
    frames = [Image.new("L", (100, 50), 255) for _ in range(3)]
    frames[0].save(add("tiff-3pages.tif"), save_all=True, append_images=frames[1:], dpi=(150, 150))
    Image.new("1", (200, 100), 1).save(add("tiff-bilevel-g4.tif"), compression="group4", dpi=(200, 200))
    Image.new("RGB", (200, 100), "white").save(add("bmp-dpi.bmp"), dpi=(96, 96))
    Image.new("RGB", (200, 100), "white").save(add("bmp-nodpi.bmp"))
    Image.new("RGBA", (200, 100)).save(add("bmp-rgba.bmp"))
    Image.new("P", (200, 100)).save(add("gif-plain.gif"))
    Image.new("RGB", (200, 100), "white").save(add("webp-rgb.webp"))
    Image.new("RGBA", (200, 100), (0, 0, 0, 0)).save(add("webp-rgba-lossless.webp"), lossless=True)
    Image.new("RGBA", (200, 100), (0, 0, 0, 10)).save(add("webp-rgba-lossy.webp"))
    try:
        import pillow_heif

        pillow_heif.register_heif_opener()
        Image.new("RGB", (64, 32), "white").save(add("heic-rgb.heic"))
        Image.new("RGBA", (64, 32), (1, 2, 3, 4)).save(add("heic-rgba.heic"))
    except Exception as e:  # pragma: no cover
        print("HEIC skipped:", e)
    return out


def image_probes(names: list[str]) -> dict:
    res = {}
    p = make_parser({})
    files = [(n, SAMPLES / n) for n in names] + [
        ("upstream:" + f.name, f) for f in sorted(UPSTREAM_SAMPLES.iterdir()) if f.suffix.lower() in {".png", ".jpg", ".tif", ".tiff", ".bmp", ".gif", ".webp", ".heic"}
    ]
    try:
        import pillow_heif

        pillow_heif.register_heif_opener()
    except Exception:
        pass
    for name, f in files:
        entry = {}
        entry["get_dpi"] = p.get_dpi(f)
        entry["calculate_a4_dpi"] = p.calculate_a4_dpi(f)
        try:
            entry["has_alpha"] = p.has_alpha(f)
        except Exception as e:
            entry["has_alpha"] = f"ERR {e.__class__.__name__}"
        with Image.open(f) as im:
            entry["size"] = list(im.size)
            entry["mode"] = im.mode
        res[name] = entry
    return res


# ---------------------------------------------------------------------------------------------
# construct_ocrmypdf_parameters
# ---------------------------------------------------------------------------------------------

ARG_CONFIGS = [
    {},
    {"mode": "force"},
    {"mode": "redo"},
    {"mode": "off"},
    {"mode": "redo", "clean": "clean-final", "deskew": True},
    {"mode": "auto", "clean": "clean-final"},
    {"mode": "force", "clean": "none", "deskew": False, "rotate": False},
    {"rotate_threshold": 7.5},
    {"pages": 3},
    {"pages": 0},
    {"output_type": "pdf"},
    {"output_type": "pdfa-1", "color_conversion_strategy": "CMYK"},
    {"output_type": "pdfa-3", "color_conversion_strategy": "LeaveColorUnchanged"},
    {"max_image_pixel": -1},
    {"max_image_pixel": 0},
    {"max_image_pixel": 2_500_000.5},
    {"user_args": {}},
    {"user_args": {"continue_on_soft_render_error": True, "language": "deu", "max_image_mpixels": 5}},
    {"user_args": [1, 2]},
    {"user_args": {"max_image_mpixels": 5}, "max_image_pixel": 1_000_000},
    {"language": "deu+eng", "image_dpi": 150},
]
ARG_REQUESTS = [
    ("application/pdf", None, False, False),
    ("application/pdf", None, True, False),
    ("application/pdf", None, False, True),
    ("application/pdf", None, True, True),
    ("image/png", "png-300dpi.png", False, False),
    ("image/png", "png-nodpi.png", False, False),
    ("image/png", "png-rgba-150dpi.png", False, False),
    ("image/jpeg", "jpeg-unit0.jpg", False, False),
    ("image/tiff", "tiff-nores.tif", False, True),
    ("image/webp", "webp-rgb.webp", True, False),
]


def args_matrix() -> list:
    out = []
    for cfg in ARG_CONFIGS:
        for mime, sample, safe, skip in ARG_REQUESTS:
            inp = SAMPLES / sample if sample else Path("input.pdf")
            p = make_parser(cfg)
            with override_settings(THREADS_PER_WORKER=2, MAX_IMAGE_PIXELS=None):
                try:
                    args = p.construct_ocrmypdf_parameters(
                        inp, mime, Path("output.pdf"), Path("sidecar.txt"), safe_fallback=safe, skip_text=skip
                    )
                    result = {"args": norm({k: v for k, v in args.items()}), "keys": list(args.keys())}
                except Exception as e:
                    result = {"error": f"{e.__class__.__name__}: {e}"}
            out.append({"cfg": cfg, "mime": mime, "sample": sample, "safe_fallback": safe, "skip_text": skip, **result})
    return out


# ---------------------------------------------------------------------------------------------
# parse() control logic
# ---------------------------------------------------------------------------------------------

LONG = "This is a test document with enough text. " * 5


def exc(name: str | None):
    if name is None:
        return None
    return {
        "DigitalSignatureError": oexc.DigitalSignatureError(),
        "EncryptedPdfError": oexc.EncryptedPdfError(),
        "PriorOcrFoundError": oexc.PriorOcrFoundError("page already has text!"),
        "InputFileError": oexc.InputFileError("bad input"),
        "TaggedPDFError": oexc.TaggedPDFError(),
        "SubprocessOutputError": oexc.SubprocessOutputError("Ghostscript PDF/A rendering failed"),
        "SubprocessOutputError2": oexc.SubprocessOutputError("tesseract crashed"),
        "ValueError": ValueError("boom"),
    }[name]


SCENARIOS = []
for mode in ("auto", "force", "redo", "off"):
    for produce in (True, False):
        for texts, tagged in (([LONG], False), ([None, LONG], False), (["short", "ocr text"], False), (["short"], True), ([None, None, None], False)):
            SCENARIOS.append(dict(mode=mode, produce=produce, mime="application/pdf", texts=texts, tagged=tagged, ocr=[]))
for first in ("DigitalSignatureError", "EncryptedPdfError", "PriorOcrFoundError", "InputFileError", "TaggedPDFError", "SubprocessOutputError", "SubprocessOutputError2", "ValueError"):
    for texts in ([LONG], [None, "fallback text"]):
        for second in (None, "ValueError"):
            SCENARIOS.append(dict(mode="auto", produce=True, mime="application/pdf", texts=texts, tagged=False, ocr=[first, second]))
for mode in ("auto", "off", "force"):
    for produce in (True, False):
        SCENARIOS.append(dict(mode=mode, produce=produce, mime="image/png", sample="png-300dpi.png", texts=["img text"], tagged=False, ocr=[]))
SCENARIOS.append(dict(mode="off", produce=True, mime="image/png", sample="png-300dpi.png", texts=[], tagged=False, ocr=[], img2pdf="ValueError"))
SCENARIOS.append(dict(mode="off", produce=True, mime="application/pdf", texts=[LONG], tagged=False, ocr=[], gs="SubprocessOutputError"))
SCENARIOS.append(dict(mode="off", produce=True, mime="application/pdf", texts=[LONG], tagged=False, ocr=[], gs="ValueError"))
SCENARIOS.append(dict(mode="auto", produce=True, mime="image/png", sample="png-nodpi.png", texts=["x"], tagged=False, ocr=[], cfg={"image_dpi": None}))


def run_scenario(s: dict) -> dict:
    p = make_parser({"mode": s["mode"], **s.get("cfg", {})})
    texts = list(s["texts"])
    extract_calls = []

    def fake_extract(sidecar_file, pdf_file):
        extract_calls.append(norm(sidecar_file) if sidecar_file is not None else None)
        if len(texts) > 1:
            return texts.pop(0)
        return texts[0] if texts else None

    side = [exc(n) for n in s["ocr"]]
    calls = []

    def fake_ocr(**kwargs):
        calls.append(norm(dict(kwargs)))
        if side:
            e = side.pop(0)
            if e is not None:
                raise e

    def fake_img(path):
        if s.get("img2pdf"):
            raise exc(s["img2pdf"])
        return TMP / "archive.pdf"

    def fake_gs(inp, out):
        if s.get("gs"):
            raise exc(s["gs"])

    doc = SAMPLES / s["sample"] if s.get("sample") else Path("doc.pdf")
    with (
        override_settings(THREADS_PER_WORKER=1, MAX_IMAGE_PIXELS=None),
        mock.patch.object(p, "extract_text", side_effect=fake_extract),
        mock.patch.object(putils, "is_tagged_pdf", return_value=s["tagged"]),
        mock.patch("ocrmypdf.ocr", side_effect=fake_ocr),
        mock.patch.object(p, "_convert_image_to_pdfa", side_effect=fake_img),
        mock.patch.object(p, "_convert_pdf_to_pdfa", side_effect=fake_gs),
    ):
        try:
            p.parse(doc, s["mime"], produce_archive=s["produce"])
            result = {"text": p.get_text(), "archive": norm(p.archive_path) if p.archive_path else None}
        except Exception as e:
            result = {"error": f"{e.__class__.__name__}: {e}"}
    return {**{k: v for k, v in s.items()}, "calls": calls, "extract_calls": extract_calls, **result}


# ---------------------------------------------------------------------------------------------
# Others
# ---------------------------------------------------------------------------------------------

TEXTS = [
    "",
    "   ",
    "\x0c\x0c",
    "  a\t\tb  \n   c  \n",
    "a \n",
    "a  \n\n  ",
    "x\0y",
    "line1 \r\n  line2  end ",
    "  lead",
    "a\x1cb\x1dc",
    "trail \t",
    "multi\n\n\n  para \n",
]

BYTES = [
    b"plain",
    b"\xef\xbb\xbfbom8",
    b"\xff\xfeu\x00t\x00f\x00",
    b"\xfe\xff\x00b\x00e",
    b"bad\xffutf8",
    b"nul\x00byte",
    b"\xef\xbb\xbfbad\xff",
]


def sidecars() -> list:
    from ocrmypdf._pipeline import merge_sidecars

    cases = [
        ["p1\x0c", "p2", None, None, None],
        [None, "x"],
        [None, None],
        ["a\x0c", None, "c\x0c", None],
        ["only"],
    ]
    out = []
    for case in cases:
        d = TMP / "sc"
        if d.exists():
            shutil.rmtree(d)
        d.mkdir()
        files = []
        for i, t in enumerate(case):
            if t is None:
                files.append(None)
            else:
                f = d / f"{i}.txt"
                f.write_text(t, encoding="utf-8")
                files.append(f)
        ctx = SimpleNamespace(get_path=lambda name: d / name)
        res = merge_sidecars(files, ctx).read_text(encoding="utf-8")
        out.append({"pages": case, "merged": res})
    return out


def archive_matrix() -> list:
    out = []
    for rendition in (True, False):
        for can in (True, False):
            for gen in ("auto", "always", "never"):
                for mime in ("application/pdf", "image/png", "text/plain", "message/rfc822"):
                    for born in (True, False):
                        parser = SimpleNamespace(requires_pdf_rendition=rendition, can_produce_archive=can)
                        with (
                            mock.patch.object(consumer, "OcrConfig", return_value=SimpleNamespace(archive_file_generation=gen)),
                            mock.patch.object(consumer, "pdf_born_digital_text", return_value=("t", born)),
                        ):
                            r = consumer.should_produce_archive(parser, mime, Path("x"))
                        out.append(dict(requires_pdf_rendition=rendition, can_produce_archive=can, generation=gen, mime=mime, born_digital=born, result=r))
    return out


def main() -> None:
    names = make_samples()
    ref = {
        "images": image_probes(names),
        "args": args_matrix(),
        "parse": [run_scenario(s) for s in SCENARIOS],
        "post_process_text": [{"in": t, "out": putils.post_process_text(t)} for t in TEXTS],
        "read_file": [],
        "sidecars": sidecars(),
        "should_produce_archive": archive_matrix(),
        "thumbnail_dpi_constants": [500, 5000, 150, 300, 2],
    }
    for b in BYTES:
        f = TMP / "rf.txt"
        f.write_bytes(b)
        ref["read_file"].append({"hex": b.hex(), "out": putils.read_file_handle_unicode_errors(f)})
    (HERE / "reference.json").write_text(json.dumps(ref, indent=1, ensure_ascii=False), encoding="utf-8")
    print("images", len(ref["images"]), "args", len(ref["args"]), "parse", len(ref["parse"]))
    shutil.rmtree(TMP, ignore_errors=True)


if __name__ == "__main__":
    main()
