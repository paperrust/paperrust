"""Deterministic test data seed of the paperless-rs reference (paperless-ngx v3.4.2).

Runs against a freshly migrated, empty database (see seed.sh). Creates everything directly via the
Django ORM - without consumer/OCR pipeline. Files (original, archive, thumbnail) are
generated and their SHA-256 checksums entered correctly.

Determinism: fixed random generator (seed 4242), fixed timestamps (base 2025-01-06 08:00 UTC),
fixed password salts, fixed API tokens, fixed task UUIDs. Two runs yield the same data
(except sequences in helper tables like django_session, which stay empty).

Call (from reference/, environment from env.sh):  "$PY" -I seed.py   (via seed.sh)
"""

from __future__ import annotations

import datetime as dt
import hashlib
import io
import os
import random
import sys
import uuid
from pathlib import Path

# -I (isolated) ignores PYTHONPATH -> set paths explicitly
for p in reversed(os.environ.get("PYTHONPATH", "").split(os.pathsep)):
    if p and p not in sys.path:
        sys.path.insert(0, p)

import django  # noqa: E402

django.setup()

from auditlog.context import set_actor  # noqa: E402
from auditlog.models import LogEntry  # noqa: E402
from django.conf import settings  # noqa: E402
from django.contrib.auth.hashers import make_password  # noqa: E402
from django.contrib.auth.models import Group  # noqa: E402
from django.contrib.auth.models import Permission  # noqa: E402
from django.contrib.auth.models import User  # noqa: E402
from django.db import connection  # noqa: E402
from django.db import transaction  # noqa: E402
from fpdf import FPDF  # noqa: E402
from guardian.shortcuts import assign_perm  # noqa: E402
from PIL import Image  # noqa: E402
from PIL import ImageDraw  # noqa: E402
from rest_framework.authtoken.models import Token  # noqa: E402

from documents.file_handling import generate_filename  # noqa: E402
from documents.models import Correspondent  # noqa: E402
from documents.models import CustomField  # noqa: E402
from documents.models import CustomFieldInstance  # noqa: E402
from documents.models import Document  # noqa: E402
from documents.models import DocumentType  # noqa: E402
from documents.models import Note  # noqa: E402
from documents.models import PaperlessTask  # noqa: E402
from documents.models import SavedView  # noqa: E402
from documents.models import SavedViewFilterRule  # noqa: E402
from documents.models import ShareLink  # noqa: E402
from documents.models import StoragePath  # noqa: E402
from documents.models import Tag  # noqa: E402
from documents.models import UiSettings  # noqa: E402
from documents.models import Workflow  # noqa: E402
from documents.models import WorkflowAction  # noqa: E402
from documents.models import WorkflowActionEmail  # noqa: E402
from documents.models import WorkflowActionWebhook  # noqa: E402
from documents.models import WorkflowTrigger  # noqa: E402
from paperless_mail.models import MailAccount  # noqa: E402
from paperless_mail.models import MailRule  # noqa: E402
from paperless_mail.models import ProcessedMail  # noqa: E402

UTC = dt.UTC
BASE = dt.datetime(2025, 1, 6, 8, 0, 0, tzinfo=UTC)
RNG = random.Random(4242)
N_DOCS = 200

