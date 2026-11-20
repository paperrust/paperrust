#!/usr/bin/env python3
"""Generates the base case catalogue ``tests/parity/<crate>/cases.base.json``.

Source of the routes: upstream/src/paperless/urls.py (router + explicit paths) and the
@action decorators in documents/views.py, paperless/views.py, paperless_mail/views.py.
Parameter combinations follow what the Angular frontend (upstream/src-ui) sends:

* AbstractPaperlessService.list: page, page_size, ordering (+ extra params)
* AbstractNameFilterService.listFiltered: name__icontains, full_perms=true
* getFew: id__in=<ids>, ordering=-id, page_size=1000
* listAll: page=1, page_size=100000
* DocumentService.get: full_perms=true (+ fields=id,versions)
* document list view: truncate_content=true, include_selection_data=true, filter vars from
  data/filter-rule-type.ts mapped by utils/query-params.ts (comma-joined ids, isnull=1/0,
  custom_field_query JSON)
* Accept: application/json; version=10 (interceptor), v9 for compatibility.

Error paths: unknown ids, invalid params, wrong role, anonymous, bad token, API version
9/10/none/invalid.

Re-run after changing this file:  python tests/parity/harness/gen_base_catalog.py
Crate owners add their own cases in ``tests/parity/<crate>/cases.json`` (not overwritten).
"""

from __future__ import annotations

import json
from pathlib import Path

PARITY = Path(__file__).resolve().parent.parent

LIST_USERS = ["admin", "user1", "noperm", "anon"]
DETAIL_USERS = ["admin", "user1", "user2", "noperm"]
ADMIN = ["admin"]
AUTH_VARIANTS = ["anon", "badtoken", "inactive", "admin@session", "admin@basic"]
V = ["10", "9"]
V10 = ["10"]
VERSION_VARIANTS = ["none", "99", "abc"]
BAD_ID = 999999999


def var(path, json_path="results.0.id", query=None, user="admin"):
    spec = {"get": path, "json": json_path, "user": user}
    if query:
        spec["query"] = query
    return spec


FIRST = {"ordering": "id", "page_size": 1}
LAST = {"ordering": "-id", "page_size": 1}

VARS = {
    "doc_id": var("/api/documents/", query={**FIRST, "fields": "id"}),
    "doc_id_last": var("/api/documents/", query={**LAST, "fields": "id"}),
    "doc_with_notes": var("/api/documents/", query={"ordering": "-num_notes", "page_size": 1, "fields": "id"}),
    "tag_id": var("/api/tags/", query=FIRST),
    "tag_id_last": var("/api/tags/", query=LAST),
    "correspondent_id": var("/api/correspondents/", query=FIRST),
    "correspondent_id_last": var("/api/correspondents/", query=LAST),
    "document_type_id": var("/api/document_types/", query=FIRST),
    "storage_path_id": var("/api/storage_paths/", query=FIRST),
    "custom_field_id": var("/api/custom_fields/", query=FIRST),
    "saved_view_id": var("/api/saved_views/", query=FIRST),
    "share_link_id": var("/api/share_links/", query=FIRST),
    "share_link_slug": var("/api/share_links/", "results.0.slug", query=FIRST),
    "share_link_bundle_id": var("/api/share_link_bundles/", query=FIRST),
    "workflow_id": var("/api/workflows/", query=FIRST),
    "workflow_trigger_id": var("/api/workflow_triggers/", query=FIRST),
    "workflow_action_id": var("/api/workflow_actions/", query=FIRST),
    "user_id": var("/api/users/", query=FIRST),
    "group_id": var("/api/groups/", query=FIRST),
    "mail_account_id": var("/api/mail_accounts/", query=FIRST),
    "mail_rule_id": var("/api/mail_rules/", query=FIRST),
    "processed_mail_id": var("/api/processed_mail/", query=FIRST),
    "task_pk": var("/api/tasks/", query={"page_size": 1}),
    "task_uuid": var("/api/tasks/", "results.0.task_id", query={"page_size": 1}),
    "log_name": var("/api/logs/", "0"),
    "user1_id": {"get": "/api/ui_settings/", "json": "user.id", "user": "user1"},
    "app_config_id": var("/api/config/", "0.id"),
}


def case(cid, route, path, *, query=None, users=None, versions=None, **kw):
    c = {"id": cid, "route": route, "method": kw.pop("method", "GET"), "path": path}
    if query:
        c["query"] = query
    c["users"] = users or ADMIN
    c["api_versions"] = versions or V10
    c.update(kw)
    return c


def auth_and_version_cases(prefix, route, path, query=None):
    """Error paths for one representative request of a route."""
    return [
        case(f"{prefix}.auth", route, path, query=query, users=AUTH_VARIANTS, expect_status=None),
        case(f"{prefix}.apiver", route, path, query=query, users=ADMIN, versions=VERSION_VARIANTS),
    ]


