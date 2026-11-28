#!/usr/bin/env python3
"""Erzeugt deterministische, frei erfundene Testdokumente für den Benchmark.

  text/  200 PDFs mit Textschicht (Rechnungen, Briefe, Mahnungen; 1–2 Seiten), eigener
         Mini-PDF-Schreiber (Helvetica, WinAnsi), keine Fremdpakete nötig.
  scan/  30 Bild-PDFs ohne Text (Graustufen, 200 dpi, leichtes Rauschen/Schräglage), damit OCR läuft.
         Braucht Pillow und eine TTF-Schrift (DejaVuSans).

Seltenes Wort für die Suche: "Zwetschgenröster" (genau 3 Text-PDFs). Häufiges Wort: "Rechnung".
Aufruf: python3 gen_docs.py <zielordner>
        python3 gen_docs.py <zielordner> --detail          (detail/: Szenario 7 je Art, mit manifest.json)
        python3 gen_docs.py <zielordner> --extra <anzahl>   (extra/e00000.pdf …, eindeutiger Inhalt)
"""
import json
import os
import random
import sys
import zlib

SEED = 20261118
FIRMEN = ["Alpenblick Druck GmbH", "Bergmann & Söhne AG", "Rheintal Elektro KG", "Seeufer Bäckerei",
          "Nordlicht Versicherungen", "Hafenstadt Logistik GmbH", "Grünwald Gartenbau", "Müller Haustechnik",
          "Sonnenhof Ferienwohnungen", "Stadtwerke Talheim", "Kaufhaus Lindenberg", "Weber IT-Service",
          "Fischer Zahnarztpraxis", "Autohaus Brückner", "Kanzlei Hofmann & Partner", "Bio-Hof Eichengrund"]
ORTE = ["Zürich", "Bern", "Basel", "Luzern", "St. Gallen", "Winterthur", "Konstanz", "Freiburg", "München"]
POSTEN = ["Wartung Heizungsanlage", "Druck Visitenkarten (500 Stk.)", "Monatsabo Internet", "Brot und Gebäck",
          "Kfz-Inspektion", "Beratung (Stunden)", "Hausratversicherung Jahresprämie", "Stromverbrauch Quartal",
          "Gartenpflege", "Zahnreinigung", "Bürostühle", "Reparatur Fenster", "Lizenz Buchhaltungssoftware",
          "Heizöl 1500 Liter", "Fahrradreparatur", "Reinigung Treppenhaus"]
FLOSKELN = ["Wir danken Ihnen für Ihren Auftrag und bitten um Überweisung innerhalb von 30 Tagen.",
            "Bei Fragen zu dieser Rechnung erreichen Sie uns werktags von 8 bis 17 Uhr.",
            "Bitte geben Sie bei der Zahlung die Rechnungsnummer an.",
            "Der Betrag ist ohne Abzug zahlbar.", "Für Rückfragen stehen wir Ihnen gerne zur Verfügung.",
            "Wir weisen darauf hin, dass die Ware bis zur vollständigen Bezahlung unser Eigentum bleibt.",
            "Leistungszeitraum entspricht dem Rechnungsdatum, sofern nicht anders angegeben."]
BRIEF = ["vielen Dank für Ihr Schreiben vom letzten Monat.", "hiermit bestätigen wir den Eingang Ihrer Unterlagen.",
         "leider müssen wir Ihnen mitteilen, dass sich der Termin verschiebt.",
         "anbei erhalten Sie die gewünschten Informationen zu Ihrem Vertrag.",
         "wir freuen uns, Ihnen ein Angebot für die Renovierung unterbreiten zu dürfen.",
         "die Jahresabrechnung für Ihre Wohnung liegt diesem Schreiben bei."]
SELTEN = "Zwetschgenröster"


def doc_lines(rng, i):
    firma = rng.choice(FIRMEN)
    ort = rng.choice(ORTE)
    tag, monat, jahr = rng.randint(1, 28), rng.randint(1, 12), rng.choice([2023, 2024, 2025, 2026])
    datum = f"{tag:02d}.{monat:02d}.{jahr}"
    art = rng.choice(["Rechnung", "Rechnung", "Rechnung", "Brief", "Mahnung", "Angebot"])
    L = [firma, f"Hauptstrasse {rng.randint(1, 120)}, {rng.randint(1000, 9999)} {ort}", "",
         "Max Muster, Musterweg 7, 8000 Zürich", "", f"{ort}, {datum}", ""]
    if art in ("Rechnung", "Mahnung"):
        nr = f"RE-{jahr}-{rng.randint(10000, 99999)}"
        L += [f"{art} Nr. {nr}", "", "Pos.  Beschreibung                         Betrag CHF"]
        total = 0.0
        for p in range(rng.randint(2, 9)):
            b = round(rng.uniform(12, 900), 2)
            total += b
            L.append(f"{p + 1:>3}   {rng.choice(POSTEN):<36} {b:>10.2f}")
        mwst = round(total * 0.081, 2)
        L += ["", f"Zwischensumme {total:.2f} CHF", f"MwSt. 8.1 %  {mwst:.2f} CHF",
              f"Gesamtbetrag {total + mwst:.2f} CHF", ""]
        if art == "Mahnung":
            L += ["Leider konnten wir bis heute keinen Zahlungseingang feststellen.",
                  f"Wir bitten Sie, die Rechnung {nr} umgehend zu begleichen."]
        L += rng.sample(FLOSKELN, 3)
    else:
        L += [f"{art}: Ihr Anliegen vom {datum}", "", "Sehr geehrter Herr Muster,", ""]
        for _ in range(rng.randint(4, 12)):
            L.append(rng.choice(BRIEF).capitalize())
        L += ["", "Mit freundlichen Grüssen", firma]
    if i in (17, 88, 151):
        L += ["", f"PS: Unser neuer {SELTEN} ist ab sofort im Laden erhältlich."]
    # second page on every third document
    pages = [L]
    if i % 3 == 0:
        pages.append(["Allgemeine Geschäftsbedingungen (Auszug)", ""] +
                     [f"§ {k + 1} " + rng.choice(FLOSKELN) for k in range(25)])
    return firma, art, datum, pages


