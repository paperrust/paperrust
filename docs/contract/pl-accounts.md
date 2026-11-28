# Vertrag `pl-accounts` – Konten-Seiten (django-allauth) ohne Django

Quelle: paperless-ngx v3.4.2 (`upstream/`), django-allauth **65.20.1** (`upstream/uv.lock`,
`reference/venv/Lib/site-packages/allauth`), Django 5.2.18. Alle Pfade upstream relativ zu
`upstream/src/` bzw. zum allauth-Paket.

| Datei | Inhalt |
|---|---|
| `paperless/urls.py` | `/accounts/*` (Teilmenge der allauth-URLs), `/api/auth/login|logout/` (Namespace `rest_framework`, **dieselben** allauth-Views), `/api/auth/headless/` (`allauth.headless.urls`) |
| `paperless/adapter.py` | `CustomAccountAdapter` (`is_open_for_signup`, `pre_authenticate` mit `DISABLE_REGULAR_LOGIN`, `is_safe_url`, `get_reset_password_from_key_url` mit `PAPERLESS_URL`, `save_user`: Erstinstallation → Superuser, Standardgruppen, `get_login_stages` → `SocialAccountAuthenticateStage`), `CustomSocialAccountAdapter`, `DrfTokenStrategy` |
| `paperless/settings/__init__.py`, `settings/custom.py` | siehe „Einstellungen“ |
| `documents/context_processors.py` | `EMAIL_ENABLED`, `DISABLE_REGULAR_LOGIN`, `REDIRECT_LOGIN_TO_SSO`, `ACCOUNT_ALLOW_SIGNUPS`, `APP_TITLE`/`APP_LOGO` (DB vor Umgebung), `FIRST_INSTALL` |
| `documents/templates/{paperless-ngx/base.html, account/*, mfa/authenticate.html, socialaccount/*}` | Seiten (in `crates/pl-accounts/templates/` nach minijinja übertragen, gleiche Leerzeichen) |
| `paperless/auth.py` | `AutoLoginMiddleware`, `HttpRemoteUserMiddleware` (Persistent) |
| `paperless/signals.py` | `handle_failed_login` (Log), `handle_social_account_updated` (Gruppen-Sync, nur Social) |
| allauth socialaccount | `providers/openid_connect/{provider,views,urls}.py`, `providers/oauth2/{client,provider,utils,views}.py`, `providers/base/{provider,views,utils}.py`, `internal/{statekit,jwtkit}.py`, `internal/flows/{login,signup,connect,email_authentication}.py`, `{adapter,models,views,forms,helpers,app_settings}.py`, `core/internal/modelkit.py`; account `internal/flows/{email_verification,manage_email,signup}.py`, `views.py::ConfirmEmailView`, Templates `allauth/layouts/base.html`, `account/{email_confirm,signup_closed}.html`, `socialaccount/login_cancelled.html`, `account/email/email_confirmation_*` |
| allauth | `account/{views,forms,adapter,app_settings,auth_backends,stages,utils,middleware,mixins}.py`, `account/internal/{flows/*,stagekit,templatekit,decorators,userkit}.py`, `core/internal/{ratelimit,httpkit}.py`, `mfa/{stages,adapter}.py`, `mfa/base/{views,forms}.py`, `mfa/base/internal/flows.py`, `mfa/totp/internal/auth.py`, `mfa/recovery_codes/internal/auth.py`, `socialaccount/{adapter,templatetags/socialaccount}.py`, Templates `account/messages/*.txt`, `account/email/*` |
| Django | `contrib/sessions/{backends/base,db,cached_db, middleware}.py`, `contrib/auth/{__init__,tokens,backends}.py`, `contrib/messages/storage/{base,cookie,fallback}.py`, `middleware/csrf.py`, `views/csrf.py` (+ `views/templates/csrf_403.html`), `views/defaults.py`, `views/decorators/cache.py`, `utils/http.py` (`url_has_allowed_host_and_scheme`), `core/mail/*` |

## Einstellungen (wie upstream gelesen)

