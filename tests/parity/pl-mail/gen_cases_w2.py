"""W2: DB state checks for mail writes (compare only - pl-mail is not changed by W2). python gen_cases_w2.py"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from w2_sql import GUARDIAN, c, req, sql  # noqa: E402

S = sql
MAIL = " union all ".join(
    f"select '{t}' as t, (to_jsonb(x) ->> 'id')::int as id, to_jsonb(x) as row from paperless_mail_{t} x"
    for t in ["mailaccount", "mailrule", "mailrule_assign_tags", "processedmail"]
) + " order by 1, 2"
TASKS = "select id, task_type, trigger_source, status, owner_id, input_data from documents_paperlesstask where id > 12 order by id"
ACC = {"name": "w2-acc", "imap_server": "imap.example.invalid", "imap_port": 993, "imap_security": 2, "username": "u",
       "password": "pw-secret", "character_set": "UTF-8", "is_token": False, "account_type": 1}

cases = [
    c("mail.acc.create.state", "mail_accounts.create", [req("POST", "/api/mail_accounts/", ACC), S(MAIL), S(GUARDIAN)],
      users=["admin", "user1", "user2", "noperm"]),
    c("mail.acc.create.perms", "mail_accounts.create", [
        req("POST", "/api/mail_accounts/", dict(ACC, name="w2-acc-p", owner=4,
                                                set_permissions={"view": {"users": [3], "groups": [2]}, "change": {"users": [], "groups": [1]}})),
        S(MAIL), S(GUARDIAN)]),
    c("mail.acc.create.dupe", "mail_accounts.create", [req("POST", "/api/mail_accounts/", dict(ACC, name="w2-dupe")),
                                                       req("POST", "/api/mail_accounts/", dict(ACC, name="w2-dupe")), S(MAIL)]),
    c("mail.acc.update.state", "mail_accounts.partial_update", [
        req("PATCH", "/api/mail_accounts/1/", {"password": "**********", "imap_port": 1993}),
        req("PATCH", "/api/mail_accounts/1/", {"password": "new-secret"}),
        req("PUT", "/api/mail_accounts/2/", dict(ACC, name="put-acc")),
        S(MAIL)]),
    c("mail.acc.update.foreign", "mail_accounts.partial_update", [req("PATCH", "/api/mail_accounts/1/", {"name": "x"}),
                                                                  req("PATCH", "/api/mail_accounts/2/", {"name": "y"}), S(MAIL)],
      users=["user1", "user2", "noperm"]),
    c("mail.acc.destroy.state", "mail_accounts.destroy", [req("DELETE", "/api/mail_accounts/1/"), S(MAIL)], users=["admin", "user1"]),
    c("mail.acc.process.state", "mail_accounts.process", [req("POST", "/api/mail_accounts/1/process/"), S(TASKS)], users=["admin", "user1"]),
    c("mail.rule.create.state", "mail_rules.create", [
        req("POST", "/api/mail_rules/", {"name": "w2-rule", "account": 1, "folder": "INBOX/sub", "filter_from": "a@example.org",
                                         "maximum_age": 10, "action": 3, "action_parameter": "", "assign_tags": [1, 2],
                                         "assign_correspondent": 1, "assign_document_type": 1, "order": 4, "attachment_type": 2,
                                         "consumption_scope": 3, "pdf_layout": 2, "assign_title_from": 2,
                                         "assign_correspondent_from": 3, "assign_owner_from_rule": False, "stop_processing": True}),
        S(MAIL), S(GUARDIAN)], users=["admin", "user1"]),
    c("mail.rule.update.state", "mail_rules.partial_update", [req("PATCH", "/api/mail_rules/1/", {"assign_tags": [], "action": 4}),
                                                              req("PATCH", "/api/mail_rules/1/", {"action": 3, "action_parameter": None}),
                                                              S(MAIL)]),
    c("mail.rule.destroy.state", "mail_rules.destroy", [req("DELETE", "/api/mail_rules/1/"), S(MAIL)], users=["admin", "user1"]),
    c("mail.processed.bulk_delete.state", "processed_mail.bulk_delete", [
        req("POST", "/api/processed_mail/bulk_delete/", {"mail_ids": [1, 2, 3]}), S(MAIL)], users=["admin", "user1", "user2", "noperm"]),
]

if __name__ == "__main__":
    out = Path(__file__).with_name("cases-w2.json")
    out.write_text(json.dumps({"crate": "pl-mail", "cases": cases}, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, len(cases))
