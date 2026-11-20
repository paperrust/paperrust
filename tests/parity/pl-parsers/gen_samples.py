"""Creates the sample files for the pl-parsers parity tests (deterministic).

Invocation (from the repo root directory):

    reference/venv/Scripts/python.exe -I tests/parity/pl-parsers/gen_samples.py

Everything is written under tests/parity/pl-parsers/samples/. Office files are built by hand
as ZIP+XML (no python-docx/openpyxl in reference/venv), PDFs with fpdf2 + pikepdf,
images with Pillow (HEIC via pillow_heif, if available).
"""

from __future__ import annotations

import io
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "samples"
FIXED_DT = (2025, 1, 6, 8, 0, 0)


def w(name: str, data: bytes) -> None:
    (OUT / name).write_bytes(data)


# ---------------------------------------------------------------------------
# Text
# ---------------------------------------------------------------------------


def text_samples() -> None:
    w("plain_utf8.txt", "Rechnung Nr. 4711\nGrüße aus Zürich – Straße\n\nBetrag: 12,50 €\n".encode())
    w("utf8_bom.txt", b"\xef\xbb\xbf" + "BOM-Text mit Ümlaut\n".encode())
    w("utf16le_bom.txt", "UTF-16 LE Text äöü\r\nZweite Zeile\r\n".encode("utf-16"))  # LE + BOM
    w("utf16be_bom.txt", b"\xfe\xff" + "UTF-16 BE Text äöü\n".encode("utf-16-be"))
    w("latin1.txt", "Lateinisch: Gr\xfc\xdfe, Stra\xdfe, caf\xe9\n".encode("latin-1"))
    w("crlf.txt", b"Zeile eins\r\nZeile zwei\r\n\r\nEnde\r\n")
    w("nul_bytes.txt", b"Text mit\x00 NUL\x00 Bytes und mehr Text danach\n")
    w("empty.txt", b"")
    w("one_line_no_newline.txt", b"Nur eine Zeile ohne Zeilenende")
    w(
        "data.csv",
        b"name,betrag,datum\nAlice,12.50,2024-01-02\nBob,7.00,2024-02-03\nCarol,3.25,2024-03-04\n",
    )
    w(
        "semicolon.csv",
        "Name;Betrag;Datum\nMüller;12,50;02.01.2024\nMeier;7,00;03.02.2024\nHuber;3,25;04.03.2024\n".encode(),
    )
    w(
        "notes.md",
        b"# Ueberschrift\n\nEin *Absatz* mit **Fett** und [Link](https://example.com).\n\n- Punkt 1\n- Punkt 2\n",
    )
    w(
        "page.html",
        b"<!DOCTYPE html>\n<html><head><title>Testseite</title><style>p{color:red}</style>"
        b"<script>var x = 1;</script></head>\n<body><h1>Kopf</h1><p>Erster&nbsp;Absatz &amp; mehr"
        b" &lt;Text&gt; &#252; &#x00E4;</p>\n<div>Zweiter<br>Block</div><ul><li>A</li><li>B</li></ul>"
        b"</body></html>\n",
    )
    w("code_like.txt", b"#include <stdio.h>\nint main(void) {\n  return 0;\n}\n")
    w("json_like.txt", b'{"a": 1, "b": [1, 2, 3], "c": "text"}\n')
    w("xml_doc.xml", b'<?xml version="1.0" encoding="UTF-8"?>\n<root><item>Wert</item></root>\n')
    w("python_like.txt", b"#!/usr/bin/env python3\nimport os\nprint(os.getcwd())\n")
    w("long_lines.txt", ("Wort " * 2000 + "\n").encode())
    w("binary_garbage.bin", bytes(range(256)) * 4)


# ---------------------------------------------------------------------------
# E-Mail
# ---------------------------------------------------------------------------


