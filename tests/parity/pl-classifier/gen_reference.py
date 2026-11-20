"""
Parity/quality reference for pl-classifier (Rust replacement of the scikit-learn classifier).

Runs the *unchanged* upstream code upstream/src/documents/classifier.py@v3.4.2
(DocumentClassifier.train/preprocess_content/predict_*, _text_analyzer, _predict_with_threshold)
with the Python environment in reference/venv (scikit-learn 1.9.1, numpy 2.4.1, scipy 1.17.0,
tantivy 0.26.2 - versions from upstream/uv.lock). Django, the ORM and the cache are replaced by
small in-memory stubs (see `_install_stubs`), so no database or Django instance is needed.
NLTK is NOT used by upstream v3.4.2 (preprocessing is tantivy based), so no NLTK data is needed.

Writes (deterministic, seed below):
  corpus.json     synthetic multi-language document corpus (de/en/fr) with correspondents,
                  document types, storage paths and tags (some non-AUTO, an inbox tag)
  reference.json  preprocessing results (exact), CountVectorizer vocabulary (exact),
                  training hash, sklearn predictions + quality metrics, timing/memory

Usage (from the repository root):
    reference/venv/Scripts/python.exe -I -X utf8 tests/parity/pl-classifier/gen_reference.py
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import platform
import random
import sys
import time
import tracemalloc
import types
from datetime import UTC, datetime, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
UPSTREAM_DOCS = ROOT / "upstream" / "src" / "documents"

SEED = 20261108
N_DOCS = 1000
MATCH_AUTO = 6
MATCH_ANY = 1
THRESHOLD = 0.3  # settings.CLASSIFIER_MATCH_THRESHOLD default
LANGUAGES = [
    "danish", "dutch", "english", "finnish", "french", "german", "italian",
    "norwegian", "portuguese", "russian", "spanish", "swedish",
]  # values of settings.CLASSIFIER_LANGUAGES
QUALITY_CONFIGS = ["english", None]
SPREAD_SEEDS = [1, 2, 3, 4]  # extra sklearn runs with other random_state (variance estimate)  # english = default (PAPERLESS_OCR_LANGUAGE=eng)

# ---------------------------------------------------------------------------
# Stubs for django / documents.models / documents.caching / paperless.signed_pickle
# ---------------------------------------------------------------------------


class _Settings:
    CLASSIFIER_LANGUAGE: str | None = "english"
    CLASSIFIER_MATCH_THRESHOLD = THRESHOLD
    SECRET_KEY = "pl-classifier-reference"
    MODEL_FILE = HERE / "unused.pickle"


class _Cache:
    def __init__(self):
        self.d = {}

    def get(self, k, default=None):
        return self.d.get(k, default)

    def set(self, k, v, timeout=None):
        self.d[k] = v

    def touch(self, k, timeout=None):
        return k in self.d


class _Obj:
    def __init__(self, pk, matching_algorithm):
        self.pk = pk
        self.id = pk
        self.matching_algorithm = matching_algorithm


class _Doc:
    def __init__(self, pk, modified, content, correspondent, document_type, storage_path, tags):
        self.pk = pk
        self.modified = modified
        self.content = content
        self.correspondent = correspondent
        self.document_type = document_type
        self.storage_path = storage_path
        self.tags_all = tags  # list of (_Obj, is_inbox)


class _DocQS:
    """Implements exactly the queryset calls DocumentClassifier.train() makes."""

    store: list[_Doc] = []

    def __init__(self, docs):
        self.docs = docs

    def exclude(self, **kw):
        assert kw == {"tags__is_inbox_tag": True}
        return _DocQS([d for d in self.docs if not any(inbox for _, inbox in d.tags_all)])

    def order_by(self, *f):
        if f == ("pk",):
            return _DocQS(sorted(self.docs, key=lambda d: d.pk))
        return self

    def count(self):
        return len(self.docs)

    def defer(self, *a):
        return self

    def only(self, *a):
        return self

    def select_related(self, *a):
        return self

    def prefetch_related(self, prefetch):
        # Prefetch("tags", Tag.objects.filter(matching_algorithm=AUTO).order_by("pk"), to_attr="auto_tags")
        for d in self.docs:
            d.auto_tags = sorted(
                (t for t, _ in d.tags_all if t.matching_algorithm == MATCH_AUTO),
                key=lambda t: t.pk,
            )
        return self

    def iterator(self, chunk_size=None):
        return iter(self.docs)

    def in_bulk(self, pks):
        by = {d.pk: d for d in self.docs}
        return {pk: by[pk] for pk in pks if pk in by}


class _DocManagerDesc:
    def __get__(self, obj, owner):
        return _DocQS(list(_DocQS.store))


class _TagQS:
    def filter(self, **kw):
        return self

    def order_by(self, *a):
        return self

    def only(self, *a):
        return self


def _install_stubs():
    dj = types.ModuleType("django")
    conf = types.ModuleType("django.conf")
    conf.settings = _Settings
    core = types.ModuleType("django.core")
    cache_mod = types.ModuleType("django.core.cache")
    cache_mod.cache = _Cache()
    cache_mod.caches = {"read-cache": _Cache(), "default": cache_mod.cache}
    db = types.ModuleType("django.db")
    dbm = types.ModuleType("django.db.models")
    dbm.Prefetch = lambda *a, **kw: (a, kw)
    for name, mod in {
        "django": dj, "django.conf": conf, "django.core": core,
        "django.core.cache": cache_mod, "django.db": db, "django.db.models": dbm,
    }.items():
        sys.modules[name] = mod

    docs_pkg = types.ModuleType("documents")
    docs_pkg.__path__ = [str(UPSTREAM_DOCS)]  # submodules load from upstream, __init__ not run
    sys.modules["documents"] = docs_pkg
    caching = types.ModuleType("documents.caching")
    caching.CACHE_5_MINUTES = 300
    caching.CACHE_50_MINUTES = 3000
    caching.CLASSIFIER_HASH_KEY = "classifier_hash"
    caching.CLASSIFIER_MODIFIED_KEY = "classifier_modified"
    caching.CLASSIFIER_VERSION_KEY = "classifier_version"
    sys.modules["documents.caching"] = caching
    models = types.ModuleType("documents.models")

    class Document:
        objects = _DocManagerDesc()

    class MatchingModel:
        MATCH_ANY = MATCH_ANY
        MATCH_AUTO = MATCH_AUTO

    class Tag:
        MATCH_AUTO = MATCH_AUTO
        objects = _TagQS()

    models.Document = Document
    models.MatchingModel = MatchingModel
    models.Tag = Tag
    sys.modules["documents.models"] = models
    paperless = types.ModuleType("paperless")
    paperless.__path__ = []
    sp = types.ModuleType("paperless.signed_pickle")
    import pickle

    class SignedPickleError(Exception):
        pass

    sp.SignedPickleError = SignedPickleError
    sp.signed_pickle_dumps = pickle.dumps
    sp.signed_pickle_loads = pickle.loads
    sys.modules["paperless"] = paperless
    sys.modules["paperless.signed_pickle"] = sp

    spec = importlib.util.spec_from_file_location("documents.classifier", UPSTREAM_DOCS / "classifier.py")
    mod = importlib.util.module_from_spec(spec)
    sys.modules["documents.classifier"] = mod
    spec.loader.exec_module(mod)
    return mod


# ---------------------------------------------------------------------------
# Synthetic corpus (deterministic)
# ---------------------------------------------------------------------------

TYPES = {
    # id: (name, auto, {lang: [phrases]})
    1: ("Rechnung", True, {
        "de": ["Rechnung Nr. {n}", "Rechnungsbetrag {a} EUR", "Bitte überweisen Sie den Betrag innerhalb von 30 Tagen",
               "Zahlbar bis {d}", "Umsatzsteuer 19 %", "Nettobetrag", "Rechnungsdatum {d}", "Leistungszeitraum"],
        "en": ["Invoice No. {n}", "Invoice total {a} USD", "Please pay within 30 days", "Due date {d}",
               "VAT 20 %", "Net amount", "Invoice date {d}", "Billing period"],
        "fr": ["Facture n° {n}", "Montant total {a} EUR", "Veuillez régler sous 30 jours", "Date d'échéance {d}",
               "TVA 20 %", "Montant hors taxes", "Date de facturation {d}"]}),
    2: ("Vertrag", True, {
        "de": ["Vertragsnummer {n}", "Vertragsbeginn {d}", "Laufzeit 24 Monate", "Kündigungsfrist drei Monate",
               "Allgemeine Geschäftsbedingungen", "Unterschrift beider Vertragsparteien", "Vertragspartner"],
        "en": ["Contract number {n}", "Start of contract {d}", "Term of 24 months", "Notice period of three months",
               "Terms and conditions", "Signature of both parties", "Contracting party"],
        "fr": ["Numéro de contrat {n}", "Début du contrat {d}", "Durée de 24 mois", "Préavis de trois mois",
               "Conditions générales", "Signature des deux parties"]}),
    3: ("Kontoauszug", True, {
        "de": ["Kontoauszug {n}", "Alter Kontostand {a} EUR", "Neuer Kontostand {a} EUR", "Buchungstag Wertstellung",
               "Lastschrift", "Gutschrift", "Dauerauftrag", "IBAN DE{n}"],
        "en": ["Account statement {n}", "Opening balance {a}", "Closing balance {a}", "Booking date value date",
               "Direct debit", "Credit transfer", "Standing order"],
        "fr": ["Relevé de compte {n}", "Ancien solde {a}", "Nouveau solde {a}", "Date d'opération", "Prélèvement",
               "Virement reçu"]}),
    4: ("Lohnabrechnung", True, {
        "de": ["Entgeltabrechnung {m}", "Bruttogehalt {a}", "Nettogehalt {a}", "Lohnsteuer", "Sozialversicherung",
               "Rentenversicherung Krankenversicherung", "Personalnummer {n}", "Auszahlungsbetrag"],
        "en": ["Payslip {m}", "Gross salary {a}", "Net pay {a}", "Income tax", "National insurance",
               "Employee number {n}", "Pension contribution"],
        "fr": ["Bulletin de salaire {m}", "Salaire brut {a}", "Salaire net {a}", "Cotisations sociales",
               "Matricule {n}", "Net à payer"]}),
    5: ("Versicherungspolice", True, {
        "de": ["Versicherungsschein {n}", "Versicherungsbeginn {d}", "Jahresbeitrag {a} EUR", "Selbstbeteiligung",
               "Versicherungsnehmer", "Deckungssumme", "Beitragsrechnung"],
        "en": ["Insurance policy {n}", "Policy start {d}", "Annual premium {a}", "Deductible", "Policyholder",
               "Coverage amount"],
        "fr": ["Police d'assurance {n}", "Prise d'effet {d}", "Prime annuelle {a}", "Franchise", "Assuré",
               "Montant de la garantie"]}),
    6: ("Mahnung", True, {
        "de": ["Zahlungserinnerung", "Mahnung zur Rechnung {n}", "Offener Betrag {a} EUR", "Mahngebühr 5,00 EUR",
               "Wir bitten um umgehende Zahlung", "Letzte Mahnung"],
        "en": ["Payment reminder", "Reminder for invoice {n}", "Outstanding amount {a}", "Late fee",
               "Please pay immediately", "Final notice"],
        "fr": ["Rappel de paiement", "Relance facture {n}", "Montant impayé {a}", "Frais de relance",
               "Dernier rappel"]}),
    7: ("Brief", True, {
        "de": ["Sehr geehrte Damen und Herren", "Mit freundlichen Grüßen", "Bezug nehmend auf Ihr Schreiben vom {d}",
               "Wir möchten Sie informieren", "Für Rückfragen stehen wir gerne zur Verfügung"],
        "en": ["Dear Sir or Madam", "Kind regards", "With reference to your letter of {d}",
               "We would like to inform you", "Please do not hesitate to contact us"],
        "fr": ["Madame, Monsieur", "Veuillez agréer nos salutations distinguées", "Suite à votre courrier du {d}",
               "Nous vous informons"]}),
    8: ("Quittung", True, {
        "de": ["Quittung", "Betrag dankend erhalten {a} EUR", "Barzahlung", "Kassenbon {n}", "Summe {a}",
               "Vielen Dank für Ihren Einkauf"],
        "en": ["Receipt", "Amount received {a}", "Paid in cash", "Receipt no {n}", "Total {a}",
               "Thank you for your purchase"],
        "fr": ["Reçu", "Montant reçu {a}", "Paiement en espèces", "Ticket {n}", "Total {a}", "Merci de votre achat"]}),
    9: ("Sonstiges", False, {
        "de": ["Information", "Hinweis", "Anlage"], "en": ["Information", "Notice", "Attachment"],
        "fr": ["Information", "Avis", "Pièce jointe"]}),
}

# id: (name, auto, langs, category, typical types, product words)
CORRESPONDENTS = {
    1: ("Stadtwerke Musterstadt", True, ["de"], "home", [1, 2, 6], ["Strom", "Gas", "Zählerstand", "kWh", "Abschlag", "Fernwärme"]),
    2: ("Telekom Deutschland GmbH", True, ["de"], "home", [1, 2, 6], ["Festnetz", "Mobilfunk", "Datenvolumen", "Router", "Tarif MagentaZuhause"]),
    3: ("Sparkasse Rheinland", True, ["de"], "finance", [3, 2, 7], ["Girokonto", "Sparbuch", "Kreditkarte", "Filiale", "Online-Banking"]),
    4: ("ACME Software AG", True, ["de", "en"], "work", [4, 7, 2], ["Arbeitgeber", "employee", "Abteilung Entwicklung", "Urlaubsanspruch", "Homeoffice"]),
    5: ("Allianz Versicherungs-AG", True, ["de"], "insurance", [5, 1, 7], ["Hausrat", "Haftpflicht", "Schadensmeldung", "Police", "Versicherungsfall"]),
    6: ("AutoHaus Becker", True, ["de"], "car", [1, 8, 2], ["Inspektion", "Ölwechsel", "Reifenwechsel", "Fahrzeug", "Kennzeichen", "TÜV"]),
    7: ("Dr. med. Anna Weber", True, ["de"], "health", [1, 7, 8], ["Behandlung", "Praxis", "Patient", "Diagnose", "GOÄ Ziffer"]),
    8: ("Amazon EU S.à r.l.", True, ["de", "en", "fr"], "shopping", [1, 8], ["Bestellung", "order", "Lieferung", "Artikel", "Versand", "Marketplace"]),
    9: ("British Gas", True, ["en"], "home", [1, 2, 6], ["electricity", "gas supply", "meter reading", "tariff", "direct debit"]),
    10: ("HSBC UK Bank plc", True, ["en"], "finance", [3, 7], ["current account", "savings", "sort code", "overdraft", "online banking"]),
    11: ("Aviva Insurance Ltd", True, ["en"], "insurance", [5, 6, 7], ["home insurance", "car insurance", "claim", "renewal", "no claims bonus"]),
    12: ("EDF Électricité de France", True, ["fr"], "home", [1, 2, 6], ["électricité", "compteur", "consommation", "abonnement", "kWh"]),
    13: ("Crédit Agricole", True, ["fr"], "finance", [3, 7, 2], ["compte courant", "carte bancaire", "agence", "épargne", "prêt immobilier"]),
    14: ("Finanzamt Musterstadt", True, ["de"], "tax", [7, 6], ["Steuerbescheid", "Einkommensteuer", "Steuernummer", "Erstattung", "Vorauszahlung"]),
    15: ("Hausverwaltung Schmidt", False, ["de"], "home", [2, 1, 7], ["Miete", "Nebenkosten", "Wohnung", "Hausordnung", "Kaution"]),
    16: ("Fitness Club Vital", False, ["de", "en"], "health", [2, 1], ["Mitgliedschaft", "membership", "Kurse", "Studio"]),
}

STORAGE_PATHS = {1: ("Finanzen", True), 2: ("Haus", True), 3: ("Versicherung", True), 4: ("Arbeit", True),
                 5: ("Auto", True), 6: ("Archiv", False)}
CATEGORY_PATH = {"finance": 1, "tax": 1, "home": 2, "insurance": 3, "work": 4, "car": 5, "health": 2, "shopping": 2}

# id: (name, auto, inbox)
TAGS = {1: ("steuer", True, False), 2: ("bezahlt", True, False), 3: ("auto", True, False),
        4: ("wichtig", True, False), 5: ("versicherung", True, False), 6: ("arbeit", True, False),
        7: ("haushalt", True, False), 8: ("gesundheit", True, False), 9: ("2024", True, False),
        10: ("2025", True, False), 11: ("privat", False, False), 12: ("Posteingang", False, True)}

FILLER = {
    "de": ["Diese Mitteilung wurde maschinell erstellt und ist ohne Unterschrift gültig.",
           "Bitte bewahren Sie dieses Dokument für Ihre Unterlagen auf.",
           "Unsere Geschäftszeiten sind Montag bis Freitag von 8 bis 18 Uhr.",
           "Weitere Informationen finden Sie auf unserer Internetseite.",
           "Bei Fragen wenden Sie sich bitte an unseren Kundenservice.",
           "Der Datenschutz ist uns wichtig, Hinweise finden Sie in der Anlage.",
           "Seite {p} von 3", "Kundennummer {n}", "Musterstraße {p}, 12345 Musterstadt",
           "Telefon 0800 {n}", "E-Mail service@example.de", "Es gelten die gesetzlichen Bestimmungen.",
           "Wir danken Ihnen für Ihr Vertrauen.", "Die Zahlung erfolgt per Lastschrift vom angegebenen Konto."],
    "en": ["This notice was generated automatically and is valid without signature.",
           "Please keep this document for your records.",
           "Our opening hours are Monday to Friday from 9am to 5pm.",
           "Further information can be found on our website.",
           "If you have any questions please contact our customer service.",
           "We take data protection seriously, see the enclosed privacy notice.",
           "Page {p} of 3", "Customer number {n}", "{p} High Street, London", "Phone 0800 {n}",
           "Email support@example.co.uk", "Thank you for choosing us.",
           "Payment will be collected by direct debit from your account."],
    "fr": ["Ce document a été généré automatiquement et est valable sans signature.",
           "Veuillez conserver ce document.", "Nos horaires sont du lundi au vendredi de 9h à 18h.",
           "Plus d'informations sur notre site internet.", "Pour toute question contactez notre service client.",
           "Page {p} sur 3", "Numéro client {n}", "{p} rue de la Paix, 75002 Paris", "Téléphone 09 {n}",
           "Courriel service@example.fr", "Nous vous remercions de votre confiance."],
}
PAID = {"de": "Betrag bereits bezahlt", "en": "Amount already paid", "fr": "Montant déjà payé"}
URGENT = {"de": "Dringend: bitte sofort bearbeiten", "en": "Urgent: please act now", "fr": "Urgent : à traiter immédiatement"}
TAXDOC = {"de": "Für das Finanzamt relevant (Steuererklärung)", "en": "Relevant for your tax return",
          "fr": "Document utile pour votre déclaration d'impôts"}
MONTHS = {"de": ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
          "en": ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
          "fr": ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"]}


def _ocr_noise(rng: random.Random, text: str, rate: float) -> str:
    out = []
    for ch in text:
        r = rng.random()
        if ch.isalpha() and r < rate:
            out.append(rng.choice("il1|rnmcoe0"))
        elif ch == " " and r < rate / 2:
            continue  # merged words
        else:
            out.append(ch)
    return "".join(out)


def make_corpus():
    rng = random.Random(SEED)
    base = datetime(2025, 1, 6, 8, 0, tzinfo=UTC)
    docs = []
    for pk in range(1, N_DOCS + 1):
        cid = rng.choice(list(CORRESPONDENTS)) if rng.random() > 0.08 else None
        if cid is not None:
            cname, _, langs, cat, typical, words = CORRESPONDENTS[cid]
            lang = rng.choice(langs)
            tid = rng.choice(typical) if rng.random() < 0.8 else rng.choice(list(TYPES))
        else:
            cname, cat, words = None, None, []
            lang = rng.choices(["de", "en", "fr"], [5, 4, 2])[0]
            tid = rng.choice(list(TYPES))
        year = rng.choice([2023, 2024, 2025])
        month = rng.randrange(12)

        def fill(s):
            return s.format(
                n=rng.randrange(10000, 99999999),
                a=f"{rng.randrange(5, 3000)},{rng.randrange(100):02d}",
                d=f"{rng.randrange(1, 29):02d}.{month + 1:02d}.{year}",
                m=f"{MONTHS[lang][month]} {year}",
                p=rng.randrange(1, 120),
            )

        lines = []
        if cname and rng.random() < 0.85:
            lines.append(cname)
        lines.append(f"{rng.randrange(1, 29)}. {MONTHS[lang][month]} {year}")
        phrases = TYPES[tid][2][lang]
        lines += [fill(p) for p in rng.sample(phrases, k=min(len(phrases), rng.randrange(2, 5)))]
        if words:
            lines += [" ".join(rng.sample(words, k=rng.randrange(1, min(4, len(words)) + 1)))]
        # tags
        tags = set()
        if tid in (3, 4) or (cat == "tax") or rng.random() < 0.05:
            tags.add(1)
            if rng.random() < 0.5:
                lines.append(TAXDOC[lang])
        if tid in (1, 8) and (tid == 8 or rng.random() < 0.5):
            tags.add(2)
            if rng.random() < 0.6:
                lines.append(PAID[lang])
        if cat == "car":
            tags.add(3)
        if rng.random() < 0.12 or tid == 6:
            tags.add(4)
            if rng.random() < 0.7:
                lines.append(URGENT[lang])
        if cat == "insurance":
            tags.add(5)
        if cat == "work":
            tags.add(6)
        if cat == "home":
            tags.add(7)
        if cat == "health":
            tags.add(8)
        if year == 2024:
            tags.add(9)
        if year == 2025:
            tags.add(10)
        if rng.random() < 0.1:
            tags.add(11)
        if rng.random() < 0.07:
            tags.add(12)  # inbox -> excluded from training
        lines += [fill(f) for f in rng.choices(FILLER[lang], k=rng.randrange(3, 10))]
        rng.shuffle(lines)
        content = _ocr_noise(rng, "\n".join(lines), rate=rng.choice([0.0, 0.0, 0.01, 0.03]))
        # label noise (wrong manual assignment)
        if cid is not None and rng.random() < 0.04:
            cid = rng.choice(list(CORRESPONDENTS))
        if rng.random() < 0.05:
            tid = rng.choice(list(TYPES))
        sp = CATEGORY_PATH.get(cat) if cat else None
        r = rng.random()
        if r < 0.1:
            sp = None
        elif r < 0.17:
            sp = rng.choice(list(STORAGE_PATHS))
        if rng.random() < 0.03:
            tags ^= {rng.choice(list(TAGS)[:10])}
        modified = base + timedelta(hours=pk * 5, seconds=rng.randrange(3600), microseconds=rng.randrange(10**6))
        docs.append({
            "pk": pk,
            "modified_us": int((modified - datetime(1970, 1, 1, tzinfo=UTC)) / timedelta(microseconds=1)),
            "lang": lang,
            "split": "test" if pk % 5 == 0 else "train",
            "content": content,
            "correspondent": cid,
            "document_type": tid,
            "storage_path": sp,
            "tags": sorted(tags),
        })
    objects = {
        "correspondents": [{"id": k, "name": v[0], "auto": v[1]} for k, v in CORRESPONDENTS.items()],
        "document_types": [{"id": k, "name": v[0], "auto": v[1]} for k, v in TYPES.items()],
        "storage_paths": [{"id": k, "name": v[0], "auto": v[1]} for k, v in STORAGE_PATHS.items()],
        "tags": [{"id": k, "name": v[0], "auto": v[1], "inbox": v[2]} for k, v in TAGS.items()],
    }
    return objects, docs


EDGE_CASES = [
    "", "   ", "Hello World", "tax_id 2026 _x_ __", "don't won't We’ve", "İstanbul IŞIK ıi",
    "ΟΔΟΣ ΣΟΦΟΣ Σ σς", "Straße STRASSE Maße ẞ", "ﬁnance ﬂow Ⅻ Ⅷ ² ³ ½ ٣ ൧", "été café",
    "देवनागरी हिन्दी", "日本語のテキスト 中文 한국어", "emoji 😀 test👍ok", "a1b2 x9 12.03.2025 1.234,56 €",
    "Ünïcödé ÄÖÜ äöü ÀÉÎÕÛ", "ﾊﾝｶｸ Ｆｕｌｌ１２３", "‿under‿tie ⁀", "Zero‍Width‌Join", "Ǆ ǅ ǆ ǈ",
    "CONSOLIDATED Invoices INVOICING running runs ran", "Rechnungen Rechnungsbetrag Versicherungen Zahlungen",
    "factures facturation électricité Électricité", "Привет МИР при прочих", "tab\tnew\nline\r\nend",
    "x" * 300, "Ab ab AB aB", "e.g. i.e. U.S.A.",
]


def _sha16(s: str) -> str:
    return hashlib.sha256(s.encode()).hexdigest()[:16]


def main():
    clf_mod = _install_stubs()
    import numpy
    import scipy
    import sklearn
    import tantivy  # noqa: F401
    from importlib.metadata import version

    objects, docs = make_corpus()
    (HERE / "corpus.json").write_text(
        json.dumps({"seed": SEED, "objects": objects, "documents": docs}, ensure_ascii=False, indent=0),
        encoding="utf-8",
    )

    auto = {
        "correspondent": {o["id"] for o in objects["correspondents"] if o["auto"]},
        "document_type": {o["id"] for o in objects["document_types"] if o["auto"]},
        "storage_path": {o["id"] for o in objects["storage_paths"] if o["auto"]},
        "tags": {o["id"] for o in objects["tags"] if o["auto"]},
    }
    inbox = {o["id"] for o in objects["tags"] if o["inbox"]}
    tag_alg = {o["id"]: (MATCH_AUTO if o["auto"] else MATCH_ANY) for o in objects["tags"]}

    def obj(kind, oid):
        if oid is None:
            return None
        return _Obj(oid, MATCH_AUTO if oid in auto[kind] else MATCH_ANY)

    def to_doc(d):
        return _Doc(
            d["pk"], datetime(1970, 1, 1, tzinfo=UTC) + timedelta(microseconds=d["modified_us"]), d["content"],
            obj("correspondent", d["correspondent"]), obj("document_type", d["document_type"]),
            obj("storage_path", d["storage_path"]),
            [(_Obj(t, tag_alg[t]), t in inbox) for t in d["tags"]],
        )

    def truth_of(test_docs):
        return [{
            "correspondent": d["correspondent"] if d["correspondent"] in auto["correspondent"] else None,
            "document_type": d["document_type"] if d["document_type"] in auto["document_type"] else None,
            "storage_path": d["storage_path"] if d["storage_path"] in auto["storage_path"] else None,
            "tags": [t for t in d["tags"] if t in auto["tags"]],
        } for d in test_docs]

    S = clf_mod.settings
    ref = {
        "generated_by": "tests/parity/pl-classifier/gen_reference.py",
        "upstream": "src/documents/classifier.py@v3.4.2 (FORMAT_VERSION %d)" % clf_mod.DocumentClassifier.FORMAT_VERSION,
        "versions": {"python": platform.python_version(), "scikit-learn": sklearn.__version__,
                     "numpy": numpy.__version__, "scipy": scipy.__version__, "tantivy": version("tantivy")},
        "threshold": THRESHOLD,
    }

    # --- preprocessing (must be exact) -------------------------------------
    pre = {"languages": [None] + LANGUAGES, "edge_cases": [], "doc_sha16": {}}
    c = clf_mod.DocumentClassifier()
    for text in EDGE_CASES:
        out = {}
        for lang in pre["languages"]:
            S.CLASSIFIER_LANGUAGE = lang
            out[str(lang)] = c.preprocess_content(text)
        pre["edge_cases"].append({"text": text, "out": out})
    for lang in pre["languages"]:
        S.CLASSIFIER_LANGUAGE = lang
        pre["doc_sha16"][str(lang)] = [_sha16(c.preprocess_content(d["content"])) for d in docs]
    ref["preprocess"] = pre

    # --- training + quality per config --------------------------------------
    train_docs = [d for d in docs if d["split"] == "train"]
    test_docs = [d for d in docs if d["split"] == "test"]
    ref["quality"] = {}
    for lang in QUALITY_CONFIGS:
        S.CLASSIFIER_LANGUAGE = lang
        _DocQS.store = [to_doc(d) for d in train_docs]
        clf = clf_mod.DocumentClassifier()
        tracemalloc.start()
        t0 = time.perf_counter()
        assert clf.train() is True
        t_train = time.perf_counter() - t0
        _, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        assert clf.train() is False  # unchanged -> no retrain
        vec = clf.data_vectorizer
        X = vec.transform([clf.preprocess_content(d["content"]) for d in train_docs
                           if not any(t in inbox for t in d["tags"])])
        vocab = sorted(vec.vocabulary_, key=lambda t: vec.vocabulary_[t])
        t0 = time.perf_counter()
        preds = []
        for d in test_docs:
            preds.append({
                "pk": d["pk"],
                "correspondent": clf.predict_correspondent(d["content"]),
                "document_type": clf.predict_document_type(d["content"]),
                "storage_path": clf.predict_storage_path(d["content"]),
                "tags": sorted(int(t) for t in clf.predict_tags(d["content"])),
            })
        t_pred = (time.perf_counter() - t0) / len(test_docs)
        truth = truth_of(test_docs)
        n_iter = {k: getattr(getattr(clf, k), "n_iter_", None) for k in
                  ["tags_classifier", "correspondent_classifier", "document_type_classifier", "storage_path_classifier"]}
        ref["quality"][str(lang)] = {
            "train_hash_hex": clf.last_auto_type_hash.hex(),
            "last_doc_change_us": int((clf.last_doc_change_time - datetime(1970, 1, 1, tzinfo=UTC)) / timedelta(microseconds=1)),
            "n_train_docs": X.shape[0],
            "vocabulary": vocab,
            "row_nnz": [int(v) for v in X.getnnz(axis=1)],
            "row_sum": [int(v) for v in X.sum(axis=1).A1],
            "tags_classes": [int(x) for x in clf.tags_binarizer.classes_],
            "n_iter": n_iter,
            "predictions": preds,
            "metrics": metrics(preds, truth),
            "train_seconds": round(t_train, 3),
            "train_peak_tracemalloc_mb": round(peak / 2**20, 1),
            "predict_ms_per_doc": round(t_pred * 1000, 3),
        }
        print(lang, json.dumps(ref["quality"][str(lang)]["metrics"]), f"train {t_train:.1f}s peak {peak / 2**20:.0f}MB",
              file=sys.stderr)

    # --- seed spread of sklearn itself (upstream hardcodes random_state=0) -----
    import sklearn.neural_network as nn

    orig = nn.MLPClassifier
    S.CLASSIFIER_LANGUAGE = "english"
    ref["sklearn_seed_spread_english"] = {}
    for seed in SPREAD_SEEDS:
        nn.MLPClassifier = lambda seed=seed, **kw: orig(**{**kw, "random_state": seed})
        _DocQS.store = [to_doc(d) for d in train_docs]
        clf = clf_mod.DocumentClassifier()
        clf.train()
        preds = [{
            "correspondent": clf.predict_correspondent(d["content"]),
            "document_type": clf.predict_document_type(d["content"]),
            "storage_path": clf.predict_storage_path(d["content"]),
            "tags": sorted(int(t) for t in clf.predict_tags(d["content"])),
        } for d in test_docs]
        ref["sklearn_seed_spread_english"][str(seed)] = metrics(preds, truth_of(test_docs))
        print("seed", seed, json.dumps(ref["sklearn_seed_spread_english"][str(seed)]), file=sys.stderr)
    nn.MLPClassifier = orig

    (HERE / "reference.json").write_text(json.dumps(ref, ensure_ascii=False, indent=1), encoding="utf-8")


def _single(pred, true):
    acc = sum(p == t for p, t in zip(pred, true)) / len(true)
    classes = sorted({x for x in pred + true if x is not None})
    f1s = []
    for c in classes:
        tp = sum(p == c and t == c for p, t in zip(pred, true))
        fp = sum(p == c and t != c for p, t in zip(pred, true))
        fn = sum(p != c and t == c for p, t in zip(pred, true))
        f1s.append(2 * tp / (2 * tp + fp + fn) if (2 * tp + fp + fn) else 0.0)
    return {"accuracy": acc, "macro_f1": sum(f1s) / len(f1s) if f1s else 0.0}


def metrics(preds, truth):
    """Same definitions as crates/pl-classifier/src/metrics.rs."""
    out = {}
    for k in ("correspondent", "document_type", "storage_path"):
        out[k] = _single([p[k] for p in preds], [t[k] for t in truth])
    tags = sorted({x for p in preds for x in p["tags"]} | {x for t in truth for x in t["tags"]})
    TP = FP = FN = 0
    f1s = []
    for c in tags:
        tp = sum(c in p["tags"] and c in t["tags"] for p, t in zip(preds, truth))
        fp = sum(c in p["tags"] and c not in t["tags"] for p, t in zip(preds, truth))
        fn = sum(c not in p["tags"] and c in t["tags"] for p, t in zip(preds, truth))
        TP, FP, FN = TP + tp, FP + fp, FN + fn
        f1s.append(2 * tp / (2 * tp + fp + fn) if (2 * tp + fp + fn) else 0.0)
    out["tags"] = {
        "micro_f1": 2 * TP / (2 * TP + FP + FN) if (2 * TP + FP + FN) else 0.0,
        "macro_f1": sum(f1s) / len(f1s) if f1s else 0.0,
        "exact_match": sum(set(p["tags"]) == set(t["tags"]) for p, t in zip(preds, truth)) / len(truth),
    }
    return out


if __name__ == "__main__":
    main()
