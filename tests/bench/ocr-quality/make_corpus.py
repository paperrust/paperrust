"""OCR test corpus with known text that behaves like real paper scans (no real data).

    python tests/bench/ocr-quality/make_corpus.py [outdir] [--seed N]

Creates in `outdir` (default results/bench/ocr-quality/corpus):
- single pages as images (PNG/JPEG, pixels only) + `<name>.gt.txt` (reference text in reading order)
- multi-page, image-only PDFs (`*.pdf`, no /Font, no text, no /Info) + `.gt.txt`
- `manifest.json`: {"images": [...], "pdfs": [...]}, each element with `group`:
  - `clean`  – small control group: clean renderings (like a digital printout before scanning)
  - `scan`   – majority: print-and-scan simulation (below)
(The third group `real` – real scans in tests/bench/ocr-quality/real/ – is deliberately NOT
copied here; it stays git-ignored in its place, see run.sh/real_report.py.)

All texts are fictitious (sample company, sample street …). Deterministic: fixed seed (`--seed`,
default 20261109) → same pixels with the same Pillow/numpy version and the same fonts.

Print-and-scan simulation (parameters per page drawn from the seed, ranges see `ScanParams`):
1. Render the page at target print quality (300 dpi), stroke possibly thinner (min filter on white) and paler
   (toner coverage 0.55–0.95).
2. Geometry in a backward mapping: rotation 0.3–3°, trapezoid/perspective distortion, curvature at the
   binding (lines bend at the left edge), margin outside the sheet = scanner lid gray.
3. Paper: gray base tone 0.86–0.96, low-frequency paper texture, uneven illumination,
   shadow at the binding (exponential towards the left edge).
4. Defocus (Gauss 0.5–1.2 px), sensor noise, toner splatter/dust.
5. Scanning at 300 or 200 dpi (downscaling), JPEG with typical scanner quality (70–90), or
   "fax": 150 dpi bitonal (threshold + noise), PNG 1 bit.

The output files are checked for metadata (`verify_no_text`): PNG without tEXt/iTXt/zTXt/eXIf,
JPEG without APP1 (EXIF/XMP) and COM, PDF without /Font, text operators and /Info.
Requires Pillow + numpy (e.g. reference/venv) and TrueType fonts (Windows: C:/Windows/Fonts,
otherwise Liberation/DejaVu, see FONT_FALLBACK).
"""

from __future__ import annotations

import io
import json
import re
import shutil
import sys
import zlib
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

REPO = Path(__file__).resolve().parents[3]
A4 = (8.27, 11.69)  # inch
RENDER_DPI = 300

FONT_DIRS = [
    Path("C:/Windows/Fonts"),
    Path("/usr/share/fonts/truetype/liberation"),
    Path("/usr/share/fonts/truetype/dejavu"),
    Path("/Library/Fonts"),
]
FONT_FALLBACK = {
    "arial.ttf": "LiberationSans-Regular.ttf",
    "arialbd.ttf": "LiberationSans-Bold.ttf",
    "times.ttf": "LiberationSerif-Regular.ttf",
    "timesbd.ttf": "LiberationSerif-Bold.ttf",
    "cour.ttf": "LiberationMono-Regular.ttf",
    "georgia.ttf": "DejaVuSerif.ttf",
    "verdana.ttf": "DejaVuSans.ttf",
    "verdanab.ttf": "DejaVuSans-Bold.ttf",
    "calibri.ttf": "DejaVuSans.ttf",
    "calibrib.ttf": "DejaVuSans-Bold.ttf",
}

# ------------------------------------------------------------------------------------------------
# Texts (fictitious)
# ------------------------------------------------------------------------------------------------

LETTER_DE_BODY = """Sehr geehrte Frau Beispiel,

für die im Februar ausgeführten Arbeiten an der Beleuchtung im Treppenhaus erlauben wir uns, Ihnen den folgenden Betrag in Rechnung zu stellen. Die Arbeiten umfassten den Austausch von zwölf Leuchten, die Prüfung der Notbeleuchtung sowie das Erstellen des Prüfprotokolls.

Bitte überweisen Sie den Gesamtbetrag von CHF 1'284.60 bis zum 13. April 2026 auf das unten genannte Konto. Für Rückfragen stehen wir Ihnen gerne zur Verfügung; außerdem weisen wir darauf hin, dass die nächste Wartung im Frühjahr 2027 fällig ist.

Mit freundlichen Grüßen

Jürgen Weiß
Abteilung Buchhaltung"""