def eml_samples() -> None:
    w(
        "mail_simple.eml",
        b"Return-Path: <sender@example.com>\r\n"
        b"From: Alice Example <alice@example.com>\r\n"
        b"To: Bob <bob@example.com>, carol@example.com\r\n"
        b"Subject: Einfache Nachricht\r\n"
        b"Date: Mon, 06 Jan 2025 09:15:00 +0100\r\n"
        b"Message-ID: <simple-1@example.com>\r\n"
        b"MIME-Version: 1.0\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"Content-Transfer-Encoding: 8bit\r\n"
        b"\r\n"
        + "Hallo Bob,\r\n\r\n  das ist   ein Test mit Ümlauten.\r\n\r\nGruss\r\nAlice\r\n".encode(),
    )
    w(
        "mail_html_alt.eml",
        b"From: \"Shop, Online\" <shop@example.org>\n"
        b"To: kunde@example.net\n"
        b"Cc: Team <team@example.org>, =?utf-8?Q?J=C3=BCrg?= <juerg@example.org>\n"
        b"Subject: =?utf-8?B?SWhyZSBCZXN0ZWxsdW5nIMOEIDEyMw==?=\n"
        b"Date: Tue, 07 Jan 2025 18:30:05 +0000\n"
        b"MIME-Version: 1.0\n"
        b"Content-Type: multipart/alternative; boundary=\"ALT-BOUNDARY\"\n"
        b"\n"
        b"This is a multi-part message in MIME format.\n"
        b"--ALT-BOUNDARY\n"
        b"Content-Type: text/plain; charset=\"iso-8859-1\"\n"
        b"Content-Transfer-Encoding: quoted-printable\n"
        b"\n"
        b"Vielen Dank f=FCr Ihre Bestellung.\n"
        b"Lange Zeile mit weichem =\n"
        b"Umbruch.\n"
        b"--ALT-BOUNDARY\n"
        b"Content-Type: text/html; charset=utf-8\n"
        b"Content-Transfer-Encoding: base64\n"
        b"\n"
        + __import__("base64").encodebytes(
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\">"
            "</head><body><p>Vielen Dank f&uuml;r Ihre <b>Bestellung</b>.</p></body></html>".encode()
        )
        + b"--ALT-BOUNDARY--\n",
    )
    pdf_payload = b"%PDF-1.4\n% kleiner Anhang\n%%EOF\n"
    w(
        "mail_attachments.eml",
        b"Received: from mx.example.com by mail.example.com; Wed, 8 Jan 2025 10:00:00 +0100\r\n"
        b"From: =?iso-8859-1?Q?M=FCller?= Hans <hans.mueller@example.ch>\r\n"
        b"To: Empf\xc3\xa4nger <empf@example.ch>\r\n"
        b"Bcc: geheim@example.ch\r\n"
        b"Subject: Anh\xc3\xa4nge und\r\n mehr\r\n"
        b"Date: Wed, 8 Jan 2025 10:00:00 +0100 (CET)\r\n"
        b"X-Custom: wert1\r\n"
        b"X-Custom: wert2\r\n"
        b"MIME-Version: 1.0\r\n"
        b"Content-Type: multipart/mixed; boundary=MIXED\r\n"
        b"\r\n"
        b"--MIXED\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"\r\n"
        b"Siehe Anhang.\r\n"
        b"--MIXED\r\n"
        b"Content-Type: application/pdf; name=\"rechnung.pdf\"\r\n"
        b"Content-Disposition: attachment; filename=\"rechnung.pdf\"\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        + __import__("base64").encodebytes(pdf_payload).replace(b"\n", b"\r\n")
        + b"--MIXED\r\n"
        b"Content-Type: image/png\r\n"
        b"Content-ID: <logo@cid>\r\n"
        b"Content-Disposition: inline\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        b"iVBORw0KGgo=\r\n"
        b"--MIXED\r\n"
        b"Content-Type: application/octet-stream\r\n"
        b"Content-Disposition: attachment; filename*=utf-8''gr%C3%B6sse%20liste.bin\r\n"
        b"\r\n"
        + b"x" * 2000
        + b"\r\n"
        b"--MIXED\r\n"
        b"Content-Type: text/plain; name=\"=?utf-8?B?bm90aXrDqC50eHQ=?=\"\r\n"
        b"Content-Disposition: attachment\r\n"
        b"\r\n"
        b"Notiz-Inhalt\r\n"
        b"--MIXED--\r\n",
    )
    w(
        "mail_mime_first.eml",
        b"MIME-Version: 1.0\n"
        b"Date: Thu, 9 Jan 2025 07:05:00 -0000\n"
        b"From: noreply@example.com\n"
        b"To: \n"
        b"Subject: Ohne Empfaenger\n"
        b"Content-Type: text/plain\n"
        b"\n"
        b"Nur\tTabs\tund\n\n\nLeerzeilen.\n",
    )
    w(
        "mail_no_date.eml",
        b"From: Ohne Datum <nodate@example.com>\n"
        b"To: x@example.com\n"
        b"Subject: Kein Datum\n"
        b"\n"
        b"Text.\n",
    )
    w(
        "mail_no_from.eml",
        b"Return-Path: <x@example.com>\n"
        b"To: x@example.com\n"
        b"Subject: Kein Absender\n"
        b"Date: Fri, 10 Jan 2025 12:00:00 +0200\n"
        b"\n"
        b"Text.\n",
    )
    inner = (
        b"From: Original <orig@example.com>\n"
        b"To: x@example.com\n"
        b"Subject: Innere Nachricht\n"
        b"Date: Sat, 11 Jan 2025 12:00:00 +0100\n"
        b"Content-Type: multipart/mixed; boundary=INNER\n"
        b"\n"
        b"--INNER\n"
        b"Content-Type: text/plain\n"
        b"\n"
        b"Innerer Text.\n"
        b"--INNER\n"
        b"Content-Type: text/csv; name=liste.csv\n"
        b"\n"
        b"a,b\n1,2\n"
        b"--INNER--\n"
    )
    w(
        "mail_forward.eml",
        b"Delivered-To: fwd@example.com\n"
        b"From: Weiterleiter <fwd@example.com>\n"
        b"To: ziel@example.com\n"
        b"Subject: Fwd: Innere Nachricht\n"
        b"Date: Sun, 12 Jan 2025 23:59:59 +0530\n"
        b"Content-Type: multipart/mixed; boundary=OUTER\n"
        b"\n"
        b"--OUTER\n"
        b"Content-Type: text/plain\n"
        b"\n"
        b"Siehe weitergeleitete Nachricht.\n"
        b"--OUTER\n"
        b"Content-Type: message/rfc822\n"
        b"Content-Disposition: attachment\n"
        b"\n" + inner + b"--OUTER--\n",
    )
    w(
        "mail_edge_headers.eml",
        b"From: \"M\xc3\xbcller, Hans\" (Chef) <hans@example.ch>\r\n"
        b"To: a@example.com,\r\n b@example.com, \"C, D\" <cd@example.com>\r\n"
        b"To: zweite@example.com\r\n"
        b"Cc: (nur Kommentar) x@example.com\r\n"
        b"Bcc: undisclosed-recipients:;\r\n"
        b"Subject: =?utf-8?Q?Teil_eins?=\r\n =?utf-8?B?IHVuZCB6d2Vp?= normal\r\n"
        b"Date: 31 Mar 2024 02:30:00 -0000\r\n"
        b"Content-Type: multipart/mixed; boundary=\"=_outer\"\r\n"
        b"\r\n"
        b"--=_outer\r\n"
        b"Content-Type: multipart/alternative; boundary=inner\r\n"
        b"\r\n"
        b"--inner\r\n"
        b"Content-Type: text/plain; charset=windows-1252\r\n"
        b"Content-Transfer-Encoding: quoted-printable\r\n"
        b"\r\n"
        b"Preis: 10=80 =E4\r\n"
        b"--inner\r\n"
        b"Content-Type: text/html; charset=\"utf-8\"\r\n"
        b"\r\n"
        b"<p>Preis: 10&euro;</p>\r\n"
        b"--inner--\r\n"
        b"\r\n"
        b"--=_outer\r\n"
        b"Content-Type: application/octet-stream\r\n"
        b"Content-Disposition: attachment; filename*0*=utf-8''la%C3%A4ng; filename*1=\"er_name.dat\"\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        b"AAECAwQFBgcICQ\r\n"
        b"--=_outer\r\n"
        b"Content-Type: text/plain; name=\"Gr\xc3\xbc\xc3\x9fe.txt\"\r\n"
        b"Content-Disposition: attachment; filename=\"Gr\xc3\xbc\xc3\x9fe.txt\"\r\n"
        b"\r\n"
        b"Inhalt\r\n"
        b"--=_outer\r\n"
        b"Content-Type: message/rfc822\r\n"
        b"\r\n"
        b"From: inner@example.com\r\n"
        b"Subject: innen\r\n"
        b"\r\n"
        b"Innen-Text\r\n"
        b"--=_outer--\r\n"
        b"Epilog\r\n",
    )
    w(
        "mail_no_boundary.eml",
        b"From: a@example.com\n"
        b"To: b@example.com\n"
        b"Subject: Ohne Grenze\n"
        b"Date: Tue, 2 Jul 2024 12:00:00 +0200\n"
        b"Content-Type: multipart/mixed\n"
        b"\n"
        b"--x\nContent-Type: text/plain\n\nText\n--x--\n",
    )
    w(
        "mail_latin1_body.eml",
        b"From: a@example.com\n"
        b"To: b@example.com\n"
        b"Subject: Latin\n"
        b"Date: Sat, 26 Oct 2024 23:59:00\n"
        b"Content-Type: text/plain; charset=\"ISO-8859-1\"\n"
        b"Content-Transfer-Encoding: 8bit\n"
        b"\n"
        b"Gr\xfc\xdfe \x85 und \xa0 Ende\n",
    )
    w(
        "mail_bad_charset.eml",
        b"From: a@example.com\n"
        b"To: b@example.com\n"
        b"Subject: =?x-unknown?Q?abc?= def\n"
        b"Date: Mon, 32 Jan 2024 10:00:00 +0100\n"
        b"Content-Type: text/plain; charset=x-unknown-charset\n"
        b"Content-Transfer-Encoding: base64\n"
        b"\n"
        b"R3LDvMOfZQ\n",
    )
    w(
        "mail_digest.eml",
        b"From: digest@example.com\n"
        b"To: list@example.com\n"
        b"Subject: Digest\n"
        b"Date: 5 Nov 2024 10:00 EST\n"
        b"Content-Type: multipart/digest; boundary=D\n"
        b"\n"
        b"--D\n"
        b"\n"
        b"From: one@example.com\nSubject: eins\n\nErster\n"
        b"--D\n"
        b"\n"
        b"From: two@example.com\nSubject: zwei\n\nZweiter\n"
        b"--D--\n",
    )
    w(
        "mail_summer.eml",
        b"From: Sommer <sommer@example.com>\n"
        b"To: a@example.com\n"
        b"Subject: Sommerzeit\n"
        b"Date: 1 Jul 2024 14:00 GMT\n"
        b"\n"
        b"Text.\n",
    )