def pdf_escape(s):
    b = s.encode("cp1252", "replace")
    return b.replace(b"\\", b"\\\\").replace(b"(", b"\\(").replace(b")", b"\\)")


def write_text_pdf(path, pages, title):
    objs = []  # (num, bytes)

    def add(body):
        objs.append(body)
        return len(objs)
    font = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    pages_num_placeholder = add(b"")  # replaced later
    page_nums = []
    for lines in pages:
        content = b"BT /F1 10 Tf 13 TL 56 790 Td "
        for ln in lines:
            content += b"(" + pdf_escape(ln) + b") Tj T* "
        content += b"ET"
        comp = zlib.compress(content, 9)
        c = add(b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(comp) + comp + b"\nendstream")
        p = add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 %d 0 R >> >> /Contents %d 0 R >>"
                % (pages_num_placeholder, font, c))
        page_nums.append(p)
    objs[pages_num_placeholder - 1] = b"<< /Type /Pages /Kids [" + b" ".join(b"%d 0 R" % n for n in page_nums) + b"] /Count %d >>" % len(page_nums)
    info = add(b"<< /Title (" + pdf_escape(title) + b") /Producer (prbench gen_docs) /CreationDate (D:20261118000000Z) >>")
    cat = add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages_num_placeholder)
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offs = []
    for n, body in enumerate(objs, 1):
        offs.append(len(out))
        out += b"%d 0 obj\n" % n + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for o in offs:
        out += b"%010d 00000 n \n" % o
    out += b"trailer\n<< /Size %d /Root %d 0 R /Info %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, cat, info, xref)
    with open(path, "wb") as f:
        f.write(out)


FIXTIME = 1795046400  # fixed time, otherwise not reproducible


def render_page(lines, rng, font_path, dpi=200):
    from PIL import Image, ImageDraw, ImageFont, ImageFilter
    w, h = int(8.27 * dpi), int(11.69 * dpi)
    img = Image.new("L", (w, h), 250)
    d = ImageDraw.Draw(img)
    font = ImageFont.truetype(font_path, 30)
    y = 160
    for ln in lines:
        d.text((150, y), ln, fill=rng.randint(10, 50), font=font)
        y += 44
        if y > h - 150:
            break
    img = img.rotate(rng.uniform(-1.2, 1.2), resample=Image.BICUBIC, fillcolor=250)
    # slight paper noise, deterministic
    noise = Image.frombytes("L", (w, h), rng.randbytes(w * h))
    img = Image.blend(img, noise, 0.06)
    return img.filter(ImageFilter.GaussianBlur(0.6))


def write_scan_pdf(path, lines, rng, font_path, more_pages=()):
    import time
    img = render_page(lines, rng, font_path)
    rest = [render_page(p, rng, font_path) for p in more_pages]
    fix = time.gmtime(FIXTIME)
    img.save(path, "PDF", resolution=200, quality=80, creationDate=fix, modDate=fix,
             save_all=bool(rest), append_images=rest)


