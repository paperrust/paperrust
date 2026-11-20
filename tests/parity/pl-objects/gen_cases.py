#!/usr/bin/env python3
"""Generates tests/parity/pl-objects/cases.json (crate-owned catalogue, complements cases.base.json).

Run from the repo root: python tests/parity/pl-objects/gen_cases.py
"""

import json
from pathlib import Path

U = ["admin", "user1", "user2", "noperm"]
VAR_OF = {"correspondents": "correspondent_id", "document_types": "document_type_id", "tags": "tag_id"}
cases: list[dict] = []


def c(**kw):
    kw.setdefault("users", ["admin"])
    kw.setdefault("api_versions", ["10"])
    cases.append(kw)


W = {"write": True}

# --- reads beyond the base catalogue ---------------------------------------------------------
c(id="correspondents.list.last-correspondence", route="correspondents.list", method="GET", path="/api/correspondents/",
  query={"last_correspondence": "true", "page_size": 100}, users=["admin", "user1", "user2"])
c(id="correspondents.list.last-correspondence-false", route="correspondents.list", method="GET",
  path="/api/correspondents/", query={"last_correspondence": "false"})
c(id="correspondents.list.order-last-corr-annotated", route="correspondents.list", method="GET",
  path="/api/correspondents/", query={"last_correspondence": "1", "ordering": "-last_correspondence"})
c(id="correspondents.detail.full-perms", route="correspondents.retrieve", method="GET",
  path="/api/correspondents/${correspondent_id}/", query={"full_perms": "true"}, users=["admin", "user1"])
c(id="tags.list.is-root", route="tags.list", method="GET", path="/api/tags/", query={"is_root": "true"},
  users=["admin", "user1"], api_versions=["10", "9"])
c(id="tags.list.is-root-false", route="tags.list", method="GET", path="/api/tags/", query={"is_root": "0"})
c(id="tags.list.is-root-garbage", route="tags.list", method="GET", path="/api/tags/", query={"is_root": "maybe"})
c(id="tags.list.children-page", route="tags.list", method="GET", path="/api/tags/",
  query={"page_size": 1, "ordering": "-name"}, users=["admin", "user1"], api_versions=["10", "9"])
c(id="tags.list.full-perms-children", route="tags.list", method="GET", path="/api/tags/",
  query={"full_perms": "true", "page_size": 100}, users=["admin", "user1"])
c(id="tags.list.id-decimal", route="tags.list", method="GET", path="/api/tags/", query={"id": "1.5"})
c(id="tags.list.id-exp", route="tags.list", method="GET", path="/api/tags/", query={"id": "1e0"})
c(id="tags.list.name-like-escape", route="tags.list", method="GET", path="/api/tags/", query={"name__icontains": "%_"})
c(id="tags.detail.ordering-children", route="tags.retrieve", method="GET", path="/api/tags/${tag_id}/",
  query={"ordering": "-document_count"}, users=["admin", "user1"])
c(id="tags.detail.full-perms", route="tags.retrieve", method="GET", path="/api/tags/${tag_id}/",
  query={"full_perms": "1"}, users=["admin", "user1"])
c(id="tags.detail.dot-pk", route="tags.retrieve", method="GET", path="/api/tags/1.5/")
c(id="tags.detail.huge-pk", route="tags.retrieve", method="GET", path="/api/tags/99999999999/")
c(id="storage_paths.detail.full-perms", route="storage_paths.retrieve", method="GET",
  path="/api/storage_paths/${storage_path_id}/", query={"full_perms": "true"}, users=["admin", "user1"])

