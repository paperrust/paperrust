"""Synthetic large data set for PaperRust (measurement setup 1 million documents, docs/PERF-SCALE.md).

Reproducible (fixed seed), stdlib + psycopg only (reference venv). No real data: all names,
texts, amounts, IBAN-like numbers are generated from syllables/word lists.

Procedure (see tests/bench/scale/README.md):
  1. create empty DB, `paperrust manage migrate` (schema v3.4.2 + paperrust_* migrations, sync triggers)
  2. this script: loads everything via COPY (trigger/FK checks off via session_replication_role=replica,
     hence superuser), sets sequences, rebuilds the sync tables (paperrust_sync_rebuild), ANALYZE
  3. index/classifier with `paperrust manage document_index reindex` / `document_create_classifier`

Invocation (Git Bash):
  "$PY" -I tests/bench/scale/gen.py --dsn "host=127.0.0.1 port=54329 dbname=pl_scale_1m user=postgres password=..." \
      --docs 1000000 --media-root <dir> [--seed 20261110]

Distributions (defaults for 1 million):
  correspondents 5000 (Zipf s=1.1), document types 60 (Zipf 1.0), tags 800 (100 roots, down to level 4,
  0-8 per document, Zipf 1.0, ancestors are set along like upstream, inbox ~2 %), storage paths 200
  (of which path 1 = "Scale rename" with exactly --rename-docs documents and real files), custom fields 30
  (all 10 types, ~3 instances/document), notes 5 %, users 200 + admin in 30 groups, owner 30 %
  admin / 62 % Zipf over users / 8 % without, guardian object permissions on 20 % of the documents (1-3 shares,
  60 % to groups, view and half also change), saved views 300, workflows 50, mail accounts 5 / rules 40,
  content 0.5-5 KB (60 % de / 40 % en), created over 15 years (growing), ASN on 60 %, trash 0.3 %.
"""

from __future__ import annotations

import argparse
import base64
import bisect
import hashlib
import itertools
import json
import math
import os
import random
import sys
import time
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import psycopg

# ---------------------------------------------------------------------------------------------------
# Fixed identifiers for the benchmark (local test values)
ADMIN_ID = 2
ADMIN_TOKEN = "5ca1e0000000000000000000000000000000ad01"
RESTRICTED_RANK = 20  # Rank in the owner Zipf distribution (medium-sized user)
RESTRICTED_TOKEN = "5ca1e0000000000000000000000000000000ee02"
# PBKDF2-SHA256 for password "scale" (Django format, 1 iteration is not enough for login in the test -
# we only use tokens; the hash is still validly formatted)
UNUSABLE = "!scale-unusable-password"

DE_WORDS = """und der die das ist nicht mit für auf dem den von zu im ein eine einer wir Sie Ihre Ihnen bitte
Rechnung Betrag Zahlung Konto Frist Mahnung Vertrag Kündigung Versicherung Police Prämie Steuer Erklärung
Bescheid Lohn Abrechnung Monat Jahr Quartal Kunde Kundennummer Auftrag Lieferung Lieferschein Bestellung
Artikel Menge Preis Summe netto brutto Mehrwertsteuer Rabatt Gutschrift Fälligkeit Überweisung Bank
Gebühr Zinsen Darlehen Hypothek Miete Nebenkosten Strom Wasser Heizung Gas Internet Telefon Mobilfunk
Arzt Praxis Behandlung Rezept Krankenkasse Leistung Selbstbehalt Franchise Termin Bestätigung Anmeldung
Schule Kurs Zeugnis Diplom Arbeitgeber Arbeitsvertrag Kündigungsfrist Ferien Urlaub Spesen Beleg Quittung
Fahrzeug Auto Reparatur Service Garage Reifen Leasing Kontrolle Prüfung Gemeinde Amt Behörde Ausweis Pass
Wohnung Haus Eigentümer Verwaltung Schlüssel Übergabe Protokoll Schaden Meldung Garantie Gewährleistung
freundlichen Grüßen Sehr geehrte geehrter Damen Herren Anlage Seite Datum Unterschrift Ort Referenz
gemäß bezüglich hiermit erhalten senden zahlen innerhalb Tagen spätestens erneut wiederholt bereits
Information Hinweis Änderung Anpassung Erhöhung Senkung neue alte gültig ab bis per sofort jährlich
monatlich vierteljährlich halbjährlich einmalig Beitrag Mitglied Verein Spende Abonnement Zeitschrift""".split()

EN_WORDS = """and the of to in is for on with as by this that your you we our please be are from at an
invoice amount payment account due reminder contract termination insurance policy premium tax return
assessment salary statement month year quarter customer number order delivery note purchase item
quantity price total net gross VAT discount credit transfer bank fee interest loan mortgage rent
utilities electricity water heating gas internet phone mobile doctor practice treatment prescription
health plan benefit deductible appointment confirmation registration school course certificate diploma
employer employment notice period vacation expenses receipt vehicle car repair service garage tyres
lease inspection municipality office authority identity passport apartment house owner management key
handover report damage claim warranty guarantee sincerely regards dear madam sir attachment page date
signature place reference according regarding hereby received send pay within days latest again already
information change adjustment increase decrease new old valid from until immediately annually monthly
quarterly semiannual once contribution member club donation subscription magazine balance outstanding""".split()

SYL = ["ka", "lo", "mi", "ber", "ten", "sch", "an", "ri", "do", "fel", "gra", "hu", "ne", "ot", "pra",
       "st", "ul", "ve", "win", "zo", "el", "mar", "kon", "tri", "bau", "lin", "ser", "dor", "fa", "gen",
       "hal", "is", "jo", "kr", "lu", "mo", "nor", "pet", "ro", "sa", "tal", "un", "vi", "wal", "zer"]
