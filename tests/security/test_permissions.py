"""Rights: model permissions, object permissions (guardian), owner, superuser, foreign objects.

Data-agnostic: the expected visibility is computed from the superuser's view of the data
(owner + permissions from ``full_perms=true``) and compared with what the user sees:

    visible(obj, u) = u.is_superuser or obj.owner is None or obj.owner == u.id
                      or u.id in obj.permissions.view.users
                      or u.groups & obj.permissions.view.groups
(upstream: documents/permissions.py has_perms_owner_aware / PermittedObjectsFilter)
"""

from __future__ import annotations

import os
import subprocess

from common import DENIED, REPO, RESET_SCRIPT, TargetCase, USERS, assert_no_leak, detail, need_user, session

OBJECTS = ["documents", "tags", "correspondents", "document_types", "storage_paths"]


def _user_info(username: str) -> dict | None:
    r = session("admin").get("/api/users/", query={"username__iexact": username, "page_size": 10})
    if r.status != 200:
        return None
    res = [u for u in r.json()["results"] if u["username"] == username]
    return res[0] if res else None


def _all(s, path: str, extra: dict | None = None) -> tuple[int, list]:
    q = {"page": 1, "page_size": 100000, "full_perms": "true"}
    if path == "/api/documents/":
        q["fields"] = "id,title,owner,permissions"
    q.update(extra or {})
    r = s.get(path, query=q)
    if r.status != 200:
        return r.status, []
    return 200, r.json().get("results", [])


def _visible(obj: dict, info: dict) -> bool:
    if info.get("is_superuser"):
        return True
    owner = obj.get("owner")
    if owner is None or owner == info["id"]:
        return True
    view = (obj.get("permissions") or {}).get("view") or {}
    if info["id"] in (view.get("users") or []):
        return True
    return bool(set(info.get("groups") or []) & set(view.get("groups") or []))


