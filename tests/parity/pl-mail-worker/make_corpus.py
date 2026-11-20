"""Creates the test mailbox content (mails/*.eml) for pl-mail-worker – deterministic, local only.

Invocation: reference/venv/Scripts/python.exe tests/parity/pl-mail-worker/make_corpus.py
"""

import base64
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "mails"

PDF = (
    b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n"
    b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n"
)
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
)


def b64(data: bytes) -> bytes:
    s = base64.b64encode(data)
    return b"\r\n".join(s[i : i + 76] for i in range(0, len(s), 76))


def crlf(s: str) -> bytes:
    return s.replace("\n", "\r\n").encode("utf-8")


MAILS: dict[str, bytes] = {}

MAILS["01_plain.eml"] = crlf(
    "From: Alice Example <alice@example.org>\n"
    "To: paperless@example.org\n"
    "Subject: Plain text only\n"
    "Date: Mon, 06 Jan 2025 08:00:00 +0100\n"
    "Message-ID: <plain@example.org>\n"
    "\n"
    "Hello,\nthis has no attachment.\n.\nBye\n"
)

MAILS["02_pdf_attachment.eml"] = (
    crlf(
        "Return-Path: <bob@example.org>\n"
        "Received: from mx.example.org by imap.example.org; Tue, 07 Jan 2025 09:00:00 +0100\n"
        "From: \"Bob, the Builder\" <bob@example.org>\n"
        "To: paperless@example.org\n"
        "Subject: Invoice 2025-001\n"
        "Date: Tue, 07 Jan 2025 09:00:00 +0100\n"
        "MIME-Version: 1.0\n"
        "Content-Type: multipart/mixed; boundary=\"XYZ\"\n"
        "\n"
        "This is a multi-part message in MIME format.\n"
        "--XYZ\n"
        "Content-Type: text/plain; charset=utf-8\n"
        "\n"
        "Please find the invoice attached.\n"
        "--XYZ\n"
        "Content-Type: application/pdf; name=\"invoice-2025-001.pdf\"\n"
        "Content-Disposition: attachment; filename=\"invoice-2025-001.pdf\"\n"
        "Content-Transfer-Encoding: base64\n"
        "\n"
    )
    + b64(PDF)
    + crlf("\n--XYZ\n" "Content-Type: application/octet-stream\n" "Content-Disposition: attachment; filename=\"signature.asc\"\n" "\n" "-----BEGIN PGP SIGNATURE-----\nabc\n-----END PGP SIGNATURE-----\n" "--XYZ--\n" "epilogue text\n")
)

MAILS["03_long_headers.eml"] = crlf(
    "From: Carol <carol@example.org>\n"
    "To: a@example.org, b@example.org, c@example.org, d@example.org, e@example.org, f@example.org,\n"
    " g@example.org\n"
    "Subject: This is a very long subject line that definitely exceeds the seventy-eight character limit of RFC 5322\n"
    "Date: Wed, 08 Jan 2025 10:30:00 -0500\n"
    "X-Long-Token: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
    "X-Semicolons: a=1; b=2; c=3; d=4; e=5; f=6; g=7; h=8; i=9; j=10; k=11; l=12; m=13; n=14; o=15\n"
    "\n"
    "Body\n"
)

MAILS["04_8bit_headers.eml"] = crlf(
    "From: Dörte Müller <doerte@example.org>\n"
    "To: paperless@example.org\n"
    "Subject: Rechnung für März – Größe\n"
    "Date: Thu, 09 Jan 2025 11:00:00 +0100\n"
    "Content-Type: text/plain; charset=utf-8\n"
    "Content-Transfer-Encoding: 8bit\n"
    "\n"
    "Grüße aus Zürich\n"
)