# --- writes (snapshot reset around A and B) --------------------------------------------------
for k in ("correspondents", "document_types", "tags"):
    p = f"/api/{k}/"
    d = f"/api/{k}/${{{VAR_OF[k]}}}/"
    c(id=f"{k}.create.ok", route=f"{k}.create", method="POST", path=p,
      json={"name": "Parity New", "matching_algorithm": 1, "match": "foo"}, users=U, **W)
    c(id=f"{k}.create.full-perms", route=f"{k}.create", method="POST", path=p, query={"full_perms": "true"},
      json={"name": "Parity Perms", "set_permissions": {"view": {"users": [], "groups": []},
                                                        "change": {"users": [], "groups": []}}}, **W)
    c(id=f"{k}.create.owner-null", route=f"{k}.create", method="POST", path=p,
      json={"name": "Parity Unowned", "owner": None}, users=["admin", "user1"], **W)
    c(id=f"{k}.create.missing-name", route=f"{k}.create", method="POST", path=p, json={"match": "x"}, **W)
    c(id=f"{k}.create.blank-name", route=f"{k}.create", method="POST", path=p, json={"name": "   "}, **W)
    c(id=f"{k}.create.long-name", route=f"{k}.create", method="POST", path=p, json={"name": "x" * 129}, **W)
    c(id=f"{k}.create.bad-types", route=f"{k}.create", method="POST", path=p,
      json={"name": ["a"], "matching_algorithm": 9, "is_insensitive": "perhaps", "owner": "abc"}, **W)
    c(id=f"{k}.create.bad-owner", route=f"{k}.create", method="POST", path=p, json={"name": "P", "owner": 999999}, **W)
    c(id=f"{k}.create.bad-regex", route=f"{k}.create", method="POST", path=p,
      json={"name": "P", "matching_algorithm": 4, "match": "(unclosed"}, **W)
    c(id=f"{k}.create.good-regex", route=f"{k}.create", method="POST", path=p,
      json={"name": "P", "matching_algorithm": 4, "match": "^inv\\d+$"}, **W)
    c(id=f"{k}.create.regex-as-string-algo", route=f"{k}.create", method="POST", path=p,
      json={"name": "P", "matching_algorithm": "4", "match": "(unclosed"}, **W)
    c(id=f"{k}.create.set-perms-unknown", route=f"{k}.create", method="POST", path=p,
      json={"name": "P", "set_permissions": {"own": {}}}, **W)
    c(id=f"{k}.create.set-perms-bad-user", route=f"{k}.create", method="POST", path=p,
      json={"name": "P", "set_permissions": {"view": {"users": [999999]}}}, **W)
    c(id=f"{k}.create.not-a-dict", route=f"{k}.create", method="POST", path=p, json=[1, 2], **W)
    c(id=f"{k}.create.duplicate", route=f"{k}.create", method="POST", path=p, steps=[
        {"method": "POST", "path": p, "json": {"name": "Dup"}},
        {"method": "POST", "path": p, "json": {"name": "Dup"}},
        {"method": "POST", "path": p, "json": {"name": "Dup", "owner": None}},
        {"method": "POST", "path": p, "json": {"name": "Dup", "owner": None}},
    ], users=["admin", "user1"], **W)
    c(id=f"{k}.patch.name", route=f"{k}.partial_update", method="PATCH", path=d,
      json={"name": "Renamed by parity"}, users=U, **W)
    c(id=f"{k}.patch.owner", route=f"{k}.partial_update", method="PATCH", path=d, json={"owner": None},
      users=["admin", "user1", "user2"], **W)
    c(id=f"{k}.patch.set-perms", route=f"{k}.partial_update", method="PATCH", path=d, query={"full_perms": "true"},
      json={"set_permissions": {"view": {"users": [], "groups": []}, "change": {"users": [], "groups": []}}},
      users=["admin", "user1", "user2"], **W)
    c(id=f"{k}.put.no-name", route=f"{k}.update", method="PUT", path=d, json={"match": "x"}, **W)
    c(id=f"{k}.put.full", route=f"{k}.update", method="PUT", path=d,
      json={"name": "Put by parity", "match": "a b", "matching_algorithm": 2, "is_insensitive": False},
      users=["admin", "user1"], **W)
    c(id=f"{k}.delete", route=f"{k}.destroy", method="DELETE", path=d, users=U, **W)
    c(id=f"{k}.delete-then-get", route=f"{k}.destroy", method="DELETE", path=d,
      steps=[{"method": "DELETE", "path": d}, {"method": "GET", "path": d}], **W)

# tag specifics (treenode, parent propagation)
c(id="tags.create.color", route="tags.create", method="POST", path="/api/tags/",
  json={"name": "Colored", "color": "#ff0000", "is_inbox_tag": True}, **W)
c(id="tags.create.bad-color", route="tags.create", method="POST", path="/api/tags/",
  json={"name": "Colored", "color": "red"}, **W)
c(id="tags.create.child", route="tags.create", method="POST", path="/api/tags/",
  json={"name": "Child by parity", "parent": "${tag_id}"}, **W)
c(id="tags.create.bad-parent", route="tags.create", method="POST", path="/api/tags/",
  json={"name": "Orphan", "parent": 999999}, **W)
c(id="tags.patch.parent-self", route="tags.partial_update", method="PATCH", path="/api/tags/${tag_id}/",
  json={"parent": "${tag_id}"}, **W)
c(id="tags.patch.parent", route="tags.partial_update", method="PATCH", path="/api/tags/${tag_id_last}/",
  json={"parent": "${tag_id}"}, **W)
c(id="tags.patch.unparent", route="tags.partial_update", method="PATCH", path="/api/tags/${tag_id_last}/",
  json={"parent": None}, **W)