# Fixed credentials (local only!) – mirrored in reference/README.md
USERS = {
    # username: (password, token, flags)
    "admin": ("admin", "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001", "superuser"),
    "alice": ("alice", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002", "editor"),
    "bob": ("bob", "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003", "viewer"),
    "carol": ("carol", "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004", "none"),
    "dave": ("dave", "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005", "inactive"),
}


def ts(minutes: float) -> dt.datetime:
    return BASE + dt.timedelta(minutes=minutes)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def tuuid(n: int) -> str:
    return str(uuid.UUID(int=(0x5EED << 112) | n))


# --------------------------------------------------------------------------- Files
def make_pdf(title: str, lines: list[str], pages: int, *, archive: bool) -> bytes:
    pdf = FPDF(format="A4")
    pdf.set_creation_date(dt.datetime(2025, 1, 1, tzinfo=UTC))
    pdf.set_producer("paperless-rs reference seed")
    pdf.set_creator("paperless-rs reference seed")
    pdf.set_title(title)
    pdf.set_compression(True)
    per_page = max(1, len(lines) // pages)
    for p in range(pages):
        pdf.add_page()
        pdf.set_font("Helvetica", "B", 16)
        pdf.cell(0, 10, title[:70], new_x="LMARGIN", new_y="NEXT")
        pdf.set_font("Helvetica", "", 9)
        pdf.cell(
            0,
            6,
            f"{'ARCHIVE (PDF/A stand-in)' if archive else 'ORIGINAL'} - page {p + 1}/{pages}",
            new_x="LMARGIN",
            new_y="NEXT",
        )
        pdf.set_font("Helvetica", "", 11)
        chunk = lines[p * per_page : (p + 1) * per_page] if p < pages - 1 else lines[p * per_page :]
        for line in chunk:
            pdf.multi_cell(0, 6, line, new_x="LMARGIN", new_y="NEXT")
    return bytes(pdf.output())


def make_png(title: str, lines: list[str]) -> bytes:
    img = Image.new("RGB", (620, 877), "white")
    d = ImageDraw.Draw(img)
    d.text((30, 30), title[:60], fill="black")
    for i, line in enumerate(lines[:30]):
        d.text((30, 70 + i * 18), line[:90], fill=(40, 40, 40))
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=False)
    return buf.getvalue()


def make_thumb(title: str, color: str, idx: int) -> bytes:
    img = Image.new("RGB", (400, 566), "white")
    d = ImageDraw.Draw(img)
    d.rectangle((0, 0, 399, 60), fill=color)
    d.text((14, 22), f"#{idx:04d}", fill="white")
    d.text((14, 80), title[:48], fill="black")
    for i in range(14):
        w = 120 + (idx * 37 + i * 53) % 240
        d.rectangle((14, 120 + i * 26, 14 + w, 128 + i * 26), fill=(190, 190, 190))
    buf = io.BytesIO()
    img.save(buf, format="WEBP", quality=80, method=4)
    return buf.getvalue()


def write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


# --------------------------------------------------------------------------- Texts
WORDS_EN = (
    "invoice payment amount due total tax insurance policy contract agreement tenant landlord "
    "rent deposit account statement balance transfer salary employer pension health doctor "
    "prescription pharmacy receipt warranty repair vehicle registration electricity water gas "
    "internet phone subscription renewal cancellation notice reminder overdue refund credit "
    "delivery order shipment customs passport certificate school tuition donation membership "
    "mortgage loan interest property survey quarterly annual report meeting minutes budget"
).split()
WORDS_DE = (
    "Rechnung Zahlung Betrag fällig Gesamtsumme Steuer Versicherung Police Vertrag Vereinbarung "
    "Mieter Vermieter Miete Kaution Kontoauszug Saldo Überweisung Lohn Arbeitgeber Rente "
    "Krankenkasse Arzt Rezept Apotheke Quittung Garantie Reparatur Fahrzeug Strom Wasser Gas "
    "Kündigung Mahnung Gutschrift Lieferung Bestellung Zoll Reisepass Zeugnis Mitgliedschaft "
    "Hypothek Darlehen Zinsen Grundstück Jahresbericht Sitzung Protokoll Budget Gemeinde Zürich"
).split()


def sentence(words: list[str], n: int) -> str:
    s = " ".join(RNG.choice(words) for _ in range(n))
    return s[0].upper() + s[1:] + "."


def content_for(i: int, title: str, corr: str | None) -> list[str]:
    words = WORDS_DE if i % 3 == 0 else WORDS_EN
    lines = [f"{title}", f"Reference REF-{i:05d}", f"From: {corr or 'unknown sender'}"]
    for _ in range(4 + i % 9):
        lines.append(sentence(words, 6 + RNG.randint(0, 10)))
    if i % 10 == 0:
        lines.append("Special marker: quarterly-tax-statement")
    if i % 25 == 0:
        lines.append("Contains umlauts: äöü ÄÖÜ ß and accents: café naïve résumé")
    return lines


# --------------------------------------------------------------------------- Seed
def seed() -> dict:
    out: dict = {}

    # ---- Groups + permissions
    def perms(*codenames: str) -> list[Permission]:
        res = list(Permission.objects.filter(codename__in=codenames))
        missing = set(codenames) - {p.codename for p in res}
        if missing:
            raise SystemExit(f"Unbekannte Rechte: {sorted(missing)}")
        return res

    models_rw = [
        "document",
        "tag",
        "correspondent",
        "documenttype",
        "storagepath",
        "savedview",
        "note",
        "customfield",
        "sharelink",
        "paperlesstask",
        "uisettings",
    ]
    editors = Group.objects.create(name="editors")
    editors.permissions.set(
        perms(
            *[f"{a}_{m}" for m in models_rw for a in ("view", "add", "change")],
            "view_workflow",
            "view_mailaccount",
            "view_mailrule",
        ),
    )
    viewers = Group.objects.create(name="viewers")
    viewers.permissions.set(
        perms(
            *[f"view_{m}" for m in ("document", "tag", "correspondent", "documenttype", "storagepath", "note", "customfield", "savedview")],
            "add_savedview",
            "change_savedview",
            "add_uisettings",
            "change_uisettings",
            "view_uisettings",
        ),
    )
    Group.objects.create(name="auditors").permissions.set(perms("view_logentry", "view_document"))

    # ---- Users
    users: dict[str, User] = {}
    for n, (uname, (pw, _tok, flag)) in enumerate(USERS.items()):
        u = User(
            username=uname,
            email=f"{uname}@example.invalid",
            first_name=uname.capitalize(),
            last_name="Reference",
            is_superuser=flag == "superuser",
            is_staff=flag == "superuser",
            is_active=flag != "inactive",
            date_joined=ts(n),
            password=make_password(pw, salt=f"paperlessrsref{uname}"),
        )
        u.save()
        users[uname] = u
    users["alice"].groups.add(editors)
    users["bob"].groups.add(viewers)
    users["dave"].groups.add(editors)
    # alice may additionally delete documents (user permission directly, not via group)
    users["alice"].user_permissions.add(*perms("delete_document", "delete_note"))
    # bob (role user2): viewers group + direct add/change permissions, owns own objects
    users["bob"].user_permissions.add(
        *perms(*[f"{a}_{m}" for m in ("document", "tag", "correspondent", "documenttype", "note") for a in ("add", "change")])
    )
    for uname, (_pw, tok, _f) in USERS.items():
        Token.objects.create(key=tok, user=users[uname])
    Token.objects.update(created=BASE)
    admin, alice, bob, carol = users["admin"], users["alice"], users["bob"], users["carol"]

    UiSettings.objects.create(
        user=admin,
        settings={
            "dark_mode": {"use_system": True, "enabled": "false", "thumb_inverted": "true"},
            "documentListSize": 50,
            "saved_views": {"warn_on_unsaved_change": True},
            "language": "en-US",
        },
    )
    UiSettings.objects.create(user=alice, settings={"language": "de-DE", "documentListSize": 25})

    # ---- Correspondents / document types / storage paths
    corr_names = [
        ("ACME Corp", 1, "acme", alice),
        ("Stadtwerke Zürich", 3, "Stadtwerke Zürich", admin),
        ("Krankenkasse Helvetia", 2, "krankenkasse helvetia", admin),
        ("Swisscom", 1, "swisscom mobile", None),
        ("Landlord Müller", 0, "", alice),
        ("Tax Office", 4, r"tax\s+office|steueramt", admin),
        ("Insurance AG", 5, "insurance", None),
        ("Bank Alpha", 6, "", admin),
        ("Doctor Weber", 1, "weber praxis", bob),
        ("Online Shop", 2, "order shipment", None),
        ("Employer GmbH", 3, "Employer GmbH", admin),
        ("ÄÖÜ Sonderzeichen & Co.", 0, "", None),
    ]
    corrs = []
    for name, algo, match, owner in corr_names:
        corrs.append(
            Correspondent.objects.create(
                name=name, matching_algorithm=algo, match=match, is_insensitive=algo != 3, owner=owner
            )
        )
    dtypes = []
    for name, algo, match, owner in [
        ("Invoice", 1, "invoice rechnung", None),
        ("Contract", 2, "contract agreement", admin),
        ("Receipt", 1, "receipt quittung", None),
        ("Bank Statement", 3, "account statement", admin),
        ("Letter", 0, "", alice),
        ("Certificate", 4, "certificate|zeugnis", None),
        ("Tax Document", 6, "", admin),
        ("Medical", 5, "doctor", bob),
    ]:
        dtypes.append(DocumentType.objects.create(name=name, matching_algorithm=algo, match=match, owner=owner))
    spaths = [
        StoragePath.objects.create(
            name="By correspondent and year",
            path="{{ correspondent }}/{{ created_year }}/{{ title }}",
            matching_algorithm=0,
            owner=admin,
        ),
        StoragePath.objects.create(
            name="Taxes",
            path="taxes/{{ created_year }}/{{ created_month }}-{{ title }}",
            matching_algorithm=1,
            match="tax steuer",
            owner=None,
        ),
        StoragePath.objects.create(
            name="Alice private",
            path="alice/{{ document_type }}/{{ title }}",
            matching_algorithm=0,
            owner=alice,
        ),
        StoragePath.objects.create(
            name="Flat ASN",
            path="asn/{{ asn }}-{{ title }}",
            matching_algorithm=6,
            owner=None,
        ),
    ]

    # ---- Tags (nested, inbox, colours)
    tag_specs = [
        # name, color, parent, inbox, algo, match, owner
        ("Inbox", "#ff0000", None, True, 0, "", None),
        ("Finance", "#1f78b4", None, False, 0, "", admin),
        ("Taxes", "#33a02c", "Finance", False, 1, "tax steuer", admin),
        ("Taxes 2024", "#b2df8a", "Taxes", False, 0, "", admin),
        ("Taxes 2025", "#b2df8a", "Taxes", False, 0, "", admin),
        ("Invoices", "#fb9a99", "Finance", False, 1, "invoice rechnung", None),
        ("Paid", "#e31a1c", "Invoices", False, 0, "", None),
        ("Unpaid", "#fdbf6f", "Invoices", False, 0, "", None),
        ("Overdue", "#ff7f00", "Unpaid", False, 3, "overdue", None),
        ("Home", "#cab2d6", None, False, 0, "", alice),
        ("Rent", "#6a3d9a", "Home", False, 1, "rent miete", alice),
        ("Utilities", "#ffff99", "Home", False, 1, "electricity water gas strom", alice),
        ("Health", "#b15928", None, False, 5, "doctor arzt", bob),
        ("Insurance", "#a6cee3", None, False, 2, "insurance policy", None),
        ("Car", "#000000", None, False, 0, "", None),
        ("Important", "#ffffff", None, False, 0, "", admin),
        ("To Do", "#00ff00", None, False, 0, "", alice),
        ("Archive", "#808080", None, False, 6, "", None),
        ("Private", "#123456", None, False, 0, "", carol),
        ("Ümlaut-Tag ä", "#abcdef", None, False, 0, "", None),
    ]
    tags: dict[str, Tag] = {}
    for name, color, parent, inbox, algo, match, owner in tag_specs:
        tags[name] = Tag.objects.create(
            name=name,
            color=color,
            tn_parent=tags[parent] if parent else None,
            is_inbox_tag=inbox,
            matching_algorithm=algo,
            match=match,
            owner=owner,
        )

    # ---- Custom fields (all data types)
    cf = {}
    for i, (name, dtype, extra) in enumerate(
        [
            ("Text", "string", None),
            ("Website", "url", None),
            ("Due date", "date", None),
            ("Paid?", "boolean", None),
            ("Count", "integer", None),
            ("Score", "float", None),
            ("Amount", "monetary", {"default_currency": "CHF"}),
            ("Amount (no default)", "monetary", None),
            ("Related docs", "documentlink", None),
            (
                "Priority",
                "select",
                {
                    "select_options": [
                        {"id": "pr1LowAAAAAAAAAA", "label": "Low"},
                        {"id": "pr2MediumAAAAAAA", "label": "Medium"},
                        {"id": "pr3HighAAAAAAAAA", "label": "High"},
                    ]
                },
            ),
            ("Notes (long)", "longtext", None),
        ]
    ):
        cf[name] = CustomField.objects.create(name=name, data_type=dtype, extra_data=extra)
        CustomField.objects.filter(pk=cf[name].pk).update(created=ts(10 + i))

    # ---- Documents
    orig_dir, arch_dir, thumb_dir = settings.ORIGINALS_DIR, settings.ARCHIVE_DIR, settings.THUMBNAIL_DIR
    for d in (orig_dir, arch_dir, thumb_dir):
        d.mkdir(parents=True, exist_ok=True)
    tag_list = list(tags.values())
    owners_cycle = [admin, admin, alice, None, admin, alice, bob, None, admin, carol]
    docs: list[Document] = []
    used_asn = set()
    for i in range(1, N_DOCS + 1):
        corr = corrs[i % len(corrs)] if i % 7 else None
        dtype = dtypes[i % len(dtypes)] if i % 5 else None
        spath = spaths[i % len(spaths)] if i % 4 == 0 else None
        owner = owners_cycle[i % len(owners_cycle)]
        kind = "pdf"
        if i % 40 == 3:
            kind = "txt"
        elif i % 33 == 5:
            kind = "png"
        mime = {"pdf": "application/pdf", "txt": "text/plain", "png": "image/png"}[kind]
        title = f"{(dtype.name if dtype else 'Document')} {corr.name if corr else 'misc'} {i:03d}"
        if i % 17 == 0:
            title = f"Long title {i} " + "lorem ipsum " * 8
            title = title[:128]
        lines = content_for(i, title, corr.name if corr else None)
        content = "\n".join(lines)
        pages = 1 + i % 3
        created = dt.date(2019, 1, 1) + dt.timedelta(days=(i * 13) % 2500)
        asn = None
        if i % 3 != 0:
            asn = 1000 + i
            used_asn.add(asn)
        with set_actor(owner or admin):
            doc = Document.objects.create(
                title=title,
                content=content,
                mime_type=mime,
                checksum="0" * 64,  # replaced below
                correspondent=corr,
                document_type=dtype,
                storage_path=spath,
                owner=owner,
                created=created,
                archive_serial_number=asn,
                page_count=pages if kind != "txt" else None,
                original_filename=f"scan_{i:04d}.{kind}",
            )
            chosen = {tag_list[(i * 3) % len(tag_list)], tag_list[(i * 7 + 1) % len(tag_list)]}
            if i % 6 == 0:
                chosen.add(tags["Inbox"])
            if i % 11 == 0:
                chosen = set()
            if chosen:
                doc.tags.add(*sorted(chosen, key=lambda t: t.pk))
        docs.append(doc)

    # Write files, file names like the consumer (generate_filename), set checksums
    for i, doc in enumerate(docs, start=1):
        doc.refresh_from_db()
        lines = doc.content.split("\n")
        if doc.mime_type == "application/pdf":
            original = make_pdf(doc.title, lines, doc.page_count or 1, archive=False)
        elif doc.mime_type == "image/png":
            original = make_png(doc.title, lines)
        else:
            original = (doc.content + "\n").encode("utf-8")
        archive = None
        if doc.mime_type in ("application/pdf", "image/png"):
            archive = make_pdf(doc.title, lines, doc.page_count or 1, archive=True)
        fname = generate_filename(doc)
        aname = generate_filename(doc, archive_filename=True) if archive else None
        write(orig_dir / fname, original)
        if archive:
            write(arch_dir / aname, archive)
        color = doc.tags.order_by("pk").first().color if doc.tags.exists() else "#444444"
        write(thumb_dir / f"{doc.pk:07}.webp", make_thumb(doc.title, color, doc.pk))
        Document.objects.filter(pk=doc.pk).update(
            checksum=sha256(original),
            archive_checksum=sha256(archive) if archive else None,
            filename=str(fname).replace("\\", "/"),
            archive_filename=str(aname).replace("\\", "/") if aname else None,
            added=ts(60 * 24 * 30 + i * 90),
            modified=ts(60 * 24 * 30 + i * 90 + 30),
        )

    # Versions: documents 1 and 2 get two versions each
    for root in docs[:2]:
        root.refresh_from_db()
        for vi in (1, 2):
            lines = root.content.split("\n") + [f"Version {vi} addendum."]
            original = make_pdf(root.title, lines, root.page_count or 1, archive=False)
            archive = make_pdf(root.title, lines, root.page_count or 1, archive=True)
            v = Document.objects.create(
                root_document=root,
                version_index=vi,
                version_label=f"v{vi}" if vi == 1 else None,
                title=root.title,
                content="\n".join(lines),
                mime_type="application/pdf",
                checksum=sha256(original),
                archive_checksum=sha256(archive),
                page_count=root.page_count,
                owner=root.owner,
                created=root.created,
                original_filename=f"scan_{root.pk:04d}_v{vi}.pdf",
            )
            fname = generate_filename(v)
            aname = generate_filename(v, archive_filename=True)
            write(orig_dir / fname, original)
            write(arch_dir / aname, archive)
            write(thumb_dir / f"{v.pk:07}.webp", make_thumb(root.title, "#336699", v.pk))
            Document.objects.filter(pk=v.pk).update(
                filename=str(fname).replace("\\", "/"),
                archive_filename=str(aname).replace("\\", "/"),
                added=ts(60 * 24 * 300 + root.pk * 10 + vi),
                modified=ts(60 * 24 * 300 + root.pk * 10 + vi),
            )

    # ---- Custom field values
    sel_ids = [o["id"] for o in cf["Priority"].extra_data["select_options"]]
    for i, doc in enumerate(docs, start=1):
        vals = []
        if i % 2 == 0:
            vals.append(("Text", "value_text", f"text value {i}"))
        if i % 9 == 0:
            vals.append(("Website", "value_url", f"https://example.invalid/doc/{i}"))
        if i % 3 == 0:
            vals.append(("Due date", "value_date", dt.date(2025, 1, 1) + dt.timedelta(days=i)))
        if i % 4 == 0:
            vals.append(("Paid?", "value_bool", i % 8 == 0))
        if i % 5 == 0:
            vals.append(("Count", "value_int", i * 3 - 100))
        if i % 6 == 0:
            vals.append(("Score", "value_float", round(i / 7, 4)))
        if i % 7 == 0:
            vals.append(("Amount", "value_monetary", f"CHF{i * 12.5:.2f}"))
        if i % 13 == 0:
            vals.append(("Amount (no default)", "value_monetary", f"{i * 3.1:.2f}" if i % 2 else f"EUR{i:.2f}"))
        if i % 8 == 0:
            vals.append(("Priority", "value_select", sel_ids[i % 3]))
        if i % 15 == 0:
            vals.append(("Notes (long)", "value_long_text", "Long text line\n" * (i // 15)))
        if i % 10 == 0:
            vals.append(("Text", "value_text", None)) if i % 2 else None
        for name, col, val in vals:
            with set_actor(admin):
                CustomFieldInstance.objects.create(document=doc, field=cf[name], **{col: val})
    # documentlink: symmetric like the serializer (reflect_doclinks)
    links = {}
    for i in range(10, 60, 10):
        a, b, c = docs[i - 1], docs[i], docs[i + 1]
        links.setdefault(a.pk, set()).update({b.pk, c.pk})
        links.setdefault(b.pk, set()).add(a.pk)
        links.setdefault(c.pk, set()).add(a.pk)
    for doc_pk, targets in sorted(links.items()):
        CustomFieldInstance.objects.create(
            document_id=doc_pk, field=cf["Related docs"], value_document_ids=sorted(targets)
        )
    CustomFieldInstance.objects.update(created=BASE + dt.timedelta(days=40))

    # ---- guardian object permissions
    for doc in docs:
        i = doc.pk
        if doc.owner == admin and i % 4 == 0:
            assign_perm("view_document", alice, doc)
        if doc.owner == admin and i % 8 == 0:
            assign_perm("change_document", alice, doc)
        if doc.owner == admin and i % 5 == 0:
            assign_perm("view_document", viewers, doc)
        if doc.owner == alice and i % 3 == 0:
            assign_perm("view_document", bob, doc)
            assign_perm("change_document", editors, doc)
        # bob documents: most NOT shared with alice, some shared via object permission/group
        if doc.owner == bob and i % 50 == 6:
            assign_perm("view_document", alice, doc)
        if doc.owner == bob and i % 50 == 16:
            assign_perm("view_document", editors, doc)
            assign_perm("change_document", editors, doc)
        if doc.owner == carol:
            assign_perm("view_document", carol, doc)
    assign_perm("view_tag", bob, tags["Home"])
    assign_perm("change_tag", alice, tags["Finance"])
    assign_perm("view_correspondent", viewers, corrs[1])
    assign_perm("view_storagepath", alice, spaths[0])
    assign_perm("view_documenttype", bob, dtypes[1])
    # bob owns: tag "Health" (shared with alice), document type "Medical" and correspondent
    # "Doctor Weber" (both NOT shared), storage path "Bob private" (shared via group editors)
    assign_perm("view_tag", alice, tags["Health"])
    bob_sp = StoragePath.objects.create(name="Bob private", path="bob/{{ title }}", matching_algorithm=0, owner=bob)
    assign_perm("view_storagepath", editors, bob_sp)

    # ---- Trash (soft delete)
    for doc in docs[-5:]:
        with set_actor(admin):
            doc.delete()
    Document.deleted_objects.update(deleted_at=BASE + dt.timedelta(days=400))

    # ---- Notes
    note_users = [admin, alice, bob, admin, alice]
    for n in range(40):
        doc = docs[(n * 5) % (N_DOCS - 5)]
        with set_actor(note_users[n % 5]):
            note = Note.objects.create(
                document=doc, user=note_users[n % 5], note=f"Note {n}: {sentence(WORDS_EN, 8)}"
            )
        Note.objects.filter(pk=note.pk).update(created=ts(60 * 24 * 200 + n * 60))
    with set_actor(admin):
        Note.objects.order_by("pk").last().delete()

    # ---- Saved Views
    sv1 = SavedView.objects.create(
        name="Inbox", owner=admin, sort_field="created", sort_reverse=True, icon="inbox"
    )
    SavedViewFilterRule.objects.create(saved_view=sv1, rule_type=6, value=str(tags["Inbox"].pk))
    sv2 = SavedView.objects.create(
        name="Unpaid invoices",
        owner=admin,
        sort_field="title",
        page_size=25,
        display_mode="table",
        display_fields=["title", "created", "tag", "correspondent", f"custom_field_{cf['Amount'].pk}"],
        icon="cash",
    )
    SavedViewFilterRule.objects.create(saved_view=sv2, rule_type=6, value=str(tags["Unpaid"].pk))
    SavedViewFilterRule.objects.create(saved_view=sv2, rule_type=4, value=str(dtypes[0].pk))
    sv3 = SavedView.objects.create(
        name="Alice home", owner=alice, sort_field="added", display_mode="smallCards", icon="house"
    )
    SavedViewFilterRule.objects.create(saved_view=sv3, rule_type=22, value=str(tags["Home"].pk))
    SavedViewFilterRule.objects.create(saved_view=sv3, rule_type=19, value="rent")
    sv4 = SavedView.objects.create(name="Custom field query", owner=admin, sort_field=f"custom_field_{cf['Count'].pk}")
    SavedViewFilterRule.objects.create(
        saved_view=sv4, rule_type=42, value=f'["{cf["Count"].pk}", "gt", 10]'
    )
    sv5 = SavedView.objects.create(name="Shared view (no owner)", owner=None, sort_field="created")
    SavedViewFilterRule.objects.create(saved_view=sv5, rule_type=9, value="2022-01-01")
    assign_perm("view_savedview", bob, sv1)
    UiSettings.objects.filter(user=admin).update(
        settings={
            **UiSettings.objects.get(user=admin).settings,
            "saved_views": {"warn_on_unsaved_change": True},
            "dashboard_views_visible_ids": [sv1.pk, sv2.pk],
            "sidebar_views_visible_ids": [sv1.pk, sv2.pk, sv4.pk],
        }
    )

    # ---- Share Links
    for n, (doc, ver, exp, owner) in enumerate(
        [
            (docs[0], "archive", None, admin),
            (docs[1], "original", None, admin),
            (docs[2], "archive", BASE - dt.timedelta(days=1), alice),  # expired
            (docs[3], "archive", dt.datetime(2099, 1, 1, tzinfo=UTC), alice),
            (docs[4], "original", None, None),
        ]
    ):
        ShareLink.objects.create(
            slug=f"refshare{n:02d}" + "x" * 40, document=doc, file_version=ver, expiration=exp, owner=owner
        )
    ShareLink.objects.update(created=ts(60 * 24 * 250))
    ShareLink.objects.filter(slug__startswith="refshare04").delete()

    # ---- Workflows
    email = WorkflowActionEmail.objects.create(
        subject="Document {doc_title} added", body="Body for {doc_url}", to="ops@example.invalid", include_document=False
    )
    hook = WorkflowActionWebhook.objects.create(
        url="http://127.0.0.1:9/hook", use_params=False, as_json=True, body='{"id": "{doc_id}"}',
        headers={"X-Test": "1"}, include_document=False,
    )
    t1 = WorkflowTrigger.objects.create(type=1, sources="1,2,3", filter_filename="*invoice*", filter_path="*/scans/*")
    t2 = WorkflowTrigger.objects.create(type=2, matching_algorithm=1, match="invoice rechnung")
    t2.filter_has_tags.set([tags["Inbox"]])
    t3 = WorkflowTrigger.objects.create(type=3, filter_has_document_type=dtypes[0])
    t3.filter_has_not_tags.set([tags["Paid"]])
    t4 = WorkflowTrigger.objects.create(
        type=4, schedule_offset_days=30, schedule_is_recurring=True, schedule_recurring_interval_days=7,
        schedule_date_field="custom_field", schedule_date_custom_field=cf["Due date"],
    )
    a1 = WorkflowAction.objects.create(type=1, order=0, assign_title="{correspondent} - {document_type}",
                                       assign_correspondent=corrs[0], assign_document_type=dtypes[0], assign_owner=alice)
    a1.assign_tags.set([tags["Invoices"], tags["Unpaid"]])
    a1.assign_view_users.set([bob])
    a1.assign_change_groups.set([editors])
    a1.assign_custom_fields.set([cf["Amount"], cf["Due date"]])
    a1.assign_custom_fields_values = {str(cf["Amount"].pk): "CHF0.00"}
    a1.save()
    a2 = WorkflowAction.objects.create(type=2, order=1, remove_all_owners=False)
    a2.remove_tags.set([tags["Inbox"]])
    a3 = WorkflowAction.objects.create(type=3, order=2, email=email)
    a4 = WorkflowAction.objects.create(type=4, order=3, webhook=hook)
    a5 = WorkflowAction.objects.create(type=2, order=0, remove_all_tags=True, remove_all_permissions=True)
    w1 = Workflow.objects.create(name="Invoice intake", order=0, enabled=True)
    w1.triggers.set([t1, t2])
    w1.actions.set([a1, a2])
    w2 = Workflow.objects.create(name="Notify on update", order=1, enabled=True)
    w2.triggers.set([t3])
    w2.actions.set([a3, a4])
    w3 = Workflow.objects.create(name="Scheduled cleanup (disabled)", order=2, enabled=False)
    w3.triggers.set([t4])
    w3.actions.set([a5])

    # ---- Mail (fake server, not reachable)
    m1 = MailAccount.objects.create(
        name="Fake IMAP", imap_server="imap.example.invalid", imap_port=993, imap_security=2,
        username="inbox@example.invalid", password="fake-mail-password", owner=admin,
    )
    m2 = MailAccount.objects.create(
        name="Fake STARTTLS (alice)", imap_server="127.0.0.1", imap_port=1143, imap_security=3,
        username="alice", password="fake-alice-password", character_set="UTF-8", owner=alice,
    )
    MailAccount.objects.create(
        name="Fake Gmail OAuth", imap_server="imap.gmail.invalid", imap_port=993, imap_security=2,
        username="oauth@example.invalid", password="fake-oauth-access-token", is_token=True,
        account_type=2, refresh_token="fake-refresh-token", expiration=dt.datetime(2030, 1, 1, tzinfo=UTC), owner=admin,
    )
    r1 = MailRule.objects.create(
        name="Invoices from ACME", account=m1, order=0, folder="INBOX", filter_from="billing@acme.invalid",
        filter_subject="Invoice", maximum_age=30, action=3, assign_title_from=1, assign_correspondent_from=4,
        assign_correspondent=corrs[0], assign_document_type=dtypes[0], owner=admin,
    )
    r1.assign_tags.set([tags["Invoices"], tags["Inbox"]])
    MailRule.objects.create(
        name="Move processed", account=m1, order=1, folder="INBOX/Scans", action=2, action_parameter="Archive",
        attachment_type=2, consumption_scope=2, pdf_layout=1, filter_attachment_filename_include="*.pdf", owner=admin,
    )
    MailRule.objects.create(
        name="Alice tag rule", account=m2, order=0, action=5, action_parameter="paperless",
        enabled=False, stop_processing=True, owner=alice,
    )
    for n in range(4):
        ProcessedMail.objects.create(
            rule=r1, folder="INBOX", uid=str(100 + n), uid_validity="1", subject=f"Invoice {n}",
            received=ts(60 * 24 * 100 + n), processed=ts(60 * 24 * 100 + n + 5),
            status="SUCCESS" if n % 2 == 0 else "FAILED", error=None if n % 2 == 0 else "Fake error", owner=admin,
        )

    # ---- PaperlessTask
    task_specs = [
        ("consume_file", "web_ui", "success", admin, {"filename": "scan_0001.pdf"}, {"document_id": docs[0].pk}),
        ("consume_file", "api_upload", "failure", alice, {"filename": "broken.pdf"}, {"error_message": "Fake failure"}),
        ("consume_file", "folder_consume", "pending", None, {"filename": "queued.pdf"}, None),
        ("consume_file", "email_consume", "started", admin, {"filename": "mail.pdf"}, None),
        ("consume_file", "web_ui", "success", alice, {"filename": "dup.pdf"}, {"duplicate_of": docs[1].pk}),
        ("train_classifier", "scheduled", "success", None, {}, {"message": "Classifier trained"}),
        ("sanity_check", "scheduled", "success", None, {}, {"messages": []}),
        ("index_optimize", "scheduled", "success", None, {}, None),
        ("mail_fetch", "scheduled", "failure", None, {}, {"error_message": "Fake IMAP unreachable"}),
        ("empty_trash", "scheduled", "revoked", None, {}, None),
        ("bulk_update", "web_ui", "success", admin, {"document_ids": [d.pk for d in docs[:3]]}, None),
        ("reprocess_document", "manual", "pending", admin, {"document_id": docs[5].pk}, None),
    ]
    for n, (ttype, src, status, owner, inp, res) in enumerate(task_specs):
        created = ts(60 * 24 * 360 + n * 7)
        started = created + dt.timedelta(seconds=2) if status not in ("pending", "revoked") else None
        done = created + dt.timedelta(seconds=12) if status in ("success", "failure", "revoked") else None
        PaperlessTask.objects.create(
            task_id=tuuid(n + 1), task_type=ttype, trigger_source=src, status=status, owner=owner,
            date_created=created, date_started=started, date_done=done,
            duration_seconds=10.0 if started and done else None, wait_time_seconds=2.0 if started else None,
            input_data=inp, result_data=res, acknowledged=n in (6, 7),
        )

    # ---- some changes for the audit log (update entries)
    with set_actor(alice):
        for doc in docs[10:15]:
            doc.refresh_from_db()
            doc.title = doc.title + " (edited)"
            doc.save()
    Document.objects.filter(pk__in=[d.pk for d in docs[10:15]]).update(modified=ts(60 * 24 * 380))
    with set_actor(admin):
        tags["Car"].name = "Vehicle"
        tags["Car"].save()

    # Make audit log timestamps deterministic (in insertion order)
    for n, pk in enumerate(LogEntry.objects.order_by("pk").values_list("pk", flat=True)):
        LogEntry.objects.filter(pk=pk).update(timestamp=ts(60 * 24 * 30 + n), remote_addr=None)

    out["documents"] = Document.global_objects.count()
    out["logentries"] = LogEntry.objects.count()
    return out


def main() -> None:
    if Document.global_objects.exists() or User.objects.exclude(username="AnonymousUser").exists():
        raise SystemExit("Datenbank ist nicht leer – seed.py erwartet eine frisch migrierte DB (seed.sh).")
    with transaction.atomic():
        res = seed()
    # Do not touch sequences, statistics only
    with connection.cursor() as c:
        c.execute("SELECT count(*) FROM guardian_userobjectpermission")
        res["guardian_user"] = c.fetchone()[0]
        c.execute("SELECT count(*) FROM guardian_groupobjectpermission")
        res["guardian_group"] = c.fetchone()[0]
    print("Seed fertig:", res)


if __name__ == "__main__":
    main()
