"""Generates cases-w2.json: W2 write cases with DB state checks (run: python gen_cases_w2.py)."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from w2_sql import EMAIL, GROUPS, TOKENS, UISETTINGS, USERS, c, req, sql  # noqa: E402

S = sql
GRANT_USER_ADMIN = req("PATCH", "/api/users/3/", {"user_permissions": [
    "add_user", "change_user", "view_user", "delete_user",
    "add_group", "change_group", "view_group", "delete_group"]}, user="admin")

cases = [
    # --- /api/token/
    c("token.wrong-password", "auth.token", [req("POST", "/api/token/", {"username": "alice", "password": "nope"}), S(TOKENS)], users=["anon"]),
    c("token.inactive", "auth.token", [req("POST", "/api/token/", {"username": "dave", "password": "dave"}), S(TOKENS)], users=["anon"]),
    c("token.missing", "auth.token", [req("POST", "/api/token/", {"username": "alice"}), req("POST", "/api/token/", {}),
                                      req("POST", "/api/token/", {"username": "", "password": ""})], users=["anon"]),
    c("token.state", "auth.token", [req("POST", "/api/token/", {"username": "bob", "password": "bob"}), S(TOKENS), S(USERS)],
      users=["anon"], leak_allow=["token"]),
    c("token.new-user", "auth.token", [
        req("POST", "/api/users/", {"username": "tokuser", "password": "Correct-Horse-9-Battery"}, user="admin"),
        req("POST", "/api/token/", {"username": "tokuser", "password": "Correct-Horse-9-Battery"}, compare="status"),
        S(TOKENS)], users=["anon"], leak_allow=["token"]),
    c("token.as-authenticated", "auth.token", [req("POST", "/api/token/", {"username": "carol", "password": "carol"})],
      users=["user1"], leak_allow=["token"]),
    c("token.throttle", "auth.token", [req("POST", "/api/token/", {"username": "alice", "password": "x"}) for _ in range(7)],
      users=["anon"]),
    # --- /api/profile/
    c("profile.email", "profile.patch", [req("PATCH", "/api/profile/", {"email": "alice-new@example.org"}), S(USERS), S(EMAIL)],
      users=["user1"], leak_allow=["token"]),
    c("profile.bad-email", "profile.patch", [req("PATCH", "/api/profile/", {"email": "not-an-email"}), S(USERS)], users=["user1"]),
    c("profile.short-password", "profile.patch", [req("PATCH", "/api/profile/", {"password": "abc"}), S(USERS)], users=["user1"]),
    c("profile.stars-password", "profile.patch", [req("PATCH", "/api/profile/", {"password": "********", "first_name": "A"}), S(USERS)],
      users=["user1"], leak_allow=["token"]),
    c("profile.readonly-fields", "profile.patch", [req("PATCH", "/api/profile/", {"username": "hacker", "is_superuser": True,
                                                                                    "auth_token": "x", "is_mfa_enabled": True}), S(USERS)],
      users=["user2"], leak_allow=["token"]),
    c("profile.form", "profile.patch", [req("PATCH", "/api/profile/", form={"first_name": "Formy", "last_name": "McForm"}), S(USERS)],
      users=["user2"], leak_allow=["token"]),
    c("profile.noperm", "profile.patch", [req("PATCH", "/api/profile/", {"first_name": "C"}), S(USERS)],
      users=["noperm", "anon"], leak_allow=["token"]),
    c("profile.session-password", "profile.patch", [req("PATCH", "/api/profile/", {"password": "a-Much-Better-Passw0rd"}),
                                                    req("GET", "/api/profile/"), S(USERS)],
      users=["user2@session"], leak_allow=["token"]),
    c("profile.token.state", "profile.generate_auth_token", [req("POST", "/api/profile/generate_auth_token/", compare="status+headers"),
                                                             S(TOKENS)], users=["user1", "noperm", "anon"]),
    c("profile.token.then-old", "profile.generate_auth_token", [req("POST", "/api/profile/generate_auth_token/", compare="status+headers"),
                                                                req("GET", "/api/profile/", compare="status")], users=["user2"]),
    c("profile.disconnect.missing", "profile.disconnect_social_account", [
        req("POST", "/api/profile/disconnect_social_account/", {"id": 1}),
        req("POST", "/api/profile/disconnect_social_account/", {})], users=["user1", "anon"]),
    c("profile.totp.errors", "profile.totp.post", [req("POST", "/api/profile/totp/", {"secret": "JBSWY3DPEHPK3PXP", "code": "000000"}),
                                                   req("POST", "/api/profile/totp/", {}),
                                                   req("POST", "/api/profile/totp/", {"code": "1"})], users=["user1", "anon"]),
    c("profile.totp.delete-none", "profile.totp.delete", [req("DELETE", "/api/profile/totp/")], users=["user1", "anon"]),
    c("users.deactivate_totp.none", "users.deactivate_totp", [req("POST", "/api/users/3/deactivate_totp/"),
                                                              req("POST", "/api/users/999/deactivate_totp/")], users=["admin", "user1"]),
    # --- /api/users/
    c("users.create.state", "users.create", [req("POST", "/api/users/", {
        "username": "stateuser", "email": "s@example.org", "password": "Correct-Horse-9-Battery",
        "groups": [2], "user_permissions": ["view_tag"]}), S(USERS), S(EMAIL)]),
    c("users.create.no-password", "users.create", [req("POST", "/api/users/", {"username": "nopw"}), S(USERS)]),
    c("users.create.stars", "users.create", [req("POST", "/api/users/", {"username": "stars", "password": "**********"}), S(USERS)]),
    c("users.create.superuser", "users.create", [req("POST", "/api/users/", {"username": "su2", "is_superuser": True, "is_staff": "true"}),
                                                 S(USERS)]),
    c("users.create.errors", "users.create", [
        req("POST", "/api/users/", {"username": "alice"}),
        req("POST", "/api/users/", {"username": "ALICE"}),
        req("POST", "/api/users/", {}),
        req("POST", "/api/users/", {"username": "bad name!"}),
        req("POST", "/api/users/", {"username": "x" * 151}),
        req("POST", "/api/users/", {"username": "pw", "password": "abc"}),
        req("POST", "/api/users/", {"username": "pw2", "password": "password"}),
        req("POST", "/api/users/", {"username": "g", "groups": [999]}),
        req("POST", "/api/users/", {"username": "p", "user_permissions": ["nope"]}),
        req("POST", "/api/users/", {"username": "p2", "user_permissions": ["add_logentry"]}),
        req("POST", "/api/users/", {"username": "e", "email": "bad"}),
        req("POST", "/api/users/", {"username": "b", "is_active": "maybe"}),
        req("POST", "/api/users/", {"username": "t", "groups": "1"}),
        S(USERS)]),
    c("users.create.denied", "users.create", [req("POST", "/api/users/", {"username": "x1"}), S(USERS)], users=["user1", "noperm", "anon"]),
    c("users.create.escalation", "users.create", [
        GRANT_USER_ADMIN,
        req("POST", "/api/users/", {"username": "esc1", "is_superuser": True}),
        req("POST", "/api/users/", {"username": "esc2", "is_staff": "1"}),
        req("POST", "/api/users/", {"username": "esc3", "is_superuser": "maybe"}),
        req("POST", "/api/users/", {"username": "ok1", "is_superuser": False, "is_staff": False, "groups": [1]}),
        S(USERS)], users=["user1"]),
    c("users.update.escalation", "users.partial_update", [
        GRANT_USER_ADMIN,
        req("PATCH", "/api/users/2/", {"first_name": "Admin"}),
        req("PATCH", "/api/users/4/", {"is_superuser": True}),
        req("PATCH", "/api/users/4/", {"is_staff": True}),
        req("PATCH", "/api/users/4/", {"is_staff": False, "is_superuser": False, "last_name": "Same"}),
        req("PUT", "/api/users/5/", {"username": "carol", "is_superuser": "on"}),
        req("PATCH", "/api/users/3/", {"first_name": "Self"}),
        S(USERS)], users=["user1"]),
    c("users.update.password", "users.partial_update", [req("PATCH", "/api/users/4/", {"password": "Another-Good-Passw0rd"}),
                                                        req("PATCH", "/api/users/5/", {"password": "**********"}),
                                                        req("PATCH", "/api/users/5/", {"password": "short"}), S(USERS)]),
    c("users.update.errors", "users.partial_update", [req("PATCH", "/api/users/4/", {"username": "alice"}),
                                                      req("PATCH", "/api/users/4/", {"groups": [99]}),
                                                      req("PATCH", "/api/users/999/", {"first_name": "x"}),
                                                      req("PATCH", "/api/users/1/", {"first_name": "anon"}),
                                                      req("PUT", "/api/users/4/", {"email": "x@example.org"}),
                                                      req("PATCH", "/api/users/abc/", {}), S(USERS)]),
    c("users.update.deactivate", "users.partial_update", [req("PATCH", "/api/users/4/", {"is_active": False}),
                                                          req("GET", "/api/profile/", user="user2"), S(USERS)]),
    c("users.update.denied", "users.partial_update", [req("PATCH", "/api/users/4/", {"first_name": "x"}), S(USERS)],
      users=["user1", "user2", "noperm"]),
    c("users.update.v9", "users.update", [req("PUT", "/api/users/5/", {"username": "carol2", "first_name": "C",
                                                                      "user_permissions": ["view_document"]}), S(USERS)],
      versions=["9", "none"]),
    # --- /api/groups/
    c("groups.create.state", "groups.create", [req("POST", "/api/groups/", {"name": "g-state", "permissions": ["view_document", "change_tag"]}),
                                               S(GROUPS)]),
    c("groups.create.errors", "groups.create", [req("POST", "/api/groups/", {"name": "editors", "permissions": []}),
                                                req("POST", "/api/groups/", {"name": "x"}),
                                                req("POST", "/api/groups/", {"permissions": []}),
                                                req("POST", "/api/groups/", {"name": "", "permissions": []}),
                                                req("POST", "/api/groups/", {"name": "y", "permissions": ["nope"]}),
                                                req("POST", "/api/groups/", {"name": "z" * 151, "permissions": []}),
                                                req("POST", "/api/groups/", {"name": "w", "permissions": "view_tag"}), S(GROUPS)]),
    c("groups.create.denied", "groups.create", [req("POST", "/api/groups/", {"name": "gx", "permissions": []}), S(GROUPS)],
      users=["user1", "noperm", "anon"]),
    c("groups.update.state", "groups.partial_update", [
        req("PATCH", "/api/groups/1/", {"name": "editors2"}),
        req("PUT", "/api/groups/3/", {"name": "auditors", "permissions": ["view_logentry", "view_document", "view_note"]}),
        req("PATCH", "/api/groups/2/", {"name": "editors"}),
        req("PATCH", "/api/groups/99/", {"name": "nn"}), S(GROUPS)]),
    c("groups.update.denied", "groups.partial_update", [req("PATCH", "/api/groups/1/", {"name": "hijack"}), S(GROUPS)],
      users=["user1", "user2", "noperm"]),
    c("groups.destroy.state", "groups.destroy", [req("DELETE", "/api/groups/1/"), req("DELETE", "/api/groups/1/"), S(GROUPS), S(USERS)]),
    c("groups.destroy.uisettings", "groups.destroy", [
        req("POST", "/api/ui_settings/", {"settings": {"permissions": {"default_view_groups": [1, 2], "default_change_groups": [2],
                                                                       "default_owner": 4, "default_view_users": [4]}}}, user="user1"),
        req("DELETE", "/api/groups/2/"), S(UISETTINGS)]),
    c("groups.destroy.denied", "groups.destroy", [req("DELETE", "/api/groups/1/"), S(GROUPS)], users=["user1", "noperm"]),
    # --- /api/ui_settings/
    c("ui_settings.state", "ui_settings.post", [req("POST", "/api/ui_settings/", {"settings": {"language": "fr-FR", "app_title": "X"}}),
                                                S(UISETTINGS)], users=["admin", "user1", "user2", "noperm"]),
    c("ui_settings.errors", "ui_settings.post", [req("POST", "/api/ui_settings/", {"settings": "x"}),
                                                 req("POST", "/api/ui_settings/", {"settings": [1]}),
                                                 req("POST", "/api/ui_settings/", form={"settings": "{}"}),
                                                 req("POST", "/api/ui_settings/", data="not json", content_type="application/json"),
                                                 S(UISETTINGS)], users=["user1"]),
    c("ui_settings.anon", "ui_settings.post", [req("POST", "/api/ui_settings/", {"settings": {}}), S(UISETTINGS)], users=["anon", "inactive"]),
    c("ui_settings.new-user", "ui_settings.post", [req("POST", "/api/ui_settings/", {"settings": {"dark_mode": {"enabled": True}}}),
                                                   req("GET", "/api/ui_settings/"), S(UISETTINGS)], users=["user2"]),
    c("ui_settings.csrf", "ui_settings.post", [req("POST", "/api/ui_settings/", {"settings": {"a": 1}}, csrf=False),
                                               req("POST", "/api/ui_settings/", {"settings": {"a": 2}}), S(UISETTINGS)],
      users=["user1@session"]),
]

if __name__ == "__main__":
    out = Path(__file__).with_name("cases-w2.json")
    out.write_text(json.dumps({"crate": "pl-auth-api", "cases": cases}, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, len(cases))
