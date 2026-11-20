"""Test data for a paperless-ngx 2.20.15 database (test values only, deterministic).

Runs with the unchanged upstream code v2.20.15 (models of 2.20.15) via
`manage.py shell -c "exec(open(<file>).read())"`, see make_v220_fixture.sh. Covers every
data migration that paperless-ngx 3.x runs when switching from 2.20:

- documents.0003  storage_type dropped (all unencrypted)
- documents.0007  permissions for share bundles to everyone with `add_document` (direct and via group)
- documents.0014  show_on_dashboard/show_in_sidebar → UiSettings.settings.saved_views (with and without
                  existing settings, settings = null, user without row)
- documents.0016  MD5 → SHA-256 from the files (original/archive, file name empty, file missing)
- documents.0017  `note:` / `custom_field:` in full-text rules (also `denote:` and `notes.note:`)
- documents.0018  rule types 0 → 48, 19 → 49
- documents.0019  PaperlessTask is emptied (old tasks)
- documents.0020  django_celery_results tables dropped
- paperless.0007  values > 32767 are capped
- paperless.0008  OCR mode `skip_noarchive` → `auto` + archive_file_generation `never`
- paperless_mail.0002 maximum_age > 32767 capped
- documents.1075  (only 2.19/2.20.0–2.20.4) WorkflowAction.order = id
"""

import hashlib
import os
from datetime import date
from datetime import datetime
from datetime import timezone as dt_tz
from pathlib import Path

from django.conf import settings
from django.contrib.auth.models import Group
from django.contrib.auth.models import Permission
from django.contrib.auth.models import User
from django.db import connection
from django.db import transaction

from documents.models import Correspondent
from documents.models import CustomField
from documents.models import CustomFieldInstance
from documents.models import Document
from documents.models import DocumentType
from documents.models import Note
from documents.models import PaperlessTask
from documents.models import SavedView
from documents.models import SavedViewFilterRule
from documents.models import ShareLink
from documents.models import StoragePath
from documents.models import Tag
from documents.models import UiSettings
from documents.models import Workflow
from documents.models import WorkflowAction
from documents.models import WorkflowTrigger
from paperless.models import ApplicationConfiguration
from paperless_mail.models import MailAccount
from paperless_mail.models import MailRule

T0 = datetime(2025, 3, 3, 9, 0, 0, tzinfo=dt_tz.utc)
MEDIA = Path(settings.MEDIA_ROOT)
ORIG = MEDIA / "documents" / "originals"
ARCH = MEDIA / "documents" / "archive"
THUMB = MEDIA / "documents" / "thumbnails"
for d in (ORIG, ARCH, THUMB):
    d.mkdir(parents=True, exist_ok=True)