def list_cases(prefix, route, path, ordering, *, name_filter=True, extra=None, full_perms=True,
               id_var=None, id_var2=None):
    out = [
        case(f"{prefix}.default", route, path, users=LIST_USERS, versions=V),
        case(f"{prefix}.page1", route, path, query={"page": 1, "page_size": 25}, users=LIST_USERS, versions=V),
        case(f"{prefix}.listall", route, path, query={"page": 1, "page_size": 100000}, users=["admin", "user1"],
             versions=V),
        case(f"{prefix}.page2", route, path, query={"page": 2, "page_size": 1}),
        case(f"{prefix}.page-too-high", route, path, query={"page": 99999, "page_size": 25}),
        case(f"{prefix}.page-invalid", route, path, query={"page": "abc"}),
        case(f"{prefix}.page-zero", route, path, query={"page": 0}),
        case(f"{prefix}.page-size-invalid", route, path, query={"page_size": "x"}),
        case(f"{prefix}.page-size-zero", route, path, query={"page_size": 0}),
        case(f"{prefix}.page-size-huge", route, path, query={"page_size": 10000000}),
        case(f"{prefix}.ordering-unknown", route, path, query={"ordering": "does_not_exist"}),
    ]
    for o in ordering:
        for sign in ("", "-"):
            out.append(case(f"{prefix}.ordering{sign or '+'}{o}", route, path,
                            query={"page": 1, "page_size": 25, "ordering": f"{sign}{o}"},
                            users=["admin", "user1"]))
    if name_filter:
        q = {"page": 1, "page_size": 25, "ordering": ordering[0] if ordering else "id", "name__icontains": "a"}
        if full_perms:
            q["full_perms"] = "true"
        out.append(case(f"{prefix}.name-filter", route, path, query=q, users=["admin", "user1"], versions=V))
        out.append(case(f"{prefix}.name-filter-empty", route, path, query={"name__icontains": ""}))
        out.append(case(f"{prefix}.name-filter-unicode", route, path, query={"name__icontains": "ä"}))
        out.append(case(f"{prefix}.name-iexact", route, path, query={"name__iexact": "A"}))
        out.append(case(f"{prefix}.name-istartswith", route, path, query={"name__istartswith": "a"}))
        out.append(case(f"{prefix}.name-iendswith", route, path, query={"name__iendswith": "e"}))
    if full_perms:
        out.append(case(f"{prefix}.full-perms", route, path, query={"full_perms": "true"}, users=["admin", "user1"],
                        versions=V))
        out.append(case(f"{prefix}.full-perms-0", route, path, query={"full_perms": "0"}))
    if id_var:
        ids = f"${{{id_var}}}" + (f",${{{id_var2}}}" if id_var2 else "")
        out.append(case(f"{prefix}.getfew", route, path,
                        query={"id__in": ids, "ordering": "-id", "page_size": 1000}, users=["admin", "user1"]))
        out.append(case(f"{prefix}.id", route, path, query={"id": f"${{{id_var}}}"}))
        out.append(case(f"{prefix}.id-in-invalid", route, path, query={"id__in": "a,b"}))
        out.append(case(f"{prefix}.id-in-empty", route, path, query={"id__in": ""}))
    for i, q in enumerate(extra or []):
        out.append(case(f"{prefix}.extra{i}", route, path, query=q, users=["admin", "user1"]))
    out += auth_and_version_cases(prefix, route, path, {"page": 1, "page_size": 25})
    return out


def detail_cases(prefix, route, base, id_var, *, query=None, users=None):
    return [
        case(f"{prefix}.detail", route, f"{base}${{{id_var}}}/", query=query, users=users or DETAIL_USERS,
             versions=V),
        case(f"{prefix}.detail-unknown", route, f"{base}{BAD_ID}/", query=query),
        case(f"{prefix}.detail-nonint", route, f"{base}abc/"),
        case(f"{prefix}.detail-anon", route, f"{base}${{{id_var}}}/", users=["anon"]),
        case(f"{prefix}.detail-noslash", route, f"{base}${{{id_var}}}", follow=False),
    ]


def method_not_allowed(prefix, route, path, methods=("PUT", "DELETE", "PATCH")):
    # Only safe probes: these methods are not routed for the given path -> 405, no write.
    return [case(f"{prefix}.405-{m.lower()}", route, path, method=m, json={}) for m in methods]


# ----------------------------------------------------------------------------- crates