class Permissions(TargetCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        need_user("admin")

    def _check_visibility(self, name: str):
        u = need_user(name)
        info = _user_info(u.username)
        if info is None:
            self.skipTest(f"cannot read user info for {name}")
        for obj in OBJECTS:
            with self.subTest(user=name, objects=obj):
                path = f"/api/{obj}/"
                st_admin, everything = _all(session("admin"), path)
                self.assertEqual(st_admin, 200)
                st, seen = _all(session(name), path)
                if st == 403:
                    continue  # no model permission: nothing visible, checked separately
                self.assertEqual(st, 200)
                expected = {o["id"] for o in everything if _visible(o, info)}
                got = {o["id"] for o in seen}
                self.assertEqual(got - expected, set(), f"{name} sees foreign {obj}")
                self.assertEqual(expected - got, set(), f"{name} misses permitted {obj}")
                # foreign objects: detail must not be reachable and must not leak the title
                foreign = [o for o in everything if not _visible(o, info)][:5]
                for o in foreign:
                    r = session(name).get(f"{path}{o['id']}/")
                    self.assertIn(r.status, (403, 404), f"{name} GET foreign {obj} {o['id']}")
                    title = o.get("title") or o.get("name")
                    if title and len(title) > 3:
                        self.assertNotIn(title, r.text)
                if obj == "documents":
                    for o in foreign:
                        for sub in ("download/", "preview/", "thumb/", "metadata/", "notes/", "history/",
                                    "share_links/", "suggestions/"):
                            r = session(name).get(f"{path}{o['id']}/{sub}")
                            self.assertIn(r.status, (403, 404), f"{name} GET foreign document {o['id']}/{sub}")

    def test_visibility_user1(self):
        self._check_visibility("user1")

    def test_visibility_user2(self):
        self._check_visibility("user2")

    def test_superuser_sees_everything(self):
        info = _user_info(need_user("admin").username)
        self.assertTrue(info and info.get("is_superuser"), "catalogue user 'admin' must be a superuser")

    def test_noperm_user_denied(self):
        need_user("noperm")
        for obj in OBJECTS + ["custom_fields", "saved_views", "workflows", "mail_accounts", "mail_rules", "users",
                              "groups", "share_links"]:
            with self.subTest(objects=obj):
                r = session("noperm").get(f"/api/{obj}/")
                if obj == "saved_views" and r.status == 200:
                    continue  # saved views are owner-scoped; model perm view_savedview checked upstream
                self.assertEqual(r.status, 403, f"/api/{obj}/")
                self.assertEqual(detail(r), DENIED)

    def test_admin_only_endpoints(self):
        for name in ("user1", "noperm"):
            u = USERS.get(name)
            if u is None:
                continue
            info = _user_info(u.username) or {}
            if info.get("is_staff") or info.get("is_superuser"):
                continue
            for p in ("/api/logs/", "/api/status/"):
                with self.subTest(user=name, path=p):
                    r = session(name).get(p)
                    self.assertEqual(r.status, 403)
                    assert_no_leak(self, r)
        for p in ("/api/logs/", "/api/status/", "/api/config/"):
            self.assertEqual(session("admin").get(p).status, 200, p)

    def test_full_perms_does_not_widen(self):
        """``full_perms`` only changes the serializer, never the queryset."""
        need_user("user1")
        st1, a = _all(session("user1"), "/api/documents/", {"full_perms": "false"})
        st2, b = _all(session("user1"), "/api/documents/", {"full_perms": "true"})
        self.assertEqual(st1, st2)
        self.assertEqual({o["id"] for o in a}, {o["id"] for o in b})

    def test_filter_params_cannot_bypass(self):
        """owner__id / shared_by__id / id__in filters stay inside the permitted set."""
        u = need_user("user1")
        info = _user_info(u.username)
        if info is None:
            self.skipTest("no user info")
        _, everything = _all(session("admin"), "/api/documents/")
        foreign = [o for o in everything if not _visible(o, info)]
        if not foreign:
            self.skipTest("no foreign documents in reference data")
        ids = ",".join(str(o["id"]) for o in foreign[:50])
        owners = {o["owner"] for o in foreign if o.get("owner")}
        probes = [{"id__in": ids}, {"id": foreign[0]["id"]}, {"owner__isnull": "false"}]
        probes += [{"owner__id": ow} for ow in list(owners)[:3]]
        probes += [{"owner__id__in": ",".join(map(str, owners))}] if owners else []
        foreign_ids = {o["id"] for o in foreign}
        for q in probes:
            with self.subTest(query=q):
                st, seen = _all(session("user1"), "/api/documents/", q)
                if st == 403:
                    continue
                self.assertEqual({o["id"] for o in seen} & foreign_ids, set())
        # selection_data / bulk_download with foreign ids must not reveal or deliver them
        r = session("user1").post("/api/documents/selection_data/", json_body={"documents": [foreign[0]["id"]]})
        self.assertIn(r.status, (200, 400, 403, 404))
        r = session("user1").post("/api/documents/bulk_download/",
                                  json_body={"documents": [foreign[0]["id"]], "content": "archive"})
        self.assertIn(r.status, (400, 403, 404), "bulk_download of a foreign document")

    def test_write_foreign_document_denied(self):
        """PATCH on a document user1 may view but not change -> 403. Needs snapshot reset."""
        throwaway = os.environ.get("SEC_DB", "").startswith("pl_sec_")
        use_reset = RESET_SCRIPT.exists() and not throwaway and os.environ.get("TARGET", "django") == "django"
        if not (use_reset or throwaway or os.environ.get("SECURITY_WRITE")):
            self.skipTest("write test needs reference/reset.sh (Django target), SEC_DB=pl_sec_* or SECURITY_WRITE=1")
        u = need_user("user1")
        info = _user_info(u.username)
        if info is None or info.get("is_superuser"):
            self.skipTest("user1 info unavailable or superuser")
        _, everything = _all(session("admin"), "/api/documents/")
        cand = [o for o in everything
                if _visible(o, info) and o.get("owner") not in (None, info["id"])
                and info["id"] not in ((o.get("permissions") or {}).get("change") or {}).get("users", [])
                and not set(info.get("groups") or []) & set(
                    ((o.get("permissions") or {}).get("change") or {}).get("groups", []))]
        if not cand:
            self.skipTest("no view-only document for user1 in reference data")
        try:
            r = session("user1").request("PATCH", f"/api/documents/{cand[0]['id']}/", json_body={"title": "pwned"})
            self.assertEqual(r.status, 403)
            r = session("user1").request("DELETE", f"/api/documents/{cand[0]['id']}/")
            self.assertEqual(r.status, 403)
        finally:
            if use_reset:
                subprocess.run(["bash", str(RESET_SCRIPT)], cwd=str(REPO), capture_output=True, timeout=600)
