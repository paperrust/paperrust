"""W2: DB state checks for object writes (compare only - pl-objects is not changed by W2). python gen_cases_w2.py"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from w2_sql import AUDIT, GUARDIAN, c, req, sql  # noqa: E402

S = sql
OBJ = " union all ".join(
    f"select '{t}' as t, (to_jsonb(x) ->> 'id')::int as id, to_jsonb(x) as row from documents_{t} x"
    for t in ["correspondent", "documenttype", "tag", "storagepath"]
) + " order by 1, 2"
DOC_LINKS = ("select id, correspondent_id, document_type_id, storage_path_id, owner_id, "
             "(select coalesce(array_agg(t.tag_id order by t.tag_id), '{}') from documents_document_tags t where t.document_id = d.id) as tags "
             "from documents_document d order by id")
TASKS = "select id, task_type, trigger_source, status, owner_id, input_data from documents_paperlesstask where id > 12 order by id"

cases = [
    c("obj.create.state", "tags.create", [
        req("POST", "/api/tags/", {"name": "w2-tag", "color": "#123456", "is_inbox_tag": True, "matching_algorithm": 1, "match": "x",
                                   "parent": 1, "set_permissions": {"view": {"users": [4], "groups": []}, "change": {"users": [], "groups": [1]}}}),
        req("POST", "/api/correspondents/", {"name": "w2-corr", "owner": 4}),
        req("POST", "/api/document_types/", {"name": "w2-type", "matching_algorithm": 6, "match": ""}),
        S(OBJ), S(GUARDIAN), S(AUDIT)], users=["admin", "user1"]),
    c("obj.update.state", "tags.partial_update", [
        req("PATCH", "/api/tags/2/", {"name": "w2-renamed", "parent": None, "color": "#000000"}),
        req("PATCH", "/api/correspondents/1/", {"match": "new", "matching_algorithm": 3}),
        req("PUT", "/api/document_types/1/", {"name": "put-type"}),
        S(OBJ), S(AUDIT)], users=["admin", "user1"]),
    c("obj.destroy.state", "tags.destroy", [
        req("DELETE", "/api/tags/1/"), req("DELETE", "/api/correspondents/1/"), req("DELETE", "/api/document_types/1/"),
        req("DELETE", "/api/storage_paths/1/"),
        S(OBJ), S(DOC_LINKS), S(GUARDIAN), S(AUDIT), S(TASKS)]),
    c("obj.destroy.foreign", "correspondents.destroy", [req("DELETE", "/api/correspondents/12/"), req("DELETE", "/api/tags/20/"),
                                                        S(OBJ)], users=["user1", "user2", "noperm"]),
    c("obj.bulk.state", "bulk_edit_objects", [
        req("POST", "/api/bulk_edit_objects/", {"objects": [1, 2], "object_type": "tags", "operation": "set_permissions",
                                                "owner": 3, "permissions": {"view": {"users": [4]}, "change": {"groups": [1]}}, "merge": False}),
        req("POST", "/api/bulk_edit_objects/", {"objects": [3], "object_type": "correspondents", "operation": "delete"}),
        S(OBJ), S(GUARDIAN), S(DOC_LINKS), S(AUDIT)], users=["admin", "user1"]),
]

if __name__ == "__main__":
    out = Path(__file__).with_name("cases-w2.json")
    out.write_text(json.dumps({"crate": "pl-objects", "cases": cases}, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, len(cases))
