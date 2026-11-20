#!/usr/bin/env python3
"""Generates cases.json: write parity for the endpoints added for standalone mode
(docs/STANDALONE-GAPS.md): config.update/partial_update/destroy, storage_paths.create/update/
partial_update, users.destroy. Run against the W2 copy only (write cases reset the snapshot).

    python -I tests/parity/standalone-gaps/gen_cases.py
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
B = "----paperrustgaps7"

CFG_SQL = ("select id, app_title, app_logo, pages, language, deskew, rotate_pages_threshold, user_args, "
           "barcode_tag_mapping, llm_api_key, remote_ocr_api_key, output_type, barcode_upscale, mode, ai_enabled, "
           "llm_context_size, llm_endpoint from paperless_applicationconfiguration order by id")
TASK_SQL = ("select task_type, trigger_source, status, owner_id, input_data, acknowledged from documents_paperlesstask "
            "where date_created > now() - interval '10 minutes' order by id")
SP_SQL = "select id, name, path, match, matching_algorithm, is_insensitive, owner_id from documents_storagepath order by id"
SP_PERM_SQL = ("select ct.model, o.object_pk, o.user_id as who, 'u' as kind, p.codename from guardian_userobjectpermission o "
               "join auth_permission p on p.id = o.permission_id join django_content_type ct on ct.id = o.content_type_id "
               "where ct.model = 'storagepath' union all select ct.model, o.object_pk, o.group_id, 'g', p.codename "
               "from guardian_groupobjectpermission o join auth_permission p on p.id = o.permission_id "
               "join django_content_type ct on ct.id = o.content_type_id where ct.model = 'storagepath' order by 1, 2, 4, 3, 5")
USER_SQL = [
    "select id, username from auth_user order by id",
    "select 'doc' as t, owner_id, count(*) from documents_document group by owner_id union all "
    "select 'note', user_id, count(*) from documents_note group by user_id union all "
    "select 'sp', owner_id, count(*) from documents_storagepath group by owner_id union all "
    "select 'tag', owner_id, count(*) from documents_tag group by owner_id union all "
    "select 'corr', owner_id, count(*) from documents_correspondent group by owner_id union all "
    "select 'view', owner_id, count(*) from documents_savedview group by owner_id union all "
    "select 'task', owner_id, count(*) from documents_paperlesstask group by owner_id union all "
    "select 'mailacc', owner_id, count(*) from paperless_mail_mailaccount group by owner_id union all "
    "select 'log', actor_id, count(*) from auditlog_logentry group by actor_id order by 1, 2",
    "select user_id, count(*) from authtoken_token group by 1 union all select -1, count(*) from documents_uisettings "
    "union all select -2, count(*) from guardian_userobjectpermission union all select -3, count(*) from auth_user_groups order by 1",
    "select id, user_id, settings -> 'permissions' as perms from documents_uisettings order by id",
]


def mp(parts):
    """multipart/form-data body; parts = [(name, filename|None, content_type|None, text)]"""
    out = []
    for name, filename, ctype, text in parts:
        disp = f'form-data; name="{name}"' + (f'; filename="{filename}"' if filename is not None else "")
        head = f"Content-Disposition: {disp}\r\n" + (f"Content-Type: {ctype}\r\n" if ctype else "")
        out.append(f"--{B}\r\n{head}\r\n{text}\r\n")
    out.append(f"--{B}--\r\n")
    return {"data": "".join(out), "content_type": f"multipart/form-data; boundary={B}"}


SVG = ('<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">'
       '<rect width="10" height="10" fill="#ff0000"/></svg>\n')
BAD_SVG = '<?xml version="1.0"?>\n<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>\n'

cases = []

# ------------------------------------------------------------------------------------------- config
cases += [
    {
        "id": "gaps.config.patch.state", "route": "config.partial_update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/1/", "json": {
                "app_title": "  Gap Title ", "pages": "3.0", "deskew": "yes", "language": "",
                "user_args": "{\"a\": 1, \"b\": [1, 2]}", "barcode_tag_mapping": "{\"TAG:(.*)\": \"\\\\g<1>\"}",
                "llm_api_key": "secret-key", "remote_ocr_api_key": "**********", "rotate_pages_threshold": 12,
                "output_type": "pdfa-2", "barcode_upscale": 1.5, "llm_context_size": 4096,
                "externally_configured_variables": ["ignored"], "id": 99}},
            {"method": "GET", "path": "/api/config/1/"},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"llm_api_key": "", "user_args": "", "language": "deu"}},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.patch.errors", "route": "config.partial_update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/1/", "json": {
                "pages": 0, "image_dpi": 40000, "mode": "x", "barcode_upscale": 0.5, "deskew": "maybe",
                "user_args": {"a": 1}, "barcode_tag_mapping": "{\"a\": 1}", "app_logo": "x.png",
                "llm_context_size": 1.5, "app_title": "x" * 49, "rotate_pages_threshold": "abc",
                "output_type": True, "color_conversion_strategy": "rgb", "language": ["x"]}},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"user_args": "[1, 2]", "llm_endpoint": "ftp://x",
                                                                  "remote_ocr_endpoint": "http://h:99999/"}},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"llm_api_key": "   ", "barcode_tag_mapping": "nope"}},
            {"method": "PATCH", "path": "/api/config/1/", "json": [1, 2]},
            {"method": "PATCH", "path": "/api/config/1/", "data": "{bad", "content_type": "application/json"},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"llm_endpoint": "http://127.0.0.1:11434",
                                                                  "llm_embedding_endpoint": "https://example.org/v1"}},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.put.json", "route": "config.update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PUT", "path": "/api/config/1/", "json": {"app_title": "Put", "barcodes_enabled": False}},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.form", "route": "config.update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/1/", "form": {"app_title": "Form", "pages": "", "language": "", "deskew": "false"}},
            {"method": "PATCH", "path": "/api/config/1/", "form": {"user_args": ""}},
            {"method": "PATCH", "path": "/api/config/1/", "form": {"user_args": "{\"x\": 2}", "llm_api_key": ""}},
            {"method": "PUT", "path": "/api/config/1/", "form": {"app_title": "PutForm"}},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.logo", "route": "config.partial_update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "my logo (1).svg", "image/svg+xml", SVG)])},
            {"method": "GET", "path": "/api/ui_settings/"},
            {"method": "GET", "path": "/logo/logo/my_logo_1.svg"},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "my logo (1).svg", "image/svg+xml", SVG),
                                                              ("app_title", None, None, "Logo")])},
            {"sql": CFG_SQL},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "bad.svg", "image/svg+xml", BAD_SVG)])},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "notes.txt", "text/plain", "hello")])},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "fake.png", "image/png", "not a png")])},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", "empty.png", "image/png", "")])},
            {"method": "PATCH", "path": "/api/config/1/", **mp([("app_logo", None, None, "text-not-file")])},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"app_logo": None}},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.denied", "route": "config.partial_update", "users": ["user1", "noperm", "anon"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/1/", "json": {"app_title": "nope"}},
            {"method": "PUT", "path": "/api/config/1/", "json": {"app_title": "nope"}},
            {"method": "DELETE", "path": "/api/config/1/"},
            {"sql": CFG_SQL},
        ],
    },
    {
        "id": "gaps.config.notfound", "route": "config.partial_update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/config/999/", "json": {"app_title": "x"}},
            {"method": "PATCH", "path": "/api/config/abc/", "json": {"app_title": "x"}},
            {"method": "DELETE", "path": "/api/config/999/"},
        ],
    },
    {
        "id": "gaps.config.destroy", "route": "config.destroy", "users": ["admin"], "write": True,
        "steps": [
            {"method": "DELETE", "path": "/api/config/1/"},
            {"method": "DELETE", "path": "/api/config/1/"},
            {"sql": "select count(*) from paperless_applicationconfiguration"},
        ],
    },
    {
        "id": "gaps.config.granted-user", "route": "config.partial_update", "users": ["user1"], "write": True,
        "steps": [
            {"user": "admin", "method": "PATCH", "path": "/api/users/3/",
             "json": {"user_permissions": ["change_applicationconfiguration", "view_applicationconfiguration"]}},
            {"method": "PATCH", "path": "/api/config/1/", "json": {"app_title": "by alice"}},
            {"method": "DELETE", "path": "/api/config/1/"},
            {"sql": CFG_SQL},
        ],
    },
]

# ------------------------------------------------------------------------------------ storage paths
cases += [
    {
        "id": "gaps.sp.create.state", "route": "storage_paths.create", "users": ["admin", "user1"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/storage_paths/", "json": {
                "name": "gap-sp", "path": "gaps/{{ created_year }}/{{ title }}", "match": "invoice",
                "matching_algorithm": 1, "is_insensitive": False,
                "set_permissions": {"view": {"users": [4], "groups": []}, "change": {"users": [], "groups": [1]}}}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "gap-old", "path": "{created_year}/{title}", "owner": None}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "gap-sp", "path": "x/{{ title }}"}},
            {"sql": SP_SQL},
            {"sql": SP_PERM_SQL},
        ],
    },
    {
        "id": "gaps.sp.create.errors", "route": "storage_paths.create", "users": ["admin"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "a", "path": "{{ nonexistent_var }}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "b", "path": ""}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "c", "path": "/etc/{{ title }}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "d", "path": "../{{ title }}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "e", "path": "{{ title | nofilter }}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "f", "path": "{% for %}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "", "path": None, "matching_algorithm": 9}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"path": "{{ title }}"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "g", "path": "{{ title }}", "match": "(", "matching_algorithm": 4}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "h", "path": 5}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "By correspondent and year", "path": "{{ title }}", "owner": None}},
            {"sql": SP_SQL},
        ],
    },
    {
        "id": "gaps.sp.update.state", "route": "storage_paths.partial_update", "users": ["admin"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/storage_paths/1/", "json": {"path": "{{ correspondent }}/{{ created_year }}/{{ asn }}-{{ title }}"}},
            {"method": "PATCH", "path": "/api/storage_paths/2/", "json": {"name": "Taxes renamed"}},
            {"method": "PUT", "path": "/api/storage_paths/4/", "json": {"name": "Flat ASN v2", "path": "{asn}/{title}"}},
            {"method": "PUT", "path": "/api/storage_paths/4/", "json": {"name": "Flat ASN v3"}},
            {"method": "PATCH", "path": "/api/storage_paths/1/", "json": {"path": "{{ nope }}"}},
            {"method": "PATCH", "path": "/api/storage_paths/999/", "json": {"name": "x"}},
            {"sql": SP_SQL},
            {"sql": TASK_SQL},
        ],
    },
    {
        "id": "gaps.sp.update.perms", "route": "storage_paths.partial_update", "users": ["user1", "user2", "noperm", "anon"], "write": True,
        "steps": [
            {"method": "PATCH", "path": "/api/storage_paths/3/", "json": {"name": "alice renamed"}},
            {"method": "PATCH", "path": "/api/storage_paths/5/", "json": {"name": "bob renamed"}},
            {"method": "POST", "path": "/api/storage_paths/", "json": {"name": "by-user", "path": "{{ title }}"}},
            {"method": "PATCH", "path": "/api/storage_paths/3/", "json": {"owner": 2}},
            {"sql": SP_SQL},
        ],
    },
    {
        "id": "gaps.sp.form", "route": "storage_paths.create", "users": ["admin"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/storage_paths/", "form": {"name": "form-sp", "path": "{{ title }}"}},
            {"sql": SP_SQL},
        ],
    },
]

# -------------------------------------------------------------------------------------------- users
cases += [
    {
        "id": "gaps.users.destroy.state", "route": "users.destroy", "users": ["admin"], "write": True,
        "steps": [
            {"user": "admin", "method": "POST", "path": "/api/ui_settings/", "json": {"settings": {
                "permissions": {"default_owner": 4, "default_view_users": [4, 3, 4], "default_change_users": [4],
                                "default_view_groups": [], "default_change_groups": []}}}},
            {"method": "DELETE", "path": "/api/users/4/"},
            {"method": "DELETE", "path": "/api/users/4/"},
            {"method": "DELETE", "path": "/api/users/6/"},
            {"method": "DELETE", "path": "/api/users/abc/"},
            {"method": "DELETE", "path": "/api/users/1/"},
            {"method": "GET", "path": "/api/users/"},
        ] + [{"sql": s} for s in USER_SQL],
    },
    {
        "id": "gaps.users.destroy.denied", "route": "users.destroy", "users": ["user1", "noperm", "anon"], "write": True,
        "steps": [
            {"method": "DELETE", "path": "/api/users/4/"},
            {"method": "DELETE", "path": "/api/users/2/"},
            {"sql": USER_SQL[0]},
        ],
    },
    {
        "id": "gaps.users.destroy.superuser-guard", "route": "users.destroy", "users": ["user1"], "write": True,
        "steps": [
            {"user": "admin", "method": "PATCH", "path": "/api/users/3/", "json": {"user_permissions": ["delete_user", "view_user"]}},
            {"method": "DELETE", "path": "/api/users/2/"},
            {"method": "DELETE", "path": "/api/users/5/"},
            {"sql": USER_SQL[0]},
        ],
    },
]

# --------------------------------------------------------------------------------- mail OAuth
# Needs PAPERLESS_GMAIL_OAUTH_CLIENT_ID/_SECRET and PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID/_SECRET for
# A and B (otherwise ui_settings has no *_oauth_url and the callback can never pass the state check).
cases += [
    {
        "id": "gaps.oauth.ui_settings", "route": "ui_settings.get", "users": ["admin", "user1"], "write": True,
        "note": "state= is random per request (accepted rule oauth-state-random)",
        "steps": [{"method": "GET", "path": "/api/ui_settings/"}],
    },
    {
        "id": "gaps.oauth.callback.errors", "route": "oauth.callback", "users": ["admin", "user1", "noperm", "anon"], "write": True,
        "steps": [
            {"method": "GET", "path": "/api/oauth/callback/"},
            {"method": "GET", "path": "/api/oauth/callback/", "query": {"code": "abc"}},
            {"method": "GET", "path": "/api/oauth/callback/", "query": {"code": "abc", "state": "wrong"}},
            {"method": "GET", "path": "/api/ui_settings/"},
            {"method": "GET", "path": "/api/oauth/callback/", "query": {"code": "abc", "state": "wrong", "scope": "openid"}},
            {"method": "GET", "path": "/api/oauth/callback/", "query": {"code": "abc", "state": ""}},
            {"method": "POST", "path": "/api/oauth/callback/", "json": {}},
            {"sql": "select count(*) from paperless_mail_mailaccount"},
        ],
    },
]

# ------------------------------------------------------------------- wrong method (standalone 405)
cases += [
    {
        "id": "gaps.method.not_allowed", "route": "profile.get", "users": ["admin", "noperm", "anon"], "write": True,
        "steps": [
            {"method": "PUT", "path": "/api/profile/", "json": {}},
            {"method": "DELETE", "path": "/api/tags/"},
            {"method": "PATCH", "path": "/api/tasks/1/", "json": {}},
            {"method": "PUT", "path": "/api/documents/selection_data/", "json": {}},
        ],
    },
]

# ------------------------------------------------- §4.1 select all with an active filter
TASK_ROWS = ("select task_type, trigger_source, status, owner_id, input_data from documents_paperlesstask "
             "where date_created > now() - interval '10 minutes' order by id")
TAG_SQL = "select document_id, tag_id from documents_document_tags where tag_id = 6 order by 1"
SEL = {"all": True, "filters": {"tags__id__all": "2", "is_tagged": 1, "bogus_field": "x", "id__in": "nope"},
       "excluded_documents": [3]}
cases += [
    {
        "id": "gaps.selall.bulk_edit", "route": "documents.bulk_edit", "users": ["admin", "user1"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {**SEL, "method": "add_tag", "parameters": {"tag": 6}}},
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {"all": True, "filters": {"title_content": "invoice", "correspondent__id__in": "1,2"},
                      "method": "remove_tag", "parameters": {"tag": 6}}},
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {"all": True, "filters": {"query": "invoice"}, "method": "add_tag", "parameters": {"tag": 6}}},
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {"all": True, "filters": {"text": "a", "title_search": "b"}, "method": "add_tag", "parameters": {"tag": 6}}},
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {"all": True, "filters": {"more_like_id": 999999}, "method": "add_tag", "parameters": {"tag": 6}}},
            {"method": "POST", "path": "/api/documents/bulk_edit/",
             "json": {"all": True, "filters": {"created__date__gt": "2024-01-01"}, "method": "set_correspondent",
                      "parameters": {"correspondent": None}}},
            {"sql": TAG_SQL},
            {"sql": "select id, correspondent_id from documents_document order by id"},
            {"sql": TASK_ROWS},
        ],
    },
    {
        "id": "gaps.selall.selection_data", "route": "documents.selection_data", "users": ["admin", "user1", "user2"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/documents/selection_data/", "json": SEL},
            {"method": "POST", "path": "/api/documents/selection_data/", "json": {"all": True, "filters": {"query": "tax"}}},
            {"method": "POST", "path": "/api/documents/selection_data/", "json": {"all": True, "filters": {}}},
            {"method": "POST", "path": "/api/documents/selection_data/", "data": "{bad", "content_type": "application/json"},
        ],
    },
    {
        "id": "gaps.selall.ops", "route": "documents.delete", "users": ["admin"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/documents/reprocess/", "json": {"all": True, "filters": {"document_type__id": 2}}},
            {"method": "POST", "path": "/api/documents/rotate/", "json": {"all": True, "filters": {"id__in": "1,2"}, "degrees": 90}},
            {"method": "POST", "path": "/api/documents/delete/", "json": {"all": True, "filters": {"tags__id__all": "5"}}},
            {"sql": "select id, deleted_at is not null as del from documents_document order by id"},
            {"sql": TASK_ROWS},
        ],
    },
    {
        "id": "gaps.selall.bulk_download", "route": "documents.bulk_download", "users": ["admin", "user1"], "write": True,
        "compare": "status+headers",
        "steps": [
            {"method": "POST", "path": "/api/documents/bulk_download/", "json": {"all": True, "filters": {"id__in": "1,2,3"}, "excluded_documents": [2]}},
            {"method": "POST", "path": "/api/documents/bulk_download/", "json": {"all": True, "filters": {"more_like_id": "x"}}},
        ],
    },
]

# ------------------------------------------- §4.3 select custom field option update (task)
cases += [
    {
        "id": "gaps.cf_select.update", "route": "custom_fields.partial_update", "users": ["admin"], "write": True,
        "note": "response only; the task runs only in a worker (B standalone: PaperRust queue)",
        "steps": [
            {"method": "PATCH", "path": "/api/custom_fields/10/", "json": {"extra_data": {"select_options": [
                {"id": "pr1LowAAAAAAAAAA", "label": "Low"}, {"id": "pr3HighAAAAAAAAA", "label": "Very high"}]}}},
        ],
    },
]

# --------------------------------------------------------------- §4.4 form bodies (pl-objects)
cases += [
    {
        "id": "gaps.form.objects", "route": "tags.create", "users": ["admin", "user1"], "write": True,
        "steps": [
            {"method": "POST", "path": "/api/tags/", "form": {"name": "form-tag", "color": "#112233", "matching_algorithm": "1", "match": "x"}},
            {"method": "POST", "path": "/api/tags/", "form": {"name": "form-tag2", "is_insensitive": "true", "owner": "", "parent": "2", "is_inbox_tag": ""}},
            {"method": "POST", "path": "/api/correspondents/", "form": {"name": "form-corr", "matching_algorithm": "", "owner": "3"}},
            {"method": "POST", "path": "/api/document_types/", "form": {"name": ""}},
            {"method": "POST", "path": "/api/tags/", **mp([("name", None, None, "mp-tag"), ("color", None, None, "#445566")])},
            {"method": "PATCH", "path": "/api/tags/2/", "form": {"name": "Finance renamed", "parent": ""}},
            {"method": "PUT", "path": "/api/document_types/1/", "form": {"name": "put form"}},
            {"method": "PATCH", "path": "/api/correspondents/1/", "form": {"matching_algorithm": "9"}},
            {"sql": "select 'tag' t, id, name, color, matching_algorithm, match, is_insensitive, is_inbox_tag, owner_id, tn_parent_id "
                    "from documents_tag union all select 'corr', id, name, null, matching_algorithm, match, is_insensitive, null, owner_id, null "
                    "from documents_correspondent union all select 'type', id, name, null, matching_algorithm, match, is_insensitive, null, owner_id, null "
                    "from documents_documenttype order by 1, 2"},
        ],
    },
]

# ------------------------------------------------------------------ §5 prefix views
cases += [
    {
        "id": "gaps.prefix_views", "route": "statistics.get", "users": ["admin"], "write": True,
        "steps": [
            {"method": "GET", "path": "/api/statistics/extra"},
            {"method": "GET", "path": "/api/search/autocomplete/x", "query": {"term": "inv"}},
            {"method": "POST", "path": "/api/documents/selection_data/more", "json": {"documents": [1]}},
            {"method": "GET", "path": "/api/oauth/callback/x/"},
            {"method": "GET", "path": "/api/ui_settings/abc"},
        ],
    },
]

for c in cases:
    c.setdefault("api_versions", ["10"])

out = {
    "crate": "standalone-gaps",
    "_doc": "Generated by gen_cases.py - docs/STANDALONE-GAPS.md. Write cases: run only against a W2 copy.",
    "cases": cases,
}
(HERE / "cases.json").write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"{len(cases)} cases")
