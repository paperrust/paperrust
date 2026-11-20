"""Additional data for the DB move test (after reference/seed.py, against the same source DB).

Covers edge cases that the reference seed does not have: Unicode of all kinds, control characters,
large `content`, timestamps with microseconds, IPv4/IPv6 in the audit log, sessions
(string PK), all custom field value types incl. boundary values for float, JSON with
special characters, bulk data (EDGE_BULK documents) for the memory/streaming test.

Invocation: "$PY" -I tests/dbmigrate/seed_edge.py   (environment as for manage.py)
"""

from __future__ import annotations

import datetime as dt
import hashlib
import os
import sys

for p in reversed(os.environ.get("PYTHONPATH", "").split(os.pathsep)):
    if p and p not in sys.path:
        sys.path.insert(0, p)

import django  # noqa: E402

django.setup()

from auditlog.models import LogEntry  # noqa: E402
from django.contrib.auth.models import User  # noqa: E402
from django.contrib.contenttypes.models import ContentType  # noqa: E402
from django.contrib.sessions.backends.db import SessionStore  # noqa: E402
from django.db import transaction  # noqa: E402

from documents.models import CustomField  # noqa: E402
from documents.models import CustomFieldInstance  # noqa: E402
from documents.models import Document  # noqa: E402
from documents.models import Note  # noqa: E402
from documents.models import Tag  # noqa: E402
from documents.models import UiSettings  # noqa: E402

UTC = dt.timezone.utc
T0 = dt.datetime(2025, 3, 4, 5, 6, 7, 123456, tzinfo=UTC)

WEIRD = (
    "Grüße aus Zürich – „Anführung“ «Guillemets» ‹›\n"
    "Emoji 😀👩‍👩‍👧‍👦🇨🇭 Kombinierend: é å\n"
    "CJK 漢字かなカナ한국어 RTL: עברית العربية\n"
    "Tab\tBackslash \\ \\N \\x00 Quote ' \" Semikolon ; CRLF\r\nEnde LS PS ​ ZWSP ﻿ BOM"
)


def sha(s: str) -> str:
    return hashlib.sha256(s.encode()).hexdigest()


def main() -> None:
    admin = User.objects.get(username="admin")
    bulk = int(os.environ.get("EDGE_BULK", "0"))
    with transaction.atomic():
        big = ("Seite mit viel Text äöü 😀 " * 40 + "\n") * 3000  # ~3,3 MB
        d1 = Document.objects.create(
            title="Ünïcødé 😀 " + "ß" * 100,
            content=WEIRD + "\n" + big,
            mime_type="application/pdf",
            checksum=sha("edge-1"),
            created=dt.date(1970, 1, 1),
            owner=admin,
        )
        Document.objects.filter(pk=d1.pk).update(modified=T0, added=T0 - dt.timedelta(microseconds=1))
        d2 = Document.objects.create(
            title="x" * 128,
            content="",
            mime_type="text/plain",
            checksum=sha("edge-2"),
            created=dt.date(2099, 12, 31),
            archive_serial_number=2147483647,
        )
        Note.objects.create(document=d1, note=WEIRD, user=admin)
        Tag.objects.create(name="Ünïcødé-Tag 😀", color="#ff00aa", match="^ä.*$", matching_algorithm=4)

        fields = {f.data_type: f for f in CustomField.objects.all()}
        vals = [
            ("string", "value_text", WEIRD[:128]),
            ("url", "value_url", "https://example.invalid/ä?q=1&x=%20"),
            ("date", "value_date", dt.date(1900, 2, 28)),
            ("boolean", "value_bool", False),
            ("integer", "value_int", -2147483648),
            ("float", "value_float", 0.1),
            ("monetary", "value_monetary", "EUR1234567.89"),
            ("documentlink", "value_document_ids", [d2.pk, d1.pk]),
            ("longtext", "value_long_text", WEIRD * 3),
        ]
        for dtype, col, v in vals:
            f = fields.get(dtype)
            if f is not None:
                CustomFieldInstance.objects.create(document=d1, field=f, **{col: v})
        if "float" in fields:
            for v in (1e-300, 1.7976931348623157e308, -0.0, 123456789.123456789, 2.0**53 + 1):
                doc = Document.objects.create(title=f"float {v!r}", content=str(v), mime_type="text/plain", checksum=sha(f"f{v!r}"), created=dt.date(2020, 1, 1))
                CustomFieldInstance.objects.create(document=doc, field=fields["float"], value_float=v)
        if "monetary" in fields:
            CustomFieldInstance.objects.create(document=d2, field=fields["monetary"], value_monetary="12.5")

        us, _ = UiSettings.objects.get_or_create(user=admin)
        us.settings = {"z": 1, "a": {"ü": ["😀", None, True, 1.5, -0.0, 10**20]}, "sep": " ", "lang": "de-de"}
        us.save()

        ct = ContentType.objects.get_for_model(Document)
        for addr in ("192.168.1.5", "2001:db8::1", "::ffff:10.0.0.1"):
            LogEntry.objects.create(
                content_type=ct, object_pk=str(d1.pk), object_id=d1.pk, object_repr=WEIRD[:50],
                action=1, changes={"title": ["alt", "neu ü"]}, remote_addr=addr, actor=admin, timestamp=T0,
                additional_data={"k": WEIRD},
            )

        for i in range(3):
            s = SessionStore()
            s["_auth_user_id"] = str(admin.pk)
            s["n"] = i
            s.set_expiry(dt.datetime(2030, 1, 1, tzinfo=UTC))
            s.create()

        if bulk:
            text = "Massendokument " + "Lorem ipsum dolor sit amet äöü 😀. " * 60
            batch = []
            for i in range(bulk):
                batch.append(
                    Document(
                        title=f"Bulk {i:06d}", content=f"{i}\n{text}", mime_type="application/pdf",
                        checksum=sha(f"bulk{i}"), created=dt.date(2021, 1, 1) + dt.timedelta(days=i % 1000),
                        modified=T0, added=T0,
                    )
                )
                if len(batch) == 2000:
                    Document.objects.bulk_create(batch)
                    batch = []
            if batch:
                Document.objects.bulk_create(batch)

    print(f"seed_edge: Dokumente gesamt {Document.global_objects.count()}, bulk {bulk}")


if __name__ == "__main__":
    main()