# ---------------------------------------------------------------------------
# PDF
# ---------------------------------------------------------------------------


def pdf_samples() -> None:
    import pikepdf
    from fpdf import FPDF

    def make(pages: list[str], *, title: str | None = None) -> bytes:
        pdf = FPDF()
        pdf.set_creation_date(__import__("datetime").datetime(2025, 1, 6, 8, 0, 0, tzinfo=__import__("datetime").timezone.utc))
        if title:
            pdf.set_title(title)
            pdf.set_author("Paperless Test")
            pdf.set_subject("Paritaet")
            pdf.set_keywords("rust parser")
            pdf.set_creator("gen_samples.py")
        pdf.set_font("helvetica", size=12)
        for text in pages:
            pdf.add_page()
            if text:
                pdf.multi_cell(0, 8, text)
        return bytes(pdf.output())

    long_text = (
        "Dies ist ein born-digital PDF mit ausreichend Text, um die Grenze von fuenfzig "
        "Zeichen sicher zu ueberschreiten. Rechnung vom 06.01.2025."
    )
    w("pdf_text.pdf", make([long_text], title="Rechnung 2025"))
    w("pdf_multipage.pdf", make(["Seite eins", "Seite zwei", "Seite drei"]))
    w("pdf_empty_page.pdf", make([""]))
    w("pdf_short_text.pdf", make(["Kurz"]))

    # XMP metadata with pikepdf (synchronizes DocInfo -> XMP)
    with pikepdf.open(io.BytesIO(make([long_text], title="Mit XMP"))) as pdf:
        with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
            meta["dc:title"] = "Mit XMP"
            meta["dc:creator"] = ["Autor Eins", "Autor Zwei"]
            meta["dc:description"] = "Beschreibung"
            meta["xmp:CreatorTool"] = "gen_samples.py"
            meta["pdf:Producer"] = "fpdf2"
            meta["xmp:CreateDate"] = "2025-01-06T08:00:00+00:00"
        buf = io.BytesIO()
        pdf.save(buf, static_id=True, deterministic_id=True)
        w("pdf_xmp.pdf", buf.getvalue())

    # Tagged (MarkInfo /Marked true) with short text
    with pikepdf.open(io.BytesIO(make(["Kurz getaggt"]))) as pdf:
        pdf.Root.MarkInfo = pikepdf.Dictionary(Marked=True)
        buf = io.BytesIO()
        pdf.save(buf, static_id=True, deterministic_id=True)
        w("pdf_tagged.pdf", buf.getvalue())

    # Encrypted with empty user password (opens without password)
    with pikepdf.open(io.BytesIO(make([long_text]))) as pdf:
        buf = io.BytesIO()
        pdf.save(
            buf,
            encryption=pikepdf.Encryption(owner="owner", user="", R=4, aes=True),
            static_id=True,
        )
        w("pdf_encrypted_empty_user.pdf", buf.getvalue())
    with pikepdf.open(io.BytesIO(make([long_text]))) as pdf:
        buf = io.BytesIO()
        pdf.save(
            buf,
            encryption=pikepdf.Encryption(owner="owner", user="secret", R=4, aes=True),
            static_id=True,
        )
        w("pdf_encrypted_password.pdf", buf.getvalue())

    good = make([long_text])
    w("pdf_truncated.pdf", good[: len(good) // 2])
    w("pdf_garbage.pdf", b"Das ist kein PDF, nur Text mit .pdf-Endung.\n")
    w("pdf_garbage_binary.pdf", b"\x00\x01\x02garbage" * 50)
    w("pdf_leading_junk.pdf", b"\x00\x00JUNK" + good)


# ---------------------------------------------------------------------------
# Images
# ---------------------------------------------------------------------------


def image_samples() -> None:
    from PIL import Image

    img = Image.new("RGB", (40, 30), (200, 30, 30))
    for fmt, name, kw in (
        ("PNG", "image.png", {}),
        ("JPEG", "image.jpg", {"quality": 80}),
        ("GIF", "image.gif", {}),
        ("BMP", "image.bmp", {}),
        ("TIFF", "image.tif", {}),
        ("WEBP", "image.webp", {"lossless": True}),
    ):
        buf = io.BytesIO()
        img.save(buf, format=fmt, **kw)
        w(name, buf.getvalue())
    buf = io.BytesIO()
    img.save(buf, format="TIFF", compression=None)
    # Big-Endian-TIFF ("MM")
    data = bytearray(buf.getvalue())
    w("image_le.tif", bytes(data))
    try:
        import pillow_heif

        pillow_heif.register_heif_opener()
        buf = io.BytesIO()
        img.save(buf, format="HEIF")
        w("image.heic", buf.getvalue())
    except Exception as e:  # pragma: no cover
        print("HEIC nicht erzeugt:", e)


# ---------------------------------------------------------------------------
# Office (ZIP + XML by hand)
# ---------------------------------------------------------------------------


def zipbytes(entries: list[tuple[str, bytes]], *, stored_first: bool = False) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for i, (name, data) in enumerate(entries):
            zi = zipfile.ZipInfo(name, date_time=FIXED_DT)
            zi.compress_type = (
                zipfile.ZIP_STORED if (stored_first and i == 0) else zipfile.ZIP_DEFLATED
            )
            z.writestr(zi, data)
    return buf.getvalue()


CT_HEAD = b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'


def rels(target: str) -> bytes:
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{target}"/>'
        '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
        '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>'
        "</Relationships>"
    ).encode()


CORE = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
    'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
    'xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
    "<dc:title>Office Titel</dc:title><dc:subject>Betreff</dc:subject><dc:creator>Erika Muster</dc:creator>"
    "<cp:keywords>a, b</cp:keywords><dc:description>Beschreibung</dc:description>"
    "<cp:lastModifiedBy>Max Muster</cp:lastModifiedBy><cp:revision>3</cp:revision>"
    '<dcterms:created xsi:type="dcterms:W3CDTF">2025-01-06T08:00:00Z</dcterms:created>'
    '<dcterms:modified xsi:type="dcterms:W3CDTF">2025-01-07T09:30:00Z</dcterms:modified>'
    "</cp:coreProperties>"
).encode()