| Setting | Quelle | Wirkung |
|---|---|---|
| `LOGIN_URL` / `LOGIN_REDIRECT_URL` / `LOGOUT_REDIRECT_URL` | `FORCE_SCRIPT_NAME` + `accounts/login/`, `dashboard`, `PAPERLESS_LOGOUT_REDIRECT_URL` (Std. `<login>?loggedout=1`) | Weiterleitungen |
| `ACCOUNT_LOGOUT_ON_GET` | `True` | GET `/accounts/logout/` meldet ab (Frontend nutzt `accounts/logout/?next=…accounts/login/?next=…`) |
| `ACCOUNT_SESSION_REMEMBER` | `PAPERLESS_ACCOUNT_SESSION_REMEMBER` (Std. true) | kein „angemeldet bleiben“-Feld; `true` → `set_expiry(SESSION_COOKIE_AGE)`, `false` → `set_expiry(0)` (Browser-Ende); `SESSION_EXPIRE_AT_BROWSER_CLOSE = not REMEMBER` |
| `SESSION_COOKIE_AGE` | `PAPERLESS_SESSION_COOKIE_AGE` (Std. 1 814 400 = 3 Wochen) | |
| `SESSION_ENGINE` | `cached_db` | DB + Django-Cache |
| `ACCOUNT_ALLOW_SIGNUPS` | `PAPERLESS_ACCOUNT_ALLOW_SIGNUPS` (Std. false); Erstinstallation (keine Nutzer außer `consumer`/`AnonymousUser` **und** keine Dokumente inkl. Papierkorb) → immer offen | |
| `ACCOUNT_DEFAULT_GROUPS` | `PAPERLESS_ACCOUNT_DEFAULT_GROUPS` | Gruppen neuer Nutzer |
| `ACCOUNT_EMAIL_VERIFICATION` | `none`, wenn `EMAIL_ENABLED` falsch, sonst `PAPERLESS_ACCOUNT_EMAIL_VERIFICATION` (Std. `optional`) | |
| `ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` | `PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` (Std. true) | Mail „Unknown Account“ bei Reset für unbekannte Adresse |
| `ACCOUNT_DEFAULT_HTTP_PROTOCOL` | `PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL` (Std. https) | absolute URLs in Mails ohne `PAPERLESS_URL` (Reset-Link) bzw. immer (Signup-Link der Unknown-Mail) |
| `DISABLE_REGULAR_LOGIN` / `REDIRECT_LOGIN_TO_SSO` | `PAPERLESS_*` | Login-Formular aus / automatische SSO-Weiterleitung (Template-Skript) |
| `SOCIALACCOUNT_PROVIDERS` | `PAPERLESS_SOCIALACCOUNT_PROVIDERS` (JSON) + Provider-Apps aus `PAPERLESS_APPS` | Provider-Liste |
| `SOCIALACCOUNT_ALLOW_SIGNUPS` | `PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS` (Std. yes) | `CustomSocialAccountAdapter.is_open_for_signup` (unabhängig von `ACCOUNT_ALLOW_SIGNUPS`) |
| `SOCIALACCOUNT_AUTO_SIGNUP` | `PAPERLESS_SOCIAL_AUTO_SIGNUP` (Std. **false**) | ohne → Signup-Formular `/accounts/3rdparty/signup/` |
| `SOCIAL_ACCOUNT_DEFAULT_GROUPS`, `SOCIAL_ACCOUNT_SYNC_GROUPS(_CLAIM)`, `…_SYNC_SUPERUSER_GROUP`, `…_SYNC_STAFF_GROUP` | `PAPERLESS_*` (Claim Std. `groups`) | `save_user` bzw. `handle_social_account_updated` |
| `SOCIAL_ACCOUNT_MFA_TRUSTED_AMR` | `get_list_from_env("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", default=["mfa"])` | **Abweichung von v3.4.2 (upstream-Fix cb3513c6f, #14693):** v3.4.2 übergab `"mfa"` als *Trenner* (Standard `[]`, 2FA trotz `amr=mfa`). PaperRust: Standard `["mfa"]`, Trenner `,`. social_parity Variante a prüft `mfa.amr_trusted` nur noch auf Rust-Seite |
| `SOCIALACCOUNT_*` (allauth-Standard) | – | `LOGIN_ON_GET` False, `STORE_TOKENS` False (keine `socialaccount_socialtoken`-Zeilen), `EMAIL_AUTHENTICATION(_AUTO_CONNECT)` False, `EMAIL_REQUIRED` False, `REQUESTS_TIMEOUT` 5 s, `UID_MAX_LENGTH` 191, `OPENID_CONNECT_URL_PREFIX` `oidc`, `EMAIL_VERIFICATION` None (→ account-Wert) |
| `ACCOUNT_*` (E-Mail, allauth-Standard) | – | `EMAIL_CONFIRMATION_HMAC` True (Schlüssel `signing.dumps(pk, salt="account")`, 3 Tage), `UNIQUE_EMAIL` True, `PREVENT_ENUMERATION` True, `CONFIRM_EMAIL_ON_GET` False, `LOGIN_ON_EMAIL_CONFIRMATION` False, `CHANGE_EMAIL` False, Begrenzung `confirm_email` 1/180s/key |
| `MFA_*` | Standard (`SUPPORTED_TYPES = [recovery_codes, totp]`, `TOTP_PERIOD 30`, Toleranz 0, kein Trust), `MFA_TOTP_ISSUER = Paperless-ngx` | |
| `ACCOUNT_RATE_LIMITS` | Standard | `login` 30/m/ip, `login_failed` 10/m/ip + 5/300s/key, `signup` 20/m/ip, `reset_password` 20/m/ip + 5/m/key, `reset_password_from_key` 20/m/ip |
| `ALLAUTH_TRUSTED_PROXY_COUNT` / `…_CLIENT_IP_HEADER` | `PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT` (Std. Anzahl `PAPERLESS_TRUSTED_PROXIES`), `PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER` | Client-IP der Begrenzung |
| `EMAIL_*` | `PAPERLESS_EMAIL_HOST/PORT/HOST_USER/HOST_PASSWORD/USE_TLS/USE_SSL/FROM`; `EMAIL_ENABLED = HOST != localhost or HOST_USER != ""` | SMTP |
| `AUTO_LOGIN_USERNAME`, `PAPERLESS_ENABLE_HTTP_REMOTE_USER(_API)`, `PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME` | | Middleware (siehe unten) |
| `COOKIE_PREFIX` | `PAPERLESS_COOKIE_PREFIX` | `sessionid`, `csrftoken`, `django_language` mit Präfix; `messages` **ohne** Präfix |

## Gemeinsames Verhalten aller Seiten

* **Middleware-Reihenfolge** (Request): Session laden → CSRF-Cookie lesen → `request.user`
  (`get_user`: Backend ∈ `AUTHENTICATION_BACKENDS`, Nutzer aktiv, `_auth_user_hash` konstantzeitig;
  Abweichung → `session.flush()`) → `AutoLoginMiddleware` (jeder Request: `auth.login` mit
  ModelBackend) → `HttpRemoteUserMiddleware` (Header gesetzt und ≠ angemeldeter Nutzer: ggf.
  Remote-Logout, `get_or_create`, `login` mit `RemoteUserBackend`; ohne Header bleibt die Sitzung) →
  CSRF-Prüfung bei POST → View.
* **CSRF** (`CsrfViewMiddleware`): Origin (bzw. Referer bei HTTPS), dann Cookie (32/64 Zeichen,
  `[A-Za-z0-9]`), Token aus Formularfeld `csrfmiddlewaretoken`, sonst Header `X-CSRFToken`.
  Fehler → **403**, eigene Seite `csrf_403.html` (gleiche Gründe „no cookie“/„no referer“ wie
  Djangos `csrf_failure`, eigene Texte). `{% csrf_token %}` → `get_token` (Cookie wird bei **jeder** Seite mit Token erneut
  gesendet, `Vary: Cookie`), Login → `rotate_token`.
* **Sitzung schreiben** (`SessionMiddleware.process_response`, `cached_db`): nur wenn verändert
  und nicht leer; neue Schlüssel 32 Zeichen `[a-z0-9]`; Daten
  `signing.dumps(dict, salt="django.contrib.sessions.SessionStore", compress=True)` (JSON
  kompakt, `ensure_ascii`); `expire_date = now + get_expiry_age()` (fehlendes/falsches
  `_session_expiry`, also auch `set_expiry(0)` = „bis der Browser schließt“ → `SESSION_COOKIE_AGE`;
  nur das Cookie wird dann ein Sitzungs-Cookie, die DB-Zeile bleibt normal gültig); Cookie
  `sessionid=<key>; expires=<http_date>; HttpOnly; Max-Age=<age>; Path=/; SameSite=Lax` bzw. ohne
  `expires`/`Max-Age` bei Browser-Ende; Cookie vorhanden + Sitzung leer → Lösch-Cookie
  (`=""; expires=Thu, 01 Jan 1970 …; Max-Age=0; Path=/; SameSite=Lax`). Antworten ≥ 500 speichern nicht.
  Vorhandene Sitzung = `save(must_create=False)`: **nur `UPDATE`** (`pl_auth_api::session::save_row`);
  ist die Zeile weg (paralleler Logout/`flush`), wird nichts geschrieben: `UpdateError` →
  `SessionInterrupted` → Djangos 400-Seite ohne die Cookies der View (`SessionError::Interrupted`
  in `web.rs::Ctx::finish`). Neue Schlüssel nur über `create` (keine gültige Sitzung) bzw.
  `cycle_key`/`flush`. Bis 2026-11-27 speicherte Rust per `INSERT … ON CONFLICT DO UPDATE` und legte
  einen per Logout gelöschten Schlüssel mit den alten Anmeldedaten wieder an; außerdem schrieb es bei
  `set_expiry(0)` `expire_date = now` (Sitzung beim nächsten Request schon abgelaufen). Beides
  behoben, Test `tests/session_races.rs` (`#[ignore]`, DB) und `session::tests::browser_close_keeps_server_side_age`.
* **`login(request, user, backend)`**: Sitzung mit anderem Nutzer/Hash → `flush`, sonst
  `cycle_key` (Daten bleiben, z. B. `account_authentication_methods`), dann `_auth_user_id`
  (String), `_auth_user_backend`, `_auth_user_hash`; CSRF rotieren; `user_logged_in` →
  `last_login = now()`. Backend: ModelBackend (exakter Nutzername), sonst
  `allauth.account.auth_backends.AuthenticationBackend` (case-insensitiv; nach MFA/Signup immer
  dieses, da `user.backend` fehlt).
* **`logout`** (`flows.logout.logout`): angemeldet → Message `auth.msg.signed_out` + `flush`;
  immer `account_login`, `account_authentication_methods`, `account_password_reset_by_code`,
  `_password_reset_key` entfernen.
* **Messages** (`FallbackStorage` → Cookie `messages`, `TimestampSigner(key="django.http.cookies"
  + SECRET_KEY, salt="django.contrib.messages")`, komprimiert; Nachricht
  `["__json_message",0,<level>,<text>,""]`), angezeigt in `layout.html`, danach gelöscht.
  Stufen wie upstream, Texte eigene: Login-Erfolg `auth.msg.signed_in` (25/success), Logout
  `auth.msg.signed_out`, Passwort gesetzt `auth.msg.password_changed`.
* **`never_cache`** (`Expires: <jetzt>`, `Cache-Control: max-age=0, no-cache, no-store,
  must-revalidate, private`) bei Login, Signup, MFA-Seite.
* **Weiterleitungsziel `next`** (`get_next_redirect_url` mit paperless `is_safe_url`):
  `url_has_allowed_host_and_scheme(url, ALLOWED_HOSTS ohne "*" ∪ {request.get_host()},
  require_https=False)` – relative Pfade, eigene Hosts; `//x`, `///x`, `\\x`, Schema ohne Host,
  fremde Hosts, `javascript:`, führende Steuerzeichen → verworfen (Standardziel).
* **Antwort-Header** wie Django; `X-Api-Version`/`X-Version`, wenn am Ende angemeldet
  (`pl_core::Authenticated`). Die übrigen Middleware-Header (Vary Accept-Language/origin/Cookie,
  Content-Language, X-Frame-Options, …) setzt pl-server.
* **Sprache**: `LocaleMiddleware`-Aushandlung (`pl_core::locale`) wie bisher; die Texte kommen
  aus dem **eigenen Katalog** (`crates/pl-accounts/i18n/<locale>.json`, siehe „Eigene
  Konto-Seiten“), nicht mehr aus paperless-/allauth-/Django-gettext.

## Routen

| id | Methode | Pfad | upstream | Status |
|---|---|---|---|---|
| `accounts.login.get` / `.post` | GET, POST | `/accounts/login/` | `LoginView` | fertig (auch `ACCOUNT_EMAIL_VERIFICATION=mandatory` und mit beliebigen Provider-Apps, S1/S2 2026-11-18) |
| `api_auth.login.get` / `.post` | GET, POST | `/api/auth/login/` | `rest_framework:login` → `LoginView` | fertig (wie oben) |
| `accounts.logout.get` / `.post` | GET, POST | `/accounts/logout/` | `LogoutView` | fertig |
| `api_auth.logout.get` / `.post` | GET, POST | `/api/auth/logout/` | `rest_framework:logout` → `LogoutView` | fertig |
| `accounts.account_inactive` | GET | `/accounts/account_inactive/` | `AccountInactiveView` | fertig |
| `accounts.mfa_authenticate.get` / `.post` | GET, POST | `/accounts/2fa/authenticate/` | `mfa.base.views.AuthenticateView` | fertig (TOTP + Recovery-Codes; WebAuthn nicht konfiguriert) |
| `accounts.signup.get` / `.post` | GET, POST | `/accounts/signup/` | `SignupView` | fertig (geschlossen → `signup_closed`, mit E-Mail inkl. Aufzählungsschutz bei `mandatory`) |
| `accounts.password_reset.get` / `.post` | GET, POST | `/accounts/password/reset/` | `PasswordResetView` | fertig (SMTP) |
| `accounts.password_reset_done` | GET | `/accounts/password/reset/done/` | `PasswordResetDoneView` | fertig |
| `accounts.password_reset_from_key.get` / `.post` | GET, POST | `/accounts/password/reset/key/{uidb36}-{key}/` | `PasswordResetFromKeyView` | fertig |
| `accounts.password_reset_from_key_done` | GET | `/accounts/password/reset/key/done/` | `PasswordResetFromKeyDoneView` | fertig |
| `accounts.oidc_login.get` / `.post` | GET, POST | `/accounts/oidc/{provider_id}/login/` | `openid_connect.views.login` → `OAuth2LoginView` | fertig (GET Zwischenseite, POST/andere Methoden → IdP) |
| `accounts.oidc_callback.get` / `.post` | GET, POST | `/accounts/oidc/{provider_id}/login/callback/` | `openid_connect.views.callback` → `OAuth2CallbackView` | fertig (auch `mandatory`); Headless-State → Login-Fehlerseite wie unbekannter State (headless nicht nachgebaut, 2026-11-20) |
| `accounts.provider_login.get` / `.post` | GET, POST | `/accounts/{provider}/login/` | `OAuth2LoginView` der Provider `github`, `gitlab`, `google`, `microsoft`, `nextcloud`, `discord`, `gitea` (`oauth2/urls.py::default_urlpatterns`) | fertig (S2); nicht installierter/unbekannter Provider → 404, installiert ohne App → 500 (`SocialApp.DoesNotExist`, wie allauth) |
| `accounts.provider_callback.get` / `.post` | GET, POST | `/accounts/{provider}/login/callback/` | `OAuth2CallbackView` derselben Provider | fertig (S2) |
| `accounts.email_verification_sent` | GET | `/accounts/confirm-email/` | `account/views.py::email_verification_sent` (`EmailVerificationSentView`) | fertig (S1); **in paperless nicht eingebunden**, siehe Known differences 2 |
| `accounts.social_signup.get` / `.post` | GET, POST | `/accounts/3rdparty/signup/` | `socialaccount.views.SignupView` | fertig (auch `mandatory`) |
| `accounts.social_login_cancelled` | GET | `/accounts/3rdparty/login/cancelled/` | `LoginCancelledView` (allauth-Layout) | fertig |
| `accounts.social_login_error` | GET | `/accounts/3rdparty/login/error/` | `LoginErrorView` (401) | fertig |
| `accounts.confirm_email.get` / `.post` | GET, POST | `/accounts/confirm-email/{key}/` | `ConfirmEmailView` (allauth-Layout) | fertig (Schlüssel außerhalb `[-:\w]+` → Djangos 404-Seite, 2026-11-20; upstream Catch-all) |
| – | | übrige Provider (`build_provider_urlpatterns`) | | nicht nachgebaut: werden übergangen (kein Knopf, Pfad 404), einmal Warnung im Log; Kontoseiten bleiben voll nutzbar |
| – | | `/accounts/3rdparty/` (Verbindungen) | – | **gibt es in paperless nicht** (`socialaccount_connections` nicht eingebunden; Pfad fällt auf den SPA-Catch-all `login_required(IndexView)`); Verbinden/Trennen läuft über das Profil (`process=connect`, `/api/profile/disconnect_social_account/`) |
| – | | `/api/auth/headless/*` | allauth headless | nur Vertrag (Django); **vom Angular-Frontend nicht benutzt** (`src-ui` ruft nur `accounts/logout/?next=…` auf) |
| `rs.desktop.login` | GET | `/rs/desktop/login` | – (PaperRust-Erweiterung, Desktop-Betrieb) | fertig (2026-11-19): nur mit `PAPERRUST_DESKTOP_LOGIN_TOKEN` + `PAPERRUST_DESKTOP_OWNER` (sonst 404), nur TCP-Loopback (sonst 403, `X-Forwarded-For` zählt nicht), Drossel `desktop_login_failed` 10/m/ip,30/h/ip, Schlüssel in konstanter Zeit, falsch/fehlend → 403 ohne Hinweis, Eigentümer aktiv → `auth::login` (Sitzung + CSRF rotieren, `last_login`), 302 auf relatives `next` sonst `/rs/app/`; `Cache-Control: no-store`, `Referrer-Policy: no-referrer`; kein MFA-Schritt. Einzelheiten docs/DESKTOP.md Abschnitt 25 |

### `LoginView` (GET/POST)

Reihenfolge: `rate_limit("login")` (nur POST, 30/m/ip → **429** `text/html` mit allauths
Standardtext, ohne `never_cache`) → `never_cache` → angemeldet → 302
`get_login_redirect_url` (`next` falls sicher, sonst `/dashboard`) → offener MFA-Schritt
(`account_login.state.stages.current == "mfa_authenticate"`, nicht abgelaufen) → 302
`/accounts/2fa/authenticate/` → Formular.

Formular (`LoginForm`, `ACCOUNT_LOGIN_METHODS = {username}`): `login` (CharField, strip, max 150,
Pflicht), `password` (Pflicht, ohne strip). Fehler je Feld „This field is required.“; dann
`clean`:
1. `DISABLE_REGULAR_LOGIN` → „Regular login is disabled“.
2. `login_failed`-Begrenzung (Schlüssel `"<get_host()>:<login.lower()>"`, SHA-256) → „Too many
   failed login attempts. Try again later.“
3. `authenticate` (ModelBackend, allauth-Backend; korrektes Passwort eines **inaktiven** Nutzers
   wird „gestasht“ → Weiterleitung `/accounts/account_inactive/`). Erfolg → Rollback des
   `login_failed`-Verbrauchs; sonst „The username and/or password you specified are not correct.“
   und Log `Login failed for user …`.

Erfolg: `record_authentication("password", username=<eingabe>)` → `Login(user,
redirect_url=next|None)` → Stufen `login_by_code`, `verify_phone`, `verify_email` (bei
`optional`/`none` ohne Signup erledigt; siehe „E-Mail-Bestätigung Pflicht“), `mfa_authenticate` (TOTP/WebAuthn vorhanden und nicht
per Social-`amr` vertraut → `stash_login` in `account_login`, 302 `/accounts/2fa/authenticate/`) →
`login()` → 302 Ziel, Message. Danach immer `set_expiry(...)` (siehe Einstellungen).
Fehlerseite: Status 200, `base.html` listet alle Fehler.

### MFA-Schritt (`/accounts/2fa/authenticate/`)

`login_stage_required`: angemeldet → 302 Ziel; kein/abgelaufener (`LOGIN_TIMEOUT` 900 s)
`account_login` oder anderer Schritt → 302 `/accounts/login/`; Nutzer ohne TOTP/WebAuthn → 302
Login. POST mit `code`: leer → Pflichtfehler; `login_failed`-Begrenzung (Schlüssel
`mfa-auth-user-<pk>`); alle Authenticators außer WebAuthn nach id: TOTP (HOTP-SHA1, 6 Stellen,
Zähler `now//30`, Toleranz 0, danach **Wiederverwendungssperre**
`cache.add("allauth.mfa.totp.used?user=<id>&code=<normalisiert>", "y", 30)` im gemeinsamen
Django-Cache), Recovery-Codes (Zeile `FOR UPDATE`, `used_mask`-Bit setzen, `migrated_codes`).
Erfolg: Begrenzung löschen, `last_used_at = now()`, `record_authentication("mfa", id, type)`,
Stufe erledigt → `login()` (allauth-Backend) → 302 Ziel. Fehler: „Incorrect code.“ (200).
„Abbrechen“ = POST `/accounts/logout/` mit `next=/accounts/login/`.

### Logout

GET = POST (`LOGOUT_ON_GET`). Ziel: sicheres `next`, sonst `LOGOUT_REDIRECT_URL`. Kein
`never_cache`. POST verlangt CSRF.

### Signup

`rate_limit("signup")` → `never_cache` → angemeldet → Ziel → `is_open_for_signup` (sonst
`CloseableSignupMixin.respond_closed` → `signup_closed.html`, 200, S3) → Formular `username` (Pflicht, max 150,
`^[\w.@+-]+\Z`, eindeutig case-insensitiv „A user with that username already exists.“), `email`
(optional; vergeben → Feldfehler, bei `mandatory` stattdessen `account_already_exists` →
`try_save` → `prevent_enumeration`, kein Konto), `password1` (Django-Validatoren mit Dummy-Nutzer),
`password2` („You must type the same password each time.“). Anlage: `is_superuser = is_staff =
FIRST_INSTALL`, `ACCOUNT_DEFAULT_GROUPS`, dann Login (`signup=True`) → 302 `next`/`/dashboard`.
Seit upstream-Fix 22746aa20 (#14688, nach v3.4.2, übernommen): `FIRST_INSTALL`-Prüfung, Anlage und
Standardgruppen in **einer** Transaktion mit `SELECT … FROM django_content_type WHERE app_label='auth'
AND model='user' FOR UPDATE` (gleiche Sperre wie Django) – gleichzeitige Erst-Registrierungen ergeben genau
einen Superuser (auch Social-Signup mit Formular). Test `crates/pl-accounts/tests/first_user_lock.rs`.

### OIDC-Login (`/accounts/oidc/<provider_id>/login/`)

Provider = `adapter.get_provider(request, provider_id)` (DB-Apps vor `SOCIALACCOUNT_PROVIDERS[...]["APPS"]`,
Filter `provider`/`provider_id`; mehrere → nur nicht-`hidden`, sonst 500; keine → **404**). Kein
`never_cache`, keine Ratenbegrenzung.

* **GET** (`LOGIN_ON_GET=False`): paperless `socialaccount/login.html` („You are about to connect a
  new third-party account from <name>.“, `REDIRECT_LOGIN_TO_SSO` → Auto-Submit-Skript), 200.
* **POST** (CSRF; jede andere Methode ebenso): Discovery `server_url` + `/.well-known/openid-configuration`
  (+ `openid_configuration_overrides`, Fehler → 500), `next` (sicher), `process` (POST, sonst GET,
  Std. `login`), Scope (`scope`/`SCOPE`, Std. `openid profile email`, + `?scope=a,b`), Auth-Params
  (`auth_params`/`AUTH_PARAMS` + `?auth_params=<urlencoded>`), PKCE (`oauth_pkce_enabled`/
  `OAUTH_PKCE_ENABLED`: `code_verifier = token_urlsafe(96)`, S256). State in der Sitzung
  `socialaccount_states = {<16 Zeichen>: [{"process", "data": null, "pkce_code_verifier", "next"?}, ts, provider_id]}`
  (abgelaufene > 600 s entfernt, max. 10). 302 auf
  `authorization_endpoint?client_id&redirect_uri&scope&response_type=code&state&<auth_params>&code_challenge_method&code_challenge`
  (`urlencode`/`quote_plus`), `redirect_uri` = absolute Callback-URL, bei
  `ACCOUNT_DEFAULT_HTTP_PROTOCOL=https` immer `https:`.

### OIDC-Callback (`…/login/callback/`)

1. Provider (404). `state` (POST, sonst GET) nur für denselben `provider_id` und ≤ 600 s; wird immer
   aus der Sitzung entfernt. Fehlt/abgelaufen/fremd → Log `Social authentication error for provider
   `<name>`: unknown (None)` + **401** paperless `authentication_error.html`.
2. `error` in GET oder kein `code` in GET: `access_denied` → 302 `/accounts/3rdparty/login/cancelled/`,
   sonst 401.
3. Token-Endpunkt (POST `redirect_uri, grant_type, code, client_id, client_secret[, code_verifier]`
   bzw. HTTP-Basic, wenn `token_auth_method=client_secret_basic` oder nur Basic angeboten), Userinfo
   (`fetch_userinfo` Std. True), ID-Token **ohne Signaturprüfung** (allauth: direkt vom
   Token-Endpunkt, `verify_signature = not did_fetch_access_token`), geprüft werden `iss`
   (= `issuer`), `aud` (enthält `client_id`), `exp` (falls vorhanden) wie PyJWT; `jti` → Sperre
   `jwt:iss=…,jti=…` im Django-Cache (Replay → 401). Keine `nonce` (allauth sendet keine). Fehler
   (`OAuth2Error`, `RequestException`) → 401; fehlende Discovery-Schlüssel, fehlende `sub` → 500.
4. `SocialLogin`: `uid = str(<userinfo|id_token>[uid_field|"sub"])`, `extra_data = {"userinfo", "id_token"}`,
   Nutzerfelder aus `preferred_username`, `email` (gültig, klein), `given_name`/`family_name` bzw.
   `name.partition(" ")`, Adressen (`email_verified`, `verified_email`/`VERIFIED_EMAIL`).
5. `pre_social_login`: `socialaccount_sociallogin` aus der Sitzung entfernen; Konto
   (`provider = provider_id`, `uid`) gefunden → `extra_data`, `last_login = now()` speichern,
   **Gruppen-Sync** (`handle_social_account_updated`, nur aktive Nutzer: Claim aus `extra_data`,
   `userinfo`, `id_token`; Gruppen ersetzt; Superuser/Staff-Gruppe). Sonst ggf. E-Mail-Authentifizierung
   (`email_authentication`/`EMAIL_AUTHENTICATION`, Passwort wird bei unbestätigter Adresse unbrauchbar).
6. `process=redirect` → `next` oder `/`; `process=connect` → angemeldet: Konto anlegen (Message „The
   third-party account has been connected.“) bzw. „… already connected to a different account.“
   (Fehler), Ziel `next` oder `/` (`reverse("base")`); anonym → `/`.
7. sonst: angemeldeter Nutzer wird abgemeldet (`flush`); bestehendes Konto → `record_authentication
   ("socialaccount", provider, uid)` → `perform_login` (inaktiv → `account_inactive`; MFA-Schritt wie
   beim Passwort-Login, `signal_kwargs = {"sociallogin": SocialLogin.serialize()}`; `amr`-Vertrauen
   siehe Einstellungen) → 302 `next`/`/dashboard`, Message „Successfully signed in as …“.
8. Neues Konto: `SOCIALACCOUNT_ALLOW_SIGNUPS` falsch → `account/signup_closed.html` (200).
   `AUTO_SIGNUP` und E-Mail eindeutig (`assess_unique_email`) → Nutzer anlegen (unbrauchbares
   Passwort, Benutzername: ungültig/vergeben → `generate_unique_username`), **kein** Erstinstallations-
   Superuser und **keine** `ACCOUNT_DEFAULT_GROUPS` (nur im Formularweg), Konto, `setup_user_email`
   (Sitzung `account_verified_email = null`), `SOCIAL_ACCOUNT_DEFAULT_GROUPS`, Gruppen-Sync,
   `complete_signup` (Bestätigungsmail bei `optional` + unbestätigter Adresse, Message „Confirmation
   email sent to …“). Sonst Sitzung `socialaccount_sociallogin = SocialLogin.serialize()` (Format von
   `serialize_instance`, DjangoJSONEncoder) → 302 `/accounts/3rdparty/signup/`.

Sitzungsformat (State, `socialaccount_sociallogin`, `account_login.signal_kwargs.sociallogin`) ist
byte-kompatibel; der Ablauf darf an jeder Stelle zwischen Rust und Django wechseln.

### E-Mail-Bestätigung Pflicht (`ACCOUNT_EMAIL_VERIFICATION=mandatory`, Lücke S1)

Quelle: allauth 65.20.1 `account/stages.py::EmailVerificationStage.handle`,
`LoginStageController.handle` (Antwort + `cont` → `stash_login`), `account/adapter.py::
respond_email_verification_sent` (→ `account_email_verification_sent`),
`account/internal/flows/email_verification.py` (`send_verification_email_at_login`,
`…_at_real_login`, `…_at_fake_login`, `get_address_for_login` mit `sync_email_address`),
`account/internal/flows/signup.py::prevent_enumeration`, `account/forms.py::BaseSignupForm.try_save`,
`socialaccount/internal/flows/signup.py::process_auto_signup_email`,
`account/adapter.py::send_account_already_exists_mail`, Vorlage
`account/email/account_already_exists_*`. Wert wie upstream: `none`, wenn kein SMTP konfiguriert
ist (`EMAIL_ENABLED`), sonst `PAPERLESS_ACCOUNT_EMAIL_VERIFICATION` (Standard `optional`).

* Stufe `verify_email` (Login per Passwort, OIDC/OAuth2, nach Registrierung): ohne bestätigte
  Adresse (`has_verified_email(user, login.email)`) → Bestätigungsmail an die erste Adresse
  (`-primary, -verified`; fehlt eine Zeile, wird `auth_user.email` als unbestätigte Adresse
  angelegt), Begrenzung `confirm_email` 1/180 s je Adresse (still), Meldung „gesendet“,
  `account_login` gemerkt (`stages.current = verify_email`), **302 `/accounts/confirm-email/`**.
  Kein Login. Nach dem Klick auf den Link (`/accounts/confirm-email/<key>/`, POST) ist die
  Adresse bestätigt; die nächste Anmeldung läuft normal (`LOGIN_ON_EMAIL_CONFIRMATION` aus).
* Nutzer ganz ohne Adresse: wie allauth nur die Hinweisseite (kein Login), dazu eine Warnung im
  Log (`paperless.auth`) mit dem Grund.
* Aufzählungsschutz (`assess_unique_email` → `None`, nur bei `mandatory`): Registrierung und
  Social-Signup-Formular mit vergebener Adresse sowie Social-Auto-Signup → `prevent_enumeration`:
  Mail „Konto existiert bereits“ mit Link zum Passwort-Reset an die Adresse, Meldung „gesendet“,
  `account_login` mit `user_pk = null`, 302 `/accounts/confirm-email/`. Kein Konto.
* `/accounts/confirm-email/` (GET/HEAD; sonst 405 `GET, HEAD, OPTIONS`): Hinweisseite
  „E-Mail-Adresse bestätigen“ im eigenen Layout, ohne CSRF-Feld.
* Mailfehler (kein SMTP erreichbar) nur ins Log (Known differences 16/17).

### OAuth2-Anbieter GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea (Lücke S2, `oauth2.rs`)

Quelle: allauth 65.20.1 `socialaccount/providers/{github,gitlab,google,microsoft,nextcloud,discord,gitea}/
{provider,views}.py`, `providers/oauth2/{provider,views,client,urls}.py`. Installiert über
`PAPERLESS_APPS` (`allauth.socialaccount.providers.<id>`), Apps aus der DB
(`socialaccount_socialapp`) oder `PAPERLESS_SOCIALACCOUNT_PROVIDERS` (`APPS`/`APP`), Einstellungen
je Provider (`SCOPE`, `AUTH_PARAMS`, `OAUTH_PKCE_ENABLED` Standard aus, `VERIFIED_EMAIL`,
`EMAIL_AUTHENTICATION`) bzw. je App (`scope`, `auth_params`, `oauth_pkce_enabled`,
`verified_email`, `email_authentication`).

| Provider | Name | Standard-Scope | authorize / token | Profil |
|---|---|---|---|---|
| `github` | GitHub | – | `<GITHUB_URL|https://github.com>/login/oauth/authorize`, `…/access_token` | `GET <api>/user`, `Authorization: token …`; uid `str(id)`, `login`, `name`, `email` |
| `gitlab` | GitLab | `read_user` | `<gitlab_url|GITLAB_URL|https://gitlab.com>/oauth/authorize`, `/oauth/token` | `GET /api/v4/user?access_token=…` (`_check_errors`) |
| `google` | Google | `profile` | `AUTHORIZE_URL`/`ACCESS_TOKEN_URL` (Standard Google) | ID-Token (ohne Signaturprüfung, `iss`/`aud`/`exp` + `jti`), sonst `IDENTITY_URL`; uid `sub`/`id`, Adresse mit `email_verified`/`verified_email` |
| `microsoft` | Microsoft | `User.Read` | `<login_url>/<tenant|TENANT|common>/oauth2/v2.0/authorize`, `/token` | `GET <graph_url>/v1.0/me?$select=…` (`_check_errors`); E-Mail `mail` oder `userPrincipalName` |
| `nextcloud` | NextCloud | `read` | `<server|SERVER>/apps/oauth2/authorize`, `/apps/oauth2/api/v1/token` | `GET <server>/ocs/v1.php/cloud/users/<user_id>?format=json` |
| `discord` | Discord | `email`, `identify` | `https://discord.com/api/oauth2/authorize`, `…/token` | `GET https://discord.com/api/users/@me`, Bearer (ohne `raise_for_status`); uid `str(id)`, `username` (auch Name), Adresse nur wenn `email` und `verified` wahr (dann bestätigt) |
| `gitea` | Gitea | – | `<GITEA_URL|https://gitea.com>/login/oauth/authorize`, `…/access_token` | `GET <web>/api/v1/user`, `Authorization: token …`; uid `str(id)`, `login`, `name`, `email` (geprüft gegen allauth 65.20.1) |

`QUERY_EMAIL` ist in paperless aus (`EMAIL_REQUIRED` falsch) → kein `user:email`/`email`-Scope und
keine GitHub-Adressliste. Ablauf danach wie OIDC (Zustand in `socialaccount_states`, Callback,
`SocialLogin`, Lookup/Verknüpfen/Registrierung, Gruppen-Sync, Stufen). Fehler beim Token/Profil
(`OAuth2Error`, HTTP-Fehler) → Fehlerseite 401; fehlende Pflichtfelder (KeyError) → 500 wie allauth.
Anmeldeseite und `/api/profile/social_account_providers/` listen alle unterstützten Apps (Name:
OIDC = App-Name, sonst Klassenname), nach Name sortiert, versteckte (`hidden`) nicht auf der
Anmeldeseite. Keycloak, Authentik, Authelia u. a. laufen über `openid_connect` (allauth 65 hat
keinen eigenen Keycloak-Provider).

### Social-Signup (`/accounts/3rdparty/signup/`)

Ohne `socialaccount_sociallogin` → 302 Login. `never_cache`. Angemeldet → **500** (upstream:
`reverse("socialaccount_connections")` existiert in paperless nicht). Offener MFA-Schritt → 302 MFA.
Geschlossen → `signup_closed.html`. Formular `username` (Pflicht, wie `clean_username`), `email`
(optional; vergeben → „An account already exists with this email address. Please sign in to that
account first, then connect your <Provider> account.“). Erfolg: `CustomAccountAdapter.save_user`
(Erstinstallation → Superuser, `ACCOUNT_DEFAULT_GROUPS`), dann wie Auto-Signup.

### E-Mail-Bestätigung (`/accounts/confirm-email/<key>/`)

GET: Schlüssel gültig (Signatur, ≤ 3 Tage, Adresse unbestätigt) → allauth-Seite „Please confirm that
<email> is an email address for user <name>.“ mit Formular (`next` als verstecktes Feld), bei
Konflikt (Adresse anderswo bestätigt) Hinweis ohne Formular; ungültig → „This email confirmation
link expired or is invalid…“ (immer 200). Ein anderer angemeldeter Nutzer wird abgemeldet. POST:
ungültig → 404; Konflikt → Message (Fehler), sonst `verified = true` (+ primär und `auth_user.email`,
falls noch keine primäre Adresse), Message „You have confirmed <email>.“; 302 `next` bzw.
angemeldet `/dashboard`, anonym `/accounts/login/`.

### Passwort zurücksetzen

* `PasswordResetView`: `email` (Pflicht, gültig, klein), Nutzer = `filter_users_by_email`
  (aktiv; bestätigte `account_emailaddress` bevorzugt, sonst `auth_user.email`), Begrenzung
  `reset_password` (IP + Schlüssel E-Mail) → 429; Mails (siehe unten), 302 `next` oder
  `/accounts/password/reset/done/`. Keine Nutzer-Aufzählung (immer gleiche Weiterleitung).
* Token (`EmailAwarePasswordResetTokenGenerator`): `<base36(ts)>-<hex(HMAC-SHA256(salt
  "django.contrib.auth.tokens.PasswordResetTokenGenerator", pk + password + last_login(UTC,
  ohne µs) + ts + email + "|".join(sorted(E-Mails)))[::2]>`, `ts` = Sekunden seit 2001-01-01 in
  lokaler Zeit (`TIME_ZONE`), gültig 3 Tage. Nebenwirkung: `sync_user_email_address` legt eine
  unbestätigte `account_emailaddress`-Zeile für `user.email` an, falls sie fehlt.
* Link: `PAPERLESS_URL` + `/accounts/password/reset/key/<uidb36>-<token>/`, sonst
  `ACCOUNT_DEFAULT_HTTP_PROTOCOL://host/…`.
* `PasswordResetFromKeyView`: `rate_limit("reset_password_from_key")`; gültiger Schlüssel im Pfad →
  Sitzung `_password_reset_key`, 302 auf `…/<uidb36>-set-password/` (+`next`); `set-password` →
  Schlüssel aus Sitzung; ungültig → Seite mit `token_fail` (200). Formular `password1`
  (Validatoren mit Zielnutzer), `password2` (gleich). Erfolg: Passwort setzen, `login_failed`-
  Zähler aller E-Mail-Adressen löschen, Message, 302 `next` oder `/accounts/password/reset/key/done/`.
  Kein automatischer Login (`LOGIN_ON_PASSWORD_RESET=False`), keine Benachrichtigungsmail
  (`EMAIL_NOTIFICATIONS=False`).
* Mails: `Subject: [<Produktname>] <auth.mail.reset_subject>` bzw. `… <auth.mail.unknown_subject>`,
  eigene Texte (Abschnitt „Eigene Konto-Seiten“), `From: DEFAULT_FROM_EMAIL`,
  `text/plain; charset="utf-8"`, 7bit/8bit. Empfänger, Auslöser, Link-Aufbau und Zeitpunkt wie upstream.

### `profile.social_account_providers` – GET `/api/profile/social_account_providers/` (W23, `src/api.rs`)

Quelle: src/paperless/views.py@v3.4.2 `SocialAccountProvidersView.get`, allauth 65.20.1
`DefaultSocialAccountAdapter.list_providers` (Registry-Klassen × `list_apps`), `OpenIDConnectProvider`
(`name = app.name`, `get_login_url` = `reverse("openid_connect_login", provider_id) + "?" +
urlencode(kwargs)`).

- DRF-View (`drf_view`, `Allow: GET, HEAD, OPTIONS`), `IsAuthenticated` (Token/Basic/Session wie
  pl-authz; anonym 401/403 wie DRF).
- Antwort 200: `[{"name": app.name, "login_url": "<BASE_URL>accounts/oidc/<provider_id>/login/?process=connect"}]`
  je `openid_connect`-App (DB-Apps zuerst, dann `SOCIALACCOUNT_PROVIDERS` inkl. `SERVERS`-Migration;
  versteckte Apps bleiben drin – upstream filtert hier nicht), stabil sortiert nach `name`
  (Codepunkte). Ohne installierten OIDC-Provider `[]`. Der `openid`-Zweig (OpenID 2, `get_brands`)
  entfällt: paperless installiert ihn nur über `PAPERLESS_SOCIAL_PROVIDERS`.
- Seit S2 (2026-11-18) alle unterstützten Provider (OIDC, GitHub, GitLab, Google, Microsoft,
  Nextcloud; Name OIDC = App-Name, sonst Klassenname, URL `/accounts/<id>/login/?process=connect`);
  unbekannte Provider-Apps fehlen in der Liste (Warnung im Log), statt die Route abzuschalten.
- Liegt hier statt in pl-auth-api, weil die Provider-Logik (`social::list_apps`) in pl-accounts liegt
  und pl-accounts von pl-auth-api abhängt (umgekehrt wäre ein Zyklus).
- Parität (W23, 2026-11-08, A = Referenz-Django :8000, B = PaperRust :8195 nur mit den W23-Routen, Rest Django; `tests/parity/pl-mail/w23b3env.sh`, Berichte `results/parity/w23/`): `tests/parity/pl-accounts/cases.w23.json` 18 Fälle (Token, Session, Basic, anonym,
  inaktiv, falscher Token; v9/v10): 18 identisch. Die Referenz hat keinen OIDC-Provider (`[]`);
  der Fall mit Apps ist nur durch den Code von `list_apps` (W12-Parität) abgedeckt.

## Sicherheit

* Brute-Force: `login_failed` 5 Fehlversuche / 300 s je `host:nutzername` und 10 / min je IP
  (Sperre `<key>:lock` wie allauth), Erfolg rollt nur den eigenen Verbrauch zurück; `login` 30
  POST/min je IP → 429; MFA-Codes ebenso begrenzt. Seit S4 (2026-11-18) liegen alle
  `allauth:rl:*`-Einträge **immer** in der gemeinsamen Tabelle `paperrust_cache` (auch beim
  prozesslokalen Standard-Cache; Tabelle wird bei Bedarf mit der pl-core-Migration angelegt, sonst
  lokal + Warnung) – alle Prozesse zählen gemeinsam wie mit Redis in ngx.
* Log fehlgeschlagener Anmeldungen mit Client-IP wie `handle_failed_login` (python-ipware mit
  `PAPERLESS_TRUSTED_PROXIES`, `pl_authz::clientip`, S7).
* Offene Weiterleitung: siehe `next`; geprüft auch für Logout, Signup, Reset.
* CSRF: jede POST-Route; Token aus Formular oder Header; Origin-Prüfung.
* Inaktive Nutzer: nie angemeldet (nur `account_inactive`), bestehende Sitzungen gelten als anonym.
* MFA-Umgehung: der Passwortschritt legt nur `account_login` ab (kein `_auth_user_id`);
  `/accounts/2fa/authenticate/` prüft Schritt, Zeitlimit und Nutzer.
* Sitzungsfixierung: Login rotiert den Sitzungsschlüssel und den CSRF-Token; unbekannte
  Sitzungsschlüssel aus dem Cookie werden nie übernommen.

## Known differences

- **Desktop-Betrieb (PaperRust-Erweiterung, kein upstream-Gegenstück):** `GET /rs/desktop/login` meldet den lokalen Eigentümer ohne Passwort und ohne Zwei-Faktor an, wenn der Desktop-Supervisor den Schlüssel gesetzt hat und die Verbindung von Loopback kommt. Die Anmeldeseite zeigt dann (nur für Loopback) den Hinweis `auth.login.desktop_hint`. Ohne die Supervisor-Variablen ist beides aus (Server-Betrieb unverändert).

> **Seit 2026-11-14 ohne Django** (überarbeitet 2026-11-20): Headless-State im OAuth-Callback → dieselbe
> Login-Fehlerseite wie bei unbekanntem State (`auth_error(…, "unknown")`; PaperRust legt nie einen
> Headless-State an, nur eine alte Django-Sitzung kann einen enthalten); Bestätigungsschlüssel außerhalb
> `[-:\w]+` → Djangos 404-Seite (upstream: kein URL-Treffer, Catch-all-Index); `social_account_providers`
> mit einem Provider ohne Klasse (seit S2 nicht mehr möglich) → 500. Eine Weiterleitung an Django gibt es
> nicht mehr. Seit 2026-11-18 (Lücken S1–S3) sind
> `ACCOUNT_EMAIL_VERIFICATION=mandatory`, geschlossene Registrierung, Signup mit E-Mail und die
> Provider GitHub/GitLab/Google/Microsoft/Nextcloud (später auch Discord/Gitea) nachgebaut; übrige Provider-Apps werden übergangen.
> Rate-Limits (`allauth:rl:*`) liegen immer gemeinsam in PostgreSQL (S4), andere Einträge in Djangos
> `default`-Cache über `pl_core::cache` (prozesslokal oder mit `PAPERRUST_CACHE=postgres` gemeinsam).
> Die Paritätsläufe unten (gemeinsamer Cache mit Django) sind Geschichte.


1. **Social-Provider außer** `openid_connect`, `github`, `gitlab`, `google`, `microsoft`,
   `nextcloud`, `discord`, `gitea` (S2) werden nicht nachgebaut (bewusst: allauth hat über 100
   Anbieter, viele mit eigenem Protokoll – OAuth1 bei Twitter, Apple mit signiertem Client-Secret,
   Facebook mit `appsecret_proof`/Graph-Versionen; Keycloak, Authentik, Authelia, Zitadel, Okta,
   Slack, LinkedIn u. a. sprechen OpenID Connect und laufen über `openid_connect`): aus `PAPERLESS_APPS` übergangen (kein Knopf, Pfad 404,
   fehlt in `/api/profile/social_account_providers/`), einmal Warnung im Log. Django zeigte sie an.
   Passwort-Login und die übrigen Anbieter bleiben nutzbar (vorher fielen alle Kontoseiten aus).
   **Headless-Abläufe** (`state["headless"]`) → Login-Fehlerseite („unknown“).
2. **`ACCOUNT_EMAIL_VERIFICATION=mandatory`** (S1, bewusst): paperless 3.4.2 bindet
   `account_email_verification_sent` (`/accounts/confirm-email/`) **nicht** ein (src/paperless/urls.py
   übernimmt nur ausgewählte allauth-Muster); `respond_email_verification_sent` endet dort mit
   `NoReverseMatch` → **500** (Login ohne bestätigte Adresse, Aufzählungsschutz) – Pflicht-Bestätigung
   ist in ngx praktisch kaputt. PaperRust verhält sich wie allauth gedacht: Hinweisseite
   `/accounts/confirm-email/` (302 dorthin), Mail, Bestätigungslink. Damit sperrt der Wert
   niemanden mehr durch einen Fehler aus; wer keine Adresse hat, sieht die Hinweisseite (Log-Warnung).
3. **Übersetzungen**: Feldlängenfehler („Ensure this value has at most 150 characters …“) und die
   Meldungen der Passwort-Validatoren (aus `pl_auth_api::password`) sind nur Englisch und kommen
   nicht aus dem eigenen Katalog (gehören pl-auth-api).
4. **Cache**: Rust schreibt keine Pickle-Kopie der Sitzung in den Cache, sondern löscht den
   `cached_db`-Eintrag nach jedem Schreiben/Löschen (Django lädt dann aus der DB). Lesen nur
   aus der DB.
5. **Messages**: nur die Cookie-Stufe von `FallbackStorage` (Sitzungs-Überlauf > 2048 Byte nicht).
6. **Mail**: kein `EMAIL_CERTIFICATE_FILE`; AUTH PLAIN/LOGIN (kein CRAM-MD5); `Message-ID`-Format
   abweichend; Fehler beim Versand → 500.
7. **Login-Fehlerlog** mit IP wie ipware (S7, `pl_authz::clientip`); nur die META-Schlüssel, die
   Django tatsächlich hat (`HTTP_*` der ipware-Liste, `REMOTE_ADDR`), IPv6 ohne Zonen-Index.
8. **Remote-User-Middleware/Auto-Login** wirken nur auf den Rust-Kontoseiten selbst (andere
   Nicht-API-Seiten laufen über Django bzw. die SPA).
9. allauth-Layout-Seiten (`signup_closed.html`, `socialaccount/login_cancelled.html`,
   `account/email_confirm.html`) nutzen das eigene Layout (ohne allauth-Menü); sie erzeugen wie
   upstream kein CSRF-Feld/-Cookie, außer im Bestätigungsformular.
10. **ID-Token-Signatur** wird wie in allauth im Code-Flow nicht geprüft (kein JWKS-Abruf); eine
    abweichende Signatur wird von beiden Seiten akzeptiert (Paritätsfall `ok.badsig_accepted`).
11. **Scope-Reihenfolge** in der IdP-URL: allauth nutzt ein Python-`set` (Reihenfolge je Prozess
    zufällig), Rust die Konfigurationsreihenfolge ohne Duplikate.
12. **messages-Cookie**: komprimierte Werte unterscheiden sich in den Bytes (zlib von Python vs.
    miniz), Inhalt gleich und gegenseitig lesbar.
13. `generate_unique_username`: NFKD→ASCII nur für gängige lateinische Zeichen (Tabelle), Zufalls-
    suffixe wie allauth (nicht reproduzierbar).
14. Benachrichtigungsmails (`EMAIL_NOTIFICATIONS`) und Signale ohne paperless-Empfänger
    (`social_account_added`, `user_signed_up`, `email_confirmed` …) entfallen wie upstream wirkungslos.

15. **Eigene Seiten, Texte und Mails** (seit 2026-11-14, bewusst): siehe „Eigene Konto-Seiten“ –
    Wortlaut, Gestaltung, Seitentitel, Mailtexte und Meldungstexte weichen absichtlich von
    upstream ab. Gleich sind URLs, Methoden, Feldnamen, Statuscodes, Weiterleitungen, Cookies,
    Meldungsstufen, Mail-Auslöser/-Empfänger und alle Sicherheitsprüfungen.
16. **Passwort vergessen ohne funktionierendes SMTP** (A-011/B-313, 2026-11-16): Scheitert der
    Mailversand (kein SMTP konfiguriert, Server nicht erreichbar, Ablehnung), leitet Rust trotzdem
    auf „E-Mail gesendet“ weiter (wie bei Erfolg) und schreibt nur eine Warnung ins Log
    (`paperless.auth`, Host/Port und kurzer Fehlertext, ohne Empfänger, Link oder Passwort).
    upstream lässt die SMTP-Exception durch → 500. So verrät die Seite weder, ob die Adresse
    existiert, noch ob die Mail rausging.
17. **Bestätigungs-Mail für E-Mail-Adressen ohne funktionierendes SMTP** (2026-11-17,
    `emailaddr.rs` `send_verification_to_address`): Scheitert der Versand, schreibt Rust nur eine
    Warnung ins Log (wie 16.), zeigt keine „gesendet“-Meldung und liefert kein 500; upstream lässt
    die Exception durch → 500. Bei der Mail „Konto existiert bereits“ (Aufzählungsschutz) erscheint
    die „gesendet“-Meldung trotzdem, damit nichts über die Adresse verraten wird.
18. **Rate-Limit-Speicher** (S4): `allauth:rl:*`, `throttle_*` und die TOTP-Wiederholsperre
    (`allauth.mfa.totp.used*`) immer in `paperrust_cache` (PostgreSQL), auch beim prozesslokalen
    Cache – bewusst, damit getrennte HTTP-/Worker-Prozesse gemeinsam drosseln. Umgesetzt in
    `pl_core::cache` für alle Schutz-Schlüssel (`is_guarded_key`); fehlt die Tabelle und lässt sie
    sich nicht anlegen, bleiben sie mit Warnung im Prozessspeicher.
19. **OAuth2-Anbieter** (S2): gegen echte GitHub/Google/Microsoft/GitLab/Nextcloud/Discord/Gitea-Server nicht
    geprüft (keine Fremddienste in Tests); geprüft gegen einen lokalen Schein-GitHub
    (`GITHUB_URL`) und mit Unit-Tests der Feldzuordnung. Google-`LoginByTokenView`
    (`/accounts/google/login/token/`, One-Tap) ist nicht nachgebaut (paperless nutzt ihn nicht).
20. **`email_verified` nur bei echtem `true`** (Sicherheitsprüfung 2026-11-19, bewusst): Die
    Bestätigungs-Angabe des IdP (OIDC `email_verified`, Google `email_verified`/`verified_email`,
    Discord `verified`) gilt nur als bestätigt, wenn sie JSON `true` oder der Text `"true"`
    (Groß-/Kleinschreibung egal) ist (`emailaddr::is_true_claim`, auch in `EmailAddr::is_verified`).
    upstream/allauth wertet Python-Wahrheit (`bool(value)`): ein IdP, der `"false"` als Text
    liefert, ergäbe dort eine bestätigte Adresse (und damit z. B. automatisches Verknüpfen mit einem
    bestehenden Konto). Die OIDC-Adresse wird als bool gespeichert. Tests:
    `emailaddr::tests::only_real_true_is_verified`,
    `socialaccount::tests::oidc_email_verified_string_false_is_unverified`.
21. **Geräte-Token nach Passwort-Reset widerrufen** (PaperRust-Erweiterung, Sicherheitsprüfung
    2026-11-19 F13): `set_password_form` (`/accounts/password/reset/key/…`) widerruft nach dem
    Speichern des neuen Passworts alle Geräte-Token (`prt_…`, `paperrust_device_token`) des
    Nutzers – seit 2026-11-27 in derselben Transaktion wie das Passwort-`UPDATE`
    (`pl_auth_api::db::set_password_and_revoke`; vorher zwei getrennte Zugriffe, ein Fehler im zweiten
    ließ die Token trotz neuem Passwort gültig). Der DRF-Token (`authtoken_token`) bleibt wie
    upstream bestehen.
22. **Fehlertexte von OAuth2-Abrufen ohne URL** (Sicherheitsprüfung 2026-11-19 F14): `oauth2.rs`
    `get` entfernt die URL aus reqwest-Fehlern (`without_url`), weil der GitLab-Profilabruf das
    Zugangstoken wie allauth als `?access_token=` mitschickt und der Fehlertext über `auth_error` ins
    Log geht. Der Abruf selbst bleibt wie upstream (Query-Parameter). Test:
    `oauth2::tests::request_error_text_has_no_access_token`.

## Eigene Konto-Seiten (seit 2026-11-14)

Keine Vorlage und kein Text stammt mehr aus paperless-ngx oder django-allauth. Vorlagen:
`crates/pl-accounts/templates/*.html` (minijinja, `src/tpl.rs`), Katalog:
`crates/pl-accounts/i18n/<locale>.json`.

**Design**: ein Layout (`layout.html`) mit kleinem eingebettetem Stylesheet im Stil der Light-UI
(eckig, 2 px Rundung, Akzentlinie oben an der Karte, hell/dunkel über `prefers-color-scheme`).
Akzentfarbe: `var(--pngx-primary)`/`--pngx-primary-hover`, die pl-server bei Branding einsetzt
(`branding.rs::css_accounts`), sonst Rost `#B7410E`. Kopf: `APP_LOGO` (Bild) bzw. `APP_TITLE`
oder Produktname als Text mit kleiner Eselsohr-Marke – Werte aus `ApplicationConfiguration` bzw.
`PAPERLESS_APP_TITLE/APP_LOGO` (Branding füllt PaperRust-Logo/-Titel); beim eingebauten Logo
`/__rs/brand/logo.svg` im Dunkelmodus `logo-inverse.svg` (`<picture>`, Kontext `APP_LOGO_DARK`). Fußzeile immer mit dem
Lizenzhinweis `auth.footer.license` („based on paperless-ngx, GPL-3.0“, übersetzt; einzige
Nennung von paperless-ngx). Seitentitel `<Seitenname> · <Produktname>`; Produktname =
`pl_core::Config::product_name`, sonst „PaperRust“ (auch bei `PAPERRUST_BRANDING=off`).
`<html lang>` = Katalogsprache. Externe Ressourcen: keine (kein `/static/`-CSS mehr).

**Sprachen**: en-US, de-DE, fr-FR, it-IT, es-ES, nl-NL, pt-BR, pl-PL, cs-CZ, sv-SE (wie die UI).
Zuordnung über die Hauptsprache des Django-Sprachcodes (`pt-pt` → pt-BR, `en-gb` → en-US), alle
anderen Sprachen → Englisch. Gleiche Schlüsselform (`auth.<bereich>.<name>`) und Platzhalter
(`{$NAME}`) wie `frontend-light/i18n`, aber eigener Katalog (die UI soll keine Anmelde-Texte
bündeln). Prüfung: `i18n::tests::catalog_complete` (gleiche Schlüssel und Platzhalter in allen
Sprachen, kein HTML), `tpl::tests::no_unused_catalog_keys`, `…::catalog_has_no_upstream_names`,
`…::templates_have_no_upstream_names`.

**Inventar** (Schnittstelle unverändert):

| Seite (Vorlage) | URL | Status | Formular (Methode → Ziel): Felder | Besonderheiten |
|---|---|---|---|---|
| Anmelden (`login.html`) | `/accounts/login/` | 200 | POST → gleiche URL: `csrfmiddlewaretoken`, `login`, `password` | `FIRST_INSTALL` → Skript-Weiterleitung auf Signup; `DISABLE_REGULAR_LOGIN` blendet Formular/Links aus; OIDC-Anbieter als POST-Formulare (erstes `id="social-login"`), bei `REDIRECT_LOGIN_TO_SSO` und nicht `?loggedout=1` automatisch abgeschickt; Links „Passwort vergessen“ (nur `EMAIL_ENABLED`) und Signup (nur `ACCOUNT_ALLOW_SIGNUPS`, mit `next`); `?sharelink_notfound` / `?sharelink_expired` → Hinweis |
| Registrieren (`signup.html`) | `/accounts/signup/` | 200 | POST: `username`, `email`, `password1`, `password2`, `next` (falls gesetzt) | erstes Konto: Hinweis auf Admin-Rechte, keine Anbieter/kein Login-Link |
| Registrierung geschlossen (`signup_closed.html`) | `/accounts/signup/`, Social-Signup | 200 | – | kein CSRF |
| Konto deaktiviert (`account_inactive.html`) | `/accounts/account_inactive/` | 200 | – | |
| Zweistufige Anmeldung (`mfa_authenticate.html`) | `/accounts/2fa/authenticate/` | 200 | POST: `code`; Abbrechen = POST → `/accounts/logout/` mit `next=/accounts/login/` | TOTP oder Wiederherstellungscode |
| Passwort zurücksetzen (`password_reset.html`) | `/accounts/password/reset/` | 200 / 429 | POST: `email` | Fehler „Pflichtfeld“/„ungültige Adresse“ |
| Link verschickt (`password_reset_done.html`) | `/accounts/password/reset/done/` | 200 | – | Text verrät nicht, ob das Konto existiert |
| Neues Passwort (`password_reset_from_key.html`) | `/accounts/password/reset/key/<uidb36>-set-password/` | 200 | POST: `password1`, `password2` | `token_fail` → Hinweis + Link zur neuen Anfrage |
| Passwort geändert (`password_reset_from_key_done.html`) | `/accounts/password/reset/key/done/` | 200 | – | |
| Externe Anmeldung bestätigen (`social_login.html`) | `/accounts/oidc/<id>/login/` (GET) | 200 | POST → gleiche URL | bei `REDIRECT_LOGIN_TO_SSO` automatisch abgeschickt |
| Externe Anmeldung fehlgeschlagen (`social_authentication_error.html`) | OIDC-Callback, `/accounts/3rdparty/login/error/` | 401 | – | |
| Anmeldung abgebrochen (`social_login_cancelled.html`) | `/accounts/3rdparty/login/cancelled/` | 200 | – | kein CSRF |
| Social-Signup (`social_signup.html`) | `/accounts/3rdparty/signup/` | 200 | POST: `username`, `email` | Werte vorbefüllt |
| E-Mail bestätigen (`email_confirm.html`) | `/accounts/confirm-email/<key>/` | 200 | POST → `/accounts/confirm-email/<key>/`: CSRF, `next` (falls gesetzt) | „bereits von anderem Konto bestätigt“ ohne Formular; ungültiger Schlüssel → Hinweis |
| CSRF-Fehler (`csrf_403.html`) | jede POST-Konto-Seite | 403 | – | Gründe „kein Referer“/„kein Cookie“ |

Unverändert und nicht aus den Vorlagen: `ratelimit::HTML_429` (Djangos Standard-429-Text) und die
500/403/404-Kurzseiten in `web.rs` (Djangos Standardtexte, keine paperless-/allauth-Vorlagen).

**Fehler und Meldungen** (Schlüssel → Anlass): `auth.error.required` (leeres Pflichtfeld),
`.email_invalid`, `.username_invalid` (nicht `[\w.@+-]`), `.username_taken`, `.email_taken`,
`.password_mismatch`, `.too_many` (`login_failed`-Sperre und MFA-Begrenzung), `.wrong_login`,
`.wrong_code`, `.external_email_taken`; Meldungen `auth.msg.signed_in`, `.signed_out`,
`.password_changed`, `.confirm_sent`, `.confirmed`, `.external_linked`, `.external_taken`,
`auth.confirm.taken`.

**Mails** (gleiche Auslöser, Empfänger, Links; Rahmen `auth.mail.greeting` … `auth.mail.signoff`
mit Produktname):

| Mail | Betreff | Inhalt |
|---|---|---|
| Passwort zurücksetzen | `[<Produkt>] auth.mail.reset_subject` | `reset_text`, Link, `reset_username` (falls Benutzername), `ignore` |
| Unbekannte Adresse (`ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS`) | `[<Produkt>] auth.mail.unknown_subject` | `unknown_text`, `ignore`, `unknown_signup`, Signup-Link |
| Adresse bestätigen | `[<Produkt>] auth.mail.confirm_subject` | `confirm_text` (Benutzer, Host), Link, `ignore` |

Mailsprache = Sprache der auslösenden Anfrage (wie upstream).

**Rauchtest** (2026-11-14, Browser-Fenster gegen Wegwerf-Server `server.sh` mit eigener DB
`pl_authpages_tmp`, Port 8347, `F0_BRANDING=on`, lokale SMTP-Senke): Anmeldeseite, falsches
Passwort (Fehlertext), richtige Anmeldung (→ `/rs/app/`, API 200), Abmelden (Meldung, `?loggedout=1`),
TOTP über `/api/profile/totp/` eingeschaltet → MFA-Seite, falscher Code (Fehlertext), richtiger
Code (→ App), „Abbrechen“ im MFA-Schritt (→ Login, abgemeldet), Passwort-Reset auf Deutsch
(Seite, Mail mit Betreff `[PaperRust] Passwort zurücksetzen`, UTF-8/8bit, Link), Link → Formular,
ungleiche Passwörter (Fehlertext), Erfolg, Wiederverwendung des Links (ungültig), Anmeldung mit
neuem Passwort, POST ohne CSRF-Token → eigene 403-Seite. Nicht im Browser geprüft: OIDC-Seiten,
Signup, E-Mail-Bestätigung, Hellmodus (nur Dunkelmodus des Fensters) – diese sind durch die
Unit-Tests (Rendern jeder Vorlage in allen Sprachen) abgedeckt, aber nicht durchgeklickt.

## Tests

* Unit (`cargo test -p pl-accounts -j 1`): Ratenbegrenzung (Fenster, Schlüssel wie allauth, IPs),
  sichere URLs (Django-Fälle), Messages-Cookie byte-gleich zu einem Django-Cookie, Login-Zustand
  (`account_login`) roundtrip-gleich zu allauth, base36, Mail-Format; eigene Vorlagen (jede Vorlage
  rendert in allen 10 Sprachen ohne unübersetzten Schlüssel, Feldnamen/Formularziele/Skripte des
  Logins, Escaping eingesetzter Werte, kein „paperless“/„allauth“ außer Lizenzhinweis), Katalog
  vollständig und ohne ungenutzte Schlüssel.
* Paritätsläufe vergleichen seit den eigenen Vorlagen die **Schnittstelle** einer Seite
  (`tests/common/mod.rs::page_skeleton`: Formulare, Feldnamen, Fehler-/Meldungsstufen,
  Skript-Weiterleitungen) statt des HTML-Wortlauts, beim messages-Cookie nur Stufen, bei Mails die
  Kopfzeilen ohne Betreff. Die Läufe unten stammen von vorher (byte-gleiches HTML).
* Parität Social/OIDC/E-Mail-Bestätigung (`tests/social_parity.rs`, `#[ignore]`): W12-Instanz, siehe
  „Paritätslauf Social/OIDC“. Variante d (`mandatory`) prüft seit S1, dass der Callback nicht mehr
  delegiert wird und dasselbe Ergebnis wie Django liefert (2026-11-18 nicht gelaufen: W12-Django
  nicht gestartet).
* Lücken S1–S4 (`tests/gaps_s1_s4.rs`, `#[ignore]`, eigene Wegwerf-DB auf der Referenz-PostgreSQL,
  Schema aus `reference/schema/postgres.sql`; 2026-11-18 grün): Login mit `mandatory` ohne bestätigte
  Adresse → 302 `/accounts/confirm-email/`, Adresse angelegt, Seite 200, nach Bestätigung Login →
  `/dashboard`; Registrierung mit vergebener Adresse → Aufzählungsschutz ohne Konto; Anmeldeseite mit
  GitHub-Knopf trotz unbekanntem `facebook`, `/accounts/facebook/login/` 404, GitHub-Start mit
  korrekter Weiterleitung, vollständiger GitHub-Ablauf gegen einen lokalen Schein-Dienst
  (Token als Formularantwort, Profil, Auto-Signup, `socialaccount_socialaccount`); geschlossene
  Registrierung → Hinweisseite; Rate-Limit-Zeilen in `paperrust_cache`.
* Unit: `oauth2::tests` (Feldzuordnung je Provider, Scopes, Fehlerprüfung GitLab/Microsoft),
  `social::tests::provider_classes_from_apps`, `tests::routes_mount_without_conflict` (axum-Router
  mit `/accounts/{provider}/login/` neben festen Pfaden und OIDC).
* Regression W21 (`tests/parity.rs::login_with_rehash_keeps_session`, `#[ignore]`, W3-Instanz):
  Login mit Passwort-Rehash (`must_update`, z. B. Salz < 128 Bit wie im Referenz-Snapshot) muss
  `_auth_user_hash` aus dem **neuen** Hash bilden (Django: Setter ändert `user.password` vor
  `login()`); vorher verwarf die nächste Anfrage die Sitzung (flush → API 401).
* Parität (`tests/parity.rs`, `#[ignore]`): eigene Django-Instanz
  (`tests/parity/pl-accounts/w3-django.sh start`, DB `pl_w3_ref` aus `paperless_snapshot`, Port
  8120, SMTP-Senke 8125, Fake-IdP 8126, kompilierte paperless-Kataloge über `w3_settings.py`).
  Stand siehe Abschnitt „Paritätslauf“.

## Paritätslauf (2026-11-08, W3-Django :8120 vs. Rust in-process, gleiche DB/Cache)

`cargo test -p pl-accounts -j 1 --test parity -- --ignored --test-threads=1`: **77 Prüfungen, 0 Abweichungen**
(333 s; PBKDF2 im Debug-Build langsam, siehe CORE-REQUEST im Bericht).

* GET-Seiten byte-gleich (CSRF-Token normalisiert): Login (en, de per Accept-Language, fr per
  Cookie, `next`, unsicheres `next`, `sharelink_notfound`, `loggedout`, ohne CSRF-Cookie),
  `/api/auth/login/`, Signup (+`next`), Konto inaktiv, Reset (+de), Reset-Done, Key-Done,
  ungültige Reset-Links, MFA ohne Schritt, Logout anonym/`next`/`//evil`.
* POST Login: falsches Passwort, unbekannt, leer, zu lang, inaktiv, Erfolg, Groß-/Kleinschreibung,
  MFA-Weiterleitung, `next` sicher/unsicher/`///` – Status, Location, Cookies, Markierung, HTML.
* CSRF: ohne Cookie, ohne Token, falscher Token, fremde Origin, eigene Origin.
* Sitzungen: Rust-Login → Django-API akzeptiert; Rust-Logout → Django lehnt ab; Messages von Rust
  auf Django-Seite gleich; Django-Login → Rust erkennt Nutzer; Rust-Logout (POST) beendet
  Django-Sitzung.
* MFA: Passwortschritt allein meldet nicht an (Umgehung), Seite/Fehler gleich, Rust-TOTP-Login →
  Django akzeptiert Sitzung, derselbe Code danach bei Django gesperrt (gemeinsamer Cache),
  Recovery-Code über Rust auf Django-Zwischenstand, `used_mask` gesetzt.
* Brute-Force: 5 Fehlversuche in Rust sperren auch Django (und Rust) für den Nutzer; 429 nach
  30 Logins/min (Body gleich).
* Reset: unbekannt/ungültig/leer gleich; Mails byte-gleich bis auf Date/Message-ID/Token; von Rust
  erzeugter Link in Django gültig, Django-Link in Rust gültig; Formular/Fehler gleich; neues
  Passwort in Django nutzbar; Link danach ungültig.
* Signup: leer, vergeben (case-insensitiv), ungleich, schwach, ungültiger Name gleich; Rust-Signup
  meldet an, Django akzeptiert die Sitzung.

## Paritätslauf Social/OIDC (W12, 2026-11-08, Django :8140 vs. Rust in-process, DB `pl_w12_ref`)

Eigene Instanz `tests/parity/pl-accounts/w12-django.sh start <a|b|c|d>` (Fake-IdP + SMTP-Senke
`w12_sinks.py` auf 8141/8142, Redis-Präfix `w12`), Lauf
`cargo test -p pl-accounts -j 1 --test social_parity social_parity_<v> -- --ignored --test-threads=1`:

| Variante | Einstellungen | Fälle | Abweichungen |
|---|---|---|---|
| a | Signup-Formular (`AUTO_SIGNUP` aus) | 68 | 0 |
| b | Auto-Signup, Gruppen-Sync, Standardgruppen, Superuser-Gruppe, `REDIRECT_LOGIN_TO_SSO`, `MFA_TRUSTED_AMR=pwd` | 12 | 0 |
| c | Social-Signup geschlossen, `DISABLE_REGULAR_LOGIN`, `ACCOUNT_SESSION_REMEMBER=false` | 4 | 0 |
| d | `ACCOUNT_EMAIL_VERIFICATION=mandatory` (Delegation) | 2 | 0 |

Jeder Fall läuft komplett über Django und über Rust (Status, Location mit normalisiertem
state/code_challenge/Scope, Cookies, HTML, Authentifiziert-Markierung, DB-Zeilen `auth_user`/Gruppen/
`socialaccount_socialaccount`/`account_emailaddress`/`socialtoken`, Mails). Abgedeckt: Zwischenseite
(en/de), 404, Signup-Formular inkl. Fehler (leer, vergeben, ungültig, E-Mail vergeben/ungültig),
Erfolg mit/ohne E-Mail, Wiederkehr, bestehendes Konto (+ `next`), inaktiv, Nutzerwechsel, MFA nötig/
`amr` vertraut, Fehler `aud`/`iss`/`exp`/Token/Userinfo/`access_denied`/`server_error`/`jti`-Replay/
State wiederverwendet/abgelaufen/falsch, Signatur egal, `aud`-Liste, ohne ID-Token, ohne E-Mail,
bestätigte E-Mail, `process=connect`, Auto-Signup mit Gruppen-Sync/Superuser/Standardgruppen/
generiertem Benutzernamen/vergebener E-Mail, Sync bestehender Konten, `signup_closed`,
Browser-Ende-Sitzung, POST ohne CSRF, Signup mit E-Mail + Bestätigungsseiten (anonym, angemeldet,
anderer Nutzer, Konflikt, erneut), Wechsel Rust↔Django: Start/Callback/Signup-Formular/MFA-Code
jeweils über Kreuz, Profil-`disconnect_social_account` (Django) → nächster Rust-Login ist wieder
Signup.

Nicht geprüft: Remote-User/Auto-Login gegen Django, `DISABLE_REGULAR_LOGIN`/`REDIRECT_LOGIN_TO_SSO`,
`PAPERLESS_ACCOUNT_SESSION_REMEMBER=false`, SMTP mit TLS/AUTH, `FORCE_SCRIPT_NAME`, Cookie-Präfix.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **a139bdff4 (#14722) – abgelaufene Sitzungen löschen.** Quelle: `documents/tasks.py::clear_expired_sessions`,
  `paperless/settings/custom.py::parse_beat_schedule`. Rust: `session::clear_expired_sessions(db, now)`
  löscht `django_session`-Zeilen mit `expire_date < now` in Durchgängen zu 500 (älteste zuerst, ein
  `DELETE … WHERE session_key IN (SELECT … LIMIT 500)` je Durchgang), Log „Cleared expired sessions“.
  Aufgerufen vom Worker-Task `documents.tasks.clear_expired_sessions` (pl-server, Beat „Clear expired
  sessions“, `PAPERLESS_SESSION_CLEANUP_TASK_CRON`, Standard `0 3 * * *`, pl-queue). Die `cached_db`-
  Cache-Kopien bleiben wie upstream liegen (laufen selbst ab). Test: `tests/clear_sessions.rs`
  (eigene Wegwerf-DB, `#[ignore]` – braucht die Referenz-PostgreSQL).

- **django-allauth 65.21.1 (Sicherheitskorrektur) – Zeitfenster im Ratenlimit-Schlüssel** (2026-11-15, eigene
  Umsetzung). `ratelimit::cache_key` hängt das Fenster der Rate an: `allauth:rl:allauth.account:<aktion>:<fenster>:<quelle>`
  (`<fenster>` = Sekunden, z. B. `60s`, `300s`; Bruchteile wie `0.5s`). Vorher teilten sich zwei Raten für
  dieselbe Aktion und Quelle mit verschiedenen Fenstern (z. B. `5/m/ip,20/h/ip`) einen Zähler, und das kurze
  Fenster schnitt beim Schreiben die Historie des langen ab. **Folge für geteilte Schlüssel:** Das Format
  weicht jetzt von allauth 65.20.1 ab; Django und Rust würden nicht mehr gemeinsam zählen. Das spielt keine
  Rolle mehr, weil Django seit 2026-11-14 nicht mitläuft (Cache nur noch prozesslokal oder `PAPERRUST_CACHE=postgres`,
  nur von PaperRust gelesen). Beim Update laufende alte Zähler (alte Schlüssel) verfallen nach ihrem Fenster von
  selbst; ein Limit startet nach dem Update einmal bei null. Der Präfix `allauth:rl:` bleibt (geschützter Bereich
  in `pl_core::cache::GUARDED_KEY_PREFIXES`). Tests: `ratelimit::tests::keys_like_allauth`,
  `ratelimit::tests::windows_count_separately`; `tests/parity.rs` (historisch, `#[ignore]`) belegt die Rust-Seite
  mit dem neuen Schlüssel vor.