CITIES_N = 400
MONTHS_DE = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"]
MONTHS_EN = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
CF_TYPES = (["string"] * 5 + ["url"] * 2 + ["date"] * 5 + ["boolean"] * 3 + ["integer"] * 3 + ["float"] * 2
            + ["monetary"] * 4 + ["documentlink"] * 2 + ["select"] * 3 + ["longtext"])

# 1x1 WebP (lossless) as thumbnail
WEBP_1X1 = base64.b64decode("UklGRhoAAABXRUJQVlA4TA0AAAAvAAAAEAcQERGIiP4HAA==")


def tiny_pdf(text: str) -> bytes:
    """Minimal, valid 1-page PDF with text (correct xref offsets)."""
    stream = f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode()
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = bytearray(b"%PDF-1.4\n")
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out))
        out += f"{i} 0 obj\n".encode() + o + b"\nendobj\n"
    x = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    for o in offs:
        out += f"{o:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    return bytes(out)


class Zipf:
    """Weighted selection over n ranks (weight 1/r^s), O(log n) per draw."""

    def __init__(self, n: int, s: float):
        acc, self.cum = 0.0, []
        for r in range(1, n + 1):
            acc += 1.0 / r ** s
            self.cum.append(acc)
        self.total, self.n = acc, n

    def rank(self, rnd: random.Random) -> int:  # 0-based
        return min(bisect.bisect_left(self.cum, rnd.random() * self.total), self.n - 1)


def esc(v) -> str:
    """Value for COPY text format."""
    if v is None:
        return r"\N"
    if isinstance(v, bool):
        return "t" if v else "f"
    s = str(v)
    if "\\" in s or "\t" in s or "\n" in s or "\r" in s:
        s = s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")
    return s


def row(*vals) -> str:
    return "\t".join(esc(v) for v in vals) + "\n"


class Gen:
    def __init__(self, a):
        self.a = a
        self.r = random.Random(a.seed)
        r = self.r
        n_syl = len(SYL)
        # Pseudo-words per language (Zipf vocabulary, yields a realistic number of terms in the index)
        def pseudo(k: int, salt: str) -> list[str]:
            seen, out = set(), []
            rr = random.Random(f"{a.seed}-{salt}")
            while len(out) < k:
                w = "".join(SYL[rr.randrange(n_syl)] for _ in range(rr.randint(2, 4)))
                if salt == "de" and rr.random() < 0.3:
                    w = w.capitalize()
                if w not in seen:
                    seen.add(w)
                    out.append(w)
            return out
        self.vocab = {
            "de": DE_WORDS + pseudo(a.vocab, "de"),
            "en": EN_WORDS + pseudo(a.vocab, "en"),
        }
        # Base words first → frequent; pseudo-words Zipf-distributed behind them
        self.vcum = {}
        for lang, v in self.vocab.items():
            acc, cum = 0.0, []
            for i in range(len(v)):
                acc += 1.0 / (i + 1) ** 1.05
                cum.append(acc)
            self.vcum[lang] = cum
        self.cities = [(f"{r.randint(1000, 9999)}", "".join(SYL[r.randrange(n_syl)] for _ in range(r.randint(2, 3))).capitalize() + r.choice(["", "dorf", "berg", "stadt", "wil", "hausen"])) for _ in range(CITIES_N)]
        self.streets = ["".join(SYL[r.randrange(n_syl)] for _ in range(2)).capitalize() + r.choice(["strasse", "weg", "gasse", "platz", " Street", " Road"]) for _ in range(800)]

    def name(self, parts=(2, 3)) -> str:
        return "".join(SYL[self.r.randrange(len(SYL))] for _ in range(self.r.randint(*parts))).capitalize()

    def words(self, lang: str, k: int) -> list[str]:
        return self.r.choices(self.vocab[lang], cum_weights=self.vcum[lang], k=k)


