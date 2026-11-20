"""Erzeugt die Office-Beispieldateien für die Tests von `Op::Office` (WordCraft).

Aufruf (aus diesem Ordner): python -I make_samples.py
Braucht python-docx und Pillow. Die erzeugten Dateien liegen eingecheckt daneben; das Skript ist nur
zur Nachvollziehbarkeit da.
"""

import io
import os
import zipfile

from docx import Document
from docx.enum.text import WD_BREAK
from docx.shared import Cm
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))


def picture() -> bytes:
    img = Image.new("RGB", (240, 120), (255, 255, 255))
    d = ImageDraw.Draw(img)
    d.rectangle([10, 10, 230, 110], outline=(20, 60, 160), width=4)
    d.ellipse([40, 30, 100, 90], fill=(200, 40, 40))
    d.rectangle([130, 30, 200, 90], fill=(40, 160, 60))
    buf = io.BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


def make_docx(path: str) -> None:
    doc = Document()
    doc.core_properties.title = "Rechnung 4711"
    doc.add_heading("Rechnung 4711", level=1)
    doc.add_paragraph("Sehr geehrte Damen und Herren, wir berechnen Ihnen folgende Leistungen. Umlaute: äöü ÄÖÜ ß, Euro €.")
    doc.add_heading("Positionen", level=2)
    table = doc.add_table(rows=1, cols=3)
    table.style = "Table Grid"
    hdr = table.rows[0].cells
    hdr[0].text, hdr[1].text, hdr[2].text = "Position", "Menge", "Betrag"
    for pos, qty, amount in [("Beratung", "2", "300,00 €"), ("Installation", "1", "150,00 €"), ("Material", "5", "42,50 €")]:
        row = table.add_row().cells
        row[0].text, row[1].text, row[2].text = pos, qty, amount
    doc.add_heading("Hinweise", level=2)
    for item in ["Zahlbar innerhalb von 30 Tagen", "Bitte Rechnungsnummer angeben", "Skonto 2 % bei Zahlung in 10 Tagen"]:
        doc.add_paragraph(item, style="List Bullet")
    for item in ["Erster Schritt", "Zweiter Schritt", "Dritter Schritt"]:
        doc.add_paragraph(item, style="List Number")
    doc.add_paragraph("Logo:")
    doc.add_picture(io.BytesIO(picture()), width=Cm(6))
    p = doc.add_paragraph("Ende Seite 1.")
    p.add_run().add_break(WD_BREAK.PAGE)
    doc.add_heading("Seite 2", level=1)
    doc.add_paragraph("Dieser Text steht nach einem Seitenumbruch auf der zweiten Seite. Suchwort: Geheimnummer 12345.")
    buf = io.BytesIO()
    doc.save(buf)
    reorder_like_word(buf.getvalue(), path)


def reorder_like_word(data: bytes, path: str) -> None:
    """Einträge wie Word ordnen ([Content_Types].xml, _rels, word/… zuerst). Die MIME-Erkennung
    (libmagic-Regel wie upstream) sieht nur die ersten vier ZIP-Einträge an; python-docx legt
    word/document.xml erst an fünfte Stelle, dann gilt die Datei als application/octet-stream."""
    src = zipfile.ZipFile(io.BytesIO(data))
    names = src.namelist()
    first = ["[Content_Types].xml", "_rels/.rels", "word/document.xml"]
    order = [n for n in first if n in names] + [n for n in names if n not in first]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as out:
        for n in order:
            out.writestr(n, src.read(n))


def make_odt(path: str) -> None:
    content = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
 xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3">
 <office:automatic-styles>
  <style:style style:name="PB" style:family="paragraph"><style:paragraph-properties fo:break-before="page"/></style:style>
 </office:automatic-styles>
 <office:body><office:text>
  <text:h text:outline-level="1">Vertrag Nummer 99</text:h>
  <text:p>Zwischen den Parteien wird Folgendes vereinbart. Umlaute: äöü ß.</text:p>
  <text:list><text:list-item><text:p>Punkt eins</text:p></text:list-item><text:list-item><text:p>Punkt zwei</text:p></text:list-item></text:list>
  <table:table table:name="T1"><table:table-column table:number-columns-repeated="2"/>
   <table:table-row><table:table-cell><text:p>Name</text:p></table:table-cell><table:table-cell><text:p>Wert</text:p></table:table-cell></table:table-row>
   <table:table-row><table:table-cell><text:p>Laufzeit</text:p></table:table-cell><table:table-cell><text:p>12 Monate</text:p></table:table-cell></table:table-row>
  </table:table>
  <text:p text:style-name="PB">Zweite Seite des Vertrags.</text:p>
 </office:text></office:body>
</office:document-content>"""
    manifest = """<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
 <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"""
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), "application/vnd.oasis.opendocument.text", compress_type=zipfile.ZIP_STORED)
        z.writestr("content.xml", content, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("META-INF/manifest.xml", manifest, compress_type=zipfile.ZIP_DEFLATED)


def make_rtf(path: str) -> None:
    rtf = (
        r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Times New Roman;}}"
        r"\f0\fs36\b Brief an die Bank\b0\fs24\par "
        r"Sehr geehrte Damen und Herren,\par "
        r"bitte \i \'fcberweisen\i0  Sie den Betrag von 99 \'80 auf das Konto.\par "
        r"\page Zweite Seite.\par }"
    )
    with open(path, "w", encoding="ascii", newline="") as f:
        f.write(rtf)


def make_broken(path: str, docx: str) -> None:
    # looks like a Word file (MIME detection: docx), but word/document.xml is truncated:
    # WordCraft rejects it ("malformed XML")
    src = zipfile.ZipFile(docx)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as out:
        for n in src.namelist():
            d = src.read(n)
            if n == "word/document.xml":
                d = d[: len(d) // 2]
            out.writestr(n, d)


if __name__ == "__main__":
    make_docx(os.path.join(HERE, "sample.docx"))
    make_odt(os.path.join(HERE, "sample.odt"))
    make_rtf(os.path.join(HERE, "sample.rtf"))
    with open(os.path.join(HERE, "sample.md"), "w", encoding="utf-8", newline="\n") as f:
        f.write("# Notiz\n\nEin **fetter** Satz und eine Liste:\n\n- eins\n- zwei\n\n| A | B |\n|---|---|\n| 1 | 2 |\n")
    make_broken(os.path.join(HERE, "broken.docx"), os.path.join(HERE, "sample.docx"))
