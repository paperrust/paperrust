"""Leak checks: hostile/invalid parameters must not produce stack traces, paths, SQL or secrets;
password fields are masked; error pages have no internals.
"""

from __future__ import annotations

from common import TargetCase, assert_no_leak, need_user, session

HOSTILE = [
    ("/api/documents/", {"ordering": "title;DROP TABLE documents_document"}),
    ("/api/documents/", {"ordering": "'\""}),
    ("/api/documents/", {"ordering": "custom_field_abc"}),
    ("/api/documents/", {"ordering": "-custom_field_99999999999999999999"}),
    ("/api/documents/", {"id__in": "1,2,x"}),
    ("/api/documents/", {"id": "99999999999999999999999"}),
    ("/api/documents/", {"tags__id__all": "1;2"}),
    ("/api/documents/", {"created__date__gt": "2020-13-45"}),
    ("/api/documents/", {"created__year": "-1"}),
    ("/api/documents/", {"created__year": "99999999999"}),
    ("/api/documents/", {"ordering": "custom_field_1,title"}),
    ("/api/documents/", {"custom_field_query": "["}),
    ("/api/documents/", {"custom_field_query": '["1","exact",{"a":1}]'}),
    ("/api/documents/", {"custom_field_query": "[" * 200 + "]" * 200}),
    ("/api/documents/", {"query": "title:(((("}),
    ("/api/documents/", {"query": "\x00"}),
    ("/api/documents/", {"more_like_id": "abc"}),
    ("/api/documents/", {"page": "-1"}),
    ("/api/documents/", {"page_size": "-5"}),
    ("/api/documents/", {"fields": "id,../../etc"}),
    ("/api/documents/", {"title__icontains": "%' OR 1=1 --"}),
    ("/api/tags/", {"name__icontains": "\\"}),
    ("/api/tags/", {"id__in": "1," * 5000}),
    ("/api/tasks/", {"task_id": "' OR '1'='1"}),
    ("/api/tasks/summary/", {"days": "abc"}),
    ("/api/search/", {"query": "a" * 5000}),
    ("/api/search/autocomplete/", {"term": "a", "limit": "-1"}),
    ("/api/logs/paperless/", {"limit": "abc"}),
    ("/api/documents/99999999999999999999/", None),
    ("/api/documents/-1/", None),
    ("/api/documents/1.5/", None),
    ("/api/tags/%00/", None),
    ("/api/does-not-exist/", None),
]


class Leaks(TargetCase):
    def test_hostile_parameters(self):
        need_user("admin")
        s = session("admin")
        for path, q in HOSTILE:
            with self.subTest(path=path, query=q):
                r = s.get(path, query=q)
                assert_no_leak(self, r, what=f"{path}?{q}")
                self.assertLess(r.status, 500, f"{path} {q} -> {r.status}")

    def test_hostile_bodies(self):
        need_user("admin")
        s = session("admin")
        bodies = [
            ("/api/documents/selection_data/", b"{not json"),
            ("/api/documents/selection_data/", b'{"documents": "' + b"A" * 100000 + b'"}'),
            ("/api/documents/bulk_edit/", b'{"documents": [1], "method": "__class__", "parameters": {}}'),
            ("/api/documents/bulk_edit/", b"[]"),
            ("/api/documents/bulk_download/", b'{"documents": ["../../etc/passwd"]}'),
            ("/api/storage_paths/test/", b'{"document": 1, "path": "{{ ().__class__.__mro__ }}"}'),
        ]
        for path, raw in bodies:
            with self.subTest(path=path):
                r = s.post(path, data=raw, content_type="application/json")
                assert_no_leak(self, r, what=path)
                self.assertLess(r.status, 500, f"{path} -> {r.status}")

    def assertMasked(self, value, what, allow_empty=False):
        """upstream ObfuscatedPasswordField: "*" * max(10, len(value)) – also for user hashes
        (paperless/serialisers.py uses the mail field), so the length varies."""
        if allow_empty and value in (None, ""):
            return
        self.assertIsInstance(value, str, what)
        self.assertGreaterEqual(len(value), 10, what)
        self.assertEqual(set(value), {"*"}, what)

    def test_password_fields_masked(self):
        need_user("admin")
        s = session("admin")
        r = s.get("/api/users/", query={"page_size": 100000})
        self.assertEqual(r.status, 200)
        for u in r.json()["results"]:
            self.assertMasked(u.get("password"), f"user {u.get('username')}", allow_empty=True)
        assert_no_leak(self, r)
        r = s.get("/api/profile/")
        self.assertMasked(r.json().get("password"), "profile")
        assert_no_leak(self, r, allow={"token"})  # own auth_token is part of the profile
        r = s.get("/api/mail_accounts/", query={"page_size": 100000})
        if r.status == 200:
            for a in r.json()["results"]:
                self.assertMasked(a.get("password"), f"mail account {a.get('id')}")
            assert_no_leak(self, r)

    def test_other_users_tokens_not_exposed(self):
        need_user("admin")
        for p in ("/api/users/", "/api/groups/", "/api/ui_settings/", "/api/status/", "/api/config/",
                  "/api/statistics/", "/api/documents/?full_perms=true", "/api/mail_accounts/"):
            with self.subTest(path=p):
                r = session("admin").get(p)
                assert_no_leak(self, r, allow={"path"} if p == "/api/status/" else None)

    def test_error_pages_without_internals(self):
        for p in ("/api/documents/abc/download/", "/does/not/exist", "/api/token/", "/admin/login/?next=%0a",
                  "/accounts/login/?next=//evil.example"):
            with self.subTest(path=p):
                r = session().get(p)
                self.assertLess(r.status, 500)
                assert_no_leak(self, r)
                loc = r.header("location") or ""
                self.assertFalse(loc.startswith("//evil") or "evil.example" in loc, f"open redirect {loc}")