def pl_objects():
    out = []
    specs = [
        ("tags", "/api/tags/", ["name", "color", "matching_algorithm", "match", "document_count"], "tag_id",
         "tag_id_last", [{"is_root": "true"}, {"is_root": "false"}]),
        ("correspondents", "/api/correspondents/",
         ["name", "matching_algorithm", "match", "document_count", "last_correspondence"],
         "correspondent_id", "correspondent_id_last",
         [{"last_correspondence": "true", "page": 1, "page_size": 25}]),
        ("document_types", "/api/document_types/", ["name", "matching_algorithm", "match", "document_count"],
         "document_type_id", None, []),
        ("storage_paths", "/api/storage_paths/", ["name", "path", "matching_algorithm", "match", "document_count"],
         "storage_path_id", None, [{"path__icontains": "{"}]),
    ]
    for name, path, ordering, idv, idv2, extra in specs:
        out += list_cases(f"{name}.list", f"{name}.list", path, ordering, id_var=idv, id_var2=idv2, extra=extra)
        out += detail_cases(f"{name}", f"{name}.retrieve", path, idv, query={"full_perms": "true"})
        out += method_not_allowed(f"{name}.list", f"{name}.list", path)
    out.append(case("bulk_edit_objects.get", "bulk_edit_objects", "/api/bulk_edit_objects/"))
    out.append(case("storage_paths.test.get", "storage_paths.test", "/api/storage_paths/test/"))
    return out


def pl_documents():
    p = "/api/documents/"
    out = [
        case("documents.list.frontend-default", "documents.list", p,
             query={"page": 1, "page_size": 50, "ordering": "-created", "truncate_content": "true",
                    "include_selection_data": "true"}, users=LIST_USERS, versions=V),
        case("documents.list.truncate", "documents.list", p, query={"truncate_content": "true"}),
        case("documents.list.fields-id", "documents.list", p, query={"page": 1, "page_size": 100000, "fields": "id"},
             users=["admin", "user1"]),
        case("documents.list.fields-id-title", "documents.list", p,
             query={"id__in": "${doc_id},${doc_id_last}", "ordering": "-id", "page_size": 1000, "fields": "id,title"}),
        case("documents.list.fields-unknown", "documents.list", p, query={"fields": "nope"}),
        case("documents.list.full-perms", "documents.list", p, query={"page": 1, "page_size": 25, "full_perms": "true"},
             users=["admin", "user1", "user2"], versions=V),
        case("documents.list.page-too-high", "documents.list", p, query={"page": 99999}),
        case("documents.list.page-invalid", "documents.list", p, query={"page": "x"}),
        case("documents.list.page-size-huge", "documents.list", p, query={"page_size": 10000000, "fields": "id"}),
        case("documents.list.v9-all", "documents.list", p, query={"page_size": 3}, versions=["9"]),
    ]
    out += detail_cases("documents", "documents.retrieve", p, "doc_id", query={"full_perms": "true"})
    out += [
        case("documents.retrieve.plain", "documents.retrieve", f"{p}${{doc_id}}/", users=DETAIL_USERS, versions=V),
        case("documents.retrieve.versions", "documents.retrieve", f"{p}${{doc_id}}/", query={"fields": "id,versions"}),
        case("documents.retrieve.fields", "documents.retrieve", f"{p}${{doc_id}}/", query={"fields": "id,title,tags"}),
        case("documents.retrieve.version-param", "documents.retrieve", f"{p}${{doc_id}}/",
             query={"full_perms": "true", "version": BAD_ID}),
        case("documents.metadata", "documents.metadata", f"{p}${{doc_id}}/metadata/", users=DETAIL_USERS),
        case("documents.metadata.unknown", "documents.metadata", f"{p}{BAD_ID}/metadata/"),
        case("documents.metadata.version", "documents.metadata", f"{p}${{doc_id}}/metadata/", query={"version": BAD_ID}),
        case("documents.notes", "documents.notes", f"{p}${{doc_with_notes}}/notes/", users=DETAIL_USERS, versions=V),
        case("documents.notes.unknown", "documents.notes", f"{p}{BAD_ID}/notes/"),
        case("documents.history", "documents.history", f"{p}${{doc_id}}/history/", users=DETAIL_USERS),
        case("documents.history.unknown", "documents.history", f"{p}{BAD_ID}/history/"),
        case("documents.share_links", "documents.share_links", f"{p}${{doc_id}}/share_links/", users=DETAIL_USERS),
        case("documents.share_links.unknown", "documents.share_links", f"{p}{BAD_ID}/share_links/"),
        case("documents.root", "documents.root", f"{p}${{doc_id}}/root/", users=DETAIL_USERS),
        case("documents.root.unknown", "documents.root", f"{p}{BAD_ID}/root/"),
        case("documents.suggestions", "documents.suggestions", f"{p}${{doc_id}}/suggestions/", users=["admin", "user1"]),
        case("documents.suggestions.unknown", "documents.suggestions", f"{p}{BAD_ID}/suggestions/"),
        case("documents.ai_suggestions", "documents.ai_suggestions", f"{p}${{doc_id}}/ai_suggestions/"),
        case("documents.next_asn", "documents.next_asn", f"{p}next_asn/", users=LIST_USERS, versions=V),
        case("documents.selection_data", "documents.selection_data", f"{p}selection_data/", method="POST",
             json={"documents": ["${doc_id}", "${doc_id_last}"]}, users=["admin", "user1", "noperm"]),
        case("documents.selection_data.empty", "documents.selection_data", f"{p}selection_data/", method="POST",
             json={"documents": []}),
        case("documents.selection_data.invalid", "documents.selection_data", f"{p}selection_data/", method="POST",
             json={"documents": "x"}),
        case("documents.selection_data.get", "documents.selection_data", f"{p}selection_data/"),
    ]
    out += auth_and_version_cases("documents.list", "documents.list", p, {"page": 1, "page_size": 25})
    out += auth_and_version_cases("documents.retrieve", "documents.retrieve", f"{p}${{doc_id}}/")
    return out