def write(path: Path, data: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return hashlib.md5(data).hexdigest()


def pdf(text: str) -> bytes:
    body = f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode()
    return (
        b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n"
        b"2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n"
        b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R>>endobj\n"
        + f"4 0 obj<</Length {len(body)}>>stream\n".encode()
        + body
        + b"\nendstream endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n"
    )


with transaction.atomic():
    # ---- Users, groups, permissions -------------------------------------------------------------
    add_doc = Permission.objects.get(codename="add_document")
    view_doc = Permission.objects.get(codename="view_document")
    admin = User.objects.create_superuser("admin", "admin@example.invalid", "admin")
    alice = User.objects.create_user("alice", "alice@example.invalid", "alice")
    bob = User.objects.create_user("bob", "bob@example.invalid", "bob")
    carol = User.objects.create_user("carol", "", "carol")
    alice.user_permissions.add(add_doc, view_doc)
    editors = Group.objects.create(name="editors")
    editors.permissions.add(add_doc, view_doc)
    viewers = Group.objects.create(name="viewers")
    viewers.permissions.add(view_doc)
    bob.groups.add(editors)
    carol.groups.add(viewers)
    for u in (admin, alice, bob, carol):
        u.date_joined = T0
        u.save()

    # ---- Master data -------------------------------------------------------------------------------
    inbox = Tag.objects.create(name="Inbox", is_inbox_tag=True, color="#a6cee3", owner=admin)
    finance = Tag.objects.create(name="Finanzen", color="#1f78b4", owner=admin)
    invoices = Tag.objects.create(name="Rechnungen", color="#b2df8a", owner=admin, tn_parent=finance)
    unpaid = Tag.objects.create(name="Offen", color="#33a02c", owner=alice, tn_parent=invoices)
    corr = Correspondent.objects.create(name="Stadtwerke Zürich", owner=admin)
    corr2 = Correspondent.objects.create(name="Krankenkasse", owner=alice)
    dtype = DocumentType.objects.create(name="Rechnung", owner=admin)
    spath = StoragePath.objects.create(name="Nach Jahr", path="{{ created_year }}/{{ title }}", owner=admin)

    # ---- Documents with files (MD5 as in 2.x) ----------------------------------------------
    specs = [
        # id, title, original (name, content), archive, mime, owner
        (1, "Stromrechnung März", ("0000001.pdf", pdf("Strom 1")), ("0000001.pdf", pdf("Strom 1 archiv")), "application/pdf", admin),
        (2, "Scan Quittung", ("0000002.png", b"\x89PNG\r\n\x1a\nfake-png-2"), ("0000002.pdf", pdf("Quittung")), "image/png", alice),
        (3, "Notiz.txt", ("0000003.txt", "Grüße aus Zürich – ünïcödé ✓\n".encode()), None, "text/plain", bob),
        (4, "Ohne Dateiname", (None, pdf("Doc 4 ohne filename")), None, "application/pdf", None),
        (5, "Datei fehlt", ("0000005.pdf", None), None, "application/pdf", admin),
        (6, "Im Papierkorb", ("0000006.pdf", pdf("Papierkorb")), ("0000006.pdf", pdf("Papierkorb archiv")), "application/pdf", carol),
    ]
    docs = []
    for i, title, (oname, odata), arch, mime, owner in specs:
        fname = oname if oname else f"{i:07}.pdf"
        if odata is not None:
            ck = write(ORIG / fname, odata)
        else:
            ck = hashlib.md5(b"verloren").hexdigest()  # File missing: checksum stays MD5
        acks = None
        aname = None
        if arch:
            aname = arch[0]
            acks = write(ARCH / aname, arch[1])
        write(THUMB / f"{i:07}.webp", b"RIFF\x00\x00\x00\x00WEBPfake")
        docs.append(
            Document(
                id=i,
                title=title,
                content=f"Inhalt {i}: {title}",
                mime_type=mime,
                checksum=ck,
                archive_checksum=acks,
                filename=oname,
                archive_filename=aname,
                storage_type="unencrypted",
                created=date(2025, 1, i),
                modified=T0,
                added=T0,
                owner=owner,
                correspondent=corr if i % 2 else corr2,
                document_type=dtype if i < 4 else None,
                storage_path=spath if i == 1 else None,
                original_filename=f"original-{i}{Path(fname).suffix}",
                archive_serial_number=1000 + i if i < 5 else None,
                page_count=1 if mime == "application/pdf" else None,
                deleted_at=T0 if i == 6 else None,
            ),
        )
    Document.objects.bulk_create(docs)
    for d in Document.global_objects.all():
        d.tags.add(inbox if d.id % 2 else invoices)
        if d.id == 1:
            d.tags.add(unpaid)
    if connection.vendor == "postgresql":
        with connection.cursor() as c:
            c.execute("SELECT setval(pg_get_serial_sequence('documents_document','id'), 10)")

    Note.objects.create(document_id=1, note="Bezahlt am 5.3.", user=admin, created=T0)
    Note.objects.create(document_id=2, note="Rückfrage", user=alice, created=T0)
    cf_text = CustomField.objects.create(name="Kundennummer", data_type="string", created=T0)
    cf_money = CustomField.objects.create(name="Betrag", data_type="monetary", created=T0, extra_data={"default_currency": "CHF"})
    CustomFieldInstance.objects.create(document_id=1, field=cf_text, value_text="K-4711", created=T0)
    CustomFieldInstance.objects.create(document_id=1, field=cf_money, value_monetary="CHF120.50", created=T0)
    ShareLink.objects.create(document_id=1, slug="v220share" + "x" * 41, owner=admin, created=T0)

    # ---- Saved views + rules (documents.0014/0017/0018) -------------------------
    v1 = SavedView.objects.create(name="B Dashboard", show_on_dashboard=True, show_in_sidebar=False, sort_field="created", owner=admin)
    v2 = SavedView.objects.create(name="A Beides", show_on_dashboard=True, show_in_sidebar=True, sort_field="title", owner=admin)
    v3 = SavedView.objects.create(name="Seitenleiste", show_on_dashboard=False, show_in_sidebar=True, sort_field="added", owner=alice)
    SavedView.objects.create(name="Ohne Besitzer", show_on_dashboard=True, show_in_sidebar=True, sort_field="created", owner=None)
    SavedView.objects.create(name="Bob versteckt", show_on_dashboard=False, show_in_sidebar=False, sort_field="created", owner=bob)
    for view, rule, value in [
        (v1, 0, "rechnung"),
        (v1, 19, "steuer 2025"),
        (v1, 20, "note:wichtig custom_field:K-4711 denote:x notes.note:schon (note:a)"),
        (v2, 20, "-note:b +custom_field:c"),
        (v2, 6, str(inbox.id)),
        (v3, 20, None),
        (v3, 19, "Zürich"),
    ]:
        SavedViewFilterRule.objects.create(saved_view=view, rule_type=rule, value=value)
    UiSettings.objects.create(user=admin, settings={"dark_mode": {"enabled": "true"}, "language": "de-de"})
    UiSettings.objects.create(user=bob, settings={"saved_views": {"dashboard_views_visible_ids": [v1.id]}})
    UiSettings.objects.create(user=carol, settings=None)

    # ---- Settings (paperless.0007/0008) -------------------------------------------------
    cfg = ApplicationConfiguration.objects.first() or ApplicationConfiguration()
    cfg.mode = "skip_noarchive"
    cfg.skip_archive_file = None
    cfg.barcode_dpi = 40000
    cfg.pages = 3
    cfg.save()

    # ---- Mail (paperless_mail.0002) ----------------------------------------------------------
    acc = MailAccount.objects.create(
        name="Testkonto",
        imap_server="127.0.0.1",
        imap_port=1143,
        imap_security=1,
        username="test",
        password="nur-testwert",
        owner=admin,
    )
    MailRule.objects.create(name="Alles", account=acc, order=5, maximum_age=40000, owner=admin)
    MailRule.objects.create(name="Rechnungen", account=acc, order=1, maximum_age=30, filter_subject="Rechnung", owner=admin)

    # ---- Workflows (documents.1075: order) -----------------------------------------------------
    trig = WorkflowTrigger.objects.create(type=1, sources="1,2,3", filter_filename="*.pdf")
    a1 = WorkflowAction.objects.create(type=1, assign_title="{{ correspondent }}")
    a2 = WorkflowAction.objects.create(type=1, assign_owner=alice)
    wf = Workflow.objects.create(name="Eingang", order=0, enabled=True)
    wf.triggers.add(trig)
    wf.actions.add(a1, a2)

    # ---- Tasks (documents.0019 empties the table) + Celery result (0020) -------------------
    PaperlessTask.objects.create(task_id="11111111-1111-1111-1111-111111111111", task_file_name="a.pdf", task_name="consume_file", status="SUCCESS", date_created=T0, owner=admin)
    PaperlessTask.objects.create(task_id="22222222-2222-2222-2222-222222222222", task_name="train_classifier", status="FAILURE", date_created=T0)
    with connection.cursor() as c:
        c.execute("INSERT INTO django_celery_results_taskresult (task_id, status, result, date_done) VALUES (%s, %s, %s, %s)", ["11111111-1111-1111-1111-111111111111", "SUCCESS", '"ok"', T0])

print(
    "seed_v220:",
    Document.global_objects.count(),
    "Dokumente,",
    User.objects.count(),
    "Benutzer,",
    SavedView.objects.count(),
    "Ansichten,",
    os.environ.get("PAPERLESS_DBNAME", "sqlite"),
)
