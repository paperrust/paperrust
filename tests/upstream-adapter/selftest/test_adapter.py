"""Checks the adapter mechanics with DRF's APITestCase, exactly like upstream tests use it."""

from __future__ import annotations

from django.contrib.auth.models import User
from rest_framework.test import APIClient, APITestCase


class AdapterTest(APITestCase):
    def setUp(self):
        # flush between tests (non-transactional TestCase path) -> always empty here
        self.assertEqual(User.objects.count(), 0)
        self.user = User.objects.create_superuser("admin", password="pw")

    def test_force_authenticate_over_http(self):
        self.client.force_authenticate(user=self.user)
        r = self.client.get("/api/whoami/", {"q": "ä b"})
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.data["user"], "admin")
        self.assertNotEqual(r.data["thread"], "MainThread")  # served by the live thread
        self.assertEqual(r.data["q"], "ä b")
        self.assertEqual(r.data["n_users"], 1)  # committed data visible to the server
        self.assertEqual(r.data["next"], "http://testserver/api/whoami/?page=2")
        self.assertEqual(r.json()["user"], "admin")
        self.assertTrue(r.adapter_target.startswith("http://127.0.0.1:"))

    def test_post_json(self):
        self.client.force_authenticate(user=self.user)
        r = self.client.post("/api/whoami/", {"a": [1, 2], "b": "x"}, format="json")
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.data["body"], {"a": [1, 2], "b": "x"})

    def test_post_multipart(self):
        self.client.force_authenticate(user=self.user)
        r = self.client.post("/api/whoami/", {"title": "t"})
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.data["body"], {"title": "t"})

    def test_anonymous(self):
        r = self.client.get("/api/whoami/")
        self.assertEqual(r.status_code, 401)
        self.assertEqual(r.data, {"detail": "Authentication credentials were not provided."})

    def test_force_authenticate_reset(self):
        self.client.force_authenticate(user=self.user)
        self.assertEqual(self.client.get("/api/whoami/").status_code, 200)
        self.client.force_authenticate(user=None)
        self.assertEqual(self.client.get("/api/whoami/").status_code, 401)

    def test_session_with_csrf(self):
        self.client.force_login(self.user)
        r = self.client.post("/api/whoami/", {"x": 1}, format="json")
        self.assertEqual(r.status_code, 200, r.content)
        self.assertEqual(r.data["user"], "admin")

    def test_enforced_csrf_stays_enforced(self):
        c = APIClient(enforce_csrf_checks=True)
        c.force_login(self.user)
        r = c.post("/api/whoami/", {"x": 1}, format="json")
        self.assertEqual(r.status_code, 403)

    def test_redirect(self):
        r = self.client.get("/api/redirect/")
        self.assertEqual(r.status_code, 302)
        self.assertEqual(r.url, "/api/whoami/")
        self.client.force_authenticate(user=self.user)
        r = self.client.get("/api/redirect/", follow=True)
        self.assertEqual(r.status_code, 200)

    def test_other_paths_in_process(self):
        r = self.client.get("/local/")
        self.assertEqual(r.json()["thread"], "MainThread")
