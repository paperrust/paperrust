"""Generates cases-w2.json: W2 write cases with DB state checks (run: python gen_cases_w2.py)."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from w2_sql import AUDIT, GUARDIAN, c, req, sql  # noqa: E402

S = sql
CF = "select id, name, data_type, extra_data, created from documents_customfield order by id"
CFI = ("select id, document_id, field_id, value_text, value_bool, value_url, value_date, value_int, value_float, "
       "value_monetary, value_document_ids, value_select, value_long_text, deleted_at from documents_customfieldinstance order by id")
SV = ("select v.id, v.name, v.sort_field, v.sort_reverse, v.page_size, v.display_mode, v.display_fields, v.owner_id, v.icon, "
      "(select coalesce(json_agg(json_build_array(r.rule_type, r.value) order by r.id), '[]') from documents_savedviewfilterrule r "
      "where r.saved_view_id = v.id) as rules from documents_savedview v order by v.id")
SL = ("select id, deleted_at is not null as deleted, created, expiration, length(slug) as slug_len, file_version, document_id, owner_id "
      "from documents_sharelink order by id")
SLB = ("select b.id, b.created, b.expiration, length(b.slug) as slug_len, b.file_version, b.status, b.size_bytes, b.last_error, "
       "b.file_path, b.built_at, b.owner_id, (select coalesce(array_agg(d.document_id order by d.document_id), '{}') "
       "from documents_sharelinkbundle_documents d where d.sharelinkbundle_id = b.id) as docs from documents_sharelinkbundle b order by b.id")
TASKS = ("select id, task_type, trigger_source, status, owner_id, input_data from documents_paperlesstask where id > 12 order by id")

SEL_OPTS = {"select_options": [{"label": "Red", "id": "r1"}, {"label": "Green", "id": "g1"}]}

cases = [
    # --- custom fields
    c("cf.create.all-types", "custom_fields.create", [
        req("POST", "/api/custom_fields/", {"name": f"w2-{t}", "data_type": t}) for t in
        ["string", "url", "date", "boolean", "integer", "float", "monetary", "documentlink", "longtext"]
    ] + [req("POST", "/api/custom_fields/", {"name": "w2-money-eur", "data_type": "monetary", "extra_data": {"default_currency": "EUR"}}),
         req("POST", "/api/custom_fields/", {"name": "w2-money-bad", "data_type": "monetary", "extra_data": {"default_currency": "EU"}}),
         req("POST", "/api/custom_fields/", {"name": "w2-money-null", "data_type": "monetary", "extra_data": {"default_currency": None}}),
         S(CF), S(AUDIT)]),
    c("cf.create.validation", "custom_fields.create", [
        req("POST", "/api/custom_fields/", {"name": "Invoice amount", "data_type": "string"}),
        req("POST", "/api/custom_fields/", {"name": "x" * 129, "data_type": "string"}),
        req("POST", "/api/custom_fields/", {"name": "nt"}),
        req("POST", "/api/custom_fields/", {"name": "sel-empty", "data_type": "select", "extra_data": {"select_options": []}}),
        req("POST", "/api/custom_fields/", {"name": "sel-dupe", "data_type": "select",
                                            "extra_data": {"select_options": [{"label": "A", "id": "x"}, {"label": "B", "id": "x"}]}}),
        req("POST", "/api/custom_fields/", {"name": "sel-nolabel", "data_type": "select", "extra_data": {"select_options": [{"id": "x"}]}}),
        req("POST", "/api/custom_fields/", {"name": "sel-strings", "data_type": "select", "extra_data": {"select_options": ["A", "B"]}}),
        req("POST", "/api/custom_fields/", {"name": "sel-blank", "data_type": "select", "extra_data": {"select_options": [{"label": ""}]}}),
        req("POST", "/api/custom_fields/", {"name": "ed-list", "data_type": "string", "extra_data": [1]}),
        req("POST", "/api/custom_fields/", data="{bad", content_type="application/json"),
        S(CF)]),
    c("cf.create.form", "custom_fields.create", [req("POST", "/api/custom_fields/", form={"name": "w2-form", "data_type": "integer"}), S(CF)]),
    c("cf.create.denied", "custom_fields.create", [req("POST", "/api/custom_fields/", {"name": "w2-den", "data_type": "string"}), S(CF)],
      users=["user2", "noperm", "anon"]),
    c("cf.update.state", "custom_fields.partial_update", [
        req("PATCH", "/api/custom_fields/1/", {"name": "Renamed string"}),
        req("PATCH", "/api/custom_fields/1/", {"data_type": "integer"}),
        req("PUT", "/api/custom_fields/2/", {"name": "url-put", "data_type": "url"}),
        req("PUT", "/api/custom_fields/2/", {"name": "url-put2"}),
        req("PATCH", "/api/custom_fields/999/", {"name": "nope"}),
        S(CF), S(AUDIT)]),
    c("cf.update.select-options", "custom_fields.partial_update", [
        req("GET", "/api/custom_fields/${cf_select_id}/"),
        req("PATCH", "/api/custom_fields/${cf_select_id}/", {"extra_data": {"select_options": [{"label": "Only", "id": "only-1"}]}}),
        S(CF), S(CFI), S(TASKS)]),
    c("cf.update.monetary-currency", "custom_fields.partial_update", [
        req("POST", "/api/custom_fields/", {"name": "w2-m", "data_type": "monetary"}, capture={"m": "id"}),
        req("PATCH", "/api/custom_fields/${m}/", {"extra_data": {"default_currency": "CHF"}}),
        req("PATCH", "/api/custom_fields/${m}/", {"extra_data": {"default_currency": "chf"}}),
        S(CF)]),
    c("cf.update.denied", "custom_fields.partial_update", [req("PATCH", "/api/custom_fields/1/", {"name": "hijack"}), S(CF)],
      users=["user2", "noperm"]),
    c("cf.destroy.state", "custom_fields.destroy", [
        req("DELETE", "/api/custom_fields/7/"), req("DELETE", "/api/custom_fields/7/"), req("DELETE", "/api/custom_fields/5/"),
        S(CF), S(CFI), S(SV), S(AUDIT)]),
    c("cf.destroy.denied", "custom_fields.destroy", [req("DELETE", "/api/custom_fields/1/"), S(CF)], users=["user1", "user2", "noperm"]),
    # --- saved views
    c("sv.create.state", "saved_views.create", [
        req("POST", "/api/saved_views/", {"name": "w2-view", "sort_field": "custom_field_1", "sort_reverse": False,
                                          "filter_rules": [{"rule_type": 6, "value": "3"}, {"rule_type": 0, "value": "abc"}],
                                          "display_fields": ["title", "custom_field_1", "note"], "display_mode": "table",
                                          "page_size": 50, "icon": "star"}),
        S(SV), S(GUARDIAN)], users=["admin", "user1", "user2"]),
    c("sv.create.set-permissions", "saved_views.create", [
        req("POST", "/api/saved_views/", {"name": "w2-shared", "sort_reverse": True, "filter_rules": [],
                                          "set_permissions": {"view": {"users": [4], "groups": [2]}, "change": {"users": [], "groups": [1]}}}),
        req("GET", "/api/saved_views/", user="user2"), S(SV), S(GUARDIAN)], users=["user1"]),
    c("sv.create.owner", "saved_views.create", [
        req("POST", "/api/saved_views/", {"name": "w2-owner", "sort_reverse": True, "filter_rules": [], "owner": 4}),
        req("POST", "/api/saved_views/", {"name": "w2-owner-null", "sort_reverse": True, "filter_rules": [], "owner": None}),
        S(SV)], users=["admin", "user1"]),
    c("sv.create.validation", "saved_views.create", [
        req("POST", "/api/saved_views/", {"name": "x"}),
        req("POST", "/api/saved_views/", {"sort_reverse": True, "filter_rules": []}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [{"rule_type": 999, "value": "1"}]}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [{"value": "1"}]}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": "nope"}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "display_fields": ["bogus"]}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "display_fields": ["custom_field_999"]}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "display_mode": "huge"}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "page_size": -1}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "page_size": 100001}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": "maybe", "filter_rules": []}),
        req("POST", "/api/saved_views/", {"name": "x" * 129, "sort_reverse": True, "filter_rules": []}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "owner": 999}),
        req("POST", "/api/saved_views/", {"name": "x", "sort_reverse": True, "filter_rules": [], "set_permissions": {"view": {"users": [999]}}}),
        S(SV)]),
    c("sv.create.denied", "saved_views.create", [req("POST", "/api/saved_views/", {"name": "d", "sort_reverse": True, "filter_rules": []}), S(SV)],
      users=["noperm", "anon"]),
    c("sv.update.state", "saved_views.partial_update", [
        req("PATCH", "/api/saved_views/1/", {"name": "Inbox 2", "filter_rules": [{"rule_type": 3, "value": "5"}]}),
        req("PATCH", "/api/saved_views/2/", {"display_fields": []}),
        req("PATCH", "/api/saved_views/4/", {"page_size": None, "sort_field": None}),
        req("PUT", "/api/saved_views/5/", {"name": "put", "sort_reverse": False, "filter_rules": []}),
        req("PUT", "/api/saved_views/5/", {"name": "put-incomplete"}),
        S(SV)]),
    c("sv.update.own", "saved_views.partial_update", [
        req("PATCH", "/api/saved_views/3/", {"name": "Alice home 2", "owner": 4}),
        req("PATCH", "/api/saved_views/3/", {"name": "after-transfer"}),
        S(SV)], users=["user1"]),
    c("sv.update.set-permissions", "saved_views.partial_update", [
        req("PATCH", "/api/saved_views/1/", {"set_permissions": {"view": {"users": [3], "groups": []}, "change": {"users": [3], "groups": []}}}),
        req("PATCH", "/api/saved_views/1/", {"name": "by-alice"}, user="user1"),
        S(SV), S(GUARDIAN)]),
    c("sv.destroy.state", "saved_views.destroy", [req("DELETE", "/api/saved_views/2/"), req("DELETE", "/api/saved_views/2/"), S(SV)]),
    c("sv.destroy.own", "saved_views.destroy", [req("DELETE", "/api/saved_views/3/"), req("DELETE", "/api/saved_views/5/"), S(SV)],
      users=["user1", "user2"]),
    # --- share links
    c("sl.create.state", "share_links.create", [
        req("POST", "/api/share_links/", {"document": 1, "file_version": "archive"}, compare="status"),
        req("POST", "/api/share_links/", {"document": 1, "expiration": "2030-05-01T12:00:00Z", "file_version": "original"}, compare="status"),
        req("POST", "/api/share_links/", {"document": 1}, compare="status"),
        S(SL)], users=["admin", "user1"]),
    c("sl.create.validation", "share_links.create", [
        req("POST", "/api/share_links/", {}),
        req("POST", "/api/share_links/", {"document": 1, "file_version": "thumb"}),
        req("POST", "/api/share_links/", {"document": "x"}),
        req("POST", "/api/share_links/", {"document": 1, "expiration": "soon"}),
        req("POST", "/api/share_links/", {"document": 196}),
        req("POST", "/api/share_links/", {"document": 201}),
        S(SL)], users=["admin"]),
    c("sl.create.foreign", "share_links.create", [req("POST", "/api/share_links/", {"document": 1}, compare="status"), S(SL)],
      users=["user1", "user2", "noperm"]),
    c("sl.destroy.state", "share_links.destroy", [req("DELETE", "/api/share_links/1/"), req("DELETE", "/api/share_links/1/"),
                                                  req("DELETE", "/api/share_links/999/"), req("GET", "/api/share_links/"), S(SL)]),
    c("sl.destroy.foreign", "share_links.destroy", [req("DELETE", "/api/share_links/1/"), req("DELETE", "/api/share_links/4/"),
                                                    req("DELETE", "/api/share_links/5/"), S(SL)], users=["user1", "user2", "noperm"]),
    # --- share link bundles
    c("slb.create.state", "share_link_bundles.create", [
        req("POST", "/api/share_link_bundles/", {"document_ids": [3, 1, 2], "file_version": "original", "expiration_days": 3}, compare="status"),
        S(SLB), S(TASKS)], users=["admin", "user1"]),
    c("slb.create.foreign", "share_link_bundles.create", [
        req("POST", "/api/share_link_bundles/", {"document_ids": [1, 999999]}),
        req("POST", "/api/share_link_bundles/", {"document_ids": [1]}),
        S(SLB)], users=["user2", "noperm"]),
    c("slb.destroy.rebuild", "share_link_bundles.destroy", [
        req("POST", "/api/share_link_bundles/", {"document_ids": [3]}, capture={"b": "id"}, compare="status"),
        req("POST", "/api/share_link_bundles/${b}/rebuild/", compare="status"),
        req("POST", "/api/share_link_bundles/${b}/rebuild/", user="user2"),
        req("DELETE", "/api/share_link_bundles/${b}/", user="user2"),
        req("DELETE", "/api/share_link_bundles/${b}/"),
        req("DELETE", "/api/share_link_bundles/${b}/"),
        S(SLB), S(TASKS)]),
]

if __name__ == "__main__":
    out = Path(__file__).with_name("cases-w2.json")
    out.write_text(json.dumps({"crate": "pl-fields-views", "cases": cases}, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, len(cases))