MAILS["05_nested_inline.eml"] = (
    crlf(
        "From: =?utf-8?q?Eve_=C3=96sterreich?= <eve@example.org>\n"
        "To: paperless@example.org\n"
        "Subject: =?utf-8?b?QmVsZWcgw7xiZXIgMTAwIENIRg==?=\n"
        "Date: Fri, 10 Jan 2025 12:00:00 +0000\n"
        "MIME-Version: 1.0\n"
        "Content-Type: multipart/mixed; boundary=outer\n"
        "\n"
        "--outer\n"
        "Content-Type: multipart/alternative; boundary=\"inner\"\n"
        "\n"
        "--inner\n"
        "Content-Type: text/plain; charset=utf-8\n"
        "Content-Transfer-Encoding: quoted-printable\n"
        "\n"
        "Gr=C3=BC=C3=9Fe\n"
        "--inner\n"
        "Content-Type: text/html; charset=utf-8\n"
        "\n"
        "<p>Hi <img src=\"cid:logo\"></p>\n"
        "--inner--\n"
        "\n"
        "--outer\n"
        "Content-Type: image/png\n"
        "Content-ID: <logo>\n"
        "Content-Disposition: inline; filename=\"logo.png\"\n"
        "Content-Transfer-Encoding: base64\n"
        "\n"
    )
    + b64(PNG)
    + crlf(
        "\n--outer\n"
        "Content-Type: application/pdf\n"
        "Content-Disposition: attachment; filename*=utf-8''Beleg%20%C3%BCber%20100.pdf\n"
        "Content-Transfer-Encoding: base64\n"
        "\n"
    )
    + b64(PDF)
    + crlf("\n--outer--\n")
)

MAILS["06_forwarded.eml"] = crlf(
    "From: frank@example.org\n"
    "To: paperless@example.org\n"
    "Subject: Fwd: original\n"
    "Date: Sat, 11 Jan 2025 13:00:00 +0100\n"
    "MIME-Version: 1.0\n"
    "Content-Type: multipart/mixed; boundary=\"b1\"\n"
    "\n"
    "--b1\n"
    "Content-Type: text/plain\n"
    "\n"
    "see below\n"
    "--b1\n"
    "Content-Type: message/rfc822\n"
    "\n"
    "From: orig@example.org\n"
    "Subject: original\n"
    "\n"
    "original body\n"
    "--b1--\n"
)

MAILS["07_lf_only_no_close.eml"] = (
    "Subject: LF only and no close boundary\n"
    "From: Grace <grace@example.org>\n"
    "Date: Sun, 12 Jan 2025 14:00:00 +0100\n"
    "Content-Type: multipart/mixed; boundary=zz\n"
    "\n"
    "--zz\n"
    "Content-Type: text/plain\n"
    "\n"
    "part one\n"
    "--zz\n"
    "Content-Type: application/pdf\n"
    "Content-Disposition: attachment; filename=scan.PDF\n"
    "\n"
).encode() + PDF

MAILS["08_no_date_bad_from.eml"] = crlf(
    "Subject: No date header\n"
    "From: just-a-name\n"
    "\n"
    "nothing\n"
)

MAILS["09_signed.eml"] = crlf(
    "From: Heidi <heidi@example.org>\n"
    "Subject: Signed message with a rather long subject so that folding would normally apply here\n"
    "Date: Mon, 13 Jan 2025 15:00:00 +0100\n"
    "Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; micalg=pgp-sha256; boundary=\"sig\"\n"
    "\n"
    "--sig\n"
    "Content-Type: text/plain\n"
    "X-Very-Long-Inner-Header: one two three four five six seven eight nine ten eleven twelve thirteen\n"
    "\n"
    "signed text\n"
    "--sig\n"
    "Content-Type: application/pgp-signature; name=\"signature.asc\"\n"
    "Content-Disposition: attachment; filename=\"signature.asc\"\n"
    "\n"
    "-----BEGIN PGP SIGNATURE-----\nxyz\n-----END PGP SIGNATURE-----\n"
    "--sig--\n"
)

MAILS["10_inline_only.eml"] = (
    crlf(
        "From: Ivan <ivan@example.org>\n"
        "To: paperless@example.org\n"
        "Subject: Inline PDF only\n"
        "Date: Tue, 14 Jan 2025 16:00:00 +0100\n"
        "Content-Type: multipart/mixed; boundary=\"q\"\n"
        "\n"
        "--q\n"
        "Content-Type: application/pdf; name=\"inline.pdf\"\n"
        "Content-Disposition: inline; filename=\"inline.pdf\"\n"
        "Content-Transfer-Encoding: base64\n"
        "\n"
    )
    + b64(PDF)
    + crlf("\n--q--\n")
)

MAILS["11_invoice_reminder.eml"] = crlf(
    "Received: from relay.example.org\n"
    "From: Zed <zed@example.org>\n"
    "To: paperless@example.org\n"
    "Subject: Invoice reminder\n"
    "Date: Wed, 15 Jan 2025 17:00:00 +0100\n"
    "\n"
    "Please pay invoice 2025-001.\n"
)


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for name, data in MAILS.items():
        (OUT / name).write_bytes(data)
    print(f"{len(MAILS)} mails written to {OUT}")


if __name__ == "__main__":
    main()
