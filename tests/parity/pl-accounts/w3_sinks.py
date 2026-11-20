"""Test sinks for the pl-accounts parity (127.0.0.1 only, local test values only).

* SMTP sink on 127.0.0.1:8125: accepts every mail (no TLS, no AUTH) and stores it as
  `<n>.eml` in W3_WORK/mail/ (order = arrival).
* Fake OIDC IdP on 127.0.0.1:8126 (`server_url`): discovery, /authorize (redirects immediately with
  `code` back; user via `login_hint` or default "oidcuser"), /token (id_token HS256,
  signature is not checked by allauth after direct token retrieval), /userinfo, /jwks.

Invocation: python -I w3_sinks.py <work_dir>
"""

import base64
import hashlib
import hmac
import json
import os
import socketserver
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

WORK = sys.argv[1] if len(sys.argv) > 1 else "."
MAIL_DIR = os.path.join(WORK, "mail")
os.makedirs(MAIL_DIR, exist_ok=True)
IDP = "http://127.0.0.1:8126"
CODES: dict[str, dict] = {}
LOCK = threading.Lock()


class SMTPHandler(socketserver.StreamRequestHandler):
    def send(self, line: str) -> None:
        self.wfile.write((line + "\r\n").encode())

    def handle(self) -> None:
        self.send("220 w3-sink ESMTP")
        rcpts: list[str] = []
        mail_from = ""
        while True:
            raw = self.rfile.readline()
            if not raw:
                return
            line = raw.decode("latin-1").rstrip("\r\n")
            cmd = line[:4].upper()
            if cmd in ("EHLO", "HELO"):
                self.send("250-w3-sink")
                self.send("250 8BITMIME")
            elif cmd == "MAIL":
                mail_from, rcpts = line[10:], []
                self.send("250 OK")
            elif cmd == "RCPT":
                rcpts.append(line[8:])
                self.send("250 OK")
            elif cmd == "DATA":
                self.send("354 go")
                data = b""
                while True:
                    chunk = self.rfile.readline()
                    if chunk in (b".\r\n", b".\n", b""):
                        break
                    if chunk.startswith(b".."):
                        chunk = chunk[1:]
                    data += chunk
                with LOCK:
                    n = len([f for f in os.listdir(MAIL_DIR) if f.endswith(".eml")])
                    path = os.path.join(MAIL_DIR, f"{n:04d}.eml")
                    with open(path, "wb") as f:
                        f.write(f"X-Sink-From: {mail_from}\r\nX-Sink-To: {','.join(rcpts)}\r\n".encode())
                        f.write(data)
                self.send("250 queued")
            elif cmd == "QUIT":
                self.send("221 bye")
                return
            elif cmd in ("RSET", "NOOP"):
                self.send("250 OK")
            else:
                self.send("502 not implemented")


def b64u(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def make_id_token(claims: dict) -> str:
    head = b64u(json.dumps({"alg": "HS256", "typ": "JWT", "kid": "w3"}).encode())
    body = b64u(json.dumps(claims).encode())
    sig = hmac.new(b"w3secret", f"{head}.{body}".encode(), hashlib.sha256).digest()
    return f"{head}.{body}.{b64u(sig)}"


class IdPHandler(BaseHTTPRequestHandler):
    def log_message(self, *a) -> None:
        pass

    def reply(self, status: int, obj=None, headers=None) -> None:
        body = json.dumps(obj).encode() if obj is not None else b""
        self.send_response(status)
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        if obj is not None:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        if u.path == "/.well-known/openid-configuration":
            self.reply(200, {
                "issuer": IDP,
                "authorization_endpoint": IDP + "/authorize",
                "token_endpoint": IDP + "/token",
                "userinfo_endpoint": IDP + "/userinfo",
                "jwks_uri": IDP + "/jwks",
                "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
            })
        elif u.path == "/jwks":
            self.reply(200, {"keys": []})
        elif u.path == "/authorize":
            user = q.get("login_hint") or "oidcuser"
            code = b64u(os.urandom(12))
            with LOCK:
                CODES[code] = {"user": user, "nonce": q.get("nonce"), "challenge": q.get("code_challenge")}
            loc = q["redirect_uri"] + "?" + urllib.parse.urlencode({"code": code, "state": q.get("state", "")})
            self.reply(302, None, {"Location": loc})
        elif u.path == "/userinfo":
            tok = self.headers.get("Authorization", "")[7:]
            user = tok.split(".", 1)[0] if tok else ""
            self.reply(200, {"sub": "sub-" + user, "email": f"{user}@example.invalid",
                             "preferred_username": user, "name": user.title(), "groups": ["editors"]})
        else:
            self.reply(404, {"error": "not_found"})

    def do_POST(self) -> None:
        n = int(self.headers.get("Content-Length") or 0)
        form = dict(urllib.parse.parse_qsl(self.rfile.read(n).decode()))
        if self.path != "/token":
            self.reply(404, {"error": "not_found"})
            return
        with LOCK:
            entry = CODES.pop(form.get("code", ""), None)
        if not entry:
            self.reply(400, {"error": "invalid_grant"})
            return
        user = entry["user"]
        now = int(time.time())
        claims = {"iss": IDP, "aud": "w3client", "sub": "sub-" + user, "iat": now, "exp": now + 300,
                  "email": f"{user}@example.invalid", "preferred_username": user}
        if entry["nonce"]:
            claims["nonce"] = entry["nonce"]
        if user.endswith("-mfa"):
            claims["amr"] = ["pwd", "mfa"]
        self.reply(200, {"access_token": f"{user}.{b64u(os.urandom(8))}", "token_type": "Bearer",
                         "expires_in": 300, "id_token": make_id_token(claims)})


def main() -> None:
    smtp = socketserver.ThreadingTCPServer(("127.0.0.1", 8125), SMTPHandler)
    smtp.daemon_threads = True
    threading.Thread(target=smtp.serve_forever, daemon=True).start()
    ThreadingHTTPServer(("127.0.0.1", 8126), IdPHandler).serve_forever()


if __name__ == "__main__":
    main()