LETTER_EN_BODY = """Dear Mr Sample,

thank you for your order of 28 May 2026. We are pleased to confirm the delivery of the following items: three boxes of printer paper, two packs of blue ballpoint pens and one desk lamp with a flexible arm. The goods will be shipped within five working days.

Please check the delivery carefully and report any damage within fourteen days. Our quality team reviews every complaint and will contact you as quickly as possible.

The total amount of USD 249.90 will be charged to your account on delivery.

Kind regards,

Margaret Collins
Customer Service"""

TERMS_DE = """Allgemeine Geschäftsbedingungen (Auszug)

§ 3 Zahlungsbedingungen. Rechnungen sind ohne Abzug innert 30 Tagen zahlbar. Bei Zahlungsverzug wird ab der zweiten Mahnung eine Gebühr von CHF 20.00 erhoben. Die Mehrwertsteuer (8,1 %) ist im Preis enthalten.

§ 4 Gewährleistung. Mängel sind unverzüglich, spätestens jedoch zehn Tage nach Erhalt der Ware, schriftlich anzuzeigen. Äußerlich erkennbare Schäden müssen sofort gemeldet werden. Für Verschleißteile gilt eine Frist von sechs Monaten.

§ 5 Eigentumsvorbehalt. Die gelieferte Ware bleibt bis zur vollständigen Bezahlung Eigentum der Musterfirma Elektro GmbH.

§ 6 Gerichtsstand ist Zürich. Es gilt ausschließlich schweizerisches Recht unter Ausschluss des Übereinkommens über den internationalen Warenkauf."""

COLUMNS_DE = (
    "Mitteilungen des Quartiervereins Musterhausen · Ausgabe 3/2026",
    """Sommerfest am Seeufer

Das diesjährige Sommerfest findet am Samstag, 22. August 2026, ab 14 Uhr auf der Wiese beim Bootshaus statt. Für Grilladen, Getränke und Kuchen ist gesorgt. Wer einen Beitrag zum Dessertbuffet leisten möchte, meldet sich bitte bis zum 15. August bei Frau Müller.

Bei schlechtem Wetter weichen wir in die Turnhalle des Schulhauses Süd aus. Die Kinderbetreuung übernimmt wie jedes Jahr die Pfadi.""",
    """Neue Öffnungszeiten

Ab dem 1. September ist die Quartierbibliothek am Dienstag und Donnerstag von 15.30 bis 19.00 Uhr sowie am Samstag von 9.00 bis 12.00 Uhr geöffnet. Die Rückgabebox steht weiterhin rund um die Uhr zur Verfügung.

Jahresbeitrag

Der Mitgliederbeitrag für 2026 beträgt CHF 35.00 für Einzelpersonen und CHF 55.00 für Familien. Einzahlungsscheine liegen dieser Ausgabe bei.""",
)

POLICY_MIXED = """Versicherungspolice / Insurance policy

Policennummer / Policy number: HV-778-2031-04
Versicherte Person / Insured person: Anna Beispiel, geb. 07.09.1984
Laufzeit / Term: 01.01.2026 - 31.12.2026

Die Jahresprämie beträgt CHF 412.50 und ist am 1. Januar fällig.
The annual premium amounts to CHF 412.50 and is due on 1 January.
Änderungen der Adresse sind uns umgehend mitzuteilen.
Please notify us immediately of any change of address.
Selbstbehalt / Deductible: CHF 300.00 pro Schadenfall / per claim"""

FAX_DE = """TELEFAX

An: Hausverwaltung Muster AG, z. Hd. Herrn Kälin
Von: Schreinerei Holzwurm GmbH
Datum: 03.11.2026        Seiten: 1

Betreff: Offerte Nr. 4471 - Ersatz Wohnungstüre 2. OG

Sehr geehrter Herr Kälin

Gerne offerieren wir Ihnen den Ersatz der Wohnungstüre gemäß Besichtigung vom 28.10.2026:
Türblatt Eiche furniert, inkl. Zarge und Montage: CHF 2'480.00
Entsorgung der alten Türe: CHF 120.00
Total exkl. MWST: CHF 2'600.00

Die Lieferfrist beträgt ca. drei Wochen ab Bestellung.

Freundliche Grüße
Peter Holzwurm"""