def pl_docfilter():
    p = "/api/documents/"
    base = {"page": 1, "page_size": 50, "ordering": "-created", "truncate_content": "true"}
    filters = {
        "title_content": "a", "title__icontains": "a", "content__icontains": "the",
        "archive_serial_number": 1, "archive_serial_number__gt": 1, "archive_serial_number__lt": 100,
        "archive_serial_number__isnull": 1, "archive_serial_number__isnull-0": 0,
        "created__date__gt": "2020-01-01", "created__date__gte": "2020-01-01",
        "created__date__lt": "2030-01-01", "created__date__lte": "2030-01-01",
        "created__year": 2024, "created__month": 1, "created__day": 1,
        "added__date__gt": "2020-01-01", "added__date__gte": "2020-01-01",
        "added__date__lt": "2030-01-01", "added__date__lte": "2030-01-01",
        "modified__date__gt": "2020-01-01", "modified__date__lt": "2030-01-01",
        "correspondent__id": "${correspondent_id}",
        "correspondent__id__in": "${correspondent_id},${correspondent_id_last}",
        "correspondent__id__none": "${correspondent_id}",
        "correspondent__isnull": 1, "correspondent__isnull-0": 0,
        "document_type__id": "${document_type_id}", "document_type__id__in": "${document_type_id}",
        "document_type__id__none": "${document_type_id}", "document_type__isnull": 1,
        "storage_path__id": "${storage_path_id}", "storage_path__id__in": "${storage_path_id}",
        "storage_path__id__none": "${storage_path_id}", "storage_path__isnull": 1,
        "tags__id__all": "${tag_id},${tag_id_last}", "tags__id__in": "${tag_id},${tag_id_last}",
        "tags__id__none": "${tag_id}", "tags__id__all-single": "${tag_id}",
        "is_tagged": 1, "is_tagged-0": 0, "is_in_inbox": 1, "is_in_inbox-false": "false",
        "owner__id": "${user1_id}", "owner__id__in": "${user1_id}", "owner__id__none": "${user1_id}",
        "owner__isnull": 1, "shared_by__id": "${user1_id}",
        "custom_fields__id__all": "${custom_field_id}", "custom_fields__id__in": "${custom_field_id}",
        "custom_fields__id__none": "${custom_field_id}", "custom_fields__icontains": "a",
        "has_custom_fields": 1, "has_custom_fields-0": 0,
        "mime_type": "application/pdf", "has_duplicates": "true",
        "id": "${doc_id}", "id__in": "${doc_id},${doc_id_last}",
    }
    out = []
    for key, val in filters.items():
        name = key
        param = key.split("-")[0]
        out.append(case(f"docfilter.{name}", "documents.list", p, query={**base, param: val},
                        users=["admin", "user1"]))
    invalid = {
        "correspondent__id__in": "a,b", "tags__id__all": "x", "created__date__gt": "not-a-date",
        "archive_serial_number": "abc", "is_tagged": "maybe", "created__year": "abc",
        "owner__id__none": ",,", "custom_field_query": "not json",
    }
    for key, val in invalid.items():
        out.append(case(f"docfilter.invalid.{key}", "documents.list", p, query={**base, key: val}))
    cfq = {
        "exists": '["%s","exists",true]' % "${custom_field_id}",
        "isnull": '["%s","isnull",true]' % "${custom_field_id}",
        "icontains": '["%s","icontains","a"]' % "${custom_field_id}",
        "and": '["AND",[["%s","exists",true],["%s","isnull",false]]]' % ("${custom_field_id}", "${custom_field_id}"),
        "or": '["OR",[["%s","exists",true],["%s","exists",false]]]' % ("${custom_field_id}", "${custom_field_id}"),
        "not": '["NOT",["%s","exists",true]]' % "${custom_field_id}",
        "unknown-field": '["no_such_field","exists",true]',
        "bad-op": '["%s","nope",1]' % "${custom_field_id}",
        "too-deep": "[\"AND\"," * 12 + '["x","exists",true]' + "]" * 12,
        "empty": "[]",
    }
    for key, val in cfq.items():
        out.append(case(f"docfilter.cfq.{key}", "documents.list", p, query={**base, "custom_field_query": val}))
    ordering = ["archive_serial_number", "correspondent__name", "title", "document_type__name", "created",
                "added", "modified", "num_notes", "owner", "page_count", "storage_path__name", "id"]
    for o in ordering:
        for sign in ("", "-"):
            out.append(case(f"docfilter.ordering{sign or '+'}{o}", "documents.list", p,
                            query={**base, "ordering": f"{sign}{o}"}, users=["admin", "user1"]))
    out.append(case("docfilter.ordering-custom-field", "documents.list", p,
                    query={**base, "ordering": "custom_field_${custom_field_id}"}))
    out.append(case("docfilter.ordering-custom-field-desc", "documents.list", p,
                    query={**base, "ordering": "-custom_field_${custom_field_id}"}))
    out.append(case("docfilter.ordering-custom-field-unknown", "documents.list", p,
                    query={**base, "ordering": f"custom_field_{BAD_ID}"}))
    out.append(case("docfilter.ordering-multi", "documents.list", p, query={**base, "ordering": "title,-created"}))
    out.append(case("docfilter.combo-frontend", "documents.list", p, query={
        **base, "include_selection_data": "true", "tags__id__all": "${tag_id}", "is_in_inbox": 0,
        "correspondent__id__in": "${correspondent_id}", "created__date__gte": "2000-01-01",
        "title_content": "e"}, users=["admin", "user1"], versions=V))
    return out