def write_photo(path, lines, rng, font_path):
    """Phone photo of an invoice: paper on a dark table, ~3.5 degrees skewed, uneven light."""
    from PIL import Image, ImageFilter
    page = render_page(lines, rng, font_path, dpi=200).convert("RGB")
    pw, ph = page.size
    W, H = int(pw * 1.25), int(ph * 1.18)
    bg = Image.new("RGB", (W, H), (70, 58, 48))
    page = page.rotate(rng.uniform(2.5, 4.5), resample=Image.BICUBIC, expand=True, fillcolor=(70, 58, 48))
    bg.paste(page, ((W - page.size[0]) // 2, (H - page.size[1]) // 2))
    # Light from one side: gradient as mask against black
    grad = Image.linear_gradient("L").resize((W, H)).rotate(35, expand=False, fillcolor=0)
    grad = grad.point(lambda v: 255 - v // 4)
    bg = Image.composite(bg, Image.new("RGB", (W, H), (0, 0, 0)), grad)
    bg = bg.filter(ImageFilter.GaussianBlur(0.8))
    if path.endswith(".png"):
        bg.save(path, "PNG")
    else:
        bg.save(path, "JPEG", quality=85)


def write_docx(path, lines):
    import zipfile
    from xml.sax.saxutils import escape
    body = "".join('<w:p><w:r><w:t xml:space="preserve">' + escape(l) + "</w:t></w:r></w:p>" for l in lines)
    doc = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
           '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
           "<w:body>" + body + "</w:body></w:document>")
    ct = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
          '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
          '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
          '<Default Extension="xml" ContentType="application/xml"/>'
          '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
          "</Types>")
    rels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
            "</Relationships>")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in (("[Content_Types].xml", ct), ("_rels/.rels", rels), ("word/document.xml", doc)):
            zi = zipfile.ZipInfo(name, date_time=(2026, 11, 18, 0, 0, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(zi, data)


def unique_word(rng, used):
    while True:
        w = "".join(rng.choice("bdfgklmnprstvz") + rng.choice("aeiou") for _ in range(4))
        if w not in used:
            used.add(w)
            return w


def find_font():
    for c in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"]:
        if os.path.exists(c):
            return c
    sys.exit("DejaVuSans.ttf nicht gefunden")


def gen_detail(out):
    """Scenario 7: a few documents per kind with unique check words (+ manifest.json with reference text)."""
    d = os.path.join(out, "detail")
    os.makedirs(d, exist_ok=True)
    font = find_font()
    rng = random.Random(SEED + 3)
    used, manifest = set(), {}
    plan = [("a_text", ".pdf", 8), ("b_scan1", ".pdf", 5), ("b_scan8", ".pdf", 3),
            ("c_photo", ".jpg", 4), ("c_photo", ".png", 2), ("d_office", ".docx", 3)]
    i = 0
    for kind, ext, n in plan:
        for j in range(n):
            _, _, _, pages = doc_lines(rng, 200000 + i)
            w1 = unique_word(rng, used)
            first = pages[0][:30] + ["", f"Kennwort {w1}"]
            i += 1
            name = f"{kind}_{j:02d}{ext}"
            words, text = [w1], list(first)
            path = os.path.join(d, name)
            if kind == "a_text":
                write_text_pdf(path, [first], name)
            elif kind == "b_scan1":
                write_scan_pdf(path, first, rng, font)
            elif kind == "b_scan8":
                more = []
                for _ in range(7):
                    _, _, _, pg = doc_lines(rng, 300000 + i)
                    more.append(pg[0][:30])
                w2 = unique_word(rng, used)
                more[-1] = more[-1] + ["", f"Schlusswort {w2}"]
                words.append(w2)
                for m in more:
                    text += m
                write_scan_pdf(path, first, rng, font, more_pages=more)
            elif kind == "c_photo":
                write_photo(path, first, rng, font)
            else:
                write_docx(path, first)
            manifest[name] = {"kind": kind, "words": words, "text": "\n".join(text)}
    with open(os.path.join(d, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=1)
    print("detail", len(manifest))


def gen_extra(out, n):
    """Additional, unique text PDFs (everyday upload, larger archive)."""
    d = os.path.join(out, "extra")
    os.makedirs(d, exist_ok=True)
    rng = random.Random(SEED + 2)
    for i in range(n):
        firma, art, datum, pages = doc_lines(rng, 100000 + i)
        pages[0] = pages[0] + ["", f"Belegnummer PRB-{i:05d}"]
        write_text_pdf(os.path.join(d, f"e{i:05d}.pdf"), pages, f"{art} {firma} {datum} #{i}")
    print("extra", n)


def main():
    out = sys.argv[1]
    if len(sys.argv) > 3 and sys.argv[2] == "--extra":
        gen_extra(out, int(sys.argv[3]))
        return
    if len(sys.argv) > 2 and sys.argv[2] == "--detail":
        gen_detail(out)
        return
    os.makedirs(os.path.join(out, "text"), exist_ok=True)
    os.makedirs(os.path.join(out, "scan"), exist_ok=True)
    rng = random.Random(SEED)
    for i in range(200):
        firma, art, datum, pages = doc_lines(rng, i)
        write_text_pdf(os.path.join(out, "text", f"t{i:03d}.pdf"), pages, f"{art} {firma} {datum}")
    font = find_font()
    srng = random.Random(SEED + 1)
    manifest = {}
    for i in range(30):
        _, _, _, pages = doc_lines(srng, 1000 + i)
        write_scan_pdf(os.path.join(out, "scan", f"s{i:03d}.pdf"), pages[0], srng, font)
        manifest[f"s{i:03d}.pdf"] = {"kind": "scan", "words": [], "text": "\n".join(pages[0][:46])}
    with open(os.path.join(out, "scan_manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=1)
    print("ok", len(os.listdir(os.path.join(out, "text"))), len(os.listdir(os.path.join(out, "scan"))))


if __name__ == "__main__":
    main()