APP = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">'
    "<Application>Microsoft Office Word</Application><Pages>1</Pages><Words>12</Words></Properties>"
).encode()

W_NS = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'

DOCX_BODY = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:document {W_NS}><w:body>'
    '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Überschrift</w:t></w:r></w:p>'
    '<w:p><w:r><w:t xml:space="preserve">Erster </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>Absatz</w:t></w:r>'
    "<w:r><w:tab/><w:t>mit Tab</w:t></w:r><w:r><w:br/><w:t>und Umbruch</w:t></w:r></w:p>"
    "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Zelle A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Zelle B1</w:t></w:r></w:p></w:tc></w:tr>"
    "<w:tr><w:tc><w:p><w:r><w:t>Zelle A2</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Zelle B2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
    '<w:p><w:r><w:t>Text &amp; Zeichen &lt;ok&gt;</w:t></w:r><w:del><w:r><w:delText>geloescht</w:delText></w:r></w:del>'
    '<w:ins><w:r><w:t> eingefuegt</w:t></w:r></w:ins></w:p>'
    "</w:body></w:document>"
).encode()


def docx(order: str) -> bytes:
    ct = CT_HEAD + (
        b'<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        b'<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>'
        b'<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>'
    )
    parts = {
        "[Content_Types].xml": ct,
        "_rels/.rels": rels("word/document.xml"),
        "word/document.xml": DOCX_BODY,
        "word/_rels/document.xml.rels": b'<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>',
        "docProps/core.xml": CORE,
        "docProps/app.xml": APP,
        "customXml/item1.xml": b"<x/>",
    }
    orders = {
        "word": ["[Content_Types].xml", "_rels/.rels", "word/document.xml", "word/_rels/document.xml.rels", "docProps/core.xml", "docProps/app.xml"],
        "lo": ["_rels/.rels", "docProps/core.xml", "docProps/app.xml", "word/_rels/document.xml.rels", "word/document.xml", "[Content_Types].xml"],
        "late": ["[Content_Types].xml", "_rels/.rels", "docProps/core.xml", "docProps/app.xml", "customXml/item1.xml", "word/document.xml"],
    }
    return zipbytes([(n, parts[n]) for n in orders[order]])


