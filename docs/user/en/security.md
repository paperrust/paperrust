# Security

[Deutsch](../security.md) · [Documentation overview](README.md)

PaperRust's standard: **not less secure than paperless-ngx v3.4.2**, stricter where possible.
Permissions, login and sessions behave as upstream (Django/DRF/allauth/guardian re-implemented and
checked against Django). Please report vulnerabilities confidentially: [SECURITY.md](../../../SECURITY.md).

<a id="deployment"></a>

## Deployment

- **Loopback by default:** PaperRust listens on `127.0.0.1:8080`. Any other address must be set
  explicitly in `PAPERRUST_BIND` (warning in the log). For network access put a reverse proxy with
  **TLS** in front.
- **`PAPERRUST_SECRET_KEY`**: long, random, secret; it signs sessions, CSRF and share links. Keep the
  previous key when reusing a paperless-ngx database.
- **`PAPERRUST_URL`, `PAPERRUST_ALLOWED_HOSTS`, `PAPERRUST_CSRF_TRUSTED_ORIGINS`** as for paperless-ngx
  ([Configuration](configuration.md#hosting-and-security)).
- **Database:** own user with own password, PostgreSQL not publicly reachable. The desktop app creates
  a random 32-character password (SCRAM) and binds to `127.0.0.1` only.
- **Multi-user computers:** other local users can reach `127.0.0.1` too – the login (MFA possible)
  protects the UI. The desktop app signs in only your own Windows
  account automatically: a random key per start in the data folder (readable by that account only),
  valid only for connections from `127.0.0.1`/`::1`, failed attempts throttled
  ([signing in to the desktop app](installation.md#desktop-login)).
- **Proxy headers:** throttling evaluates `X-Forwarded-For` like DRF. Configure the outer reverse
  proxy to replace client-supplied `X-Forwarded-For` values.

<a id="accounts"></a>

## Login and permissions

- Passwords with PBKDF2-SHA256 like Django; login throttling, TOTP MFA with replay protection,
  password reset, OpenID Connect (state per session, PKCE, iss/aud/exp checks as in allauth).
- Permissions (global and per object) are **always checked server-side**, also for search,
  suggestions, AI and MCP. Filters from request parameters cannot bypass permissions.
- Error messages and logs contain no passwords, tokens or internals.

<a id="files"></a>

## Files, import, outgoing connections

- **Paths:** storage-path templates and file operations stay inside the media folders (source and
  target checked; `..`, `\`, NUL rejected).
- **Import stricter than upstream:** file names in the manifest must be relative and stay inside the
  media folders, referenced files inside the import folder; symlinks are not followed; ZIP limits
  against zip bombs (`PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES`) and ZIP64 checks; PBKDF2 parameters bounded.
  Still import exports from third parties only deliberately.
- **Uploads** are streamed; size limits for non-file data as in Django (2.5 MiB).
- **Outgoing targets** (webhooks, remote OCR, LLM endpoints) are validated as upstream; webhooks only
  `http`/`https`.
- **Pickle** is never read from untrusted sources; the classifier uses its own format.

<a id="ai"></a>

## AI

Document content is untrusted; automatic AI processing may only suggest and assign, has quantity limits
and logs every change. MCP rate limiting happens before the token check. Details:
[AI → Security](ai.md#security).

<a id="reviews"></a>

## Reviews

- Independent security review (2026-11-08): 0 critical, 2 high, 6 medium, 20 low; all high and
  medium findings are fixed.
- Second independent code review before the release (2026-11-19,
  [report](../../../results/release/review/security-2026-11-19.md), German): no critical or high
  findings; the medium and low findings are fixed.
- Security test suite against Django and PaperRust: `tests/security/` (authentication, CSRF, headers,
  leaks, permissions, path traversal, rate limiting).
- Parity tests: same status codes and responses as Django, including denied access.