def tag_tree(ids: list[int], parent: dict[int, int | None], names: dict[int, str]):
    """django-treenode fields (tn_*) like upstream: siblings sorted by (priority, name), DFS order."""
    children: dict[int | None, list[int]] = {}
    for i in ids:
        children.setdefault(parent[i], []).append(i)
    for k in children:
        children[k].sort(key=lambda t: (names[t], t))
    out, order = {}, itertools.count()

    def anc(i):
        a, p = [], parent[i]
        while p is not None:
            a.append(p)
            p = parent[p]
        return list(reversed(a))

    def desc(i):
        res = []
        for c in children.get(i, []):
            res.append(c)
            res.extend(desc(c))
        return res

    def depth(i):
        cs = children.get(i, [])
        return 0 if not cs else 1 + max(depth(c) for c in cs)

    def walk(i, level):
        sib = children[parent[i]]
        a, d = anc(i), desc(i)
        out[i] = dict(
            anc=",".join(map(str, a)), anc_n=len(a),
            ch=",".join(map(str, children.get(i, []))), ch_n=len(children.get(i, [])),
            depth=depth(i), desc=",".join(map(str, d)), desc_n=len(d),
            index=sib.index(i), level=level, order=next(order),
            sib=",".join(str(s) for s in sib if s != i), sib_n=len(sib) - 1,
        )
        for c in children.get(i, []):
            walk(c, level + 1)

    for root in children.get(None, []):
        walk(root, 1)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--docs", type=int, default=1_000_000)
    ap.add_argument("--seed", type=int, default=20261110)
    ap.add_argument("--correspondents", type=int, default=5000)
    ap.add_argument("--doctypes", type=int, default=60)
    ap.add_argument("--tags", type=int, default=800)
    ap.add_argument("--storage-paths", type=int, default=200)
    ap.add_argument("--users", type=int, default=200)
    ap.add_argument("--groups", type=int, default=30)
    ap.add_argument("--saved-views", type=int, default=300)
    ap.add_argument("--workflows", type=int, default=50)
    ap.add_argument("--vocab", type=int, default=40000, help="Pseudowörter je Sprache")
    ap.add_argument("--media-root", required=True, help="PAPERLESS_MEDIA_ROOT der Messinstanz")
    ap.add_argument("--media-docs", type=int, default=2000, help="Dokumente mit echter Originaldatei + Vorschau")
    ap.add_argument("--rename-docs", type=int, default=50000, help="Dokumente in Speicherpfad 1 (mit Datei)")
    ap.add_argument("--skip-sync-rebuild", action="store_true")
    a = ap.parse_args()

    t_all = time.time()
    g = Gen(a)
    r = g.r
    N = a.docs
    media = Path(a.media_root)
    stats: dict[str, object] = {"seed": a.seed, "docs": N}
    now = datetime(2026, 11, 1, 12, 0, tzinfo=timezone.utc)
    start = datetime(2011, 11, 1, tzinfo=timezone.utc)
    span_days = (now - start).days

    conn = psycopg.connect(a.dsn, autocommit=False)
    cur = conn.cursor()
    cur.execute("SET session_replication_role = replica")  # no trigger/FK checks while loading
    cur.execute("SET synchronous_commit = off")
    cur.execute("SELECT count(*) FROM documents_document")
    if cur.fetchone()[0]:
        print("DB enthält bereits Dokumente – Abbruch (frische, migrierte DB erwartet)", file=sys.stderr)
        return 2

    def ct(app, model):
        cur.execute("SELECT id FROM django_content_type WHERE app_label=%s AND model=%s", (app, model))
        return cur.fetchone()[0]

    def perm(code):
        cur.execute("SELECT id FROM auth_permission WHERE codename=%s", (code,))
        return cur.fetchone()[0]

    CT_DOC = ct("documents", "document")
    CT = {m: ct("documents", m) for m in ("tag", "correspondent", "documenttype", "storagepath", "savedview")}
    P = {}
    for m in ("document", "tag", "correspondent", "documenttype", "storagepath", "savedview", "note",
              "customfield", "sharelink", "uisettings", "workflow", "mailaccount", "mailrule", "paperlesstask"):
        for act in ("view", "add", "change", "delete"):
            try:
                P[f"{act}_{m}"] = perm(f"{act}_{m}")
            except TypeError:
                pass

    def copy(table: str, cols: list[str], rows_iter, label=None):
        t0 = time.time()
        n = 0
        with cur.copy(f"COPY {table} ({', '.join(cols)}) FROM STDIN") as cp:
            buf = []
            for line in rows_iter:
                buf.append(line)
                n += 1
                if len(buf) >= 2000:
                    cp.write("".join(buf))
                    buf.clear()
            if buf:
                cp.write("".join(buf))
        dt = time.time() - t0
        stats[f"rows.{table}"] = n
        print(f"  {table}: {n} Zeilen in {dt:.1f}s", flush=True)
        return n

    # --- Users / groups -----------------------------------------------------------------------------
    print("Nutzer, Gruppen, Rechte …", flush=True)
    joined = "2015-01-01 00:00:00+00"
    users = list(range(3, 3 + a.users))  # 1 = AnonymousUser (migrate), 2 = admin
    restricted = users[RESTRICTED_RANK - 1]
    def user_rows():
        yield row(ADMIN_ID, UNUSABLE, None, True, "admin", "Ada", "Admin", "admin@scale.invalid", True, True, joined)
        for u in users:
            uname = "restricted" if u == restricted else f"user{u:03d}"
            yield row(u, UNUSABLE, None, False, uname, g.name(), g.name(), f"{uname}@scale.invalid", False, True, joined)
    copy("auth_user", ["id", "password", "last_login", "is_superuser", "username", "first_name", "last_name", "email", "is_staff", "is_active", "date_joined"], user_rows())
    groups = list(range(1, a.groups + 1))
    copy("auth_group", ["id", "name"], (row(gid, f"group-{gid:02d}-{g.name()}") for gid in groups))
    member: dict[int, list[int]] = {}
    ug = []
    for u in users:
        k = 2 if u == restricted else r.choice([1, 1, 2, 2, 3])
        member[u] = r.sample(groups, k)
        ug += [(u, gg) for gg in member[u]]
    copy("auth_user_groups", ["id", "user_id", "group_id"], (row(i, u, gg) for i, (u, gg) in enumerate(ug, 1)))
    # Model permissions: all groups view on the objects; 2/3 also add/change; groups of the restricted user full
    gp = []
    for gid in groups:
        full = gid in member[restricted] or gid % 3 != 0
        for m in ("document", "tag", "correspondent", "documenttype", "storagepath", "savedview", "note", "customfield", "uisettings", "sharelink"):
            gp.append((gid, P[f"view_{m}"]))
            if full:
                gp += [(gid, P[f"add_{m}"]), (gid, P[f"change_{m}"])]
        gp += [(gid, P["view_workflow"]), (gid, P["view_mailrule"]), (gid, P["view_mailaccount"]), (gid, P["view_paperlesstask"])]
    copy("auth_group_permissions", ["id", "group_id", "permission_id"], (row(i, gg, p) for i, (gg, p) in enumerate(gp, 1)))
    copy("authtoken_token", ["key", "created", "user_id"], iter([row(ADMIN_TOKEN, joined, ADMIN_ID), row(RESTRICTED_TOKEN, joined, restricted)]))

    owner_z = Zipf(a.users, 1.0)
    def pick_owner():
        x = r.random()
        if x < 0.30:
            return ADMIN_ID
        if x < 0.92:
            return users[owner_z.rank(r)]
        return None

    def obj_owner():  # Labels mostly without owner
        x = r.random()
        return None if x < 0.75 else (ADMIN_ID if x < 0.85 else users[owner_z.rank(r)])

    algos = [6] * 70 + [0] * 10 + [1] * 8 + [2] * 4 + [3] * 4 + [4] * 2 + [5] * 2

    # --- Labels ---------------------------------------------------------------------------------------
    print("Labels …", flush=True)
    obj_guardian_u, obj_guardian_g = [], []
    def label_rows(n, kind, extra=None):
        names = set()
        out = []
        for i in range(1, n + 1):
            while True:
                nm = g.name((2, 4)) + (" " + r.choice(["AG", "GmbH", "SA", "Ltd", "Inc", "KG", "& Co", "Versicherung", "Bank", "Treuhand", ""]) if kind == "correspondent" else "")
                nm = nm.strip()
                if nm.lower() not in names:
                    names.add(nm.lower())
                    break
            alg = r.choice(algos)
            match = "" if alg in (0, 6) else (nm.split()[0].lower() if alg != 4 else f"{nm.split()[0].lower()}.*")
            own = obj_owner()
            if own is not None and own != ADMIN_ID and r.random() < 0.5:
                obj_guardian_g.append((str(i), CT[kind], r.choice(groups), P[f"view_{kind}"]))
            out.append((i, nm, match, alg, own))
        return out

    corrs = label_rows(a.correspondents, "correspondent")
    copy("documents_correspondent", ["id", "name", "match", "matching_algorithm", "is_insensitive", "owner_id"],
         (row(i, nm, m, al, True, o) for i, nm, m, al, o in corrs))
    dtypes_raw = label_rows(a.doctypes, "documenttype")
    dt_words = ["Rechnung", "Invoice", "Vertrag", "Contract", "Kontoauszug", "Statement", "Police", "Policy", "Lohnausweis", "Payslip", "Brief", "Letter", "Quittung", "Receipt", "Mahnung", "Reminder", "Offerte", "Quote", "Bescheid", "Notice"]
    dtypes = [(i, (dt_words[i - 1] if i <= len(dt_words) else nm), m, al, o) for i, nm, m, al, o in dtypes_raw]
    copy("documents_documenttype", ["id", "name", "match", "matching_algorithm", "is_insensitive", "owner_id"],
         (row(i, nm, m, al, True, o) for i, nm, m, al, o in dtypes))
    templates = ["{{ correspondent }}/{{ created_year }}/{{ title }}", "{{ document_type }}/{{ created }} {{ title }}",
                 "{{ created_year }}/{{ created_month }}/{{ correspondent }} - {{ title }}", "{{ owner_username }}/{{ document_type }}/{{ title }}",
                 "archiv/{{ created_year }}/{{ asn }} {{ title }}", "{% if correspondent %}{{ correspondent }}{% else %}none{% endif %}/{{ title }}"]
    sps_raw = label_rows(a.storage_paths, "storagepath")
    sps = []
    for i, nm, m, al, o in sps_raw:
        if i == 1:
            sps.append((i, "Scale rename", "", 0, None, "sp1/{{ created_year }}/{{ doc_pk }}"))
        else:
            # Path matches the generated file name (sp<i>/<year>/<pk>.pdf): bulk edits do not move
            # anything, only the rename in the scenario "rename" (template variety: `templates`, not used)
            sps.append((i, nm, m, al, o, f"sp{i}/{{{{ created_year }}}}/{{{{ doc_pk }}}}"))
    copy("documents_storagepath", ["id", "name", "match", "matching_algorithm", "is_insensitive", "path", "owner_id"],
         (row(i, nm, m, al, True, p, o) for i, nm, m, al, o, p in sps))

    # Tags: 1 = inbox (root), 2..100 roots, rest with parents down to level 4
    tag_parent: dict[int, int | None] = {}
    tag_level: dict[int, int] = {}
    tag_names: dict[int, str] = {}
    tags_raw = label_rows(a.tags, "tag")
    roots = max(1, a.tags // 8)
    for i, nm, *_ in tags_raw:
        tag_names[i] = "Inbox" if i == 1 else nm
        if i <= roots:
            tag_parent[i], tag_level[i] = None, 1
        else:
            cands = [t for t in range(2, i) if tag_level[t] < 4]
            p = r.choice(cands)
            tag_parent[i], tag_level[i] = p, tag_level[p] + 1
    tree = tag_tree(list(tag_names), tag_parent, tag_names)
    tag_anc = {i: ([int(x) for x in tree[i]["anc"].split(",")] if tree[i]["anc"] else []) for i in tag_names}
    colors = ["#a6cee3", "#1f78b4", "#b2df8a", "#33a02c", "#fb9a99", "#e31a1c", "#fdbf6f", "#ff7f00", "#cab2d6", "#6a3d9a"]
    # Since pl_objects.0001_paperrust_tag_adjacency only the parent edge;
    # write the treenode fields only into a DB without this migration (pure v3.4.2 schema).
    cur.execute("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'documents_tag' "
                "AND column_name = 'tn_ancestors_pks')")
    treenode_cols = cur.fetchone()[0]
    def tag_rows():
        for i, nm, m, al, o in tags_raw:
            t = tree[i]
            core = (tag_names[i], m if i != 1 else "", al if i != 1 else 0, True, colors[i % len(colors)], i == 1,
                    o if i != 1 else None, tag_parent[i])
            if treenode_cols:
                yield row(i, t["anc"], t["anc_n"], t["ch"], t["ch_n"], t["depth"], t["desc"], t["desc_n"], t["index"], t["level"], 0,
                          t["order"], t["sib"], t["sib_n"], *core)
            else:
                yield row(i, *core)
    tree_cols = ["tn_ancestors_pks", "tn_ancestors_count", "tn_children_pks", "tn_children_count", "tn_depth",
                 "tn_descendants_pks", "tn_descendants_count", "tn_index", "tn_level", "tn_priority", "tn_order",
                 "tn_siblings_pks", "tn_siblings_count"] if treenode_cols else []
    copy("documents_tag", ["id", *tree_cols, "name", "match", "matching_algorithm", "is_insensitive",
                           "color", "is_inbox_tag", "owner_id", "tn_parent_id"], tag_rows())
    stats["tags.max_level"] = max(tag_level.values())

    # Custom Fields
    cfields = []
    for i, typ in enumerate(CF_TYPES, 1):
        extra = None
        if typ == "select":
            extra = json.dumps({"select_options": [{"id": f"opt{i:02d}{k:02d}" + "x" * 9, "label": g.name()} for k in range(r.randint(3, 12))]})
        elif typ == "monetary" and i % 2 == 0:
            extra = json.dumps({"default_currency": "CHF"})
        cfields.append((i, f"{typ.capitalize()} {g.name()}", typ, extra))
    copy("documents_customfield", ["id", "created", "name", "data_type", "extra_data"],
         (row(i, joined, nm, t, e) for i, nm, t, e in cfields))

    # --- Documents ------------------------------------------------------------------------------------
    print("Dokumente …", flush=True)
    corr_z, dt_z, sp_z, tag_z, cf_z = Zipf(a.correspondents, 1.1), Zipf(a.doctypes, 1.0), Zipf(a.storage_paths - 1, 1.0), Zipf(a.tags - 1, 1.0), Zipf(len(cfields), 0.6)
    tag_rank = list(range(2, a.tags + 1))
    r.shuffle(tag_rank)
    # Storage path 1: exactly rename-docs documents (evenly distributed)
    rename_ids = set(r.sample(range(1, N + 1), min(a.rename_docs, N)))
    media_ids: set[int] = set()
    while len(media_ids) < min(a.media_docs, N - len(rename_ids)):
        x = r.randint(1, N)
        if x not in rename_ids:
            media_ids.add(x)
    asn_ids = sorted(r.sample(range(1, N + 1), int(N * 0.6)))
    asn_of = {d: 1000 + k for k, d in enumerate(asn_ids)}  # ASN rises with the id (as assigned sequentially)

    doc_tags: list[tuple[int, int]] = []
    doc_meta: list[tuple[int, int | None]] = []  # (owner) for permissions
    cf_rows: list[str] = []
    note_rows: list[str] = []
    guard_u: list[tuple] = []
    guard_g: list[tuple] = []
    bytes_content = 0
    files_written = 0
    cf_id = itertools.count(1)
    note_id = itertools.count(1)

    def created_for(i: int) -> datetime:
        # growing stock: density ~ linearly rising over 15 years; id order ~ chronological with noise
        u = (i - 0.5) / N
        frac = math.sqrt(u)  # inverse CDF of the linear density
        d = start + timedelta(days=frac * span_days + r.uniform(-20, 20))
        return min(max(d, start), now)

    def content_for(lang, corr_name, dt_name, created: datetime, tags_here, asn):
        target = int(math.exp(r.uniform(math.log(500), math.log(5000))))
        city = r.choice(g.cities)
        street = r.choice(g.streets)
        if lang == "de":
            head = (f"{corr_name}\n{street} {r.randint(1, 200)}\n{city[0]} {city[1]}\n\n{city[1]}, {created.day}. {MONTHS_DE[created.month - 1]} {created.year}\n\n"
                    f"{dt_name} Nr. {dt_name[:2].upper()}-{created.year}-{r.randint(1, 999999):06d}\nKundennummer {r.randint(10000, 9999999)}\n\n")
            tail = f"\n\nBetrag: {r.randint(5, 25000)}.{r.randint(0, 99):02d} CHF\nIBAN CH{r.randint(10, 99)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(0, 9)}\nFreundliche Grüsse\n"
        else:
            head = (f"{corr_name}\n{r.randint(1, 200)} {street}\n{city[1]} {city[0]}\n\n{MONTHS_EN[created.month - 1]} {created.day}, {created.year}\n\n"
                    f"{dt_name} No. {dt_name[:2].upper()}-{created.year}-{r.randint(1, 999999):06d}\nCustomer number {r.randint(10000, 9999999)}\n\n")
            tail = f"\n\nTotal: EUR {r.randint(5, 25000)}.{r.randint(0, 99):02d}\nIBAN DE{r.randint(10, 99)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(1000, 9999)} {r.randint(10, 99)}\nKind regards\n"
        if asn is not None:
            tail += f"ASN{asn}\n"
        body_len = max(100, target - len(head) - len(tail))
        nwords = max(10, body_len // 7)
        ws = g.words(lang, nwords)
        # Sprinkle in tag names/keywords (matching/classifier have a signal)
        for t in tags_here[:3]:
            ws[r.randrange(len(ws))] = tag_names[t].split()[0].lower()
        lines, k = [], 0
        while k < len(ws):
            n = r.randint(6, 16)
            s = " ".join(ws[k:k + n])
            lines.append(s[:1].upper() + s[1:] + ".")
            k += n
        body = []
        for j in range(0, len(lines), r.randint(3, 6)):
            body.append(" ".join(lines[j:j + 5]))
        return head + "\n\n".join(body) + tail

    def doc_rows():
        nonlocal bytes_content, files_written
        for i in range(1, N + 1):
            created = created_for(i)
            added = created + timedelta(days=r.expovariate(1 / 10), seconds=r.randint(0, 86399))
            added = min(added, now)
            modified = added + timedelta(seconds=r.randint(0, 3600)) if r.random() < 0.7 else min(now, added + timedelta(days=r.uniform(0, 400)))
            lang = "de" if r.random() < 0.6 else "en"
            c = corrs[corr_z.rank(r)] if r.random() < 0.92 else None
            d = dtypes[dt_z.rank(r)] if r.random() < 0.88 else None
            if i in rename_ids:
                sp = 1
            elif r.random() < 0.55:
                sp = 2 + sp_z.rank(r)
            else:
                sp = None
            # Tags: count ~ geometric, mean ~3; set ancestors too
            ntag = min(8, int(r.expovariate(1 / 2.6)))
            tset = set()
            for _ in range(ntag):
                t = tag_rank[tag_z.rank(r)]
                tset.add(t)
                tset.update(tag_anc[t])
            inbox = (r.random() < 0.012) or (created > now - timedelta(days=60) and r.random() < 0.5)
            if inbox:
                tset.add(1)
            tags_here = sorted(tset)
            for t in tags_here:
                doc_tags.append((i, t))
            owner = pick_owner()
            asn = asn_of.get(i)
            content = content_for(lang, c[1] if c else g.name(), d[1] if d else "Dokument", created, [t for t in tags_here if t != 1], asn)
            bytes_content += len(content.encode())
            month = (MONTHS_DE if lang == "de" else MONTHS_EN)[created.month - 1]
            title = r.choice([
                f"{d[1] if d else 'Scan'} {c[1] if c else ''} {month} {created.year}",
                f"{c[1] if c else 'Brief'} {created:%Y-%m-%d}",
                " ".join(g.words(lang, r.randint(2, 7))).capitalize(),
                f"{d[1] if d else 'Dokument'} {r.randint(1, 99999)}",
            ]).replace("  ", " ").strip()[:128]
            ext = ".pdf"
            mime = "application/pdf"
            if sp == 1:
                filename = f"sp1/{created.year}/{i:07d}{ext}"
            elif sp is not None:
                filename = f"sp{sp}/{created.year}/{i:07d}{ext}"
            else:
                filename = f"{i:07d}{ext}"
            if i in media_ids or i in rename_ids:
                data = tiny_pdf(f"PaperRust scale document {i}")
                checksum = hashlib.sha256(data).hexdigest()
                p = media / "documents" / "originals" / filename
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(data)
                (media / "documents" / "thumbnails" / f"{i:07d}.webp").write_bytes(WEBP_1X1)
                files_written += 1
            else:
                checksum = hashlib.sha256(f"scale-{a.seed}-{i}".encode()).hexdigest()
            deleted = (now - timedelta(days=r.randint(1, 25))).isoformat() if r.random() < 0.003 else None
            # Object permissions (guardian) on ~20 %
            if r.random() < 0.20:
                for _ in range(r.randint(1, 3)):
                    change = r.random() < 0.5
                    if r.random() < 0.6:
                        gg = r.choice(groups)
                        guard_g.append((str(i), gg, P["view_document"]))
                        if change:
                            guard_g.append((str(i), gg, P["change_document"]))
                    else:
                        uu = users[r.randrange(len(users))]
                        guard_u.append((str(i), uu, P["view_document"]))
                        if change:
                            guard_u.append((str(i), uu, P["change_document"]))
            # Custom field instances ~3
            k = min(len(cfields), max(0, int(r.gauss(3, 1.3) + 0.5)))
            fids = set()
            while len(fids) < k:
                fids.add(1 + cf_z.rank(r))
            for f in sorted(fids):
                typ = cfields[f - 1][2]
                v = dict(text=None, b=None, url=None, dt=None, int=None, fl=None, mon=None, docs=None, sel=None, lt=None)
                if r.random() < 0.08:
                    pass  # empty value (null) possible as in the UI
                elif typ == "string":
                    v["text"] = " ".join(g.words(lang, r.randint(1, 4)))[:128]
                elif typ == "url":
                    v["url"] = f"https://example.invalid/{g.name()}/{r.randint(1, 99999)}"
                elif typ == "date":
                    v["dt"] = (created + timedelta(days=r.randint(-30, 400))).date().isoformat()
                elif typ == "boolean":
                    v["b"] = r.random() < 0.5
                elif typ == "integer":
                    v["int"] = r.randint(0, 100000)
                elif typ == "float":
                    v["fl"] = round(r.uniform(0, 1000), 3)
                elif typ == "monetary":
                    v["mon"] = f"{r.choice(['CHF', 'EUR', 'USD'])}{r.randint(1, 50000)}.{r.randint(0, 99):02d}"
                elif typ == "documentlink":
                    v["docs"] = json.dumps(sorted(r.sample(range(1, N + 1), r.randint(1, 3))))
                elif typ == "select":
                    opts = json.loads(cfields[f - 1][3])["select_options"]
                    v["sel"] = r.choice(opts)["id"]
                elif typ == "longtext":
                    v["lt"] = " ".join(g.words(lang, r.randint(10, 60)))
                cf_rows.append(row(next(cf_id), None, None, None, added.isoformat(), v["text"], v["b"], v["url"], v["dt"], v["int"], v["fl"], v["mon"], v["docs"], v["sel"], v["lt"], f, i))
            if r.random() < 0.05:
                for _ in range(r.randint(1, 3)):
                    note_rows.append(row(next(note_id), None, None, None, " ".join(g.words(lang, r.randint(4, 40))),
                                         (added + timedelta(days=r.uniform(0, 100))).isoformat(), i, r.choice(users + [ADMIN_ID])))
            if i % 100000 == 0:
                print(f"    … {i} Dokumente ({time.time() - t_docs:.0f}s)", flush=True)
            yield row(i, deleted, None, None, title, content, mime, checksum, None, r.randint(1, 12), created.date().isoformat(),
                      modified.isoformat(), added.isoformat(), filename, None, f"scan_{i:07d}.pdf", asn,
                      c[0] if c else None, d[0] if d else None, sp, owner, None, None, None)

    (media / "documents" / "thumbnails").mkdir(parents=True, exist_ok=True)
    t_docs = time.time()
    copy("documents_document", ["id", "deleted_at", "restored_at", "transaction_id", "title", "content", "mime_type", "checksum",
                                "archive_checksum", "page_count", "created", "modified", "added", "filename", "archive_filename",
                                "original_filename", "archive_serial_number", "correspondent_id", "document_type_id",
                                "storage_path_id", "owner_id", "root_document_id", "version_label", "version_index"], doc_rows())
    stats["content_bytes"] = bytes_content
    stats["media_files"] = files_written
    copy("documents_document_tags", ["id", "document_id", "tag_id"], (row(k, d, t) for k, (d, t) in enumerate(doc_tags, 1)))
    del doc_tags
    copy("documents_customfieldinstance", ["id", "deleted_at", "restored_at", "transaction_id", "created", "value_text", "value_bool",
                                           "value_url", "value_date", "value_int", "value_float", "value_monetary", "value_document_ids",
                                           "value_select", "value_long_text", "field_id", "document_id"], iter(cf_rows))
    del cf_rows
    copy("documents_note", ["id", "deleted_at", "restored_at", "transaction_id", "note", "created", "document_id", "user_id"], iter(note_rows))
    gu = sorted(set(guard_u))
    copy("guardian_userobjectpermission", ["id", "object_pk", "content_type_id", "permission_id", "user_id"],
         (row(k, o, CT_DOC, p, u) for k, (o, u, p) in enumerate(gu, 1)))
    def gg_rows():
        k = 0
        for t in sorted(set(guard_g)):
            k += 1
            yield row(k, t[0], CT_DOC, t[1], t[2])
        for o, ctid, gid, p in sorted(set(obj_guardian_g)):
            k += 1
            yield row(k, o, ctid, gid, p)
    copy("guardian_groupobjectpermission", ["id", "object_pk", "content_type_id", "group_id", "permission_id"], gg_rows())

    # --- Saved Views, UI-Settings ---------------------------------------------------------------------
    print("Saved Views, Workflows, Mail …", flush=True)
    sv, rules = [], []
    sorts = ["created", "added", "title", "correspondent__name", "document_type__name", "archive_serial_number", "modified", "num_notes", "custom_field_3", None]
    rid = itertools.count(1)
    for i in range(1, a.saved_views + 1):
        own = ADMIN_ID if i <= 40 else (restricted if i <= 55 else (None if i <= 70 else r.choice(users)))
        sv.append(row(i, f"View {i} {g.name()}", r.choice(sorts), r.random() < 0.7, r.choice([None, 10, 25, 50]),
                      r.choice([None, "table", "smallCards", "largeCards"]), None, own, ""))
        kinds = r.sample(["tag", "corr", "dt", "date", "inbox", "tc", "cfq", "tagin", "notag", "owner"], r.randint(1, 3))
        for kd in kinds:
            if kd == "tag":
                rules.append(row(next(rid), 6, tag_rank[tag_z.rank(r)], i))
            elif kd == "tagin":
                for _ in range(2):
                    rules.append(row(next(rid), 22, tag_rank[tag_z.rank(r)], i))
            elif kd == "notag":
                rules.append(row(next(rid), 17, tag_rank[tag_z.rank(r)], i))
            elif kd == "corr":
                rules.append(row(next(rid), 26, corrs[corr_z.rank(r)][0], i))
            elif kd == "dt":
                rules.append(row(next(rid), 4, dtypes[dt_z.rank(r)][0], i))
            elif kd == "date":
                rules.append(row(next(rid), 9, f"{r.randint(2012, 2025)}-01-01", i))
            elif kd == "inbox":
                rules.append(row(next(rid), 6, 1, i))
            elif kd == "tc":
                rules.append(row(next(rid), 19, r.choice(DE_WORDS[20:200] + EN_WORDS[20:200]), i))
            elif kd == "cfq":
                f = r.choice([c for c in cfields if c[2] in ("integer", "date", "monetary")])
                q = {"integer": [f[0], "gt", 50000], "date": [f[0], "gte", "2020-01-01"], "monetary": [f[0], "gt", 1000]}[f[2]]
                rules.append(row(next(rid), 42, json.dumps(q), i))
            elif kd == "owner":
                rules.append(row(next(rid), 33, ADMIN_ID, i))
    copy("documents_savedview", ["id", "name", "sort_field", "sort_reverse", "page_size", "display_mode", "display_fields", "owner_id", "icon"], iter(sv))
    copy("documents_savedviewfilterrule", ["id", "rule_type", "value", "saved_view_id"], iter(rules))
    ui = []
    for uid, ids in ((ADMIN_ID, list(range(1, 9))), (restricted, [41, 42, 43, 44, 56, 57])):
        ui.append(row(len(ui) + 1, json.dumps({"saved_views": {"dashboard_views_visible_ids": ids, "sidebar_views_visible_ids": ids}, "dark_mode": {"use_system": True}}), uid))
    copy("documents_uisettings", ["id", "settings", "user_id"], iter(ui))

    # --- Workflows (assign/remove only; scheduled ones disabled) -------------------------------------
    wf, wft, wfa, wf_t, wf_a, wfa_tags, wft_tags = [], [], [], [], [], [], []
    for i in range(1, a.workflows + 1):
        ttype = r.choice([1, 1, 2, 2, 3, 4])
        wf.append(row(i, f"Workflow {i} {g.name()}", i, ttype != 4))
        alg = r.choice([0, 0, 1, 3])
        wft.append(row(i, ttype, "1,2,3", None, r.choice([None, "*.pdf", "*scan*"]), "" if alg == 0 else r.choice(DE_WORDS[20:150]),
                       alg, True, None, 0, False, 1, "added", r.choice([None, None, corrs[corr_z.rank(r)][0]]), None, None, None, None))
        if r.random() < 0.3:
            wft_tags.append(row(len(wft_tags) + 1, i, tag_rank[tag_z.rank(r)]))
        wfa.append(row(i, r.choice([1, 1, 1, 2]), 0, r.choice([None, "{{ correspondent }} {{ document_type }}"]), None,
                       False, False, False, False, False, False, False,
                       r.choice([None, corrs[corr_z.rank(r)][0]]), r.choice([None, dtypes[dt_z.rank(r)][0]]), None, None,
                       None, None, None, False, False, None))
        wfa_tags.append(row(len(wfa_tags) + 1, i, tag_rank[tag_z.rank(r)]))
        wf_t.append(row(i, i, i))
        wf_a.append(row(i, i, i))
    copy("documents_workflow", ["id", "name", '"order"', "enabled"], iter(wf))
    copy("documents_workflowtrigger", ["id", "type", "sources", "filter_path", "filter_filename", "match", "matching_algorithm", "is_insensitive",
                                       "filter_custom_field_query", "schedule_offset_days", "schedule_is_recurring", "schedule_recurring_interval_days",
                                       "schedule_date_field", "filter_has_correspondent_id", "filter_has_document_type_id", "filter_has_storage_path_id",
                                       "filter_mailrule_id", "schedule_date_custom_field_id"], iter(wft))
    copy("documents_workflowaction", ["id", "type", '"order"', "assign_title", "assign_custom_fields_values", "remove_all_tags",
                                      "remove_all_document_types", "remove_all_correspondents", "remove_all_storage_paths", "remove_all_owners",
                                      "remove_all_permissions", "remove_all_custom_fields", "assign_correspondent_id", "assign_document_type_id",
                                      "assign_owner_id", "assign_storage_path_id", "email_id", "webhook_id", "passwords", "ai_create_missing",
                                      "ai_overwrite_existing", "ai_suggestion_fields"], iter(wfa))
    copy("documents_workflow_triggers", ["id", "workflow_id", "workflowtrigger_id"], iter(wf_t))
    copy("documents_workflow_actions", ["id", "workflow_id", "workflowaction_id"], iter(wf_a))
    copy("documents_workflowaction_assign_tags", ["id", "workflowaction_id", "tag_id"], iter(wfa_tags))
    copy("documents_workflowtrigger_filter_has_tags", ["id", "workflowtrigger_id", "tag_id"], iter(wft_tags))

    # --- Mail (Fake-Server, *.invalid) ----------------------------------------------------------------
    copy("paperless_mail_mailaccount", ["id", "name", "imap_server", "imap_port", "imap_security", "username", "password", "is_token",
                                        "character_set", "account_type", "refresh_token", "expiration", "owner_id"],
         (row(i, f"Konto {i}", f"imap{i}.scale.invalid", 993, 2, f"box{i}", "x", False, "UTF-8", 1, None, None, ADMIN_ID) for i in range(1, 6)))
    copy("paperless_mail_mailrule", ["id", "name", '"order"', "enabled", "folder", "filter_from", "filter_to", "filter_subject", "filter_body",
                                     "filter_attachment_filename_include", "filter_attachment_filename_exclude", "maximum_age", "attachment_type",
                                     "consumption_scope", "pdf_layout", "action", "action_parameter", "assign_title_from", "assign_correspondent_from",
                                     "assign_owner_from_rule", "account_id", "assign_correspondent_id", "assign_document_type_id", "owner_id", "stop_processing"],
         (row(i, f"Regel {i}", i, True, "INBOX", r.choice([None, "rechnung@scale.invalid"]), None, r.choice([None, "Rechnung", "Invoice"]), None,
              "*.pdf", None, 30, 1, 1, 0, 3, None, 1, 1, True, 1 + i % 5, None, None, ADMIN_ID, False) for i in range(1, 41)))

    # --- Sequences, completion -------------------------------------------------------------------------
    print("Sequenzen …", flush=True)
    cur.execute("""
        DO $$ DECLARE t record; s text; BEGIN
          FOR t IN SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
                   JOIN pg_attribute a ON a.attrelid=c.oid AND a.attname='id'
                   WHERE n.nspname='public' AND c.relkind='r' LOOP
            s := pg_get_serial_sequence(quote_ident(t.relname), 'id');
            IF s IS NOT NULL THEN
              EXECUTE format('SELECT setval(%L, GREATEST((SELECT coalesce(max(id),0) FROM %I), 1))', s, t.relname);
            END IF;
          END LOOP; END $$""")
    conn.commit()
    cur.execute("SET session_replication_role = origin")
    stats["t_load_s"] = round(time.time() - t_all, 1)
    if not a.skip_sync_rebuild:
        t0 = time.time()
        cur.execute("SELECT paperrust_sync_rebuild()")
        stats["sync_rebuild_rows"] = cur.fetchone()[0]
        conn.commit()
        stats["t_sync_rebuild_s"] = round(time.time() - t0, 1)
        print(f"  Sync-Rebuild: {stats['sync_rebuild_rows']} in {stats['t_sync_rebuild_s']}s", flush=True)
    conn.autocommit = True
    t0 = time.time()
    cur.execute("VACUUM (ANALYZE)")
    stats["t_vacuum_analyze_s"] = round(time.time() - t0, 1)
    stats["restricted_user_id"] = restricted
    stats["restricted_groups"] = member[restricted]
    stats["t_total_s"] = round(time.time() - t_all, 1)
    print(json.dumps(stats, indent=1))
    out = Path(a.media_root).parent / "gen-stats.json"
    out.write_text(json.dumps(stats, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
