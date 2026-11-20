"""Test sinks for the social/OIDC parity of W12 (127.0.0.1 only, local test values only).

* SMTP sink 127.0.0.1:8142: accepts every mail, stores it as <n>.eml in W12_WORK/mail/.
* Fake OIDC IdP 127.0.0.1:8141 (`server_url`): discovery, /authorize (redirects immediately back
  with `code`), /token (checks client_secret, redirect_uri and PKCE), /userinfo, /jwks, /_last (last
  requests). Behavior via `login_hint` = "<user>~flag~flag" (via `auth_params=login_hint%3D...`):
  mfa (amr pwd+mfa), badaud, listaud, badiss, expired, badsig, noemail, verified (email_verified),
  g=<group> (group claim, multiple), deny (error=access_denied), err (error=server_error),
  tokenfail, nouserinfo, noid (no id_token), jti (fixed jti), refresh (refresh_token).

Invocation: python -I w12_sinks.py <work_dir>
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
IDP = "http://127.0.0.1:8141"
CLIENT_ID = "w12client"
CLIENT_SECRET = "w12secret"
CODES: dict = {}
LAST: dict = {}
LOCK = threading.Lock()


class SMTPHandler(socketserver.StreamRequestHandler):
    def send(self, line):
        self.wfile.write((line + "\r\n").encode())

    def handle(self):
        self.send("220 w12-sink ESMTP")
        rcpts = []
        mail_from = ""
        while True:
            raw = self.rfile.readline()
            if not raw:
                return
            line = raw.decode("latin-1").rstrip("\r\n")
            cmd = line[:4].upper()
            if cmd in ("EHLO", "HELO"):
                self.send("250-w12-sink")
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
                    with open(os.path.join(MAIL_DIR, "%04d.eml" % n), "wb") as f:
                        f.write(("X-Sink-From: %s\r\nX-Sink-To: %s\r\n" % (mail_from, ",".join(rcpts))).encode())
                        f.write(data)
                self.send("250 queued")
            elif cmd == "QUIT":
                self.send("221 bye")
                return
            elif cmd in ("RSET", "NOOP"):
                self.send("250 OK")
            else:
                self.send("502 not implemented")


def b64u(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def parse_hint(hint):
    parts = (hint or "oidcuser").split("~")
    flags = set(p for p in parts[1:] if not p.startswith("g="))
    groups = [p[2:] for p in parts[1:] if p.startswith("g=")]
    return parts[0], flags, groups


def make_id_token(claims, flags):
    head = b64u(json.dumps({"alg": "HS256", "typ": "JWT", "kid": "w12"}).encode())
    body = b64u(json.dumps(claims).encode())
    sig = hmac.new(CLIENT_SECRET.encode(), (head + "." + body).encode(), hashlib.sha256).digest()
    if "badsig" in flags:
        sig = b"\x00" * 32
    return head + "." + body + "." + b64u(sig)


class IdPHandler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, status, obj=None, headers=None):
        body = json.dumps(obj).encode() if obj is not None else b""
        self.send_response(status)
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        if obj is not None:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query, keep_blank_values=True))
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
        elif u.path == "/_last":
            with LOCK:
                self.reply(200, dict(LAST))
        elif u.path == "/authorize":
            user, flags, _ = parse_hint(q.get("login_hint", ""))
            with LOCK:
                LAST["authorize"] = q
            if "deny" in flags or "err" in flags:
                err = "access_denied" if "deny" in flags else "server_error"
                loc = q["redirect_uri"] + "?" + urllib.parse.urlencode({"error": err, "state": q.get("state", "")})
                self.reply(302, None, {"Location": loc})
                return
            code = b64u(os.urandom(12))
            with LOCK:
                CODES[code] = {"hint": q.get("login_hint", ""), "challenge": q.get("code_challenge"),
                               "redirect_uri": q.get("redirect_uri")}
            loc = q["redirect_uri"] + "?" + urllib.parse.urlencode({"code": code, "state": q.get("state", "")})
            self.reply(302, None, {"Location": loc})
        elif u.path == "/userinfo":
            tok = self.headers.get("Authorization", "")[7:]
            hint = base64.urlsafe_b64decode(tok.split(".", 1)[0] + "==").decode() if tok else ""
            user, flags, groups = parse_hint(hint)
            if "nouserinfo" in flags:
                self.reply(500, {"error": "boom"})
                return
            info = {"sub": "sub-" + user, "preferred_username": user, "name": user.title() + " Tester"}
            if "noemail" not in flags:
                info["email"] = user + "@example.invalid"
            if "verified" in flags:
                info["email_verified"] = True
            if groups:
                info["groups"] = groups
            self.reply(200, info)
        else:
            self.reply(404, {"error": "not_found"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        form = dict(urllib.parse.parse_qsl(self.rfile.read(n).decode(), keep_blank_values=True))
        if self.path != "/token":
            self.reply(404, {"error": "not_found"})
            return
        basic = self.headers.get("Authorization", "")
        with LOCK:
            LAST["token"] = {k: v for k, v in form.items() if k != "client_secret"}
            LAST["token"]["has_secret"] = form.get("client_secret") == CLIENT_SECRET
            LAST["token"]["basic"] = basic.startswith("Basic ")
            entry = CODES.pop(form.get("code", ""), None)
        if not entry:
            self.reply(400, {"error": "invalid_grant"})
            return
        if form.get("client_secret") != CLIENT_SECRET or form.get("client_id") != CLIENT_ID:
            self.reply(401, {"error": "invalid_client"})
            return
        if entry["challenge"]:
            v = form.get("code_verifier", "")
            if b64u(hashlib.sha256(v.encode()).digest()) != entry["challenge"]:
                self.reply(400, {"error": "invalid_grant", "error_description": "pkce"})
                return
        if form.get("redirect_uri") != entry["redirect_uri"]:
            self.reply(400, {"error": "invalid_grant", "error_description": "redirect_uri"})
            return
        user, flags, groups = parse_hint(entry["hint"])
        if "tokenfail" in flags:
            self.reply(400, {"error": "invalid_request"})
            return
        now = int(time.time())
        if "badaud" in flags:
            aud = "someone-else"
        elif "listaud" in flags:
            aud = [CLIENT_ID, "x"]
        else:
            aud = CLIENT_ID
        claims = {"iss": IDP + ("/other" if "badiss" in flags else ""), "aud": aud,
                  "sub": "sub-" + user, "iat": now, "exp": now - 30 if "expired" in flags else now + 300,
                  "preferred_username": user}
        if "noemail" not in flags:
            claims["email"] = user + "@example.invalid"
        if "mfa" in flags:
            claims["amr"] = ["pwd", "mfa"]
        if "jti" in flags:
            claims["jti"] = "fixed-jti-" + user
        if groups:
            claims["groups"] = groups
        resp = {"access_token": b64u(entry["hint"].encode()) + "." + b64u(os.urandom(8)),
                "token_type": "Bearer", "expires_in": 300}
        if "refresh" in flags:
            resp["refresh_token"] = "r-" + b64u(os.urandom(6))
        if "noid" not in flags:
            resp["id_token"] = make_id_token(claims, flags)
        self.reply(200, resp)


def main():
    smtp = socketserver.ThreadingTCPServer(("127.0.0.1", 8142), SMTPHandler)
    smtp.daemon_threads = True
    threading.Thread(target=smtp.serve_forever, daemon=True).start()
    ThreadingHTTPServer(("127.0.0.1", 8141), IdPHandler).serve_forever()


if __name__ == "__main__":
    main()