def xlsx() -> bytes:
    ct = CT_HEAD + (
        b'<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        b'<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        b'<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        b'<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'
        b'<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>'
    )
    S = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
    wb = f'<?xml version="1.0"?><workbook {S}><sheets><sheet name="Umsatz" sheetId="1" r:id="rId1"/><sheet name="Leer &amp; Zwei" sheetId="2" r:id="rId2"/></sheets></workbook>'.encode()
    wbrels = (
        b'<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
        b'<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
        b'<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>'
        b"</Relationships>"
    )
    sst = f'<?xml version="1.0"?><sst {S} count="4" uniqueCount="4"><si><t>Monat</t></si><si><t>Betrag</t></si><si><t>Jänner</t></si><si><r><t>Fe</t></r><r><t>bruar</t></r></si></sst>'.encode()
    sheet1 = (
        f'<?xml version="1.0"?><worksheet {S}><sheetData>'
        '<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>'
        '<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>1234.5</v></c></row>'
        '<row r="3"><c r="A3" t="s"><v>3</v></c><c r="C3" t="inlineStr"><is><t>inline</t></is></c></row>'
        '<row r="4"><c r="A4" t="b"><v>1</v></c><c r="B4" t="str"><f>A1</f><v>Formel</v></c></row>'
        "</sheetData></worksheet>"
    ).encode()
    sheet2 = f'<?xml version="1.0"?><worksheet {S}><sheetData/></worksheet>'.encode()
    return zipbytes(
        [
            ("[Content_Types].xml", ct),
            ("_rels/.rels", rels("xl/workbook.xml")),
            ("xl/workbook.xml", wb),
            ("xl/_rels/workbook.xml.rels", wbrels),
            ("xl/sharedStrings.xml", sst),
            ("xl/worksheets/sheet1.xml", sheet1),
            ("xl/worksheets/sheet2.xml", sheet2),
            ("docProps/core.xml", CORE),
        ]
    )


def pptx(show: bool = False) -> bytes:
    main = "slideshow" if show else "presentation"
    ct = CT_HEAD + (
        f'<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.{main}.main+xml"/>'
        '<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>'
        '<Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>'
        '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>'
    ).encode()
    P = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
    pres = f'<?xml version="1.0"?><p:presentation {P}><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst></p:presentation>'.encode()
    prels = (
        b'<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        b'<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>'
        b'<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>'
        b"</Relationships>"
    )

    def slide(texts: list[str]) -> bytes:
        body = "".join(
            f'<p:sp><p:txBody><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp>' for t in texts
        )
        return f'<?xml version="1.0"?><p:sld {P}><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>'.encode()

    return zipbytes(
        [
            ("[Content_Types].xml", ct),
            ("_rels/.rels", rels("ppt/presentation.xml")),
            ("ppt/presentation.xml", pres),
            ("ppt/_rels/presentation.xml.rels", prels),
            ("ppt/slides/slide1.xml", slide(["Folie Zwei Titel", "Zweiter Punkt"])),
            ("ppt/slides/slide2.xml", slide(["Folie Eins Titel", "Erster Punkt &amp; mehr"])),
            ("docProps/core.xml", CORE),
        ]
    )


ODF_NS = (
    'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
    'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
    'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
    'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
    'xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" '
    'xmlns:dc="http://purl.org/dc/elements/1.1/" '
    'xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3"'
)

ODF_META = (
    f'<?xml version="1.0" encoding="UTF-8"?>\n<office:document-meta {ODF_NS}><office:meta>'
    "<meta:generator>LibreOffice/24.2</meta:generator><dc:title>ODF Titel</dc:title>"
    "<dc:creator>Erika Muster</dc:creator><meta:initial-creator>Max Muster</meta:initial-creator>"
    "<meta:creation-date>2025-01-06T08:00:00</meta:creation-date><dc:date>2025-01-07T09:30:00.123</dc:date>"
    "<meta:keyword>eins</meta:keyword><meta:keyword>zwei</meta:keyword>"
    '<meta:document-statistic meta:page-count="1" meta:word-count="5"/>'
    "</office:meta></office:document-meta>"
).encode()