INVOICE_ROWS = [
    ["Pos.", "Artikel", "Menge", "Preis", "Betrag"],
    ["1", "Schraube M4x20 verzinkt", "100", "0.12", "12.00"],
    ["2", "Dübel 6 mm", "50", "0.08", "4.00"],
    ["3", "Kabelbinder schwarz", "200", "0.03", "6.00"],
    ["4", "Wasserwaage 60 cm", "1", "24.90", "24.90"],
    ["5", "Bohrer-Set Holz (5-teilig)", "2", "17.50", "35.00"],
    ["6", "Arbeitsstunde Monteur", "3.5", "96.00", "336.00"],
]

FOOTER_DE = "Musterfirma Elektro GmbH · Musterstraße 12 · 8000 Zürich · Tel. 044 000 00 00 · IBAN CH93 0076 2011 6238 5295 7 · UID CHE-123.456.789 MWST"


# ------------------------------------------------------------------------------------------------
# Page layout
# ------------------------------------------------------------------------------------------------


def font_path(name: str) -> str:
    for d in FONT_DIRS:
        for n in (name, FONT_FALLBACK.get(name, "")):
            if n and (d / n).is_file():
                return str(d / n)
    raise SystemExit(f"font {name} not found (searched {[str(d) for d in FONT_DIRS]})")


class Page:
    """A4 page; text is placed in reading order and simultaneously collected as reference."""

    def __init__(self, dpi: int = RENDER_DPI):
        self.dpi = dpi
        self.img = Image.new("L", (round(A4[0] * dpi), round(A4[1] * dpi)), 255)
        self.d = ImageDraw.Draw(self.img)
        self.gt: list[str] = []
        self._fonts: dict = {}

    def font(self, name: str, pt: float):
        key = (name, pt)
        if key not in self._fonts:
            self._fonts[key] = ImageFont.truetype(font_path(name), round(pt * self.dpi / 72))
        return self._fonts[key]

    def px(self, inch: float) -> int:
        return round(inch * self.dpi)

    def wrap(self, text: str, f, width_px: int) -> list[str]:
        out: list[str] = []
        for para in text.split("\n"):
            if not para.strip():
                out.append("")
                continue
            line = ""
            for word in para.split(" "):
                cand = f"{line} {word}" if line else word
                if self.d.textlength(cand, font=f) <= width_px or not line:
                    line = cand
                else:
                    out.append(line)
                    line = word
            out.append(line)
        return out

    def block(self, text: str, font: str, pt: float, x: float, y: float, w: float, align: str = "left", spacing: float = 1.3) -> float:
        """Text block from (x, y) inches, width w inches; returns the new y (inches)."""
        f = self.font(font, pt)
        lines = self.wrap(text, f, self.px(w))
        lh = pt * spacing / 72
        for line in lines:
            if line:
                tw = self.d.textlength(line, font=f)
                xx = self.px(x) if align == "left" else self.px(x + w) - tw
                self.d.text((xx, self.px(y)), line, font=f, fill=0)
            y += lh
        self.gt.append("\n".join(lines))
        return y

    def hline(self, x: float, y: float, w: float, thick: float = 0.01):
        self.d.rectangle([self.px(x), self.px(y), self.px(x + w), self.px(y + thick)], fill=0)

    def text_gt(self) -> str:
        return "\n\n".join(t for t in self.gt if t.strip())


def letterhead(p: Page, small: bool = False) -> float:
    # Logo block + company name (letterhead)
    p.d.rectangle([p.px(0.8), p.px(0.6), p.px(1.35), p.px(1.15)], fill=0)
    p.d.ellipse([p.px(0.9), p.px(0.7), p.px(1.25), p.px(1.05)], fill=255)
    p.block("Musterfirma Elektro GmbH", "arialbd.ttf", 16 if not small else 13, 1.5, 0.62, 5)
    p.block("Installationen · Beleuchtung · Service", "arial.ttf", 9, 1.5, 0.95, 5)
    p.hline(0.8, 1.3, 6.67, 0.015)
    return 1.45


def footer(p: Page):
    p.hline(0.8, 10.9, 6.67, 0.008)
    p.block(FOOTER_DE, "arial.ttf", 6.5, 0.8, 10.97, 6.67)