def pl_files():
    p = "/api/documents/"
    users = ["admin", "user1", "user2", "noperm", "anon"]
    out = []
    for kind in ("download", "preview", "thumb"):
        route = f"documents.{kind}"
        out += [
            case(f"files.{kind}", route, f"{p}${{doc_id}}/{kind}/", users=users),
            case(f"files.{kind}.unknown", route, f"{p}{BAD_ID}/{kind}/"),
            case(f"files.{kind}.version-unknown", route, f"{p}${{doc_id}}/{kind}/", query={"version": BAD_ID}),
        ]
        if kind != "thumb":
            out.append(case(f"files.{kind}.original", route, f"{p}${{doc_id}}/{kind}/", query={"original": "true"}))
    out.append(case("files.download.follow-formatting", "documents.download", f"{p}${{doc_id}}/download/",
                    query={"follow_formatting": "true"}))
    out.append(case("files.download.session", "documents.download", f"{p}${{doc_id}}/download/",
                    users=["admin@session"]))
    out += [
        case("files.share.valid", "share", "/share/${share_link_slug}", users=["anon"]),
        case("files.share.valid-slash", "share", "/share/${share_link_slug}/", users=["anon"]),
        case("files.share.unknown", "share", "/share/doesnotexist123", users=["anon", "admin"]),
        case("files.share.traversal", "share", "/share/..%2F..%2Fetc%2Fpasswd", users=["anon"]),
        case("files.logo", "logo", "/logo/", users=["anon", "admin"]),
        case("files.logo.unknown", "logo", "/logo/does-not-exist.png", users=["anon", "admin"]),
        case("files.logo.traversal", "logo", "/logo/..%2F..%2Fpaperless.conf", users=["anon", "admin"]),
        case("files.logo.traversal2", "logo", "/logo/../../../../etc/passwd", users=["anon"]),
        case("files.fetch.doc", "fetch", "/fetch/doc/${doc_id}", users=["admin", "anon"]),
        case("files.fetch.thumb", "fetch", "/fetch/thumb/${doc_id}", users=["admin"]),
        case("files.fetch.preview", "fetch", "/fetch/preview/${doc_id}", users=["admin"]),
        case("files.bulk_download.get", "bulk_download", f"{p}bulk_download/"),
        case("files.bulk_download.archive", "bulk_download", f"{p}bulk_download/", method="POST",
             json={"documents": ["${doc_id}"], "content": "archive", "compression": "none"},
             compare="status+headers", users=["admin", "user1", "noperm"]),
        case("files.bulk_download.invalid", "bulk_download", f"{p}bulk_download/", method="POST",
             json={"documents": [BAD_ID]}),
    ]
    return out