def odf(kind: str, body: str) -> bytes:
    mt = f"application/vnd.oasis.opendocument.{kind}".encode()
    content = f'<?xml version="1.0" encoding="UTF-8"?>\n<office:document-content {ODF_NS}><office:body>{body}</office:body></office:document-content>'.encode()
    manifest = b'<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>'
    return zipbytes(
        [
            ("mimetype", mt),
            ("Configurations2/", b""),
            ("content.xml", content),
            ("meta.xml", ODF_META),
            ("META-INF/manifest.xml", manifest),
        ],
        stored_first=True,
    )


def office_samples() -> None:
    w("office_word_order.docx", docx("word"))
    w("office_lo_order.docx", docx("lo"))
    w("office_late_word.docx", docx("late"))
    w("office.xlsx", xlsx())
    w("office.pptx", pptx())
    w("office.ppsx", pptx(show=True))
    w(
        "office.odt",
        odf(
            "text",
            '<office:text><text:h text:outline-level="1">ODF Überschrift</text:h>'
            '<text:p>Erster<text:s/>Absatz<text:tab/>mit Tab<text:line-break/>Umbruch</text:p>'
            '<text:p>Zweiter <text:span>Absatz</text:span> &amp; mehr<text:s text:c="3"/>Ende</text:p>'
            "<text:list><text:list-item><text:p>Listenpunkt</text:p></text:list-item></text:list>"
            "<table:table><table:table-row><table:table-cell><text:p>T1</text:p></table:table-cell>"
            "<table:table-cell><text:p>T2</text:p></table:table-cell></table:table-row></table:table>"
            "</office:text>",
        ),
    )
    w(
        "office.ods",
        odf(
            "spreadsheet",
            '<office:spreadsheet><table:table table:name="Tabelle1"><table:table-row>'
            "<table:table-cell><text:p>Monat</text:p></table:table-cell>"
            '<table:table-cell office:value-type="float" office:value="12.5"><text:p>12,50</text:p></table:table-cell>'
            '</table:table-row><table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="16384"/></table:table-row>'
            '</table:table><table:table table:name="Zweite"><table:table-row><table:table-cell table:number-columns-repeated="2"><text:p>wdh</text:p></table:table-cell></table:table-row></table:table>'
            "</office:spreadsheet>",
        ),
    )
    w(
        "office.odp",
        odf(
            "presentation",
            '<office:presentation><draw:page draw:name="S1"><draw:frame><draw:text-box>'
            "<text:p>Folie eins</text:p></draw:text-box></draw:frame></draw:page>"
            '<draw:page draw:name="S2"><draw:frame><draw:text-box><text:p>Folie zwei</text:p>'
            "</draw:text-box></draw:frame></draw:page></office:presentation>",
        ),
    )
    w(
        "office.odg",
        odf(
            "graphics",
            '<office:drawing><draw:page draw:name="S1"><draw:custom-shape><text:p>Grafiktext</text:p>'
            "</draw:custom-shape></draw:page></office:drawing>",
        ),
    )
    w(
        "office.rtf",
        b"{\\rtf1\\ansi\\ansicpg1252\\deff0{\\fonttbl{\\f0 Arial;}}{\\info{\\title RTF Titel}{\\author Erika}}"
        b"{\\*\\generator Hand;}\\f0\\fs24 Erster Absatz mit Umlaut: \\'fc\\'e4\\par\n"
        b"Zweiter\\tab Absatz mit \\b fett\\b0  und Unicode \\u8364?.\\par\n"
        b"{\\pict\\pngblip 89504e47}Ende\\line Zeile\\par}\n",
    )
    w("plain.zip", zipbytes([("readme.txt", b"hallo"), ("data/x.bin", b"\x00" * 10)]))
    # Zip bomb: 20 MiB of zeros in document.xml (~20 KiB compressed)
    bomb = (
        DOCX_BODY.replace(b"</w:body>", b"<w:p><w:r><w:t>" + b"A" * (20 * 1024 * 1024) + b"</w:t></w:r></w:p></w:body>")
    )
    parts = docx("word")
    with zipfile.ZipFile(io.BytesIO(parts)) as z:
        entries = [(n, z.read(n)) for n in z.namelist()]
    entries = [(n, bomb if n == "word/document.xml" else d) for n, d in entries]
    w("office_zipbomb.docx", zipbytes(entries))
    full = docx("word")
    w("office_truncated.docx", full[: len(full) // 2])
    w("office_not_zip.docx", b"Kein ZIP, nur Text.\n")


PROBES: dict[str, bytes] = {
    # CSV heuristic (libmagic is_csv)
    "csv_one_line": b"a,b\n",
    "csv_two_lines": b"a,b\n1,2\n",
    "csv_three_lines": b"a,b\n1,2\n3,4\n",
    "csv_no_final_eol": b"a,b\n1,2\n3,4",
    "csv_quoted": b'a,"b,c"\n1,"x ""y"", z"\n3,4\n',
    "csv_inconsistent": b"a,b\n1,2,3\n4,5\n",
    "csv_crlf": b"a,b\r\n1,2\r\n3,4\r\n",
    "csv_twelve_lines": b"a,b\n" * 11 + b"x\n",
    "csv_tab": b"a\tb\n1\t2\n3\t4\n",
    # JSON
    "json_number": b"123\n",
    "json_string": b'"abc"\n',
    "json_array": b"[1, 2, 3]\n",
    "json_ndjson": b'{"a": 1}\n{"a": 2}\n',
    "json_invalid": b'{"a": 1,}\n',
    # Mail header lines at the file start
    "mail_subject_first": b"Subject: Hallo\nFrom: a@example.com\n\nText\n",
    "mail_date_first": b"Date: Mon, 6 Jan 2025 10:00:00 +0100\nFrom: a@example.com\n\nText\n",
    "mail_messageid_first": b"Message-ID: <x@y>\nFrom: a@example.com\n\nText\n",
    "mail_mozilla_first": b"X-Mozilla-Status: 0001\nX-Mozilla-Status2: 00000000\nFrom: a@example.com\n\nText\n",
    "mail_to_first": b"To: b@example.com\nFrom: a@example.com\n\nText\n",
    "mail_reply_to_first": b"Reply-To: b@example.com\nFrom: a@example.com\n\nText\n",
    "mail_content_type_first": b"Content-Type: text/plain\nFrom: a@example.com\n\nText\n",
    "mail_xheader_first": b"X-Foo: bar\nFrom: a@example.com\n\nText\n",
    "mail_mbox": b"From a@example.com Mon Jan  6 10:00:00 2025\nFrom: a@example.com\n\nText\n",
    "mail_return_path_lower": b"return-path: <a@example.com>\nFrom: a@example.com\n\nText\n",
    "mail_delivered_to": b"Delivered-To: a@example.com\nFrom: a@example.com\n\nText\n",
    "mail_received_lower": b"received: from x\nFrom: a@example.com\n\nText\n",
    "mail_from_lower": b"from: a@example.com\n\nText\n",
    "mail_from_nospace": b"From:a@example.com\n\nText\n",
    "mail_leading_blank": b"\nFrom: a@example.com\n\nText\n",
    "mail_arc_first": b"ARC-Seal: i=1; a=rsa-sha256\nFrom: a@example.com\n\nText\n",
    "mail_dkim_first": b"DKIM-Signature: v=1\nFrom: a@example.com\n\nText\n",
    "mail_authres_first": b"Authentication-Results: mx\nFrom: a@example.com\n\nText\n",
    # HTML features
    "html_doctype": b"<!DOCTYPE html><p>x</p>\n",
    "html_doctype_lower_late": b"Some text first\n" * 20 + b"<!doctype html>\n",
    "html_head": b"text <head> more\n",
    "html_title": b"text <title>x</title>\n",
    "html_html": b"text <html> more\n",
    "html_body": b"text <body> more\n",
    "html_script": b"text <script> more\n",
    "html_style": b"text <style> more\n",
    "html_table": b"text <table> more\n",
    "html_a_href": b'text <a href="x">link</a>\n',
    "html_div": b"text <div>x</div>\n",
    "html_p": b"<p>Absatz</p>\n",
    "html_br": b"Zeile<br>Zeile\n",
    "html_comment": b"<!-- Kommentar -->\nText\n",
    "html_upper": b"<HTML><BODY>X</BODY></HTML>\n",
    "html_h1": b"<h1>Titel</h1>\n",
    "html_late": b"x" * 5000 + b"\n<html>\n",
    "xhtml": b'<?xml version="1.0"?>\n<html xmlns="http://www.w3.org/1999/xhtml"><body>x</body></html>\n',
    "xml_no_decl": b"<root><a>1</a></root>\n",
    "xml_svg": b'<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg"></svg>\n',
    # Source code heuristics
    "c_define": b"#define X 1\nText\n",
    "c_include_late": b"Text\nmehr Text\n#include <x.h>\n",
    "c_comment": b"/* Kommentar */\nText\n",
    "c_main": b"Text\nint main(int argc, char **argv)\n",
    "c_struct": b"struct foo {\n int a;\n};\n",
    "c_char": b"char *p = 0;\n",
    "sh_shebang": b"#!/bin/sh\necho hi\n",
    "bash_shebang": b"#!/bin/bash\necho hi\n",
    "perl_shebang": b"#!/usr/bin/perl\nprint 1;\n",
    "env_node": b"#!/usr/bin/env node\nconsole.log(1)\n",
    "php": b"<?php echo 1; ?>\n",
    "python_import": b"import os\nimport sys\n",
    "python_def": b"def foo():\n    return 1\n",
    "python_class": b"class Foo:\n    pass\n",
    "postscript": b"%!PS-Adobe-3.0\n%%EOF\n",
    "vcard": b"BEGIN:VCARD\nVERSION:3.0\nFN:Max\nEND:VCARD\n",
    "vcalendar": b"BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n",
    "pgp": b"-----BEGIN PGP MESSAGE-----\nabc\n-----END PGP MESSAGE-----\n",
    "pem_cert": b"-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----\n",
    "rtf_lower": b"{\\rtf1 hallo}\n",
    "tex": b"\\documentclass{article}\n\\begin{document}x\\end{document}\n",
    "makefile": b"all:\n\tcc -o x x.c\n",
    "ini": b"[section]\nkey=value\n",
    "diff": b"diff --git a/x b/x\n--- a/x\n+++ b/x\n",
    "markdown_heading": b"# Titel\n\nText mit `code`.\n",
    "javascript": b"function foo() { return 1; }\n",
    "java": b"public class Foo {\n}\n",
    "svg_plain": b'<svg xmlns="http://www.w3.org/2000/svg"></svg>\n',
    "troff": b".TH FOO 1\n.SH NAME\n",
    "german_invoice": "Rechnung\nPos. 1: Stuhl, 2 Stück, 50,00 €\nSumme: 100,00 €\n".encode(),
    # Binary/control characters
    "ctrl_esc": b"Text mit \x1b[0m ESC\n",
    "ctrl_formfeed": b"Seite 1\x0cSeite 2\n",
    "ctrl_bel": b"Ding\x07Dong\n",
    "ctrl_0x01": b"Text\x01mit SOH\n",
    "ctrl_0x7f": b"Text\x7fmit DEL\n",
    "ctrl_0x1c": b"Text\x1cmit FS\n",
    "c1_bytes": b"Text mit \x85\x96 Bytes\n",
    "utf16le_nobom": "Hallo Welt\n".encode("utf-16-le"),
    "utf8_bad_cont": b"Text \xc3 kaputt\n",
    "only_newlines": b"\n\n\n",
    "only_spaces": b"   ",
    "single_byte": b"a",
    # Further binary formats
    "gzip": b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03" + b"\x00" * 20,
    "jpeg_short": b"\xff\xd8\xff\xe0\x00\x10JFIF\x00",
    "png_sig_only": b"\x89PNG\r\n\x1a\n",
    "bmp_bad_header": b"BMxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "tiff_be": b"MM\x00*\x00\x00\x00\x08" + b"\x00" * 20,
    "riff_wave": b"RIFF\x24\x00\x00\x00WAVEfmt " + b"\x00" * 20,
    "pdf_bom": b"\xef\xbb\xbf%PDF-1.4\n%%EOF\n",
    "pdf_space": b" %PDF-1.4\n%%EOF\n",
    "pdf_late_header": b"\n\n%PDF-1.4\n%%EOF\n",
    "ftyp_mif1": b"\x00\x00\x00\x18ftypmif1\x00\x00\x00\x00mif1heic" + b"\x00" * 16,
    "ftyp_heix": b"\x00\x00\x00\x18ftypheix\x00\x00\x00\x00mif1heix" + b"\x00" * 16,
    "ftyp_avif": b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00mif1avif" + b"\x00" * 16,
    "cfb_empty": b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + b"\x00" * 504,
}


def zip_probes() -> dict[str, bytes]:
    import random

    rnd = random.Random(42)
    big = bytes(rnd.getrandbits(8) for _ in range(9000))  # not compressible
    ct = b"<Types/>"
    p = {
        "zip_ooxml_xl_third": zipbytes([("[Content_Types].xml", ct), ("_rels/.rels", b"<r/>"), ("xl/workbook.xml", b"<w/>")]),
        "zip_ooxml_second_word": zipbytes([("[Content_Types].xml", ct), ("word/document.xml", b"<w/>"), ("_rels/.rels", b"<r/>")]),
        "zip_ooxml_first_word": zipbytes([("word/document.xml", b"<w/>"), ("[Content_Types].xml", ct)]),
        "zip_ooxml_big_second": zipbytes([("[Content_Types].xml", ct), ("_rels/.rels", big), ("word/document.xml", b"<w/>")]),
        "zip_ooxml_big_first": zipbytes([("[Content_Types].xml", big), ("_rels/.rels", b"<r/>"), ("word/document.xml", b"<w/>")]),
        "zip_ooxml_ppt_fourth": zipbytes([("_rels/.rels", b"<r/>"), ("docProps/app.xml", b"<a/>"), ("docProps/core.xml", b"<c/>"), ("ppt/presentation.xml", b"<p/>")]),
        "zip_ooxml_fifth": zipbytes([("_rels/.rels", b"<r/>"), ("docProps/app.xml", b"<a/>"), ("docProps/core.xml", b"<c/>"), ("customXml/x.xml", b"<c/>"), ("word/document.xml", b"<p/>")]),
        "zip_ooxml_visio": zipbytes([("[Content_Types].xml", ct), ("_rels/.rels", b"<r/>"), ("visio/document.xml", b"<w/>")]),
        "zip_docprops_first_only": zipbytes([("docProps/core.xml", b"<c/>"), ("data.bin", b"x")]),
        "zip_customxml_first": zipbytes([("customXml/item.xml", b"<c/>"), ("_rels/.rels", b"<r/>"), ("word/document.xml", b"<w/>")]),
        "zip_odt_template": zipbytes([("mimetype", b"application/vnd.oasis.opendocument.text-template"), ("content.xml", b"<x/>")], stored_first=True),
        "zip_ods_template": zipbytes([("mimetype", b"application/vnd.oasis.opendocument.spreadsheet-template"), ("content.xml", b"<x/>")], stored_first=True),
        "zip_odf_formula": zipbytes([("mimetype", b"application/vnd.oasis.opendocument.formula"), ("content.xml", b"<x/>")], stored_first=True),
        "zip_odt_deflated_mimetype": zipbytes([("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", b"<x/>")]),
        "zip_odt_not_first": zipbytes([("content.xml", b"<x/>"), ("mimetype", b"application/vnd.oasis.opendocument.text")], stored_first=True),
        "zip_epub": zipbytes([("mimetype", b"application/epub+zip"), ("META-INF/container.xml", b"<x/>")], stored_first=True),
        "zip_jar": zipbytes([("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"), ("a.class", b"\xca\xfe\xba\xbe")]),
        "zip_empty": b"PK\x05\x06" + b"\x00" * 18,
    }
    return p


def probe_samples() -> None:
    d = OUT / "mime"
    d.mkdir(exist_ok=True)
    for name, data in {**PROBES, **zip_probes()}.items():
        (d / name).write_bytes(data)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    probe_samples()
    text_samples()
    eml_samples()
    pdf_samples()
    image_samples()
    office_samples()
    print("Samples geschrieben:", len(list(OUT.iterdir())))


if __name__ == "__main__":
    main()
