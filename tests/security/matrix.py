#!/usr/bin/env python3
"""Rechte-Matrix: jede Route × Rolle × Objektzustand gegen einen Wegwerf-Server.

    bash tests/security/server.sh reset && bash tests/security/server.sh start
    SECURITY_URL=http://127.0.0.1:8370 SEC_DB=pl_sec_matrix python tests/security/matrix.py [-k route-teil]

Nur gegen eine Datenbank `pl_sec_*` (legt Nutzer, Tokens und Objekte an, löscht und ändert). Schreibt
results/release/security/matrix.{json,md}. Exit 0 = keine Abweichung der Klassen "kritisch" und
"funktion", 1 sonst.

Rollen
  anon       ohne Anmeldung
  inactive   dave (is_active=false, Token)
  none       carol (keine Modellrechte)
  reader     sec_reader: alle view_*-Rechte (documents, paperless_mail, auth user/group, auditlog,
             paperless.view_applicationconfiguration)
  editor     sec_editor: view/add/change/delete auf documents.* und paperless_mail.*, auditlog.view_logentry
  staff      sec_staff: is_staff + editor + auth.*_user/_group + paperless.*_applicationconfiguration
  superuser  admin
  Besitzer ist kein eigenes Konto, sondern der Objektzustand "own" (reader/editor/staff besitzen das Objekt).

Objektzustände (nur besitzerbezogene Objekte): own (Rolle ist Besitzer), foreign (admin besitzt, nicht
geteilt), sview (fremd, view-Recht für die Rolle), schange (fremd, view+change). Andere Objekte: any.

Erwartung (aus upstream documents/permissions.py, DRF und den PaperRust-Verträgen):
  A erlaubt (2xx/3xx außer Login-Umleitung, 400/409/412/422/429/501/502 = Rechteprüfung bestanden)
  D verweigert (401/403, Login-Umleitung)    H verborgen (404)    X D oder H    P öffentlich (kein 5xx)
Abweichungsklassen: kritisch (erwartet D/H/X, bekommen A), funktion (erwartet A, bekommen D/H),
info (D statt H oder umgekehrt), fehler (5xx).
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
import subprocess
import sys
import time
import uuid as uuidlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(REPO / "tests" / "parity" / "harness"))
from plharness.http import Response, Session  # noqa: E402

BASE = os.environ.get("SECURITY_URL", "http://127.0.0.1:8370").rstrip("/")
DB = os.environ.get("SEC_DB", "pl_sec_matrix")
if not DB.startswith("pl_sec_"):
    raise SystemExit("SEC_DB must start with pl_sec_ (throwaway database)")
ADMIN_TOKEN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"
TOKENS = {
    "inactive": "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005",
    "none": "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004",
    "reader": "5ec0000000000000000000000000000000000001",
    "editor": "5ec0000000000000000000000000000000000002",
    "staff": "5ec0000000000000000000000000000000000003",
    "superuser": ADMIN_TOKEN,
}
ROLES = ["anon", "inactive", "none", "reader", "editor", "staff", "superuser"]
OWNER_STATES = ["own", "foreign", "sview", "schange"]

# --------------------------------------------------------------------------- DB (only pl_sec_*)


def _env() -> dict:
    out = subprocess.run(["bash", "-c", f"source '{REPO}/reference/env.sh' >/dev/null 2>&1; env"],
                         capture_output=True, text=True).stdout
    return dict(line.split("=", 1) for line in out.splitlines() if "=" in line)


ENV = _env()


def sql(q: str) -> str:
    pg_bin = ENV["PG_BIN"]
    if len(pg_bin) > 2 and pg_bin[0] == "/" and pg_bin[2] == "/":  # Git Bash path /c/... -> C:/...
        pg_bin = f"{pg_bin[1].upper()}:{pg_bin[2:]}"
    psql = str(Path(pg_bin) / "psql")
    r = subprocess.run([psql, "-h", "127.0.0.1", "-p", ENV.get("PGPORT", "54329"), "-U", ENV["PG_SUPERUSER"],
                        "-d", DB, "-v", "ON_ERROR_STOP=1", "-Atq", "-c", q],
                       capture_output=True, text=True, env={**os.environ, "PGPASSWORD": ENV["PG_SUPERPASS"]})
    if r.returncode != 0:
        raise RuntimeError(f"sql failed: {r.stderr.strip()[:300]}")
    return r.stdout.strip()


ROLE_PERMS = {
    "reader": "(ct.app_label in ('documents','paperless_mail') and p.codename like 'view\\_%') "
              "or (ct.app_label='auth' and p.codename in ('view_user','view_group')) "
              "or (ct.app_label='auditlog' and p.codename='view_logentry') "
              "or (ct.app_label='paperless' and p.codename='view_applicationconfiguration')",
    "editor": "(ct.app_label in ('documents','paperless_mail')) "
              "or (ct.app_label='auditlog' and p.codename='view_logentry')",
    "staff": "(ct.app_label in ('documents','paperless_mail')) "
             "or (ct.app_label='auditlog' and p.codename='view_logentry') "
             "or (ct.app_label='auth' and p.codename ~ '_(user|group)$') "
             "or (ct.app_label='paperless' and p.codename like '%applicationconfiguration')",
}


def setup_users() -> dict[str, int]:
    ids: dict[str, int] = {}
    for role in ("reader", "editor", "staff"):
        name = f"sec_{role}"
        staff = "true" if role == "staff" else "false"
        sql(f"insert into auth_user (password,last_login,is_superuser,username,first_name,last_name,email,is_staff,"
            f"is_active,date_joined) values ('!',null,false,'{name}','','','{name}@example.invalid',{staff},true,now()) "
            f"on conflict (username) do nothing")
        uid = int(sql(f"select id from auth_user where username='{name}'"))
        ids[role] = uid
        sql(f"delete from auth_user_user_permissions where user_id={uid}")
        sql(f"insert into auth_user_user_permissions (user_id, permission_id) select {uid}, p.id from auth_permission p "
            f"join django_content_type ct on ct.id=p.content_type_id where {ROLE_PERMS[role]}")
    restore_tokens(ids)
    ids["none"] = int(sql("select id from auth_user where username='carol'"))
    ids["inactive"] = int(sql("select id from auth_user where username='dave'"))
    ids["superuser"] = int(sql("select id from auth_user where username='admin'"))
    ids["alice"] = int(sql("select id from auth_user where username='alice'"))
    return ids


def restore_tokens(ids: dict[str, int]) -> None:
    for role in ("reader", "editor", "staff"):
        uid = ids[role]
        sql(f"delete from authtoken_token where user_id={uid}")
        sql(f"insert into authtoken_token (key, created, user_id) values ('{TOKENS[role]}', now(), {uid})")
    for name, key in (("admin", ADMIN_TOKEN), ("carol", TOKENS["none"]), ("dave", TOKENS["inactive"])):
        sql(f"delete from authtoken_token where user_id=(select id from auth_user where username='{name}')")
        sql(f"insert into authtoken_token (key, created, user_id) select '{key}', now(), id from auth_user "
            f"where username='{name}'")


# --------------------------------------------------------------------------- HTTP


def sess(role: str) -> Session:
    s = Session(BASE, api_version="10", timeout=60)
    if role != "anon":
        s.auth = ("token", TOKENS[role])
    return s


ADMIN = None  # type: ignore


def admin() -> Session:
    global ADMIN
    if ADMIN is None:
        ADMIN = sess("superuser")
    return ADMIN


def must(r: Response, what: str) -> Any:
    if r.status >= 300:
        raise RuntimeError(f"fixture {what}: HTTP {r.status} {r.text[:300]}")
    return r.json()


def uniq(p: str = "sec") -> str:
    return f"{p}-{uuidlib.uuid4().hex[:10]}"


# --------------------------------------------------------------------------- Fixtures


@dataclass
class Ctx:
    ids: dict[str, int]
    docs: list[int] = field(default_factory=list)
    v2_uuid: dict[tuple[str, int], str] = field(default_factory=dict)
    acc: int = 0
    tag: int = 0
    cur: tuple[str, str] = ("own", "superuser")


def perms_for(state: str, uid: int) -> dict:
    view = [uid] if state in ("sview", "schange") else []
    change = [uid] if state == "schange" else []
    return {"view": {"users": view, "groups": []}, "change": {"users": change, "groups": []}}


def owner_for(state: str, uid: int, ctx: Ctx) -> int | None:
    return uid if state == "own" else ctx.ids["superuser"]


LABEL_BODY: dict[str, Callable[[Ctx], dict]] = {
    "tags": lambda c: {"name": uniq("tag"), "matching_algorithm": 0},
    "correspondents": lambda c: {"name": uniq("corr"), "matching_algorithm": 0},
    "document_types": lambda c: {"name": uniq("dt"), "matching_algorithm": 0},
    "storage_paths": lambda c: {"name": uniq("sp"), "path": "sec/{{ title }}", "matching_algorithm": 0},
    "saved_views": lambda c: {"name": uniq("sv"), "show_on_dashboard": False, "show_in_sidebar": False,
                              "sort_field": "created", "sort_reverse": True, "filter_rules": []},
    "mail_accounts": lambda c: {"name": uniq("acc"), "imap_server": "127.0.0.1", "imap_port": 9, "imap_security": 1,
                                "username": "u", "password": "secret-local-only", "character_set": "UTF-8"},
    "mail_rules": lambda c: {"name": uniq("rule"), "account": c.acc, "folder": "INBOX", "action": 3, "order": 0,
                             "maximum_age": 30, "attachment_type": 1, "consumption_scope": 1, "pdf_layout": 0,
                             "assign_title_from": 1, "assign_correspondent_from": 1, "assign_tags": []},
    "custom_fields": lambda c: {"name": uniq("cf"), "data_type": "string"},
    "groups": lambda c: {"name": uniq("grp"), "permissions": []},
    "users": lambda c: {"username": uniq("usr"), "password": "Sec-Local-Only-123!", "is_active": True},
    "workflows": lambda c: {"name": uniq("wf"), "order": 0, "enabled": False,
                            "triggers": [{"type": 1, "sources": [1, 2, 3], "filter_filename": "sec-none-*"}],
                            "actions": [{"type": 1, "assign_title": "x"}]},
    "workflow_triggers": lambda c: {"type": 1, "sources": [1], "filter_filename": "sec-none-*"},
    "workflow_actions": lambda c: {"type": 1, "assign_title": "x"},
}
OWNER_AWARE = {"tags", "correspondents", "document_types", "storage_paths", "saved_views", "mail_accounts",
               "mail_rules", "documents", "share_links", "share_link_bundles"}


def make(kind: str, ctx: Ctx, state: str, role: str) -> int:
    """Fresh object of `kind` in `state` relative to `role` (created by admin)."""
    uid = ctx.ids.get(role, ctx.ids["alice"])
    if kind == "documents":
        own = owner_for(state, uid, ctx) if role != "superuser" else ctx.ids["alice"]
        body = {"owner": own, "set_permissions": perms_for(state, uid) if role != "superuser" else perms_for("foreign", 0)}
        for _ in range(len(ctx.docs)):
            d = ctx.docs[hash((state, role, time.time_ns())) % len(ctx.docs)]
            r = admin().request("PATCH", f"/api/documents/{d}/", json_body=body)
            if r.status == 404 and len(ctx.docs) > 3:
                # became a version through merge_as_versions or was deleted: take it from the stock
                ctx.docs.remove(d)
                continue
            must(r, f"doc {d} {state}")
            return d
        raise RuntimeError("document pool exhausted")
    if kind in ("share_links", "share_link_bundles"):
        d = make("documents", ctx, "own", "superuser")
        if kind == "share_links":
            oid = must(admin().post("/api/share_links/", json_body={"document": d, "file_version": "original"}), kind)["id"]
            table = "documents_sharelink"
        else:
            oid = must(admin().post("/api/share_link_bundles/", json_body={"document_ids": [d]}), kind)["id"]
            table = "documents_sharelinkbundle"
        own = uid if state == "own" else ctx.ids["superuser"]
        sql(f"update {table} set owner_id={own} where id={oid}")
        return oid
    body = LABEL_BODY[kind](ctx)
    if kind in OWNER_AWARE and role != "superuser":
        body["owner"] = owner_for(state, uid, ctx)
        body["set_permissions"] = perms_for(state, uid)
    elif kind in OWNER_AWARE:
        body["owner"] = ctx.ids["alice"]
    return must(admin().post(f"/api/{kind}/", json_body=body), f"create {kind}")["id"]


def v2_uuid(ctx: Ctx, plural: str, legacy: int) -> str:
    key = (plural, legacy)
    if key in ctx.v2_uuid:
        return ctx.v2_uuid[key]
    cursor = None
    for _ in range(200):
        q = {"limit": 200}
        if cursor:
            q["cursor"] = cursor
        j = must(admin().get(f"/rs/api/v2/{plural}", query=q, api_version=False), f"v2 {plural}")
        for it in j.get("items", []):
            ctx.v2_uuid[(plural, it.get("legacy_id"))] = it["uuid"]
        if key in ctx.v2_uuid or not j.get("next_cursor"):
            break
        cursor = j["next_cursor"]
    return ctx.v2_uuid.get(key, "00000000-0000-0000-0000-000000000000")


# --------------------------------------------------------------------------- Rules

R_PERMS: dict[str, set[str]] = {}


def role_perms() -> None:
    for role, where in ROLE_PERMS.items():
        rows = sql(f"select ct.app_label||'.'||p.codename from auth_permission p join django_content_type ct "
                   f"on ct.id=p.content_type_id where {where}")
        R_PERMS[role] = set(rows.splitlines())
    R_PERMS["none"] = set()
    R_PERMS["inactive"] = set()
    R_PERMS["anon"] = set()


def active(role: str) -> bool:
    return role not in ("anon", "inactive")


def has(role: str, *perms: str) -> bool:
    if role == "superuser":
        return True
    return active(role) and all(p in R_PERMS.get(role, set()) for p in perms)


def staff(role: str) -> bool:
    return role in ("staff", "superuser")


Rule = Callable[[str, str], str]


def public(role: str, state: str) -> str:
    return "P"


def auth(role: str, state: str) -> str:
    return "A" if active(role) else "D"


def su(role: str, state: str) -> str:
    return "A" if role == "superuser" else "D"


def is_staff(role: str, state: str) -> str:
    return "A" if staff(role) else "D"


def model(*perms: str) -> Rule:
    return lambda role, state: "A" if has(role, *perms) else "D"


def obj(perms: tuple[str, ...], action: str, foreign: str = "H", denied: str = "D") -> Rule:
    """Model perms, then owner/guardian object rule (upstream has_object_permission)."""

    def r(role: str, state: str) -> str:
        if not has(role, *perms):
            return "D"
        if role == "superuser" or state in ("own", "any"):
            return "A"
        if state == "foreign":
            return foreign
        if action == "view":
            return "A"
        if action == "change":
            return "A" if state == "schange" else denied
        return denied  # delete / owner-only

    return r


def doc_op(extra: tuple[str, ...] = (), owner_only: bool = False, foreign: str = "D") -> Rule:
    """DocumentOperationPermissionMixin: change_document global + change on every doc (owner for some)."""

    def r(role: str, state: str) -> str:
        if not active(role):
            return "D"
        if role == "superuser":
            return "A"
        if not has(role, "documents.change_document", *extra):
            return "D"
        if state == "own":
            return "A"
        if owner_only:
            return "D"
        return "A" if state == "schange" else foreign

    return r


def owner_change(perms: tuple[str, ...]) -> Rule:
    """Model permissions, then has_perms_owner_aware(change_document) in the handler (403 instead of 404)."""

    def r(role: str, state: str) -> str:
        if not has(role, *perms):
            return "D"
        if role == "superuser" or state in ("own", "schange"):
            return "A"
        return "D" if state == "sview" else "X"

    return r


MATCHING_KINDS = {"tags", "correspondents", "document_types", "storage_paths"}


def doc_view_sel(role: str, state: str) -> str:
    """ViewDocumentsPermissions + selection of visible documents only (selection_data, bulk_download)."""
    if not has(role, "documents.view_document"):
        return "D"
    if role == "superuser" or state != "foreign":
        return "A"
    return "X"


def owner_or_su(role: str, state: str) -> str:
    return "A" if role == "superuser" or (active(role) and state == "own") else "D"


@dataclass
class Route:
    id: str
    method: str
    path: str
    rule: Rule
    kind: str | None = None  # fixture kind for {id}
    body: Callable[[Ctx, int], Any] | None = None
    query: Callable[[Ctx, int], dict] | None = None
    form: bool = False
    states: list[str] | None = None
    roles: list[str] | None = None
    note: str = ""


def viewset(name: str, perm_model: str, kind: str | None = None, owner_aware: bool | None = None,
            app: str = "documents", pk: str = "id", skip: tuple = ()) -> list[Route]:
    kind = kind or name
    oa = (kind in OWNER_AWARE) if owner_aware is None else owner_aware
    st = OWNER_STATES if oa else ["any"]
    p = lambda a: f"{app}.{a}_{perm_model}"  # noqa: E731
    base = f"/api/{name}/"
    det = f"/api/{name}/{{{pk}}}/"
    out = [
        Route(f"{name}.list", "GET", base, model(p("view"))),
        Route(f"{name}.create", "POST", base, model(p("add")), body=lambda c, i, k=kind: LABEL_BODY[k](c)),
        Route(f"{name}.retrieve", "GET", det, obj((p("view"),), "view"), kind=kind, states=st),
        Route(f"{name}.update", "PUT", det,
              obj((p("change"),), "change") if kind not in MATCHING_KINDS else
              (lambda r, s, f=obj((p("change"),), "change"): "D" if s == "schange" and r != "superuser" else f(r, s)),
              kind=kind, states=st, body=lambda c, i, k=kind: LABEL_BODY[k](c),
              note=("PUT ohne owner setzt owner=None (DRF-Standard None wegen unique(name, owner)) -> "
                    "nur Besitzer/Superuser") if kind in MATCHING_KINDS else ""),
        Route(f"{name}.partial_update", "PATCH", det, obj((p("change"),), "change"), kind=kind, states=st,
              body=lambda c, i: {}),
        Route(f"{name}.destroy", "DELETE", det, obj((p("delete"),), "delete"), kind=kind, states=st),
    ]
    return [r for r in out if r.id.split(".")[-1] not in skip]


V = "documents.view_document"
C = "documents.change_document"
ADD = "documents.add_document"
DEL = "documents.delete_document"


def doc_routes() -> list[Route]:
    D = "documents"
    S = OWNER_STATES
    det = lambda sub: f"/api/documents/{{id}}/{sub}"  # noqa: E731
    view_obj = obj((V,), "view", foreign="X")
    pdftool = obj((ADD, C), "change", foreign="X")
    return [
        Route("documents.list", "GET", "/api/documents/", model(V)),
        Route("documents.retrieve", "GET", "/api/documents/{id}/", obj((V,), "view"), D, states=S),
        Route("documents.update", "PUT", "/api/documents/{id}/", obj((C,), "change"), D, states=S,
              body=lambda c, i: {"title": f"sec put {i}"}),
        Route("documents.partial_update", "PATCH", "/api/documents/{id}/", obj((C,), "change"), D, states=S,
              body=lambda c, i: {"title": f"sec patch {i}"}),
        Route("documents.destroy", "DELETE", "/api/documents/{id}/", obj((DEL,), "delete"), D, states=S),
        Route("documents.download", "GET", det("download/"), view_obj, D, states=S),
        Route("documents.preview", "GET", det("preview/"), view_obj, D, states=S),
        Route("documents.thumb", "GET", det("thumb/"), view_obj, D, states=S),
        Route("documents.metadata", "GET", det("metadata/"), view_obj, D, states=S),
        Route("documents.suggestions", "GET", det("suggestions/"), owner_change((V,)), D, states=S,
              note="upstream: has_perms_owner_aware(change_document)"),
        Route("documents.root", "GET", det("root/"), view_obj, D, states=S),
        Route("documents.share_links", "GET", det("share_links/"), owner_change((V,)), D, states=S,
              note="upstream: has_perms_owner_aware(change_document)"),
        Route("documents.history", "GET", det("history/"),
              lambda r, s: ("A" if r == "superuser" or (has(r, V, "auditlog.view_logentry") and s == "own")
                            else ("D" if not has(r, V) or s != "foreign" else "X")), D, states=S,
              note="upstream: view_logentry und Besitzer (fremd geteilt -> 403)"),
        Route("documents.notes.list", "GET", det("notes/"), obj((V, "documents.view_note"), "view", foreign="X"), D,
              states=S),
        Route("documents.notes.create", "POST", det("notes/"),
              obj((V, C, "documents.add_note"), "change", foreign="X"), D, states=S,
              body=lambda c, i: {"note": "sec note"}),
        Route("documents.notes.destroy", "DELETE", det("notes/"),
              obj((V, C, "documents.delete_note"), "change", foreign="X"), D, states=S,
              query=lambda c, i: {"id": add_note(i)}),
        Route("documents.email_document", "POST", det("email/"), view_obj, D, states=S,
              body=lambda c, i: {"addresses": "sec@example.invalid", "subject": "s", "message": "m"},
              note="Mailversand ist im Testserver nicht eingerichtet (400 = Rechte bestanden)"),
        Route("documents.convert_pdf", "POST", det("convert_pdf/"), pdftool, D, states=S, body=lambda c, i: {}),
        Route("documents.redact", "POST", det("redact/"), pdftool, D, states=S,
              body=lambda c, i: {"terms": ["sec-not-found-term"]}),
        Route("documents.redact_search", "POST", det("redact/search/"), pdftool, D, states=S,
              body=lambda c, i: {"terms": ["sec"]}),
        Route("documents.shrink", "POST", det("shrink/"), pdftool, D, states=S, body=lambda c, i: {}),
        Route("documents.update_version", "POST", det("update_version/"),
              obj((ADD, C), "change", foreign="X"), D, states=S, form=True),
        Route("documents.versions.partial_update", "PATCH", "/api/documents/{id}/versions/{id}/",
              obj((C,), "change", foreign="X"), D, states=S, body=lambda c, i: {"version_label": "sec"},
              note="Version = Wurzel selbst (Rechteprüfung vor Versionsprüfung)"),
        Route("documents.versions.destroy", "DELETE", "/api/documents/{id}/versions/{id}/",
              obj((DEL,), "delete", foreign="X"), D, states=S,
              note="Wurzelversion löschen -> 400 nach bestandener Rechteprüfung"),
        Route("documents.ai_suggestions", "GET", det("ai_suggestions/"), owner_change((V,)), D,
              states=S, note="KI aus: 400 nach bestandener Rechteprüfung"),
        Route("documents.bulk_edit", "POST", "/api/documents/bulk_edit/", doc_op(), D, states=S,
              body=lambda c, i: {"documents": [i], "method": "add_tag", "parameters": {"tag": c.tag}}),
        Route("documents.delete", "POST", "/api/documents/delete/", doc_op(owner_only=True), D, states=S,
              body=lambda c, i: {"documents": [i]}),
        Route("documents.rotate", "POST", "/api/documents/rotate/", doc_op(owner_only=True), D, states=S,
              body=lambda c, i: {"documents": [i], "degrees": 90}),
        Route("documents.merge", "POST", "/api/documents/merge/", doc_op(), D, states=S,
              body=lambda c, i: {"documents": [i]}),
        Route("documents.merge_as_versions", "POST", "/api/documents/merge_as_versions/", doc_op(owner_only=True), D,
              states=S, body=lambda c, i: {"documents": [i, second_doc(c, i)], "root_document_id": i}),
        Route("documents.reprocess", "POST", "/api/documents/reprocess/", doc_op(), D, states=S,
              body=lambda c, i: {"documents": [i]}),
        Route("documents.edit_pdf", "POST", "/api/documents/edit_pdf/", doc_op(owner_only=True), D, states=S,
              body=lambda c, i: {"documents": [i], "operations": [{"page": 1, "rotate": 90}]}),
        Route("documents.remove_password", "POST", "/api/documents/remove_password/", doc_op(owner_only=True), D,
              states=S, body=lambda c, i: {"documents": [i], "password": "x"}),
        Route("documents.selection_data", "POST", "/api/documents/selection_data/", doc_view_sel, D, states=S,
              body=lambda c, i: {"documents": [i]}),
        Route("documents.bulk_download", "POST", "/api/documents/bulk_download/", doc_view_sel, D, states=S,
              body=lambda c, i: {"documents": [i], "content": "originals"}),
        Route("documents.email", "POST", "/api/documents/email/", doc_view_sel, D, states=S,
              body=lambda c, i: {"documents": [i], "addresses": "sec@example.invalid", "subject": "s",
                                 "message": "m"}),
        Route("documents.chat", "POST", "/api/documents/chat/",
              public, D, states=S, body=lambda c, i: {"q": "hi", "document_id": i},
              note="KI im Testserver aus: Prüfung 'KI aktiv' kommt vor der Objektprüfung, Rechte hier nicht prüfbar"),
        Route("documents.next_asn", "GET", "/api/documents/next_asn/", model(V)),
        Route("documents.post_document", "POST", "/api/documents/post_document/", model(ADD), form=True),
        Route("ui.fetch_doc", "GET", "/fetch/doc/{id}", public, D, states=["foreign"],
              note="upstream: öffentliche Umleitung auf /api/documents/{id}/download/"),
        Route("ui.fetch_preview", "GET", "/fetch/preview/{id}", public, D, states=["foreign"]),
        Route("ui.fetch_thumb", "GET", "/fetch/thumb/{id}", public, D, states=["foreign"]),
    ]


NOTE_SEQ = itertools.count()


def second_doc(c: "Ctx", first: int) -> int:
    """Second document in the same state as the current one (for operations with 2 documents)."""
    state, role = c.cur
    for _ in range(20):
        d = make("documents", c, state, role)
        if d != first:
            return d
    return first


def add_note(doc: int) -> int:
    r = admin().post(f"/api/documents/{doc}/notes/", json_body={"note": f"sec fixture {next(NOTE_SEQ)}"})
    notes = must(r, "note")
    return max(n["id"] for n in notes) if isinstance(notes, list) and notes else 0


def v1_routes() -> list[Route]:
    S = OWNER_STATES
    out: list[Route] = []
    out += viewset("tags", "tag")
    out += viewset("correspondents", "correspondent")
    out += viewset("document_types", "documenttype")
    out += viewset("storage_paths", "storagepath")
    out += viewset("saved_views", "savedview")
    out += viewset("custom_fields", "customfield")
    out += viewset("workflows", "workflow")
    out += viewset("workflow_triggers", "workflowtrigger")
    out += viewset("workflow_actions", "workflowaction")
    out += viewset("mail_accounts", "mailaccount", app="paperless_mail")
    out += viewset("mail_rules", "mailrule", app="paperless_mail")
    out += viewset("groups", "group", app="auth")
    out += viewset("users", "user", app="auth")
    out += doc_routes()
    sl = "documents.%s_sharelink"
    out += [
        Route("share_links.list", "GET", "/api/share_links/", model(sl % "view")),
        Route("share_links.create", "POST", "/api/share_links/", obj((sl % "add",), "view", foreign="X"),
              "documents", states=S, body=lambda c, i: {"document": i, "file_version": "original"},
              note="upstream: add_sharelink; Dokument muss sichtbar sein"),
        Route("share_links.retrieve", "GET", "/api/share_links/{id}/", obj((sl % "view",), "view"), "share_links",
              states=["own", "foreign"]),
        Route("share_links.destroy", "DELETE", "/api/share_links/{id}/", obj((sl % "delete",), "delete"),
              "share_links", states=["own", "foreign"]),
    ]
    b = "documents.%s_sharelinkbundle"
    out += [
        Route("share_link_bundles.list", "GET", "/api/share_link_bundles/", model(b % "view")),
        Route("share_link_bundles.create", "POST", "/api/share_link_bundles/",
              lambda r, s: "D" if not has(r, b % "add", V) else ("A" if r == "superuser" or s != "foreign" else "X"),
              "documents", states=S, body=lambda c, i: {"document_ids": [i]}),
        Route("share_link_bundles.retrieve", "GET", "/api/share_link_bundles/{id}/", obj((b % "view",), "view"),
              "share_link_bundles", states=["own", "foreign"]),
        Route("share_link_bundles.rebuild", "POST", "/api/share_link_bundles/{id}/rebuild/",
              obj((b % "add",), "delete"), "share_link_bundles", states=["own", "foreign"]),
        Route("share_link_bundles.destroy", "DELETE", "/api/share_link_bundles/{id}/", obj((b % "delete",), "delete"),
              "share_link_bundles", states=["own", "foreign"]),
    ]
    pm = "paperless_mail.%s_processedmail"
    out += [
        Route("processed_mail.list", "GET", "/api/processed_mail/", model(pm % "view")),
        Route("processed_mail.retrieve", "GET", "/api/processed_mail/{id}/", obj((pm % "view",), "view"),
              "processed_mail", states=["own", "foreign"]),
        Route("processed_mail.bulk_delete", "POST", "/api/processed_mail/bulk_delete/", model(pm % "delete"),
              body=lambda c, i: {"mail_ids": [99999999]}),
        Route("mail_accounts.test", "POST", "/api/mail_accounts/test/", model("paperless_mail.add_mailaccount"),
              body=lambda c, i: {"imap_server": "127.0.0.1", "imap_port": 9, "imap_security": 1, "username": "u",
                                 "password": "p"},
              note="upstream: neue Verbindung braucht add_mailaccount (SSRF-Thema, siehe Bericht)"),
        Route("mail_accounts.process", "POST", "/api/mail_accounts/{id}/process/",
              obj(("paperless_mail.add_mailaccount",), "delete"), "mail_accounts", states=S),
    ]
    t = "documents.%s_paperlesstask"
    out += [
        Route("tasks.list", "GET", "/api/tasks/", model(t % "view")),
        Route("tasks.retrieve", "GET", "/api/tasks/{pk}/",
              lambda r, s: "D" if not has(r, t % "view") else ("A" if r == "superuser" or s == "own" else "H"),
              "tasks", states=["own", "foreign"],
              note="upstream: Objektprüfung PaperlessObjectPermissions -> fremde Aufgabe 404 auch für staff"),
        Route("tasks.active", "GET", "/api/tasks/active/", model(t % "view")),
        Route("tasks.status_counts", "GET", "/api/tasks/status_counts/", model(t % "view")),
        Route("tasks.summary", "GET", "/api/tasks/summary/",
              lambda r, s: "A" if staff(r) or has(r, t % "view") else "D"),
        Route("tasks.acknowledge", "POST", "/api/tasks/acknowledge/", model(t % "change"),
              body=lambda c, i: {"tasks": [99999999]}),
        Route("tasks.run", "POST", "/api/tasks/run/", lambda r, s: "A" if r == "superuser" else "D",
              body=lambda c, i: {"task_type": "sanity_check"}),
        Route("trash.list", "GET", "/api/trash/", model(V)),
        Route("trash.action", "POST", "/api/trash/", model(DEL),
              body=lambda c, i: {"action": "restore", "documents": [99999999]}),
        Route("ui_settings.get", "GET", "/api/ui_settings/", model("documents.view_uisettings")),
        Route("ui_settings.post", "POST", "/api/ui_settings/", model("documents.add_uisettings"),
              body=lambda c, i: {"settings": {"sec": True}}),
        Route("bulk_edit_objects", "POST", "/api/bulk_edit_objects/",
              lambda r, s: "A" if r == "superuser" or (has(r, "documents.change_tag") and s == "own") else "D", "tags",
              states=S, body=lambda c, i: {"objects": [i], "object_type": "tags", "operation": "set_permissions",
                                           "permissions": perms_for("foreign", 0), "merge": True},
              note="upstream: set_permissions nur Besitzer/Superuser"),
        Route("storage_paths.test", "POST", "/api/storage_paths/test/", model(V),
              body=lambda c, i: {"document": c.docs[0], "path": "{{ title }}"}),
        Route("search.global", "GET", "/api/search/", auth, query=lambda c, i: {"query": "invoice"}),
        Route("search.autocomplete", "GET", "/api/search/autocomplete/", model(V), query=lambda c, i: {"term": "inv"}),
        Route("statistics.get", "GET", "/api/statistics/", auth),
        Route("status.get", "GET", "/api/status/", lambda r, s: "A" if staff(r) else "D"),
        Route("logs.list", "GET", "/api/logs/", is_staff),
        Route("logs.retrieve", "GET", "/api/logs/{pk}/", lambda r, s: "AH" if staff(r) else "D", kind="log",
              states=["any"], note="Logdatei fehlt im Testserver evtl. -> 404"),
        Route("config.list", "GET", "/api/config/", model("paperless.view_applicationconfiguration")),
        Route("config.retrieve", "GET", "/api/config/{pk}/", model("paperless.view_applicationconfiguration"),
              "config", states=["any"]),
        Route("config.create", "POST", "/api/config/", model("paperless.add_applicationconfiguration"),
              body=lambda c, i: {}, note="upstream: 405 nach bestandener Rechteprüfung"),
        Route("config.update", "PUT", "/api/config/{pk}/", model("paperless.change_applicationconfiguration"),
              "config", states=["any"], body=lambda c, i: {}),
        Route("config.partial_update", "PATCH", "/api/config/{pk}/",
              model("paperless.change_applicationconfiguration"), "config", states=["any"], body=lambda c, i: {}),
        Route("config.destroy", "DELETE", "/api/config/{pk}/",
              lambda r, s: "AH" if has(r, "paperless.delete_applicationconfiguration") else "D",
              "config", states=["any"], body=None, note="läuft zuletzt"),
        Route("profile.get", "GET", "/api/profile/", auth),
        Route("profile.patch", "PATCH", "/api/profile/", auth, body=lambda c, i: {"first_name": "Sec"}),
        Route("profile.social_account_providers", "GET", "/api/profile/social_account_providers/", auth),
        Route("profile.disconnect_social_account", "POST", "/api/profile/disconnect_social_account/", auth,
              body=lambda c, i: {"id": 99999999}),
        Route("profile.totp.get", "GET", "/api/profile/totp/", auth),
        Route("profile.totp.post", "POST", "/api/profile/totp/", auth,
              body=lambda c, i: {"secret": "JBSWY3DPEHPK3PXP", "code": "000000"}),
        Route("profile.totp.delete", "DELETE", "/api/profile/totp/", lambda r, s: "AH" if active(r) else "D",
              note="ohne TOTP: 404 'TOTP not found'"),
        Route("profile.generate_auth_token", "POST", "/api/profile/generate_auth_token/", auth, body=lambda c, i: {},
              note="läuft zuletzt, Tokens werden danach per SQL zurückgesetzt"),
        Route("users.deactivate_totp", "POST", "/api/users/{id}/deactivate_totp/",
              lambda r, s: "AH" if r == "superuser" else "D", "users",
              states=["any"], body=lambda c, i: {}, note="upstream: nur Superuser oder der Nutzer selbst"),
        Route("remote_version.get", "GET", "/api/remote_version/", public),
        Route("schema.get", "GET", "/api/schema/", public),
        Route("schema.swagger_ui", "GET", "/api/schema/view/", public),
        Route("ui.api_root", "GET", "/api/", public),
        Route("oauth.callback", "GET", "/api/oauth/callback/", auth, query=lambda c, i: {"code": "x", "state": "y"}),
        Route("auth.token", "POST", "/api/token/", public, roles=["anon"],
              body=lambda c, i: {"username": "sec-nobody", "password": "wrong"}),
        Route("api_auth.login.get", "GET", "/api/auth/login/", public),
        Route("api_auth.logout.get", "GET", "/api/auth/logout/", public),
        Route("share.get", "GET", "/share/refshare01xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx/", public),
        Route("share.get_noslash", "GET", "/share/refshare01xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", public),
        Route("favicon.get", "GET", "/favicon.ico", public),
        Route("logo.get", "GET", "/logo/", public),
        Route("logo.get_noslash", "GET", "/logo", public),
        Route("logo.get_file", "GET", "/logo/sec-none.png", public),
        Route("static.files", "GET", "/static/sec-none.css", public),
        Route("static.swagger_ui_dist", "GET", "/static/drf_spectacular_sidecar/swagger-ui-dist/swagger-ui.css", public),
        Route("ui.assets", "GET", "/assets/sec-none.js", public),
        Route("ui.index", "GET", "/", public),
        Route("rs.docs.index", "GET", "/rs/docs/", public),
        Route("rs.docs.page", "GET", "/rs/docs/de/usage.md", public),
    ]
    # allauth HTML pages: public (login/account flows), here only "no 5xx"; contents in test_auth_flows
    for rid, m, p in [
        ("accounts.account_inactive", "GET", "/accounts/account_inactive/"),
        ("accounts.confirm_email.get", "GET", "/accounts/confirm-email/sec-bad-key/"),
        ("accounts.confirm_email.post", "POST", "/accounts/confirm-email/sec-bad-key/"),
        ("accounts.email_verification_sent", "GET", "/accounts/confirm-email/"),
        ("accounts.login.get", "GET", "/accounts/login/"),
        ("accounts.logout.get", "GET", "/accounts/logout/"),
        ("accounts.mfa_authenticate.get", "GET", "/accounts/2fa/authenticate/"),
        ("accounts.oidc_callback.get", "GET", "/accounts/oidc/sec-none/login/callback/"),
        ("accounts.oidc_login.get", "GET", "/accounts/oidc/sec-none/login/"),
        ("accounts.password_reset.get", "GET", "/accounts/password/reset/"),
        ("accounts.password_reset_done", "GET", "/accounts/password/reset/done/"),
        ("accounts.password_reset_from_key.get", "GET", "/accounts/password/reset/key/sec-bad/"),
        ("accounts.password_reset_from_key_done", "GET", "/accounts/password/reset/key/done/"),
        ("accounts.provider_callback.get", "GET", "/accounts/sec-none/login/callback/"),
        ("accounts.provider_login.get", "GET", "/accounts/sec-none/login/"),
        ("accounts.signup.get", "GET", "/accounts/signup/"),
        ("accounts.social_login_cancelled", "GET", "/accounts/3rdparty/login/cancelled/"),
        ("accounts.social_login_error", "GET", "/accounts/3rdparty/login/error/"),
        ("accounts.social_signup.get", "GET", "/accounts/3rdparty/signup/"),
    ]:
        out.append(Route(rid, m, p, public))
    # POST variants of the account pages without CSRF: must reject (403) or show the form, never 5xx
    for rid, p in [("accounts.login.post", "/accounts/login/"), ("accounts.logout.post", "/accounts/logout/"),
                   ("accounts.mfa_authenticate.post", "/accounts/2fa/authenticate/"),
                   ("accounts.oidc_callback.post", "/accounts/oidc/sec-none/login/callback/"),
                   ("accounts.oidc_login.post", "/accounts/oidc/sec-none/login/"),
                   ("accounts.password_reset.post", "/accounts/password/reset/"),
                   ("accounts.password_reset_from_key.post", "/accounts/password/reset/key/sec-bad/"),
                   ("accounts.provider_callback.post", "/accounts/sec-none/login/callback/"),
                   ("accounts.provider_login.post", "/accounts/sec-none/login/"),
                   ("accounts.signup.post", "/accounts/signup/"),
                   ("accounts.social_signup.post", "/accounts/3rdparty/signup/"),
                   ("api_auth.login.post", "/api/auth/login/"), ("api_auth.logout.post", "/api/auth/logout/")]:
        out.append(Route(rid, "POST", p, public, roles=["anon"], form=True))
    return out


def x_routes() -> list[Route]:
    add = model(ADD)
    S2 = ["own", "foreign"]
    return [
        # pl-ai
        Route("ai.status", "GET", "/api/x-ai/status/", auth),
        Route("ai.review.list", "GET", "/api/x-ai/review/", model(V)),
        Route("ai.review.retrieve", "GET", "/api/x-ai/review/99999999/", lambda r, s: "H" if has(r, V) else "D"),
        Route("ai.review.apply", "POST", "/api/x-ai/review/99999999/apply/", lambda r, s: "H" if has(r, C) else "D",
              body=lambda c, i: {}),
        Route("ai.review.reject", "POST", "/api/x-ai/review/99999999/reject/", lambda r, s: "H" if has(r, C) else "D",
              body=lambda c, i: {}),
        Route("ai.review.undo", "POST", "/api/x-ai/review/99999999/undo/", lambda r, s: "H" if has(r, C) else "D",
              body=lambda c, i: {}),
        Route("ai.run", "POST", "/api/x-ai/run/", doc_op(), "documents", states=OWNER_STATES,
              body=lambda c, i: {"documents": [i], "mode": "suggest"}),
        Route("ai.prompts.list", "GET", "/api/x-ai/prompts/", su),
        Route("ai.prompts.update", "PUT", "/api/x-ai/prompts/title/", su, body=lambda c, i: {"template": "{{content}}"}),
        Route("ai.prompts.reset", "DELETE", "/api/x-ai/prompts/title/", su),
        Route("ai.prompts.preview", "POST", "/api/x-ai/prompts/title/preview/", su, body=lambda c, i: {}),
        Route("ai.options.get", "GET", "/api/x-ai/options/", su),
        Route("ai.options.patch", "PATCH", "/api/x-ai/options/", su, body=lambda c, i: {}),
        Route("ai.provider.check", "POST", "/api/x-ai/provider/check/", su, body=lambda c, i: {}),
        Route("ai.jobs.list", "GET", "/api/x-ai/jobs/", su),
        Route("ai.jobs.create", "POST", "/api/x-ai/jobs/", su, body=lambda c, i: {"mode": "suggest"}),
        Route("ai.jobs.preview", "POST", "/api/x-ai/jobs/preview/", su, body=lambda c, i: {}),
        Route("ai.jobs.retrieve", "GET", "/api/x-ai/jobs/99999999/", lambda r, s: "H" if r == "superuser" else "D"),
        Route("ai.jobs.cancel", "POST", "/api/x-ai/jobs/99999999/cancel/",
              lambda r, s: "H" if r == "superuser" else "D", body=lambda c, i: {}),
        Route("ai.jobs.resume", "POST", "/api/x-ai/jobs/99999999/resume/",
              lambda r, s: "H" if r == "superuser" else "D", body=lambda c, i: {}),
        Route("ai.mcp_scopes.list", "GET", "/api/x-ai/mcp-scopes/", su),
        Route("ai.mcp_scopes.create", "POST", "/api/x-ai/mcp-scopes/", su,
              body=lambda c, i: {"user_id": c.ids["reader"], "scopes": ["read"]}),
        Route("ai.mcp_scopes.delete", "DELETE", "/api/x-ai/mcp-scopes/99999999/",
              lambda r, s: "H" if r == "superuser" else "D"),
        Route("ai.assist.verdict", "POST", "/api/x-ai/assist/verdict/", obj((ADD, V), "view"), "documents",
              states=OWNER_STATES, body=lambda c, i: {"document": i},
              note="Vertrag sagt view_document; Code verlangt zusätzlich add_document (POST)"),
        Route("ai.assist.workflow_draft", "POST", "/api/x-ai/assist/workflow-draft/", model("documents.add_workflow"),
              body=lambda c, i: {"description": "tag invoices"}),
        Route("ai.mcp.get", "GET", "/mcp", public),
        Route("ai.mcp.post", "POST", "/mcp", public, body=lambda c, i: {"jsonrpc": "2.0", "id": 1, "method": "ping"}),
        Route("ai.mcp.delete", "DELETE", "/mcp", public),
        # pl-scan
        Route("scan.scanners.list", "GET", "/api/x-scan/scanners/", add),
        Route("scan.scanners.create", "POST", "/api/x-scan/scanners/", su, body=lambda c, i: {"url": "http://127.0.0.1:9/"}),
        Route("scan.scanners.update", "PATCH", "/api/x-scan/scanners/99999999/",
              lambda r, s: "H" if r == "superuser" else "D", body=lambda c, i: {"name": "x"}),
        Route("scan.scanners.delete", "DELETE", "/api/x-scan/scanners/99999999/",
              lambda r, s: "H" if r == "superuser" else "D"),
        Route("scan.scanners.capabilities", "GET", "/api/x-scan/scanners/99999999/capabilities/",
              lambda r, s: "H" if has(r, ADD) else "D"),
        Route("scan.discover", "GET", "/api/x-scan/discover/", su),
        Route("scan.jobs.create", "POST", "/api/x-scan/jobs/", add, body=lambda c, i: {"scanner": 99999999}),
        Route("scan.jobs.retrieve", "GET", "/api/x-scan/jobs/00000000-0000-0000-0000-000000000000/",
              lambda r, s: "H" if has(r, ADD) else "D"),
        Route("scan.jobs.cancel", "POST", "/api/x-scan/jobs/00000000-0000-0000-0000-000000000000/cancel/",
              lambda r, s: "H" if has(r, ADD) else "D", body=lambda c, i: {}),
        Route("scan.devices.list", "GET", "/api/x-scan/devices/", su),
        Route("scan.devices.create", "POST", "/api/x-scan/devices/", su,
              body=lambda c, i: {"name": uniq("dev"), "owner": c.ids["editor"]}),
        Route("scan.devices.revoke", "POST", "/api/x-scan/devices/99999999/revoke/",
              lambda r, s: "H" if r == "superuser" else "D", body=lambda c, i: {}),
        Route("scan.devices.delete", "DELETE", "/api/x-scan/devices/99999999/",
              lambda r, s: "H" if r == "superuser" else "D"),
        # pl-guard
        Route("guard.status", "GET", "/api/x-guard/status/", auth),
        Route("guard.events.list", "GET", "/api/x-guard/events/", su),
        Route("guard.events.retrieve", "GET", "/api/x-guard/events/99999999/",
              lambda r, s: "H" if r == "superuser" else "D"),
        Route("guard.lift", "POST", "/api/x-guard/lift/", su, body=lambda c, i: {}),
        Route("guard.backup.status", "GET", "/api/x-guard/backup/", su),
        Route("guard.backup.run", "POST", "/api/x-guard/backup/run/", su, body=lambda c, i: {}),
    ]


V2 = "/rs/api/v2"


def v2_write(view: str, write: tuple[str, ...], action: str) -> Rule:
    """v2 (pl-store): read permission + visibility first (foreign -> 404), then v1 write check."""
    inner = obj(write, action)

    def r(role: str, state: str) -> str:
        if not has(role, view):
            return "D"
        if state == "foreign" and role != "superuser":
            return "H"
        return inner(role, state)

    return r


def v2_routes() -> list[Route]:
    S = OWNER_STATES
    u = lambda kind: (lambda c, i: v2_uuid(c, kind, i))  # noqa: E731
    ac_view = lambda r, s: "A" if has(r, "paperless.view_applicationconfiguration") else "D"  # noqa: E731
    out = [
        Route("v2.server", "GET", f"{V2}/server", public),
        Route("v2.root", "GET", f"{V2}/", public),
        Route("v2.me", "GET", f"{V2}/me", auth),
        Route("v2.ui_settings.get", "GET", f"{V2}/ui_settings", auth),
        Route("v2.ui_settings.put", "PUT", f"{V2}/ui_settings", model("documents.add_uisettings"),
              body=lambda c, i: {"settings": {"sec": True}}),
        Route("v2.documents.list", "GET", f"{V2}/documents", model(V)),
        Route("v2.documents.query", "POST", f"{V2}/documents/query", model(V), body=lambda c, i: {}),
        Route("v2.documents.get", "GET", f"{V2}/documents/{{uuid}}", obj((V,), "view"), "documents", states=S),
        Route("v2.documents.patch", "PATCH", f"{V2}/documents/{{uuid}}", v2_write(V, (C,), "change"), "documents",
              states=S, body=lambda c, i: {"title": f"sec v2 {i}"}),
        Route("v2.documents.notes.get", "GET", f"{V2}/documents/{{uuid}}/notes",
              obj((V, "documents.view_note"), "view"), "documents", states=S),
        Route("v2.documents.notes.post", "POST", f"{V2}/documents/{{uuid}}/notes",
              v2_write(V, (V, C, "documents.add_note"), "change"), "documents", states=S,
              body=lambda c, i: {"note": "sec"}),
        Route("v2.documents.files", "GET", f"{V2}/documents/{{uuid}}/files/original", obj((V,), "view"),
              "documents", states=S),
        Route("v2.objects.batch", "POST", f"{V2}/objects/batch", model("documents.view_tag"),
              body=lambda c, i: {"kind": "tag", "uuids": []}),
        Route("v2.sync.changes", "GET", f"{V2}/sync/changes", auth),
        Route("v2.sync.ids", "GET", f"{V2}/sync/ids", model("documents.view_tag"), query=lambda c, i: {"kind": "tag"}),
        Route("v2.sync.resolve", "POST", f"{V2}/sync/resolve", model(V), body=lambda c, i: {}),
        Route("v2.auth.devices.list", "GET", f"{V2}/auth/devices", auth),
        Route("v2.auth.devices.revoke", "DELETE", f"{V2}/auth/devices/00000000-0000-0000-0000-000000000000",
              lambda r, s: "H" if active(r) else "D"),
        Route("v2.settings.list", "GET", f"{V2}/settings", ac_view),
        Route("v2.settings.patch", "PATCH", f"{V2}/settings", ac_view, body=lambda c, i: {"values": {}}),
        Route("v2.settings.schema", "GET", f"{V2}/settings/schema", ac_view),
        Route("v2.settings.history", "GET", f"{V2}/settings/history", ac_view),
        Route("v2.settings.restart", "POST", f"{V2}/settings/restart", su, body=lambda c, i: {"password": "wrong"}),
        Route("v2.settings.origin", "GET", f"{V2}/settings/origin", su),
        Route("v2.settings.adopt", "POST", f"{V2}/settings/adopt", su, body=lambda c, i: {"keys": []}),
        Route("v2.settings.slim_env", "GET", f"{V2}/settings/slim-env", su),
        Route("v2.settings.check_file", "POST", f"{V2}/settings/check-file", su, body=lambda c, i: "X=1"),
        Route("v2.settings.export", "POST", f"{V2}/settings/export", su, body=lambda c, i: {}),
        Route("v2.settings.import", "POST", f"{V2}/settings/import", su, body=lambda c, i: {"data": {}, "dry_run": True}),
        Route("v2.settings.get_one", "GET", f"{V2}/settings/PAPERLESS_OCR_LANGUAGE", ac_view),
        Route("v2.settings.reset", "DELETE", f"{V2}/settings/PAPERLESS_OCR_LANGUAGE",
              lambda r, s: "A" if has(r, "paperless.change_applicationconfiguration") else
              ("D" if not active(r) or has(r, "paperless.view_applicationconfiguration") else "H"),
              note="nicht lesbar -> 404 (verbirgt den Schlüssel)"),
        Route("v2.asn.presets", "GET", f"{V2}/asn/presets", model(C)),
        Route("v2.asn.config", "GET", f"{V2}/asn/config", model(C)),
        Route("v2.asn.config.put", "PUT", f"{V2}/asn/config", su, body=lambda c, i: {}),
        Route("v2.asn.ranges", "GET", f"{V2}/asn/ranges", model(C)),
        Route("v2.asn.ranges.post", "POST", f"{V2}/asn/ranges", su,
              body=lambda c, i: {"name": uniq("r"), "code": uniq("C")[:8], "first_asn": 900000, "last_asn": 900010}),
        Route("v2.asn.preview", "POST", f"{V2}/asn/preview", model(C), body=lambda c, i: {}),
        Route("v2.asn.printed", "GET", f"{V2}/asn/printed", model(C)),
        Route("v2.asn.separator", "POST", f"{V2}/asn/separator", model(C), body=lambda c, i: {}),
        Route("v2.life_areas.list", "GET", f"{V2}/life-areas", auth),
        Route("v2.life_areas.suggestions", "GET", f"{V2}/life-areas/suggestions", model(V)),
        Route("v2.timeline", "GET", f"{V2}/timeline", model(V)),
        Route("v2.timeline.upcoming", "GET", f"{V2}/timeline/upcoming", model(V)),
        Route("v2.timeline.reminders", "GET", f"{V2}/timeline/reminders", model(V)),
        Route("v2.timeline.config", "GET", f"{V2}/timeline/config", model(V)),
        Route("v2.timeline.backfill", "POST", f"{V2}/timeline/backfill", su, body=lambda c, i: {}),
        Route("v2.roles.list", "GET", f"{V2}/roles", lambda r, s: "A" if staff(r) or has(r, "auth.view_user") else "D"),
        Route("v2.roles.mode", "PUT", f"{V2}/roles/mode", su, body=lambda c, i: {"mode": "classic"}),
        Route("v2.roles.create", "POST", f"{V2}/roles", su, body=lambda c, i: {"name": uniq("role")}),
        Route("v2.ask.conversations", "GET", f"{V2}/ask/conversations", auth),
        Route("v2.ask", "POST", f"{V2}/ask", model(V), body=lambda c, i: {"question": "hi"}),
    ]
    for plural, perm, kind in [("tags", "documents.view_tag", "tags"),
                               ("correspondents", "documents.view_correspondent", "correspondents"),
                               ("document_types", "documents.view_documenttype", "document_types"),
                               ("storage_paths", "documents.view_storagepath", "storage_paths"),
                               ("saved_views", "documents.view_savedview", "saved_views"),
                               ("custom_fields", "documents.view_customfield", "custom_fields"),
                               ("users", "auth.view_user", "users"), ("groups", "auth.view_group", "groups")]:
        st = OWNER_STATES if kind in OWNER_AWARE else ["any"]
        out.append(Route(f"v2.{plural}.list", "GET", f"{V2}/{plural}", model(perm)))
        out.append(Route(f"v2.{plural}.get", "GET", f"{V2}/{plural}/{{uuid}}", obj((perm,), "view"), kind, states=st))
        if plural in ("tags", "correspondents", "document_types"):
            m = perm.replace("view_", "")
            out.append(Route(f"v2.{plural}.patch", "PATCH", f"{V2}/{plural}/{{uuid}}",
                             v2_write(perm, (m.replace("documents.", "documents.change_"),), "change"), kind, states=st,
                             body=lambda c, i: {"name": uniq("v2")}))
            out.append(Route(f"v2.{plural}.delete", "DELETE", f"{V2}/{plural}/{{uuid}}",
                             v2_write(perm, (m.replace("documents.", "documents.delete_"),), "delete"), kind, states=st))
    A = f"{V2}/automation"
    vw = "documents.view_workflow"
    out += [
        Route("auto.workflows.list", "GET", f"{A}/workflows", model(vw)),
        Route("auto.workflows.get", "GET", f"{A}/workflows/{{id}}", model(vw), "workflows", states=["any"]),
        Route("auto.workflows.patch", "PATCH", f"{A}/workflows/{{id}}", model("documents.change_workflow"),
              "workflows", states=["any"], body=lambda c, i: {}),
        Route("auto.workflows.runs", "GET", f"{A}/workflows/{{id}}/runs", model(vw), "workflows", states=["any"]),
        Route("auto.dry_run", "POST", f"{A}/dry-run", model(vw, V), body=lambda c, i: {"workflow_id": 1}),
        Route("auto.runs.cleanup", "POST", f"{A}/runs/cleanup", su, body=lambda c, i: {"older_than_days": 3650}),
        Route("auto.suggestions", "GET", f"{A}/suggestions", model(vw, V)),
        Route("auto.mail_flows", "GET", f"{A}/mail-flows", model(vw, "paperless_mail.view_mailrule")),
        Route("auto.export_ngx", "GET", f"{A}/export-ngx", model(vw)),
    ]
    for r in out:
        if r.path.find("{uuid}") >= 0 and r.kind:
            r.note = (r.note + " uuid").strip()
    return out


# --------------------------------------------------------------------------- Procedure


def classify(r: Response) -> str:
    s = r.status
    if s in (501, 502, 503):
        return "A"  # Service/tool missing in the test server: permission check passed
    if s >= 500:
        return "E"
    if s == 400 and "Insufficient permissions" in r.text:
        return "D"  # upstream reports some refusals as validation errors (400)
    if s in (401, 403):
        return "D"
    if s == 404:
        return "H"
    if s in (301, 302, 303, 307, 308):
        loc = r.header("location") or ""
        return "D" if "/accounts/login" in loc or "login" in loc.split("?")[0] else "A"
    return "A"


def compare(exp: str, got: str) -> str:
    if got == "E":
        return "fehler"
    if exp == "P" or exp == got:
        return "ok"
    if exp == "X" and got in ("D", "H"):
        return "ok"
    if exp == "AH" and got in ("A", "H"):
        return "ok"
    if got == "A":
        return "kritisch"
    if exp == "A":
        return "funktion"
    return "info"


MINI_PDF = (b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj 2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj "
            b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n")


def multipart(fields: dict, file: tuple) -> tuple:
    b = "----secboundary" + uuidlib.uuid4().hex
    out = b""
    for k, v in fields.items():
        out += f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
    name, fname, data = file
    out += (f'--{b}\r\nContent-Disposition: form-data; name="{name}"; filename="{fname}"\r\n'
            f"Content-Type: application/pdf\r\n\r\n").encode() + data + f"\r\n--{b}--\r\n".encode()
    return out, f"multipart/form-data; boundary={b}"


def fill_path(route: Route, ctx: Ctx, oid: int) -> str:
    p = route.path
    if "{uuid}" in p:
        plural = route.kind or "documents"
        p = p.replace("{uuid}", v2_uuid(ctx, plural, oid))
    for ph in ("{id}", "{pk}"):
        p = p.replace(ph, str(oid))
    return p


def fixture(route: Route, ctx: Ctx, state: str, role: str) -> int:
    k = route.kind
    if not k:
        return 0
    if k == "config":
        return 1
    if k == "log":
        return 0
    if k == "processed_mail":
        oid = int(sql("select min(id) from paperless_mail_processedmail"))
        own = ctx.ids.get(role, ctx.ids["alice"]) if state == "own" else ctx.ids["superuser"]
        sql(f"update paperless_mail_processedmail set owner_id={own} where id={oid}")
        return oid
    if k == "tasks":
        oid = int(sql("select min(id) from documents_paperlesstask"))
        own = ctx.ids.get(role, ctx.ids["alice"]) if state == "own" else ctx.ids["superuser"]
        sql(f"update documents_paperlesstask set owner_id={own} where id={oid}")
        return oid
    return make(k, ctx, state, role)


def cleanup(route: Route, ctx: Ctx, oid: int) -> None:
    if not route.kind or route.kind in ("config", "log", "processed_mail", "tasks"):
        return
    if route.kind == "documents":
        if route.method == "DELETE" or route.id in ("documents.delete",):
            admin().post("/api/trash/", json_body={"action": "restore", "documents": [oid]})
        return
    plural = route.kind
    if route.method != "DELETE":
        admin().request("DELETE", f"/api/{plural}/{oid}/")


def run(routes: list[Route], ctx: Ctx, log) -> list[dict]:
    rows: list[dict] = []
    order = {"GET": 0, "HEAD": 0, "POST": 1, "PUT": 1, "PATCH": 1, "DELETE": 2}
    late = {"profile.generate_auth_token", "config.destroy"}
    routes = sorted(routes, key=lambda r: (r.id in late, order.get(r.method, 1)))
    for route in routes:
        roles = route.roles or ROLES
        for role in roles:
            for state in (route.states or ["-"]):
                exp = route.rule(role, state)
                try:
                    oid = fixture(route, ctx, state if state != "-" else "any", role)
                    path = fill_path(route, ctx, oid)
                    if route.id == "logs.retrieve":
                        path = "/api/logs/paperless/"
                    s = sess(role)
                    kw: dict[str, Any] = {}
                    if route.query:
                        kw["query"] = route.query(ctx, oid)
                    ctx.cur = (state if state != "-" else "any", role)
                    if route.form and path.startswith("/api/"):
                        data, ctype = multipart({"title": "sec", "version_label": "sec"},
                                                ("document", "sec.pdf", MINI_PDF))
                        kw["data"], kw["content_type"] = data, ctype
                    elif route.form:
                        kw["form"] = {"title": "sec"}
                        kw["csrf"] = False
                    elif route.body is not None:
                        b = route.body(ctx, oid)
                        if isinstance(b, str):
                            kw["data"] = b.encode()
                            kw["content_type"] = "text/plain"
                        else:
                            kw["json_body"] = b
                    if path.startswith(V2):
                        kw["api_version"] = False
                    r = s.request(route.method, path, **kw)
                    got = classify(r)
                    res = compare(exp, got)
                    rows.append({"route": route.id, "method": route.method, "path": route.path, "role": role,
                                 "state": state, "expected": exp, "status": r.status, "got": got, "result": res,
                                 "detail": r.text[:160] if res != "ok" else "", "note": route.note})
                    cleanup(route, ctx, oid)
                except Exception as e:  # fixture problem: not a finding of the server under test
                    rows.append({"route": route.id, "method": route.method, "path": route.path, "role": role,
                                 "state": state, "expected": exp, "status": 0, "got": "?", "result": "nicht-geprüft",
                                 "detail": str(e)[:200], "note": route.note})
                log(rows[-1])
        if route.id == "profile.generate_auth_token":
            restore_tokens(ctx.ids)
    return rows


def all_routes() -> list[Route]:
    return v1_routes() + x_routes() + v2_routes()


def write_report(rows: list[dict], active_ids: set[str], covered: set[str]) -> Path:
    out = REPO / "results" / "release" / "security"
    out.mkdir(parents=True, exist_ok=True)
    (out / "matrix.json").write_text(json.dumps(rows, indent=1, ensure_ascii=False), encoding="utf-8")
    counts: dict[str, int] = {}
    for r in rows:
        counts[r["result"]] = counts.get(r["result"], 0) + 1
    lines = ["# Rechte-Matrix", "", f"Server {BASE}, DB {DB}, {time.strftime('%Y-%m-%d %H:%M')}", "",
             "Legende: A erlaubt, D verweigert (401/403/Login), H 404, X D oder H, P öffentlich (kein 5xx). "
             "Zeilen = Route, Spalten = Rolle; Zellen `erwartet/bekommen(Status)` je Objektzustand.", "",
             "| Ergebnis | Anzahl |", "|---|---:|"]
    lines += [f"| {k} | {v} |" for k, v in sorted(counts.items())]
    missing = sorted(active_ids - covered)
    lines += ["", f"## Abdeckung: {len(covered & active_ids)}/{len(active_ids)} aktive v1-Routen geprüft", ""]
    lines += [f"- nicht in der Matrix: `{m}`" for m in missing] or ["- alle aktiven v1-Routen abgedeckt"]
    bad = [r for r in rows if r["result"] not in ("ok",)]
    lines += ["", "## Abweichungen", "", "| Klasse | Route | Rolle | Zustand | erwartet | Status | Ausschnitt |",
              "|---|---|---|---|---|---:|---|"]
    for r in sorted(bad, key=lambda x: (x["result"], x["route"], x["role"])):
        det = r["detail"].replace("|", "\\|").replace("\n", " ")[:120]
        lines.append(f"| {r['result']} | {r['route']} | {r['role']} | {r['state']} | {r['expected']} | {r['status']} | {det} |")
    lines += ["", "## Matrix", "", "| Route | " + " | ".join(ROLES) + " |", "|---|" + "---|" * len(ROLES)]
    by: dict[str, dict[str, list[str]]] = {}
    for r in rows:
        cell = f"{r['state'][:2] + ':' if r['state'] not in ('-', 'any') else ''}{r['expected']}/{r['got']}{r['status']}"
        if r["result"] != "ok":
            cell = f"**{cell}**"
        by.setdefault(f"{r['method']} {r['path']} `{r['route']}`", {}).setdefault(r["role"], []).append(cell)
    for k in by:
        lines.append(f"| {k} | " + " | ".join(" ".join(by[k].get(role, ["–"])) for role in ROLES) + " |")
    p = out / "matrix.md"
    p.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return p


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-k", action="append")
    ap.add_argument("--routes-file", default=str(HERE / ".work" / "routes_v1.tsv"))
    a = ap.parse_args()
    ids = setup_users()
    role_perms()
    ctx = Ctx(ids=ids)
    j = must(admin().get("/api/documents/", query={"page_size": 500, "fields": "id,mime_type,archived_file_name",
                                                    "mime_type": "application/pdf"}), "docs")
    ctx.docs = [d["id"] for d in j["results"] if d.get("archived_file_name")][:80]
    ctx.acc = make("mail_accounts", ctx, "own", "superuser")
    ctx.tag = make("tags", ctx, "own", "superuser")
    routes = all_routes()
    if a.k:
        routes = [r for r in routes if any(k in r.id for k in a.k)]
    n = [0]

    def log(row):
        n[0] += 1
        if row["result"] not in ("ok",):
            print(f"{row['result']:9} {row['route']:38} {row['role']:9} {row['state']:7} exp={row['expected']} "
                  f"got={row['status']} {row['detail'][:80]}", flush=True)

    rows = run(routes, ctx, log)
    active_ids = set()
    rf = Path(a.routes_file)
    if not rf.exists():  # active routes from the startup log of the test server
        out = subprocess.run(["bash", str(HERE / "server.sh"), "routes"], capture_output=True, text=True).stdout
        if out.strip():
            rf.parent.mkdir(parents=True, exist_ok=True)
            rf.write_text(out, encoding="utf-8")
    if rf.exists():
        active_ids = {ln.split("\t")[0] for ln in rf.read_text(encoding="utf-8").splitlines() if ln.strip()}
    covered = {r.id for r in all_routes()}
    p = write_report(rows, active_ids, covered)
    bad = [r for r in rows if r["result"] in ("kritisch", "funktion", "fehler")]
    print(f"{len(rows)} Proben, {len(bad)} Abweichungen (kritisch/funktion/fehler) -> {p}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