def pl_documents_write():
    p = "/api/documents/"
    out = list_cases("trash.list", "trash.list", "/api/trash/", [], name_filter=False, full_perms=False)
    out += [
        case("documents_write.post_document.get", "documents.post_document", f"{p}post_document/"),
        case("documents_write.bulk_edit.get", "documents.bulk_edit", f"{p}bulk_edit/"),
        case("documents_write.bulk_edit.invalid", "documents.bulk_edit", f"{p}bulk_edit/", method="POST",
             json={"documents": [], "method": "nope", "parameters": {}}),
        case("documents_write.bulk_edit.missing", "documents.bulk_edit", f"{p}bulk_edit/", method="POST",
             json={}),
        case("documents_write.delete.invalid", "documents.delete", f"{p}delete/", method="POST",
             json={"documents": "x"}),
        case("documents_write.rotate.invalid", "documents.rotate", f"{p}rotate/", method="POST",
             json={"documents": [BAD_ID], "degrees": 45}),
        case("documents_write.merge.invalid", "documents.merge", f"{p}merge/", method="POST", json={}),
        case("documents_write.email.invalid", "documents.email", f"{p}email/", method="POST", json={}),
        case("documents_write.trash.invalid-action", "trash.action", "/api/trash/", method="POST",
             json={"action": "nope"}),
        # write flows (snapshot reset before A, between A and B, after B)
        case("documents_write.patch-title", "documents.partial_update", f"{p}${{doc_id}}/", write=True,
             users=["admin", "user1"], steps=[
                 {"method": "PATCH", "path": f"{p}${{doc_id}}/", "json": {"title": "parity patched"}},
                 {"method": "GET", "path": f"{p}${{doc_id}}/", "query": {"full_perms": "true"}},
             ]),
        case("documents_write.bulk-add-tag", "documents.bulk_edit", f"{p}bulk_edit/", write=True, steps=[
            {"method": "POST", "path": f"{p}bulk_edit/",
             "json": {"documents": ["${doc_id}"], "method": "add_tag", "parameters": {"tag": "${tag_id}"}}},
            {"method": "GET", "path": f"{p}${{doc_id}}/"},
        ]),
        case("documents_write.delete-to-trash", "documents.destroy", f"{p}${{doc_id_last}}/", write=True, steps=[
            {"method": "DELETE", "path": f"{p}${{doc_id_last}}/"},
            {"method": "GET", "path": "/api/trash/", "query": {"page": 1, "page_size": 25}},
        ]),
    ]
    return out


def pl_fields_views():
    out = []
    out += list_cases("custom_fields.list", "custom_fields.list", "/api/custom_fields/", ["name"],
                      id_var="custom_field_id")
    out += detail_cases("custom_fields", "custom_fields.retrieve", "/api/custom_fields/", "custom_field_id")
    out += list_cases("saved_views.list", "saved_views.list", "/api/saved_views/", ["name"], name_filter=False)
    out += detail_cases("saved_views", "saved_views.retrieve", "/api/saved_views/", "saved_view_id")
    out += list_cases("share_links.list", "share_links.list", "/api/share_links/",
                      ["created", "expiration", "document__title"], name_filter=False, full_perms=False,
                      extra=[{"created__date__gt": "2000-01-01"}, {"expiration__isnull": "true"}])
    out += detail_cases("share_links", "share_links.retrieve", "/api/share_links/", "share_link_id")
    out += list_cases("share_link_bundles.list", "share_link_bundles.list", "/api/share_link_bundles/",
                      ["created", "expiration", "status"], name_filter=False, full_perms=False,
                      extra=[{"documents": "${doc_id}"}, {"documents": "x"}, {"status": "ready"}])
    out += detail_cases("share_link_bundles", "share_link_bundles.retrieve", "/api/share_link_bundles/",
                        "share_link_bundle_id")
    return out


def pl_workflows():
    out = []
    for name, idv in (("workflows", "workflow_id"), ("workflow_triggers", "workflow_trigger_id"),
                      ("workflow_actions", "workflow_action_id")):
        path = f"/api/{name}/"
        out += list_cases(f"{name}.list", f"{name}.list", path, [], name_filter=False, full_perms=False)
        out += detail_cases(name, f"{name}.retrieve", path, idv)
    return out


def pl_search():
    return [
        case("search.global", "search.global", "/api/search/", query={"query": "a"}, users=LIST_USERS, versions=V),
        case("search.global.db_only", "search.global", "/api/search/", query={"query": "a", "db_only": "true"},
             users=["admin", "user1"]),
        case("search.global.short", "search.global", "/api/search/", query={"query": "a"}),
        case("search.global.missing", "search.global", "/api/search/"),
        case("search.global.long", "search.global", "/api/search/", query={"query": "x" * 600}),
        case("search.autocomplete", "search.autocomplete", "/api/search/autocomplete/", query={"term": "a"},
             users=LIST_USERS),
        case("search.autocomplete.limit", "search.autocomplete", "/api/search/autocomplete/",
             query={"term": "th", "limit": 5}),
        case("search.autocomplete.limit-invalid", "search.autocomplete", "/api/search/autocomplete/",
             query={"term": "th", "limit": "x"}),
        case("search.autocomplete.missing", "search.autocomplete", "/api/search/autocomplete/"),
        case("search.documents.query", "documents.list", "/api/documents/",
             query={"query": "the", "page": 1, "page_size": 25, "truncate_content": "true"},
             users=["admin", "user1"], versions=V),
        case("search.documents.query-ordering-score", "documents.list", "/api/documents/",
             query={"query": "the", "ordering": "-score"}),
        case("search.documents.query-page2", "documents.list", "/api/documents/",
             query={"query": "a", "page": 2, "page_size": 5}),
        case("search.documents.query-syntax-error", "documents.list", "/api/documents/", query={"query": "title:("}),
        case("search.documents.query-date-range", "documents.list", "/api/documents/",
             query={"query": "created:[2020 to"}),
        case("search.documents.more_like", "documents.list", "/api/documents/",
             query={"more_like_id": "${doc_id}", "page": 1, "page_size": 10}, users=["admin", "user1"]),
        case("search.documents.more_like-unknown", "documents.list", "/api/documents/",
             query={"more_like_id": BAD_ID}),
        case("search.documents.more_like-invalid", "documents.list", "/api/documents/",
             query={"more_like_id": "abc"}),
        case("search.documents.query-with-filter", "documents.list", "/api/documents/",
             query={"query": "a", "tags__id__all": "${tag_id}"}),
    ]


