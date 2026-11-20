"""Generate QR/SVG + TOTP/recovery-code test vectors with the exact upstream libraries.

Usage: python -I qr_vectors.py <site-packages-with-qrcode-8.2-and-lxml> <out-file>
Upstream: allauth 65.20.1 DefaultMFAAdapter.build_totp_svg / build_totp_url,
qrcode 8.2 SvgPathImage (serialised by lxml.etree).
"""
import random
import sys

sys.path.insert(0, sys.argv[1])
from io import BytesIO  # noqa: E402
from urllib.parse import quote, urlencode  # noqa: E402

import qrcode  # noqa: E402
from qrcode.image.svg import SvgPathImage  # noqa: E402

import qrcode.base as qb  # noqa: E402

rnd = random.Random(4711)
B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"


def url(label, secret):
    params = {"secret": secret, "issuer": "Paperless-ngx"}
    return f"otpauth://totp/{quote(label)}?{urlencode(params, quote_via=quote)}"


inputs = [
    "otpauth://totp/a?secret=AB",
    "A",
    "12345678901234567890123",
    "HELLO WORLD 1234567890 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "x" * 150,
    "x" * 300,
    "31415" * 140,
]
labels = ["admin", "test@example.com", "Jöran Müller", "very.long.address+tag@sub.example.org", "u" * 120]
for lab in labels:
    for _ in range(3):
        inputs.append(url(lab, "".join(rnd.choice(B32) for _ in range(32))))

with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as out:
    for data in inputs:
        img = qrcode.make(data, image_factory=SvgPathImage)
        buf = BytesIO()
        img.save(buf)
        svg = buf.getvalue().decode("utf8")
        assert "\n" not in data
        out.write(data + "\n")
        out.write(svg.replace(chr(10), chr(92) + "n") + chr(10))

# M rows of RS_BLOCK_TABLE for cross-checking the Rust table
print([list(qb.RS_BLOCK_TABLE[(v - 1) * 4 + 1]) for v in range(1, 41)])
