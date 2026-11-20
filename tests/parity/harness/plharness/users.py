"""User catalogue for parity and security tests.

Sources (first hit wins):
  1. ``PARITY_USERS_FILE`` (JSON)
  2. ``reference/users.json``
  3. a fenced block ```` ```json parity-users ```` in ``reference/README.md``
  4. heuristics on ``reference/README.md``: markdown table rows containing a 40-hex token

JSON format::

    {"users": [
       {"name": "admin",   "username": "admin", "password": "...", "token": "<40 hex>",
        "role": "superuser"},
       {"name": "user1",   "username": "alice", "password": "...", "token": "...", "role": "user"},
       ...
    ]}

Logical role names used by the catalogues (missing ones -> cases are skipped):
  admin     superuser
  user1     regular user with model permissions, owns some objects
  user2     regular user, owns objects not shared with user1
  noperm    authenticated, no model permissions at all
  inactive  is_active = false (token still in DB)

User references in cases:
  "anon"            no credentials
  "badtoken"        syntactically valid but unknown token
  "<name>"          token auth (default)
  "<name>@session"  allauth session login (cookie + CSRF)
  "<name>@basic"    HTTP basic auth (username/password)
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path

from .config import REFERENCE_DIR

BAD_TOKEN = "0" * 40
TOKEN_RE = re.compile(r"\b[0-9a-f]{40}\b")


@dataclass
class User:
    name: str
    username: str | None = None
    password: str | None = None
    token: str | None = None
    role: str | None = None


class UserCatalog:
    def __init__(self, users: list[User], source: str) -> None:
        self.users = {u.name: u for u in users}
        self.source = source

    def get(self, name: str) -> User | None:
        return self.users.get(name)

    def secrets(self) -> list[str]:
        out = []
        for u in self.users.values():
            if u.token:
                out.append(u.token)
            if u.password and len(u.password) >= 6:
                out.append(u.password)
        return out

    def resolve(self, ref: str) -> tuple[str, tuple | None, User | None] | None:
        """Map a user reference to (mode, auth tuple, user). None if unavailable."""
        if ref == "anon":
            return ("anon", None, None)
        if ref == "badtoken":
            return ("token", ("token", BAD_TOKEN), None)
        name, _, mode = ref.partition("@")
        mode = mode or "token"
        user = self.get(name)
        if user is None:
            return None
        if mode == "token":
            return ("token", ("token", user.token), user) if user.token else None
        if mode == "basic":
            if user.username and user.password:
                return ("basic", ("basic", user.username, user.password), user)
            return None
        if mode == "session":
            if user.username and user.password:
                return ("session", None, user)
            return None
        return None


def _from_json(data: dict, source: str) -> UserCatalog:
    users = []
    raw = data.get("users", data)
    if isinstance(raw, dict):  # {"admin": {...}}
        raw = [dict(v, name=k) for k, v in raw.items()]
    for entry in raw:
        users.append(
            User(
                name=entry["name"],
                username=entry.get("username", entry["name"]),
                password=entry.get("password"),
                token=entry.get("token"),
                role=entry.get("role"),
            )
        )
    return UserCatalog(users, source)


def _from_readme(text: str, source: str) -> UserCatalog | None:
    m = re.search(r"```json\s+parity-users\s*\n(.*?)```", text, re.S)
    if m:
        return _from_json(json.loads(m.group(1)), source + "#parity-users")
    users: list[User] = []
    header: list[str] | None = None
    for line in text.splitlines():
        if not line.strip().startswith("|"):
            header = None
            continue
        cells = [c.strip().strip("`") for c in line.strip().strip("|").split("|")]
        if all(set(c) <= set("-: ") for c in cells):
            continue
        tok = next((c for c in cells if TOKEN_RE.fullmatch(c)), None)
        if tok is None:
            if header is None:
                header = [c.lower() for c in cells]
            continue
        name = cells[0]
        pw = None
        role = None
        if header:
            for i, h in enumerate(header):
                if i < len(cells) and ("passw" in h or h == "pw"):
                    pw = cells[i]
                if i < len(cells) and ("rolle" in h or "role" in h):
                    role = cells[i]
        users.append(User(name=name, username=name, password=pw, token=tok, role=role))
    if users:
        return UserCatalog(users, source + "#table")
    return None


def load(users_file: str | None = None) -> UserCatalog:
    if users_file:
        p = Path(users_file)
        return _from_json(json.loads(p.read_text(encoding="utf-8")), str(p))
    p = REFERENCE_DIR / "users.json"
    if p.exists():
        return _from_json(json.loads(p.read_text(encoding="utf-8")), str(p))
    readme = REFERENCE_DIR / "README.md"
    if readme.exists():
        cat = _from_readme(readme.read_text(encoding="utf-8"), str(readme))
        if cat:
            return cat
    return UserCatalog([], "none")