def pl_system():
    out = list_cases("tasks.list", "tasks.list", "/api/tasks/",
                     ["date_created", "date_done", "status", "task_type", "duration_seconds", "wait_time_seconds"],
                     name_filter=False, full_perms=False,
                     extra=[{"acknowledged": "false"}, {"status": "FAILURE"}, {"task_type": "consume_file"},
                            {"is_complete": "true"}, {"name": "a"}, {"result": "x"},
                            {"task_id": "${task_uuid}"}, {"task_name": "consume_file"}, {"type": "auto_task"},
                            {"date_created_after": "2020-01-01T00:00:00Z"}, {"status": "NOPE"}])
    out += detail_cases("tasks", "tasks.retrieve", "/api/tasks/", "task_pk")
    out += [
        case("tasks.status_counts", "tasks.status_counts", "/api/tasks/status_counts/", query={"acknowledged": "false"},
             users=LIST_USERS, versions=V),
        case("tasks.summary", "tasks.summary", "/api/tasks/summary/", users=["admin", "user1"]),
        case("tasks.summary.days", "tasks.summary", "/api/tasks/summary/", query={"days": 7}),
        case("tasks.summary.days-invalid", "tasks.summary", "/api/tasks/summary/", query={"days": "x"}),
        case("tasks.active", "tasks.active", "/api/tasks/active/", users=["admin", "user1"]),
        case("statistics", "statistics", "/api/statistics/", users=LIST_USERS + ["user2"], versions=V),
        case("status", "status", "/api/status/", users=["admin", "user1", "anon"], leak_allow=["path"]),
        case("config.list", "config.list", "/api/config/", users=["admin", "user1", "noperm", "anon"]),
        case("config.retrieve", "config.retrieve", "/api/config/${app_config_id}/", users=["admin", "user1"]),
        case("config.retrieve-unknown", "config.retrieve", f"/api/config/{BAD_ID}/"),
        case("logs.list", "logs.list", "/api/logs/", users=["admin", "user1", "anon"]),
        case("logs.retrieve", "logs.retrieve", "/api/logs/${log_name}/", query={"limit": 10}, compare="status+headers",
             leak_allow=["path", "stacktrace", "sql"]),
        case("logs.retrieve.unknown", "logs.retrieve", "/api/logs/doesnotexist/"),
        case("logs.retrieve.traversal", "logs.retrieve", "/api/logs/..%2F..%2Fpaperless/"),
        case("logs.retrieve.limit-invalid", "logs.retrieve", "/api/logs/${log_name}/", query={"limit": "x"},
             compare="status"),
        case("remote_version", "remote_version", "/api/remote_version/", users=["admin", "user1", "anon"]),
    ]
    out += auth_and_version_cases("statistics", "statistics", "/api/statistics/")
    return out


def pl_mail():
    out = []
    for name, idv, ordering in (("mail_accounts", "mail_account_id", []), ("mail_rules", "mail_rule_id", []),
                                ("processed_mail", "processed_mail_id", [])):
        path = f"/api/{name}/"
        extra = [{"rule": "${mail_rule_id}"}, {"status": "SUCCESS"}] if name == "processed_mail" else None
        out += list_cases(f"{name}.list", f"{name}.list", path, ordering, name_filter=False,
                          full_perms=name != "processed_mail", extra=extra)
        out += detail_cases(name, f"{name}.retrieve", path, idv)
    out += [
        case("mail.oauth.callback-empty", "oauth.callback", "/api/oauth/callback/", users=["admin", "anon"]),
        case("mail.oauth.callback-bad-state", "oauth.callback", "/api/oauth/callback/",
             query={"code": "x", "state": "invalid"}),
        case("mail_accounts.test.get", "mail_accounts.test", "/api/mail_accounts/test/"),
    ]
    return out