def page_letter_de(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    y = letterhead(p)
    p.block("Musterfirma Elektro GmbH, Musterstraße 12, 8000 Zürich", "arial.ttf", 7, 0.8, 1.75, 4)
    p.block("Frau\nAnna Beispiel\nRosenweg 7a\n8045 Zürich", "arial.ttf", 11, 0.8, 1.95, 3.5)
    p.block("Zürich, 14. März 2026", "arial.ttf", 11, 4.5, 3.1, 2.97, align="right")
    y = p.block("Rechnung Nr. 2026-0417 · Kundennummer 55 128", "arialbd.ttf", 12, 0.8, 3.55, 6.67)
    p.block(LETTER_DE_BODY, "arial.ttf", 11, 0.8, y + 0.25, 6.67)
    footer(p)
    return p


def page_letter_en(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    p.block("Example Supplies Ltd.", "timesbd.ttf", 18, 0.9, 0.7, 5)
    p.block("42 Sample Road · Springfield · Phone 555-0142", "times.ttf", 10, 0.9, 1.05, 6)
    p.hline(0.9, 1.35, 6.5, 0.01)
    p.block("Mr John Sample\n17 Example Street\nLittle Whinging", "times.ttf", 12, 0.9, 1.8, 3.5)
    p.block("2 June 2026", "times.ttf", 12, 4.5, 2.9, 2.9, align="right")
    y = p.block("Order confirmation 88-3171", "timesbd.ttf", 13, 0.9, 3.35, 6.5)
    p.block(LETTER_EN_BODY, "times.ttf", 12, 0.9, y + 0.25, 6.5)
    return p


def page_invoice_de(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    letterhead(p, small=True)
    p.block("Rechnung", "arialbd.ttf", 18, 0.8, 1.7, 4)
    p.block("Rechnungsnummer: 2026-0532\nRechnungsdatum: 02.10.2026\nLieferdatum: 29.09.2026\nKundennummer: 55 128", "arial.ttf", 10, 0.8, 2.15, 3.5)
    p.block("Hausverwaltung Muster AG\nBahnhofplatz 3\n3000 Bern", "arial.ttf", 10, 4.6, 2.15, 2.8)
    # Table with grid
    cols = [0.55, 2.75, 0.9, 1.1, 1.37]
    xs = [0.8]
    for c in cols:
        xs.append(xs[-1] + c)
    y0, rh = 3.3, 0.36
    pt = 10
    rows_gt = []
    for r, row in enumerate(INVOICE_ROWS):
        f = p.font("arialbd.ttf" if r == 0 else "arial.ttf", pt)
        y = y0 + r * rh
        for c, cell in enumerate(row):
            tw = p.d.textlength(cell, font=f)
            x = p.px(xs[c] + 0.08) if c in (0, 1) else p.px(xs[c + 1] - 0.08) - tw
            p.d.text((x, p.px(y + 0.09)), cell, font=f, fill=0)
        rows_gt.append(" ".join(row))
    lw = max(1, p.dpi // 150)
    for r in range(len(INVOICE_ROWS) + 1):
        yy = p.px(y0 + r * rh)
        p.d.line([(p.px(xs[0]), yy), (p.px(xs[-1]), yy)], fill=0, width=lw)
    for x in xs:
        p.d.line([(p.px(x), p.px(y0)), (p.px(x), p.px(y0 + len(INVOICE_ROWS) * rh))], fill=0, width=lw)
    p.gt.append("\n".join(rows_gt))
    y = y0 + len(INVOICE_ROWS) * rh + 0.25
    p.block("Zwischensumme CHF 417.90\nMWST 8,1 % CHF 33.85\nTotal CHF 451.75", "arialbd.ttf", 10, 4.5, y, 2.97, align="right")
    p.block("Zahlbar innert 30 Tagen ohne Abzug. Vielen Dank für Ihren Auftrag!", "arial.ttf", 10, 0.8, y + 0.9, 6.67)
    footer(p)
    return p


def page_columns_de(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    title, left, right = COLUMNS_DE
    p.block(title, "georgia.ttf", 14, 0.8, 0.8, 6.67)
    p.hline(0.8, 1.2, 6.67, 0.01)
    p.block(left, "georgia.ttf", 10, 0.8, 1.45, 3.15)
    p.block(right, "georgia.ttf", 10, 4.3, 1.45, 3.17)
    p.block("Redaktion: Quartierverein Musterhausen, Postfach 12, 8049 Zürich · Druck: Muster Druck AG", "georgia.ttf", 7, 0.8, 10.95, 6.67)
    return p


def page_terms_de(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    p.block(TERMS_DE, "times.ttf", 8.5, 0.8, 0.8, 6.67, spacing=1.2)
    p.block(POLICY_MIXED, "verdana.ttf", 9.5, 0.8, 4.2, 6.67)
    footer(p)
    return p


def page_fax_de(dpi=RENDER_DPI) -> Page:
    p = Page(dpi)
    p.block(FAX_DE, "cour.ttf", 11, 0.9, 0.9, 6.5)
    return p


# ------------------------------------------------------------------------------------------------
# Print-and-scan simulation
# ------------------------------------------------------------------------------------------------


@dataclass
class ScanParams:
    angle: float  # degrees, 0.3–3 (sign random)
    keystone: float  # relative width change top/bottom
    curl: float  # curvature at the binding (inches, displacement at the edge)
    shadow: float  # binding shadow (0–0.45)
    paper: float  # Paper base tone
    fade: float  # toner coverage
    thin: bool  # thinner strokes
    blur: float  # defocus (px at 300 dpi)
    noise: float  # sensor noise (gray values)
    speckle: float  # toner splatter per pixel
    dpi: int  # scan resolution
    jpeg: int | None  # JPEG quality, None = PNG
    bilevel: bool  # fax-like bitonal


def draw_params(rng: np.random.Generator, dpi: int, strength: str, bilevel: bool = False) -> ScanParams:
    hi = strength == "strong"
    if bilevel:
        # Fax/bitonal scanner: laid flat, strong toner, threshold does the rest
        return ScanParams(
            angle=float(rng.choice([-1, 1]) * rng.uniform(0.3, 1.5)),
            keystone=float(rng.uniform(-0.004, 0.004)),
            curl=0.0,
            shadow=float(rng.uniform(0.0, 0.08)),
            paper=float(rng.uniform(0.92, 0.97)),
            fade=float(rng.uniform(0.9, 1.0)),
            thin=False,
            blur=float(rng.uniform(0.5, 0.8)),
            noise=float(rng.uniform(2, 5)),
            speckle=float(rng.uniform(5e-5, 2e-4)),
            dpi=dpi,
            jpeg=None,
            bilevel=True,
        )
    return ScanParams(
        angle=float(rng.choice([-1, 1]) * rng.uniform(0.3, 3.0 if hi else 1.5)),
        keystone=float(rng.uniform(-0.012, 0.012) if hi else rng.uniform(-0.005, 0.005)),
        curl=float(rng.uniform(0.05, 0.14) if hi else rng.uniform(0.0, 0.06)),
        shadow=float(rng.uniform(0.25, 0.45) if hi else rng.uniform(0.05, 0.25)),
        paper=float(rng.uniform(0.86, 0.92) if hi else rng.uniform(0.90, 0.96)),
        fade=float(rng.uniform(0.55, 0.75) if hi else rng.uniform(0.75, 0.95)),
        thin=bool(rng.random() < (0.5 if hi else 0.25)),
        blur=float(rng.uniform(0.8, 1.2) if hi else rng.uniform(0.5, 0.9)),
        noise=float(rng.uniform(6, 12) if hi else rng.uniform(2, 6)),
        speckle=float(rng.uniform(2e-4, 6e-4) if hi else rng.uniform(2e-5, 1.5e-4)),
        dpi=dpi,
        jpeg=None if bilevel else int(rng.integers(70, 80) if hi else rng.integers(80, 91)),
        bilevel=bilevel,
    )


def lowfreq(rng: np.random.Generator, h: int, w: int, cells: int) -> np.ndarray:
    """Smooth noise field in [-1, 1] (small random grid, enlarged bicubically)."""
    small = rng.uniform(-1, 1, (cells, max(2, round(cells * w / h)))).astype(np.float32)
    im = Image.fromarray(((small + 1) * 127.5).astype(np.uint8), "L").resize((w, h), Image.BICUBIC)
    return np.asarray(im, dtype=np.float32) / 127.5 - 1


def bilinear(src: np.ndarray, sx: np.ndarray, sy: np.ndarray, fill: float) -> np.ndarray:
    h, w = src.shape
    x0 = np.floor(sx).astype(np.int32)
    y0 = np.floor(sy).astype(np.int32)
    fx = (sx - x0).astype(np.float32)
    fy = (sy - y0).astype(np.float32)
    inside = (x0 >= 0) & (y0 >= 0) & (x0 < w - 1) & (y0 < h - 1)
    x0c = np.clip(x0, 0, w - 2)
    y0c = np.clip(y0, 0, h - 2)
    a = src[y0c, x0c]
    b = src[y0c, x0c + 1]
    c = src[y0c + 1, x0c]
    d = src[y0c + 1, x0c + 1]
    out = (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy
    return np.where(inside, out, fill).astype(np.float32)


def print_and_scan(page: Image.Image, prm: ScanParams, rng: np.random.Generator) -> Image.Image:
    img = page
    if prm.thin:
        img = img.filter(ImageFilter.MaxFilter(3))  # White grows → strokes thinner
    ink = 1.0 - np.asarray(img, dtype=np.float32) / 255.0  # 1 = toner
    ink *= prm.fade
    h, w = ink.shape
    dpi = RENDER_DPI
    # Backward mapping: output pixel → source pixel (rotation, trapezoid, curvature at the binding)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    cx, cy = w / 2, h / 2
    a = np.deg2rad(prm.angle)
    xr = cx + (xx - cx) * np.cos(a) - (yy - cy) * np.sin(a)
    yr = cy + (xx - cx) * np.sin(a) + (yy - cy) * np.cos(a)
    xr = cx + (xr - cx) * (1 + prm.keystone * (yr - cy) / h)
    wc = 0.15 * w
    t = np.clip((wc - xr) / wc, 0, 1) ** 2
    yr = yr + prm.curl * dpi * t * (yr - cy) / (h / 2)
    xr = xr + 0.3 * prm.curl * dpi * t
    del xx, yy
    ink = bilinear(ink, xr, yr, fill=-1.0)
    del xr, yr, t
    outside = ink < -0.5
    ink = np.clip(ink, 0, 1)
    # Paper, structure, illumination, binding shadow
    x = np.arange(w, dtype=np.float32)[None, :]
    light = 1.0 - prm.shadow * np.exp(-x / (0.05 * w)) - 0.04 * lowfreq(rng, h, w, 4)
    paper = prm.paper + 0.015 * lowfreq(rng, h, w, 60)
    val = paper * light * (1.0 - 0.92 * ink)
    val = np.where(outside, 0.35 + 0.05 * lowfreq(rng, h, w, 6), val)
    # Toner splatter / dust (small dark dots)
    n = int(prm.speckle * h * w)
    if n:
        ys = rng.integers(0, h - 3, n)
        xs = rng.integers(0, w - 3, n)
        dark = rng.uniform(0.1, 0.6, n).astype(np.float32)
        for dy in range(2):
            for dx in range(2):
                val[ys + dy, xs + dx] = np.minimum(val[ys + dy, xs + dx], dark)
    out = Image.fromarray(np.clip(val * 255, 0, 255).astype(np.uint8), "L")
    del val, ink
    out = out.filter(ImageFilter.GaussianBlur(prm.blur))
    # Scanning with scan resolution
    if prm.dpi != RENDER_DPI:
        out = out.resize((round(w * prm.dpi / RENDER_DPI), round(h * prm.dpi / RENDER_DPI)), Image.LANCZOS)
    arr = np.asarray(out, dtype=np.float32) + rng.normal(0, prm.noise, (out.height, out.width)).astype(np.float32)
    out = Image.fromarray(np.clip(arr, 0, 255).astype(np.uint8), "L")
    if prm.bilevel:
        arr = np.asarray(out, dtype=np.float32) + rng.normal(0, 10, (out.height, out.width)).astype(np.float32)
        thr = 0.72 * prm.paper * 255
        out = Image.fromarray(np.where(arr > thr, 255, 0).astype(np.uint8), "L").convert("1", dither=Image.NONE)
    return out


# ------------------------------------------------------------------------------------------------
# Output (without metadata) + check
# ------------------------------------------------------------------------------------------------


def save_image(img: Image.Image, path: Path, dpi: int, jpeg: int | None):
    """Only pixels + resolution (PNG pHYs or JFIF density); no text/EXIF/XMP blocks."""
    clean = Image.frombytes(img.mode, img.size, img.tobytes())  # discards img.info
    if jpeg:
        clean.save(path, "JPEG", quality=jpeg, dpi=(dpi, dpi), optimize=False)
    else:
        clean.save(path, "PNG", dpi=(dpi, dpi))


def image_pdf(pages: list[tuple[Path, int]], out: Path):
    """Pure image PDF: one image XObject per page (JPEG unchanged as DCTDecode, PNG as Flate),
    no /Font, no text, no /Info."""
    objs: list[bytes] = []

    def add(b: bytes) -> int:
        objs.append(b)
        return len(objs)

    kids = []
    pages_id = 2
    add(b"")  # 1: Catalog (later)
    add(b"")  # 2: Pages (later)
    for path, dpi in pages:
        im = Image.open(path)
        w, h = im.size
        if path.suffix.lower() in (".jpg", ".jpeg"):
            data = path.read_bytes()
            filt, bpc = b"/DCTDecode", 8
        elif im.mode == "1":
            data = zlib.compress(im.tobytes())
            filt, bpc = b"/FlateDecode", 1
        else:
            data = zlib.compress(im.convert("L").tobytes())
            filt, bpc = b"/FlateDecode", 8
        img_id = add(
            b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceGray /BitsPerComponent %d /Filter %s /Length %d >>\nstream\n"
            % (w, h, bpc, filt, len(data))
            + data
            + b"\nendstream"
        )
        pw, ph = w * 72 / dpi, h * 72 / dpi
        content = b"q %.2f 0 0 %.2f 0 0 cm /Im0 Do Q" % (pw, ph)
        c_id = add(b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream")
        kids.append(
            add(
                b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %.2f %.2f] /Resources << /XObject << /Im0 %d 0 R >> >> /Contents %d 0 R >>"
                % (pages_id, pw, ph, img_id, c_id)
            )
        )
    objs[0] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objs[1] = b"<< /Type /Pages /Kids [%s] /Count %d >>" % (b" ".join(b"%d 0 R" % k for k in kids), len(kids))
    buf = io.BytesIO()
    buf.write(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(buf.tell())
        buf.write(b"%d 0 obj\n" % i + o + b"\nendobj\n")
    x = buf.tell()
    buf.write(b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1))
    for o in offs:
        buf.write(b"%010d 00000 n \n" % o)
    buf.write(b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, x))
    out.write_bytes(buf.getvalue())


def verify_no_text(path: Path) -> list[str]:
    """Finds readable metadata/text in an output file (empty = clean)."""
    data = path.read_bytes()
    bad = []
    suf = path.suffix.lower()
    if suf == ".png":
        pos = 8
        while pos + 8 <= len(data):
            ln = int.from_bytes(data[pos : pos + 4], "big")
            typ = data[pos + 4 : pos + 8]
            if typ in (b"tEXt", b"iTXt", b"zTXt", b"eXIf"):
                bad.append(f"PNG chunk {typ.decode()}")
            pos += 12 + ln
    elif suf in (".jpg", ".jpeg"):
        i = 2
        while i + 4 <= len(data) and data[i] == 0xFF:
            m = data[i + 1]
            if m == 0xDA:  # Start of scan
                break
            ln = int.from_bytes(data[i + 2 : i + 4], "big")
            if m == 0xE1:
                bad.append("JPEG APP1 (EXIF/XMP)")
            if m == 0xFE:
                bad.append("JPEG COM")
            i += 2 + ln
    elif suf == ".pdf":
        for pat in (rb"/Font", rb"/Info", rb"\bTj\b", rb"\bTJ\b", rb"/Metadata", rb"BT\s"):
            # search only outside the image data: object headers and content streams are ASCII
            for m in re.finditer(rb"obj\n(<<.*?>>)\n", data, re.S):
                if re.search(pat, m.group(1)):
                    bad.append(f"PDF {pat.decode()}")
        for m in re.finditer(rb"stream\n(q [^\n]*?Q)\nendstream", data):
            if re.search(rb"BT|Tj|TJ", m.group(1)):
                bad.append("PDF text operator")
    return sorted(set(bad))


# ------------------------------------------------------------------------------------------------


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    seed = 20261109
    if "--seed" in sys.argv:
        seed = int(sys.argv[sys.argv.index("--seed") + 1])
        args = [a for a in args if a != str(seed)]
    out = Path(args[0]) if args else REPO / "results/bench/ocr-quality/corpus"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    rng = np.random.default_rng(seed)
    images: list[dict] = []
    pdfs: list[dict] = []

    builders = {
        "letter_de": page_letter_de,
        "letter_en": page_letter_en,
        "invoice_de": page_invoice_de,
        "columns_de": page_columns_de,
        "terms_de": page_terms_de,
        "fax_de": page_fax_de,
    }
    rendered = {k: f() for k, f in builders.items()}

    def emit(name: str, img: Image.Image, gt: str, dpi: int, group: str, kind: str, jpeg: int | None, params: dict | None = None):
        p = out / f"{name}.{'jpg' if jpeg else 'png'}"
        save_image(img, p, dpi, jpeg)
        (out / f"{name}.gt.txt").write_text(gt + "\n", encoding="utf-8")
        images.append({"name": name, "image": p.name, "gt": f"{name}.gt.txt", "dpi": dpi, "group": group, "kind": kind, "params": params or {}})
        return p

    # Control group: clean, 300 dpi, PNG
    for k in ["letter_de", "invoice_de", "columns_de", "letter_en"]:
        emit(f"clean_{k}_300", rendered[k].img, rendered[k].text_gt(), 300, "clean", k, None)

    # Scan-like: each template several times, 300/200 dpi, mild/strong; fax 150 dpi bitonal
    plan = [
        ("letter_de", 300, "mild"), ("letter_de", 200, "strong"), ("letter_de", 300, "strong"),
        ("invoice_de", 300, "mild"), ("invoice_de", 200, "mild"), ("invoice_de", 300, "strong"),
        ("columns_de", 300, "mild"), ("columns_de", 200, "strong"),
        ("terms_de", 300, "mild"), ("terms_de", 200, "mild"), ("terms_de", 300, "strong"),
        ("letter_en", 300, "mild"), ("letter_en", 200, "strong"),
        ("fax_de", 150, "fax"), ("letter_de", 150, "fax"), ("invoice_de", 150, "fax"),
    ]
    scan_paths: dict[str, Path] = {}
    for k, dpi, strength in plan:
        fax = strength == "fax"
        prm = draw_params(rng, dpi, "mild" if fax else strength, bilevel=fax)
        img = print_and_scan(rendered[k].img, prm, rng)
        name = f"scan_{k}_{dpi}_{strength}"
        scan_paths[name] = emit(name, img, rendered[k].text_gt(), dpi, "scan", k, prm.jpeg, prm.__dict__)

    # Multi-page image PDFs (ingestion path) from the scan pages
    def make_pdf(name: str, names: list[str]):
        pages = [(scan_paths[n], next(i["dpi"] for i in images if i["name"] == n)) for n in names]
        path = out / f"{name}.pdf"
        image_pdf(pages, path)
        gt = "\n\n".join((out / f"{n}.gt.txt").read_text(encoding="utf-8").strip() for n in names)
        (out / f"{name}.gt.txt").write_text(gt + "\n", encoding="utf-8")
        pdfs.append({"name": name, "pdf": path.name, "gt": f"{name}.gt.txt", "pages": len(names), "group": "scan"})

    make_pdf("scanpdf_letter_invoice_terms_300", ["scan_letter_de_300_mild", "scan_invoice_de_300_mild", "scan_terms_de_300_mild"])
    make_pdf("scanpdf_mixed_200", ["scan_letter_de_200_strong", "scan_invoice_de_200_mild", "scan_columns_de_200_strong", "scan_terms_de_200_mild", "scan_letter_en_200_strong"])
    make_pdf("scanpdf_fax_150", ["scan_fax_de_150_fax", "scan_letter_de_150_fax", "scan_invoice_de_150_fax"])

    # Check: synthetic files without text/metadata
    problems = {}
    for f in sorted(out.iterdir()):
        if f.suffix.lower() in (".png", ".jpg", ".pdf"):
            bad = verify_no_text(f)
            if bad:
                problems[f.name] = bad
    if problems:
        raise SystemExit(f"metadata/text found: {problems}")

    (out / "manifest.json").write_text(json.dumps({"seed": seed, "images": images, "pdfs": pdfs}, indent=1, ensure_ascii=False), encoding="utf-8")
    groups = {}
    for i in images:
        groups[i["group"]] = groups.get(i["group"], 0) + 1
    print(f"{len(images)} images {groups}, {len(pdfs)} PDFs -> {out} (seed {seed}); metadata check ok")


if __name__ == "__main__":
    main()