c(id="tags.create.chain-depth", route="tags.create", method="POST", path="/api/tags/", steps=[
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D1"}, "capture": {"d1": "id"}},
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D2", "parent": "${d1}"}, "capture": {"d2": "id"}},
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D3", "parent": "${d2}"}, "capture": {"d3": "id"}},
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D4", "parent": "${d3}"}, "capture": {"d4": "id"}},
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D5", "parent": "${d4}"}, "capture": {"d5": "id"}},
    {"method": "POST", "path": "/api/tags/", "json": {"name": "D6", "parent": "${d5}"}},
    {"method": "PATCH", "path": "/api/tags/${d1}/", "json": {"parent": "${d3}"}},
    {"method": "GET", "path": "/api/tags/${d1}/"},
    {"method": "DELETE", "path": "/api/tags/${d2}/"},
    {"method": "GET", "path": "/api/tags/", "query": {"name__istartswith": "D"}},
], **W)
c(id="storage_paths.delete", route="storage_paths.destroy", method="DELETE",
  path="/api/storage_paths/${storage_path_id}/", users=U, **W)

# --- /api/bulk_edit_objects/ ----------------------------------------------------------------
B = "/api/bulk_edit_objects/"
c(id="bulk.missing-fields", route="bulk_edit_objects", method="POST", path=B, json={},
  users=["admin", "noperm", "anon"], **W)
c(id="bulk.bad-choices", route="bulk_edit_objects", method="POST", path=B,
  json={"object_type": "x", "operation": "y", "objects": "1"}, **W)
c(id="bulk.objects-required", route="bulk_edit_objects", method="POST", path=B,
  json={"object_type": "tags", "operation": "delete"}, **W)
c(id="bulk.objects-empty", route="bulk_edit_objects", method="POST", path=B,
  json={"object_type": "tags", "operation": "delete", "objects": []}, **W)
c(id="bulk.objects-unknown", route="bulk_edit_objects", method="POST", path=B,
  json={"object_type": "tags", "operation": "delete", "objects": [999999]}, **W)
c(id="bulk.perms-empty", route="bulk_edit_objects", method="POST", path=B,
  json={"object_type": "tags", "operation": "set_permissions", "objects": ["${tag_id}"], "permissions": {}}, **W)
for ot, var in (("tags", "tag_id"), ("correspondents", "correspondent_id"),
                ("document_types", "document_type_id"), ("storage_paths", "storage_path_id")):
    ref = "${" + var + "}"
    c(id=f"bulk.{ot}.set-perms", route="bulk_edit_objects", method="POST", path=B,
      json={"object_type": ot, "operation": "set_permissions", "objects": [ref], "owner": None,
            "permissions": {"view": {"users": [], "groups": []}}, "merge": False}, users=U, **W)
    c(id=f"bulk.{ot}.set-perms-merge", route="bulk_edit_objects", method="POST", path=B,
      json={"object_type": ot, "operation": "set_permissions", "objects": [ref], "owner": None, "merge": True}, **W)
    c(id=f"bulk.{ot}.delete", route="bulk_edit_objects", method="POST", path=B,
      json={"object_type": ot, "operation": "delete", "objects": [ref]}, users=U, **W)
    c(id=f"bulk.{ot}.delete-all-filtered", route="bulk_edit_objects", method="POST", path=B,
      json={"object_type": ot, "operation": "delete", "all": True, "filters": {"name__istartswith": "zzz-none"}},
      users=["admin", "user1"], **W)
c(id="bulk.tags.delete-all", route="bulk_edit_objects", method="POST", path=B, steps=[
    {"method": "POST", "path": B, "json": {"object_type": "tags", "operation": "delete", "all": True}},
    {"method": "GET", "path": "/api/tags/"},
], users=["admin", "user1"], **W)


def var(path: str, order: str) -> dict:
    return {"get": path, "json": "results.0.id", "user": "admin", "query": {"ordering": order, "page_size": 1}}


doc = {
    "crate": "pl-objects",
    "generated_by": "tests/parity/pl-objects/gen_cases.py",
    "vars": {
        "correspondent_id": var("/api/correspondents/", "id"),
        "document_type_id": var("/api/document_types/", "id"),
        "storage_path_id": var("/api/storage_paths/", "id"),
        "tag_id": var("/api/tags/", "id"),
        "tag_id_last": var("/api/tags/", "-id"),
    },
    "cases": cases,
}
out = Path(__file__).with_name("cases.json")
out.write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n", encoding="utf8")
print(f"{len(cases)} cases -> {out}")