def pl_auth_api():
    out = list_cases("users.list", "users.list", "/api/users/", ["username"], name_filter=False,
                     extra=[{"username__icontains": "a"}, {"username__iexact": "admin"}], id_var=None)
    out += detail_cases("users", "users.retrieve", "/api/users/", "user_id")
    out += list_cases("groups.list", "groups.list", "/api/groups/", ["name"])
    out += detail_cases("groups", "groups.retrieve", "/api/groups/", "group_id")
    out += [
        case("profile", "profile", "/api/profile/", users=["admin", "user1", "noperm", "anon"], leak_allow=["token"]),
        case("profile.session", "profile", "/api/profile/", users=["admin@session", "admin@basic"],
             leak_allow=["token"]),
        case("profile.totp", "profile.totp", "/api/profile/totp/", users=["admin", "user1"], compare="status"),
        case("profile.social_account_providers", "profile.social_account_providers",
             "/api/profile/social_account_providers/", users=["admin", "user1", "anon"]),
        case("profile.generate_auth_token.get", "profile.generate_auth_token", "/api/profile/generate_auth_token/"),
        case("profile.disconnect_social_account.get", "profile.disconnect_social_account",
             "/api/profile/disconnect_social_account/"),
        case("ui_settings", "ui_settings", "/api/ui_settings/", users=["admin", "user1", "user2", "noperm", "anon"],
             versions=V),
        case("ui_settings.session", "ui_settings", "/api/ui_settings/", users=["admin@session"]),
        case("token.get", "token", "/api/token/", users=["anon"]),
        case("token.bad-credentials", "token", "/api/token/", method="POST", users=["anon"],
             json={"username": "no-such-user", "password": "wrong"}),
        case("token.missing-fields", "token", "/api/token/", method="POST", users=["anon"], json={}),
    ]
    out += auth_and_version_cases("ui_settings", "ui_settings", "/api/ui_settings/")
    return out


def pl_server():
    return [
        case("server.api-root", "api.root", "/api/", users=["admin", "anon"], versions=["10", "none"]),
        case("server.schema", "schema", "/api/schema/", users=["admin", "anon"], compare="status+headers"),
        case("server.schema-view", "schema.view", "/api/schema/view/", users=["admin"], compare="status+headers"),
        case("server.favicon", "favicon", "/favicon.ico", users=["anon"]),
        case("server.assets", "assets", "/assets/logo.svg", users=["anon"]),
        case("server.index", "index", "/", users=["anon", "admin@session"], compare="status+headers"),
        case("server.index-deep", "index", "/documents/1/details", users=["anon"], compare="status+headers"),
        case("server.accounts-login", "accounts.login", "/accounts/login/", users=["anon"]),
        case("server.unknown-api", "api.unknown", "/api/does_not_exist/", users=["admin", "anon"]),
        case("server.unknown-api-deep", "api.unknown", "/api/documents/1/does_not_exist/", users=["admin"]),
        case("server.trailing-slash", "documents.list", "/api/documents", users=["admin"]),
        case("server.options", "documents.list", "/api/documents/", method="OPTIONS", users=["admin"]),
        case("server.head", "documents.list", "/api/documents/", method="HEAD", users=["admin"]),
        case("server.admin", "admin", "/admin/", users=["anon"], compare="status+headers"),
        case("server.headless-config", "auth.headless", "/api/auth/headless/browser/v1/config", users=["anon"]),
        case("server.ws-plain-get", "ws.status", "/ws/status/", users=["admin"], compare="status"),
    ]


CRATES = {
    "pl-objects": pl_objects,
    "pl-documents": pl_documents,
    "pl-docfilter": pl_docfilter,
    "pl-files": pl_files,
    "pl-documents-write": pl_documents_write,
    "pl-fields-views": pl_fields_views,
    "pl-workflows": pl_workflows,
    "pl-search": pl_search,
    "pl-system": pl_system,
    "pl-mail": pl_mail,
    "pl-auth-api": pl_auth_api,
    "pl-server": pl_server,
}


def used_vars(obj) -> set[str]:
    import re
    return set(re.findall(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", json.dumps(obj)))


def main() -> None:
    total = 0
    for crate, fn in CRATES.items():
        cs = fn()
        for c in cs:
            if c.get("expect_status") is None:
                c.pop("expect_status", None)
        names = used_vars(cs)
        doc = {
            "crate": crate,
            "generated_by": "tests/parity/harness/gen_base_catalog.py (do not edit; add cases.json instead)",
            "vars": {k: VARS[k] for k in sorted(names)},
            "cases": cs,
        }
        d = PARITY / crate
        d.mkdir(exist_ok=True)
        (d / "cases.base.json").write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
        n = sum(len(c["users"]) * len(c["api_versions"]) for c in cs)
        total += n
        print(f"{crate}: {len(cs)} cases, {n} expanded")
    print(f"total expanded: {total}")


if __name__ == "__main__":
    main()
