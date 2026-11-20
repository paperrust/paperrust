"""Generates cases-w2.json: W2 write cases with DB state checks (run: python gen_cases_w2.py)."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from w2_sql import c, req, sql  # noqa: E402

S = sql
TABLES = [
    "documents_workflow", "documents_workflow_actions", "documents_workflow_triggers", "documents_workflowaction",
    "documents_workflowaction_assign_change_groups", "documents_workflowaction_assign_change_users",
    "documents_workflowaction_assign_custom_fields", "documents_workflowaction_assign_tags",
    "documents_workflowaction_assign_view_groups", "documents_workflowaction_assign_view_users",
    "documents_workflowaction_remove_change_groups", "documents_workflowaction_remove_change_users",
    "documents_workflowaction_remove_correspondents", "documents_workflowaction_remove_custom_fields",
    "documents_workflowaction_remove_document_types", "documents_workflowaction_remove_owners",
    "documents_workflowaction_remove_storage_paths", "documents_workflowaction_remove_tags",
    "documents_workflowaction_remove_view_groups", "documents_workflowaction_remove_view_users",
    "documents_workflowactionemail", "documents_workflowactionwebhook", "documents_workflowrun", "documents_workflowtrigger",
    "documents_workflowtrigger_filter_has_all_tags", "documents_workflowtrigger_filter_has_any_correspondents",
    "documents_workflowtrigger_filter_has_any_document_types", "documents_workflowtrigger_filter_has_any_storage_paths",
    "documents_workflowtrigger_filter_has_not_correspondents", "documents_workflowtrigger_filter_has_not_document_types",
    "documents_workflowtrigger_filter_has_not_storage_paths", "documents_workflowtrigger_filter_has_not_tags",
    "documents_workflowtrigger_filter_has_tags",
]
# whole workflow state: every row of every workflow table (row as jsonb minus nothing; ids are deterministic)
WF = " union all ".join(
    f"select '{t.removeprefix('documents_')}' as t, (to_jsonb(x) ->> 'id')::int as id, to_jsonb(x) as row from {t} x" for t in TABLES
) + " order by 1, 2"

NESTED = {
    "name": "w2 wf", "order": 7, "enabled": True,
    "triggers": [
        {"type": 1, "sources": [1, 3], "filter_filename": "*.pdf", "filter_path": "/in/*", "filter_has_tags": [1, 2],
         "filter_has_all_tags": [3], "filter_has_not_tags": [4], "filter_has_any_correspondents": [1],
         "filter_has_not_correspondents": [2], "filter_has_any_document_types": [1], "filter_has_not_document_types": [2],
         "filter_has_any_storage_paths": [1], "filter_has_not_storage_paths": [2], "match": "invoice", "matching_algorithm": 1,
         "is_insensitive": False},
        {"type": 4, "schedule_offset_days": 3, "schedule_is_recurring": True, "schedule_recurring_interval_days": 7,
         "schedule_date_field": "custom_field", "schedule_date_custom_field": 4},
    ],
    "actions": [
        {"type": 1, "assign_title": "{{ correspondent }}", "assign_tags": [1, 5], "assign_correspondent": 2,
         "assign_document_type": 1, "assign_storage_path": 1, "assign_owner": 3, "assign_view_users": [3, 4],
         "assign_view_groups": [1], "assign_change_users": [3], "assign_change_groups": [2], "assign_custom_fields": [1, 2],
         "assign_custom_fields_values": {"1": "abc", "2": "https://example.org"}},
        {"type": 2, "remove_tags": [1], "remove_all_tags": False, "remove_correspondents": [1], "remove_document_types": [1],
         "remove_storage_paths": [1], "remove_custom_fields": [1], "remove_owners": [2], "remove_view_users": [3],
         "remove_view_groups": [1], "remove_change_users": [4], "remove_change_groups": [2]},
        {"type": 3, "email": {"subject": "S {{ doc_title }}", "body": "B", "to": "a@example.org, b@example.org", "include_document": True}},
        {"type": 4, "webhook": {"url": "http://127.0.0.1:9/hook", "use_params": True, "params": {"a": "1"},
                                "headers": {"X-A": "b"}, "include_document": False}},
    ],
}

cases = [
    c("wf.create.nested-state", "workflows.create", [req("POST", "/api/workflows/", NESTED), S(WF)]),
    c("wf.create.denied-state", "workflows.create", [req("POST", "/api/workflows/", {"name": "x", "triggers": [{"type": 2}], "actions": [{"type": 1}]}),
                                                     S(WF)], users=["user1", "user2", "noperm", "anon"]),
    c("wf.create.existing-ids", "workflows.create", [
        req("POST", "/api/workflows/", {"name": "reuse", "triggers": [{"id": 1, "type": 2}], "actions": [{"id": 1, "type": 1, "assign_title": "t"}]}),
        S(WF)]),
    c("wf.create.bad-refs", "workflows.create", [
        req("POST", "/api/workflows/", {"name": "refs", "triggers": [{"type": 2, "filter_has_tags": [9999]}], "actions": [{"type": 1}]}),
        req("POST", "/api/workflows/", {"name": "refs2", "triggers": [{"type": 2}], "actions": [{"type": 1, "assign_owner": 9999}]}),
        req("POST", "/api/workflows/", {"name": "refs3", "triggers": [{"type": 2}], "actions": [{"type": 1, "assign_custom_fields_values": {"999": 1}}]}),
        req("POST", "/api/workflows/", {"name": "refs4", "triggers": [{"type": 1, "sources": [7]}], "actions": [{"type": 1}]}),
        req("POST", "/api/workflows/", {"name": "refs5", "triggers": [], "actions": []}),
        S(WF)]),
    c("wf.update.replace-prunes", "workflows.partial_update", [
        req("POST", "/api/workflow_triggers/", {"type": 2, "filter_filename": "orphan"}),
        req("POST", "/api/workflow_actions/", {"type": 1, "assign_title": "orphan"}),
        req("PATCH", "/api/workflows/1/", {"triggers": [{"type": 3, "filter_filename": "new.pdf"}],
                                           "actions": [{"type": 2, "remove_all_tags": True}]}),
        S(WF)]),
    c("wf.update.keep-ids", "workflows.partial_update", [
        req("GET", "/api/workflows/2/"),
        req("PATCH", "/api/workflows/2/", {"name": "w2-renamed", "order": 0, "enabled": False}),
        S(WF)]),
    c("wf.update.put-full", "workflows.update", [req("PUT", "/api/workflows/1/", NESTED), S(WF)]),
    c("wf.update.put-missing", "workflows.update", [req("PUT", "/api/workflows/1/", {"name": "only"}), S(WF)]),
    c("wf.update.email-to-webhook", "workflows.partial_update", [
        req("PATCH", "/api/workflows/1/", {"actions": [{"type": 4, "webhook": {"url": "http://127.0.0.1:9/x"}}]}), S(WF)]),
    c("wf.update.denied", "workflows.partial_update", [req("PATCH", "/api/workflows/1/", {"name": "hijack"}), S(WF)],
      users=["user1", "user2", "noperm"]),
    c("wf.update.missing", "workflows.partial_update", [req("PATCH", "/api/workflows/999/", {"name": "x"}),
                                                        req("PATCH", "/api/workflows/abc/", {"name": "x"})]),
    c("wf.destroy.state", "workflows.destroy", [req("DELETE", "/api/workflows/1/"), req("DELETE", "/api/workflows/1/"), S(WF)]),
    c("wf.destroy.denied", "workflows.destroy", [req("DELETE", "/api/workflows/1/"), S(WF)], users=["user1", "noperm"]),
    # standalone trigger / action endpoints
    c("trig.create.state", "workflow_triggers.create", [
        req("POST", "/api/workflow_triggers/", {"type": 2, "filter_has_tags": [1, 2], "filter_has_not_correspondents": [3],
                                                "filter_custom_field_query": "[\"AND\", [[1, \"exists\", true]]]"}),
        S(WF)]),
    c("trig.update.state", "workflow_triggers.partial_update", [
        req("PATCH", "/api/workflow_triggers/1/", {"filter_has_tags": [], "filter_filename": None}),
        req("PUT", "/api/workflow_triggers/2/", {"type": 3, "filter_has_any_document_types": [1, 2]}),
        S(WF)]),
    c("trig.destroy.state", "workflow_triggers.destroy", [req("DELETE", "/api/workflow_triggers/1/"), S(WF)]),
    c("trig.denied", "workflow_triggers.create", [req("POST", "/api/workflow_triggers/", {"type": 2}),
                                                  req("PATCH", "/api/workflow_triggers/1/", {"filter_filename": "x"}),
                                                  req("DELETE", "/api/workflow_triggers/1/"), S(WF)], users=["user1", "noperm"]),
    c("act.create.state", "workflow_actions.create", [
        req("POST", "/api/workflow_actions/", {"type": 1, "assign_tags": [2, 1], "assign_view_users": [4], "assign_custom_fields": [3],
                                               "assign_custom_fields_values": {"3": "2025-01-01"}}),
        req("POST", "/api/workflow_actions/", {"type": 3, "email": {"subject": "s", "body": "b", "to": "x@example.org"}}),
        req("POST", "/api/workflow_actions/", {"type": 4, "webhook": {"url": "http://127.0.0.1:9/h", "as_json": True, "body": "{}"}}),
        S(WF)]),
    c("act.update.state", "workflow_actions.partial_update", [
        req("PATCH", "/api/workflow_actions/1/", {"assign_tags": [3], "assign_title": None}),
        req("PUT", "/api/workflow_actions/2/", {"type": 2, "remove_all_tags": True}),
        S(WF)]),
    c("act.update.email-change", "workflow_actions.partial_update", [
        req("GET", "/api/workflow_actions/"),
        req("PATCH", "/api/workflow_actions/3/", {"email": {"subject": "changed", "body": "b", "to": "c@example.org"}}),
        req("PATCH", "/api/workflow_actions/4/", {"webhook": {"url": "http://127.0.0.1:9/changed"}}),
        S(WF)]),
    c("act.destroy.state", "workflow_actions.destroy", [req("DELETE", "/api/workflow_actions/1/"), req("DELETE", "/api/workflow_actions/3/"), S(WF)]),
    c("act.denied", "workflow_actions.create", [req("POST", "/api/workflow_actions/", {"type": 1}),
                                                req("PATCH", "/api/workflow_actions/1/", {"assign_title": "x"}),
                                                req("DELETE", "/api/workflow_actions/1/"), S(WF)], users=["user1", "noperm"]),
]

if __name__ == "__main__":
    out = Path(__file__).with_name("cases-w2.json")
    out.write_text(json.dumps({"crate": "pl-workflows", "cases": cases}, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, len(cases))
