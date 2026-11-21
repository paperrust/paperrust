# Vertrag `pl-auth-api`

Quelle: paperless-ngx v3.4.2 (`upstream/`), Django 5.2.18, DRF 3.18.3, django-allauth 65.20.1
(`[mfa,socialaccount]`), qrcode 8.2 (+ lxml 6.1.0, wird von pikepdf mitinstalliert → qrcode
serialisiert mit `lxml.etree`), django-filter 26.2. Alle Pfade unter `upstream/src/`.

| Datei | Inhalt |
|---|---|
| `paperless/views.py` | `PaperlessObtainAuthTokenView`, `StandardPagination`, `UserViewSet` (+ `deactivate_totp`), `GroupViewSet`, `ProfileView`, `TOTPView`, `GenerateAuthTokenView`, `DisconnectSocialAccountView`, `SocialAccountProvidersView` |
| `paperless/serialisers.py` | `PasswordValidationMixin`, `PaperlessAuthTokenSerializer`, `UserSerializer`, `GroupSerializer`, `SocialAccountSerializer`, `ProfileSerializer` |
| `paperless/filters.py` | `UserFilterSet`, `GroupFilterSet` (`CHAR_KWARGS` aus `documents/filters.py`) |
| `paperless/urls.py` | Routing (`re_path("^ui_settings/")`, `path("token/")`, `re_path("^profile/")` + Unterpfade, Router `users`/`groups`) |
| `paperless/auth.py` | `PaperlessBasicAuthentication` (MFA-Sperre, `WWW-Authenticate`) |
| `paperless/adapter.py` | `CustomAccountAdapter` (`pre_authenticate` greift **nicht** bei `/api/token/`) |
| `paperless/signals.py` | `handle_failed_login` (Log bei `user_login_failed`) |
| `documents/views.py` | `UiSettingsView` |
| `documents/serialisers.py` | `UiSettingsViewSerializer` |
| `documents/permissions.py` | `PaperlessObjectPermissions` |
| `documents/signals/handlers.py` | `cleanup_user_deletion` (post_delete User/Group) |
| `paperless/config.py` | `GeneralConfig`, `AIConfig`, `RemoteOCRConfig`, `BaseConfig._get_config_instance` |
| `paperless/parsers/remote.py` | `RemoteEngineConfig.engine_is_valid` |
| `paperless/settings/__init__.py` | `REST_FRAMEWORK` (Throttle `login`), `MFA_TOTP_ISSUER`, `AUTH_PASSWORD_VALIDATORS`, Feature-Flags |
| allauth | `mfa/adapter.py`, `mfa/totp/internal/auth.py`, `mfa/recovery_codes/internal/{auth,flows}.py`, `mfa/base/internal/flows.py`, `account/auth_backends.py`, `account/internal/userkit.py`, `socialaccount/providers/base/provider.py`, `socialaccount/providers/openid_connect/provider.py` |
| DRF | `authtoken/views.py`, `authtoken/serializers.py`, `throttling.py` (`SimpleRateThrottle`, `ScopedRateThrottle`), `exceptions.py::Throttled` |
| Django | `contrib/auth/password_validation.py`, `contrib/auth/backends.py::ModelBackend` |
| Tests | `paperless/tests/test_views.py`, `documents/tests/test_api_profile.py`, `documents/tests/test_api_uisettings.py`, `documents/tests/test_api_permissions.py` (Users/Groups), `paperless/tests/test_auth_middleware.py` |

Tabellen: `auth_user`, `auth_group`, `auth_user_groups`, `auth_user_user_permissions`,
`auth_group_permissions`, `auth_permission`, `django_content_type`, `authtoken_token`
(`key` varchar(40) PK, `user_id` unique, `created`), `mfa_authenticator` (`id`, `user_id`, `type`,
`data` jsonb, `created_at`, `last_used_at`; Unique (user,type) für `totp`/`recovery_codes`),
`socialaccount_socialaccount`, `socialaccount_socialapp`, `socialaccount_socialtoken`,
`documents_uisettings` (`id`, `user_id` unique, `settings` jsonb NULL),
`paperless_applicationconfiguration`. Keine DB-seitigen ON-DELETE-Kaskaden (Django kaskadiert in Python).

## Gemeinsames

### Authentifizierung (DRF `DEFAULT_AUTHENTICATION_CLASSES`)
Reihenfolge: `PaperlessBasicAuthentication`, `TokenAuthentication`, `SessionAuthentication`
(+ ggf. Remote-User davor). Umsetzung **über `pl-authz`** (Session, Token, Basic, CSRF).
- Basic mit gültigen Zugangsdaten eines Nutzers mit MFA (irgendein `mfa_authenticator`) → 401
  `{"detail":"MFA required"}`.
- Unauthentifiziert auf `IsAuthenticated`-Route → **401**
  `{"detail":"Authentication credentials were not provided."}`, Header `WWW-Authenticate: Token`
  (bzw. `Basic realm="api"`, wenn der Request einen `Authorization: Basic …`-Header trug;
  `PaperlessBasicAuthentication.authenticate_header`).
- Session-Auth: CSRF-Prüfung nur für Session-authentifizierte unsichere Methoden
  (`{"detail":"CSRF Failed: <Grund>"}`, 403).

### Globale Rechte (`PaperlessObjectPermissions` = `DjangoObjectPermissions`)
`GET/HEAD/OPTIONS → <app>.view_<model>`, `POST → add_`, `PUT/PATCH → change_`, `DELETE → delete_`.
Modelle: `auth.user` (Users), `auth.group` (Groups), `documents.uisettings` (UI-Settings).
Fehlt → **403** `{"detail":"You do not have permission to perform this action."}`.
Objektebene: `User`/`Group` haben kein `owner` → `has_object_permission` immer `True`.
Globale Rechte = `ModelBackend`: inaktiv → keine; aktiver Superuser → alle; sonst Nutzer- ∪ Gruppenrechte.

### Fehlerformate
- DRF-Validierung: `{"feld": ["…"]}`, Objektfehler `{"non_field_errors": ["…"]}`, 400.
- Django-`HttpResponseForbidden`/`HttpResponseNotFound`/`HttpResponseBadRequest`: Klartext,
  `Content-Type: text/html; charset=utf-8`, Status 403/404/400.
- Unbehandelte Ausnahmen (KeyError, DoesNotExist, IntegrityError, TypeError …) → **500**, Django-
  Standardseite (HTML `<h1>Server Error (500)</h1>`), `DEBUG=False`.
- Detail-`pk` nicht als int parsbar → 404 `{"detail":"Not found."}`; parsbar, aber nicht im Queryset →
  404 `{"detail":"No User matches the given query."}` bzw. `"No Group matches the given query."`.
- Methode nicht erlaubt → 405 `{"detail":"Method \"X\" not allowed."}`.
- Sprache: DRF/Django-Texte werden per `LocaleMiddleware` übersetzt (Accept-Language /
  `django_language`-Cookie). Rust liefert Englisch → siehe Known differences.

### Paginierung (`StandardPagination`)
`page` (Default 1; ungültig/außerhalb → 404 `{"detail":"Invalid page."}`), `page_size` (Default 25,
max 100000, ≤0/ungültig → 25). Antwort `count`, `next`, `previous`, [`all` nur API v9: alle IDs in
Sortierreihenfolge], `results`. `next`/`previous` absolute URLs (`build_absolute_uri` +
`replace_query_param`/`remove_query_param`, auf Seite 2 wird `page` beim `previous` entfernt).

### Filter / Sortierung
- Users: `username__istartswith`, `username__iendswith`, `username__icontains`, `username__iexact`
  (PG: `UPPER(username::text) LIKE UPPER(...)` mit Escaping von `%`, `_`, `\`; leerer Wert → Filter ignoriert).
- Groups: dieselben auf `name`.
- `ordering`: erlaubt nur `username` (Users) bzw. `name` (Groups), mit optionalem `-`; Liste
  kommasepariert; ungültige Terme werden verworfen; keiner gültig → Default.
- Default: Users `ORDER BY LOWER(username)` und `username NOT IN ('consumer','AnonymousUser')`;
  Groups `ORDER BY LOWER(name)`.

---

## Routen

### `auth.token` – POST `/api/token/`
Quelle: `paperless/views.py::PaperlessObtainAuthTokenView` → DRF `ObtainAuthToken.post`,
`paperless/serialisers.py::PaperlessAuthTokenSerializer`.
- Rechte: keine (`permission_classes = ()`); Authentifizierung läuft trotzdem (ungültiger
  Token-Header → 401 `{"detail":"Invalid token."}`; gültige Auth ändert nur den Throttle-Schlüssel).
- Parser: Form, Multipart, JSON. Andere Content-Types → 415
  `{"detail":"Unsupported media type \"<ct>\" in request."}`.
- Renderer nur JSON.
- **Throttle** (`ScopedRateThrottle`, Scope `login`, Rate `PAPERLESS_TOKEN_THROTTLE_RATE`, Default
  `5/min`; Perioden `s/m/h/d` nach erstem Zeichen): Prüfung in `initial()` **vor** Methoden-Dispatch
  (zählt also auch GET/405 und fehlgeschlagene Logins). Schlüssel `throttle_login_<ident>`,
  `ident` = User-PK bei authentifiziertem Request, sonst `''.join(X-Forwarded-For.split())` falls
  Header vorhanden, sonst `REMOTE_ADDR` (`NUM_PROXIES=None`). Sliding Window über Zeitstempelliste.
  Gedrosselt → **429** `{"detail":"Request was throttled. Expected available in N seconds."}`
  (`N=ceil(wait)`, Singular `second` bei N=1), Header `Retry-After: N`.
- **Andere Methoden** (Paritätslauf 2026-11-20, F2): die Route nimmt jede Methode an
  (`TOKEN_METHODS`), weil DRF erst nach `initial()` (Version, Authentifizierung, Throttle) den Handler
  sucht: `GET`/`PUT`/`PATCH`/`DELETE`/`TRACE` (HEAD wie GET) → nach dem Throttle-Zähler **405**
  `{"detail":"Method \"GET\" not allowed."}`, `Allow: POST, OPTIONS`, ohne `Vary: Accept` (ein
  Renderer). `OPTIONS` → 200 mit `SimpleMetadata` (`token::options_metadata`: name „Paperless Obtain
  Auth Token“, description „“, renders `["application/json"]`, parses Form/Multipart/JSON,
  `actions.POST` mit `username`, `password`, `token` (read_only), `code`). Beide zählen in der Drossel,
  der nächste POST kann also 429 bekommen – wie upstream.
- Request-Felder: `username` (CharField, Pflicht, nicht leer, getrimmt), `password` (Pflicht,
  `trim_whitespace=False`), `code` (optional, MFA).
- Ablauf `validate`:
  1. Fehlende Felder → `{"username":["This field is required."]}` / `{"password":[…]}`;
     leer → `"This field may not be blank."`; `null` → `"This field may not be null."`.
  2. `authenticate(username, password)`: Backends `guardian` (nichts), `ModelBackend`
     (`username` exakt, Passwort, `is_active`), allauth `AuthenticationBackend` (Username
     case-insensitiv: `username__iexact`, nur wenn genau **ein** Treffer mit `lower()==lower()`,
     Passwort, `is_active`). Kein Treffer → `user_login_failed` (nur Log
     `Login failed for user \`<u>\` from [private ]IP \`<ip>\`.`) und **400**
     `{"non_field_errors":["Unable to log in with provided credentials."]}`.
     Erfolgreicher Passwort-Check kann den Hash upgraden (Iterationen) → `auth_user.password` wird
     neu geschrieben (Django `check_password(setter)`).
  3. MFA aktiv (irgendein Authenticator): `code` fehlt/leer → 400
     `{"non_field_errors":["MFA code is required"]}`; TOTP-Authenticator laden
     (`Authenticator.objects.get(type=totp)`; existiert nur Recovery/WebAuthn → **500**);
     `TOTP.validate_code(code)`: Code normalisieren (Satzzeichen + Whitespace entfernen, lower),
     HOTP-SHA1 mit Zähler `now//30` (Toleranz 0), 6 Stellen; danach Replay-Sperre
     `cache.add("allauth.mfa.totp.used?user=<id>&code=<code>", "y", timeout=30)` – schon benutzt →
     ungültig. Ungültig → 400 `{"non_field_errors":["Invalid MFA code"]}`.
- Erfolg: `Token.objects.get_or_create(user)` → **200** `{"token":"<40 hex>"}`. Neuer Token:
  `key = binascii.hexlify(os.urandom(20))`, `created = now()`.

### `profile.get` – GET `/api/profile/`
Quelle: `paperless/views.py::ProfileView.get`, `ProfileSerializer`.
URL ist `re_path("^profile/")` + `re_path("^$")` → nur exakt `/api/profile/`.
- Rechte: `IsAuthenticated`.
- Antwort 200 (Reihenfolge):
  `email` (str), `password` (`"*" * max(10, len(auth_user.password))`), `first_name`, `last_name`,
  `auth_token` (Token-Key oder `null`), `social_accounts` (Liste `{id, provider, name}` in
  DB-Reihenfolge von `user.socialaccount_set.all()`), `has_usable_password` (bool:
  Hash beginnt nicht mit `!`), `is_mfa_enabled` (bool: irgendein `mfa_authenticator`).
- `social_accounts[].name` = `account.get_provider_account().to_str()`; `SocialApp.DoesNotExist`
  → `"Unknown App"`. OIDC: Nutzerdaten = `extra_data.userinfo` → `extra_data.id_token` →
  `extra_data`; Tabelle `username|userName|user_name|login|handle` → `email|Email|mail|email_address`
  → `name|display_name|…|nickname` → `full_name|fullName` → `first_name…`+`last_name…`;
  sonst Markenname (OIDC: `SocialApp.name`).
- GET mit Body: Serializer wird mit `data` erzeugt, aber nicht validiert (nur `to_representation`).

### `profile.patch` – PATCH `/api/profile/`
Quelle: `ProfileView.patch`, `ProfileSerializer`, `PasswordValidationMixin`.
- Rechte: `IsAuthenticated`. Nicht-partieller Serializer (`partial=False`), aber alle Felder optional.
- Felder: `email` (EmailField, `allow_blank`, max 254 → `"Enter a valid email address."`,
  `"Ensure this field has no more than 254 characters."`), `password` (ObfuscatedPasswordField:
  CharField, nicht leer → `"This field may not be blank."`, nicht null), `first_name`, `last_name`
  (CharField max 150, `allow_blank`). Andere Felder (`auth_token`, `social_accounts`, …) read-only/ignoriert.
- `validate_password`: nur wenn `value.replace("*","") != ""`; Django-Validatoren mit
  `user = request.user` (Werte **vor** der Änderung):
  1. `UserAttributeSimilarityValidator` (username, first_name, last_name, email; `quick_ratio ≥ 0.7`,
     Teile `re.split(r"\W+")` + Gesamtwert, Längenquotienten-Abkürzung) →
     `"The password is too similar to the <username|first name|last name|email address>."`
  2. `MinimumLengthValidator(8)` → `"This password is too short. It must contain at least 8 characters."`
  3. `CommonPasswordValidator` (Django-Liste 20000, `lower().strip()`) → `"This password is too common."`
  4. `NumericPasswordValidator` → `"This password is entirely numeric."`
  Alle Fehler gesammelt → 400 `{"password":[…]}`.
- Nebenwirkungen: echtes Passwort → `set_password` (pbkdf2_sha256, 1 000 000 Iterationen) + save;
  danach `email/first_name/last_name` setzen + save (`UPDATE auth_user` aller Spalten).
  **Kein** `update_session_auth_hash` → die aktuelle Session wird beim nächsten Request ungültig
  (Session-Auth-Hash-Prüfung), Token/Basic bleiben gültig.
- Antwort 200: wie `profile.get` mit neuen Werten.

### `profile.generate_auth_token` – POST `/api/profile/generate_auth_token/`
Quelle: `GenerateAuthTokenView.post`. `IsAuthenticated`. Bestehenden Token löschen, neuen anlegen
(40 hex). Antwort 200: JSON-String `"<key>"`.

### `profile.disconnect_social_account` – POST `/api/profile/disconnect_social_account/`
Quelle: `DisconnectSocialAccountView.post`. `IsAuthenticated`.
- `request.data["id"]` fehlt → 500 (KeyError); nicht int-konvertierbar → 500 (ValueError).
- Nicht gefunden (nur eigene Accounts) → 400 Klartext `Social account not found`.
- Erfolg: Account löschen (Kaskade `socialaccount_socialtoken`), Antwort 200: die ID (int).

### `profile.social_account_providers` – GET `/api/profile/social_account_providers/`
Quelle: `SocialAccountProvidersView.get`. **Umgesetzt in pl-accounts (W23)**, weil die
Provider-/App-Liste (`list_apps`, `SOCIALACCOUNT_PROVIDERS`) dort liegt und pl-accounts schon von
pl-auth-api abhängt – Vertrag siehe `docs/contract/pl-accounts.md`. Antwort: Liste
`{name, login_url}` sortiert nach `name`.

### `profile.totp.get` – GET `/api/profile/totp/`
Quelle: `TOTPView.get`, allauth `get_totp_secret(regenerate=True)`, `build_totp_url`, `build_totp_svg`.
- `IsAuthenticated`.
- `secret` = `base64.b32encode(secrets.token_bytes(20))` (32 Zeichen A–Z2–7, ohne Padding).
  Nebenwirkung upstream: Session-Key `mfa.totp.secret` wird gesetzt (Session-Schreibzugriff) →
  `SessionMiddleware` speichert die Sitzung (`cached_db`) und setzt `Set-Cookie: sessionid=…`
  (auch bei Token-/Basic-Auth: dann neue Sitzung mit nur diesem Schlüssel). Rust: `src/session.rs`
  `set_session_value` – vorhandene, gültige Sitzung aus dem Cookie laden (sonst neuer 32-Zeichen-
  Schlüssel `[a-z0-9]`), Wert setzen, `signing.dumps(compress=True)` wie `SessionBase.encode` in
  `django_session` schreiben (Ablauf `get_expiry_age`), danach den `cached_db`-Eintrag im
  Django-Cache löschen (Django lädt beim nächsten Zugriff die DB-Fassung); Cookie wie
  `SessionMiddleware.process_response` (`expires`, `HttpOnly`, `Max-Age`, `Path=/`, `SameSite=Lax`;
  bei Browser-Ende-Sitzungen ohne Ablauf).
- `url` = `otpauth://totp/{quote(label)}?secret=<secret>&issuer=Paperless-ngx`
  (`label` = E-Mail, sonst Username, sonst `str(user)`; `quote` safe=`/`; Query via
  `urlencode(quote_via=quote)` safe=``; `digits`/`period` nur wenn ≠ 6/30 → nie).
- `qr_svg` = `qrcode.make(url, image_factory=SvgPathImage)`: EC-Level M, `box_size=10`, `border=4`,
  Daten-Optimierung `optimize=20` (Numeric-/Alnum-Läufe ≥ 20 Zeichen als eigene Segmente),
  kleinste passende Version, beste Maske nach `lost_point`. Ausgabe (lxml):
  `<?xml version='1.0' encoding='UTF-8'?>\n<svg width="Nmm" height="Nmm" version="1.1" viewBox="0 0 N N" xmlns="http://www.w3.org/2000/svg"><path d="M{x},{y}H{x+1}V{y+1}H{x}z…" id="qr-path" fill="#000000" fill-opacity="1" fill-rule="nonzero" stroke="none"/></svg>`
  (N = Module + 8, Pfad zeilenweise je dunklem Modul, x = Spalte+4, y = Zeile+4).
- Antwort 200: `{"url","qr_svg","secret"}`.

### `profile.totp.post` – POST `/api/profile/totp/`
Quelle: `TOTPView.post`.
- `IsAuthenticated`. `secret`/`code` fehlen → 500 (KeyError).
- `validate_totp_code(secret, code)` (ohne Replay-Cache). Ungültiges Base32 → 500 (binascii.Error).
- Gültig: `INSERT mfa_authenticator(type='totp', data={"secret": secret}, created_at=now)`
  (existiert schon → IntegrityError → 500); Signal `authenticator_added` (keine Empfänger);
  `auto_generate_recovery_codes`: existiert noch kein `recovery_codes`-Authenticator →
  `INSERT (type='recovery_codes', data={"seed": token_hex(40), "used_mask": 0})` und
  Django-Message „A new set of recovery codes has been generated." (Cookie-Storage).
  `recovery_codes` = unbenutzte Codes: HMAC-SHA1 (Key = Seed ASCII), kumulativ
  `update(f"{i:3},")` für i=0..9, erste 4 Bytes big-endian `% 10**8`, 8-stellig mit Nullen.
  Existierten Recovery-Codes schon → `recovery_codes: null`.
- Antwort 200: `{"success": <bool>, "recovery_codes": [..]|null}`.

### `profile.totp.delete` – DELETE `/api/profile/totp/`
Quelle: `TOTPView.delete`, allauth `delete_and_cleanup`.
- `IsAuthenticated`. Kein TOTP → 404 Klartext `TOTP not found`.
- Sonst TOTP löschen; gibt es danach keinen Nicht-Recovery-Authenticator mehr → alle Authenticators
  des Nutzers löschen. Signale `authenticator_removed` (keine Empfänger). Antwort 200 `true`.

### `users.list` – GET `/api/users/`
Quelle: `UserViewSet` (ListModelMixin), `UserSerializer`, `UserFilterSet`.
- Rechte: authentifiziert + `auth.view_user`.
- Queryset, Filter, Sortierung, Paginierung siehe oben.
- Objekt (Reihenfolge): `id`, `username`, `email`, `password` (`*`×max(10,len(hash))),
  `first_name`, `last_name`, `date_joined` (DRF-DateTime in `PAPERLESS_TIME_ZONE`),
  `is_staff`, `is_active`, `is_superuser`, `groups` (IDs; Reihenfolge ohne ORDER BY),
  `user_permissions` (Codenames, geordnet `app_label, model, codename`),
  `inherited_permissions` (Set `"app_label.codename"`: inaktiv → leer, Superuser → **alle**
  Permissions, sonst Gruppenrechte; Python-Set → Reihenfolge undefiniert),
  `is_mfa_enabled`, `last_login` (PaperRust-Erweiterung, siehe Known differences 20).

### `users.retrieve` – GET `/api/users/{id}/`
`auth.view_user`; 404 wie oben (inkl. `consumer`/`AnonymousUser` → 404).

### `users.create` – POST `/api/users/`
Quelle: `UserViewSet.create`, `UserSerializer.create`.
- Rechte: `auth.add_user`.
- Vor-Prüfung (Nicht-Superuser): `is_superuser` als Boolean `True` → 403 Klartext
  `Superuser status can only be granted by a superuser`; `is_staff` True → 403
  `Staff status can only be granted by a superuser`. Bool-Parsing wie DRF `BooleanField`
  (`true/True/TRUE/t/T/y/Y/yes/Yes/YES/on/On/ON/1/1.0`; ungültig → wird nicht gewertet).
- Felder: `username` (Pflicht, max 150, Regex `^[\w.@+-]+\Z` →
  `"Enter a valid username. This value may contain only letters, numbers, and @/./+/-/_ characters."`,
  eindeutig → `"A user with that username already exists."`), `email` (EmailField, blank ok,
  max 254), `password` (optional, nicht leer), `first_name`/`last_name` (max 150, blank ok),
  `date_joined` (DateTime, optional), `is_staff`/`is_active`/`is_superuser` (Boolean, optional,
  `"Must be a valid boolean."`), `groups` (PK-Liste: `"Invalid pk \"X\" - object does not exist."`,
  `"Incorrect type. Expected pk value, received str."`, kein Array →
  `"Expected a list of items but got type \"str\"."`), `user_permissions` (Codename-Liste aus
  Permissions **ohne** `admin`-App: `"Object with codename=X does not exist."`, mehrdeutig →
  `"Invalid value."`).
- Passwortvalidierung mit `user = request.user` (der anlegende Nutzer! upstream-Eigenheit).
- Anlage: `INSERT auth_user` (Defaults: `password=''`, `is_staff=false`, `is_active=true`,
  `is_superuser=false`, `date_joined=now`, `last_login=NULL`, Namen/E-Mail `''`), Gruppen/Rechte
  setzen, echtes Passwort hashen, save. Antwort **201** (Objekt wie list).

### `users.update` / `users.partial_update` – PUT/PATCH `/api/users/{id}/`
Quelle: `UserViewSet.update`, `UserSerializer.update`.
- Rechte: `auth.change_user`; Objekt holen (404).
- Nicht-Superuser: Ziel ist Superuser → 403 `Superusers can only be modified by other superusers`;
  `is_superuser` angegeben und ≠ aktuell → 403 `Superuser status can only be changed by a superuser`;
  `is_staff` angegeben und ≠ aktuell → 403 `Staff status can only be changed by a superuser`.
- PUT: `username` Pflicht; PATCH: alles optional. Passwortvalidierung mit `user = Zielnutzer`
  (alte Werte). Echtes Passwort → `set_password` + save; dann Felder + m2m `.set()`.
  Ändert man das eigene Passwort, wird die eigene Session ungültig (kein `update_session_auth_hash`).
- Antwort 200 (Objekt).

### `users.destroy` – DELETE `/api/users/{id}/`
Quelle: `UserViewSet.destroy`. **Umgesetzt (Standalone 2026-11-09, `users::destroy`)**: Kaskade als
SQL in einer Transaktion (`USER_CASCADE`: m2m, EmailAddress→EmailConfirmation, SocialAccount→
SocialToken, Token, UiSettings, Authenticator, guardian, admin-Log; `USER_SET_NULL`: alle `owner`,
`Note.user`, `assign_owner`, auditlog-`actor`; `paperrust_*` per DB `ON DELETE SET NULL`), dann
`cleanup_user_deletion`. Kaskadierte Modelle haben keine Auditlog-Registrierung/Signale (geprüft).
Rechte `auth.delete_user`;
Nicht-Superuser löscht Superuser → 403 `Superusers can only be deleted by other superusers`.
Löschen über Django-Collector (Kaskaden/SET_NULL über alle Modelle mit FK auf `auth_user`:
Dokumente/Objekte `owner` SET_NULL, Notes, UiSettings, Token, Authenticators, Social Accounts,
guardian-Objektrechte, Mail-Konten …, inkl. `post_delete`-Signale/Auditlog der kaskadierten
Objekte) + `cleanup_user_deletion` (entfernt den Nutzer aus `settings.permissions.default_owner`,
`default_view_users`, `default_change_users` aller UiSettings). Antwort 204.

### `users.deactivate_totp` – POST `/api/users/{id}/deactivate_totp/`
Quelle: `UserViewSet.deactivate_totp`.
- Rechte: authentifiziert + **`auth.add_user`** (POST-Mapping von `perms_map`).
- `User.objects.get(pk=id)` (ohne Queryset-Ausschluss): nicht vorhanden/kein int → **500**.
- Nicht-Superuser und nicht er selbst → 403 Klartext
  `You do not have permission to deactivate TOTP for this user`.
- Kein TOTP → 404 Klartext `TOTP not found`; sonst `delete_and_cleanup` → 200 `true`.

### `groups.list` / `groups.retrieve` / `groups.create` / `groups.update` / `groups.partial_update` / `groups.destroy`
Quelle: `GroupViewSet`, `GroupSerializer`.
- Rechte `auth.view_group` / `add_group` / `change_group` / `delete_group`.
- Objekt: `id`, `name`, `permissions` (Codenames, geordnet `app_label, model, codename`).
- Felder: `name` (Pflicht bei POST/PUT, max 150, eindeutig → `"group with this name already exists."`),
  `permissions` (Pflicht bei POST/PUT; Codename-Liste ohne `admin`-App, Fehler wie oben).
- Create 201, Update 200, Destroy 204 (leer).
- Destroy-Nebenwirkungen: m2m (`auth_user_groups`, `auth_group_permissions`, Workflow-Aktions-
  m2m-Tabellen) und `guardian_groupobjectpermission` (CASCADE) löschen; `cleanup_user_deletion`
  entfernt die Gruppe aus `default_view_groups`/`default_change_groups` aller UiSettings
  (Fehler werden nur geloggt).

### `ui_settings.get` – GET `/api/ui_settings/`
Quelle: `documents/views.py::UiSettingsView.get`. URL `re_path("^ui_settings/")` → **jeder** Pfad
mit Präfix `/api/ui_settings/` landet hier.
- Rechte: authentifiziert + `documents.view_uisettings`.
- Body (falls vorhanden) wird validiert (`settings` muss Dict/null sein).
- `settings` = gespeichertes `documents_uisettings.settings` (jsonb-Reihenfolge) oder `{}`
  (gespeichertes `null` → 500, `None` hat kein `in`). Danach in dieser Reihenfolge setzen
  (vorhandene Schlüssel behalten ihre Position):
  1. `update_checking.backend_setting` = `PAPERLESS_ENABLE_UPDATE_CHECK` (`"default"` oder bool);
     existiert `update_checking` nicht → `{"backend_setting": …}` anhängen.
  2. `trash_delay` = `max(PAPERLESS_EMPTY_TRASH_DELAY or 30, 1)`.
  3. `version` = `"3.4.2"`.
  4. `app_title` = `PAPERLESS_APP_TITLE` (oder null); DB `app_title` nicht leer → DB-Wert.
  5. `app_logo` = `PAPERLESS_APP_LOGO` (oder null); DB `app_logo` nicht leer → Storage-URL
     (`<FORCE_SCRIPT_NAME>/` + `filepath_to_uri(name)`).
  6. `auditlog_enabled` = `PAPERLESS_AUDIT_LOG_ENABLED` (Default true).
  7. `remote_ocr` = `{"configured": engine=="azureai" && api_key!=None && endpoint!=None,
     "mode": DB-Mode oder PAPERLESS_REMOTE_OCR_MODE (Default "always")}` (DB-Wert `or` Env).
  8. Gmail-/Outlook-OAuth aktiv → `gmail_oauth_url`/`outlook_oauth_url` und Session-Key
     `oauth_state` (**Session-Schreibzugriff**).
  9. `email_enabled` = `EMAIL_HOST != "localhost" or EMAIL_HOST_USER != ""`.
  10. `ai_enabled` = DB `ai_enabled` falls nicht null, sonst `PAPERLESS_AI_ENABLED`.
- `user` = `{id, username, is_staff, is_superuser, groups:[ids], [first_name], [last_name]}`
  (Namen nur wenn nicht leer).
- `permissions` = alle Rechte (`get_all_permissions`) ohne `app_label.`-Präfix
  (`re.sub(r"^\w+.", "")`); inaktiv → leer, Superuser → alle; Reihenfolge undefiniert (Set).
- Fehlt die `ApplicationConfiguration`-Zeile, legt upstream sie an.
- Antwort 200 `{"user", "settings", "permissions"}`.
- **R4 (2026-11-09, Bericht O1):** gespeicherte Einstellungen und `ApplicationConfiguration` in
  **einer** Abfrage (`LEFT JOIN documents_uisettings` + `LEFT JOIN LATERAL` auf die erste
  Konfigurationszeile) statt zwei Round-Trips; Ergebnis und Fehlerfälle unverändert.
- **Branding (pl-server, nicht pl-auth-api):** mit `PAPERRUST_BRANDING` an (Standard) setzt
  pl-server beim Start `Config.app_title = "PaperRust"` und `Config.app_logo =
  "/__rs/brand/logo.svg"`, wenn `PAPERLESS_APP_TITLE`/`PAPERLESS_APP_LOGO` nicht gesetzt sind
  (`crates/pl-server/src/branding.rs::apply_ui_defaults`). Django liest seine eigene Umgebung →
  dort `null`. Das sind die Paritätsdiffs `ui_settings*` (`$.settings.app_title`,
  `$.settings.app_logo`) – gewollt, Regel-Vorschlag im Bericht R4.
- **„Django Admin öffnen“ (Oberfläche):** `settings.component.html` verlinkt statisch `admin/`
  (kein Feld aus `ui_settings`). Die Oberfläche nutzt aus der Antwort `user.is_staff`
  (Systemstatus/Logs-Rechte, `permissions.service.ts`) und `user.is_superuser` – beides bleibt.
  Backend-seitig wird nichts entfernt; den Link entfernt R2 in der Oberfläche.
- `settings.version` bleibt `"3.4.2"` (`pl_core::brand::UPSTREAM_VERSION`, Seitenleiste der
  unveränderten Oberfläche); die PaperRust-Version liefert `/api/status/` `paperrust.version`.

### `ui_settings.post` – POST `/api/ui_settings/`
Quelle: `UiSettingsView.post`, `UiSettingsViewSerializer`.
- Rechte: authentifiziert + `documents.add_uisettings`.
- `settings`: DictField, optional, `null` erlaubt; kein Objekt → 400
  `{"settings":["Expected a dictionary of items but got type \"<pytype>\"."]}`
  (`str`, `list`, `int`, `float`, `bool`). `validate_settings`: `settings=null` → 500 (TypeError);
  `update_checking` vorhanden: Dict → `backend_setting` entfernen; String/Zahl ohne `pop` → 500.
- `UiSettings.update_or_create(user, defaults={"settings": settings oder None})` (fehlt `settings`
  → `NULL` speichern).
- Antwort 200 `{"success": true}`.

## Nicht in diesem Crate portiert

Seit 2026-11-14 gibt es keine Weiterleitung an Django mehr: was keine Rust-Route hat (auch in anderen
Crates, z. B. allauth-Seiten in pl-accounts), antwortet 404. Stand der Liste unten: Welle 1.
- `/api/auth/headless/*` (allauth headless, inkl. Login/MFA/Passkeys) und `/accounts/*`
  (allauth-Seiten, Social-Login, Passwort-Reset).
- `/api/auth/login/`, `/api/auth/logout/` (DRF-Login-Views auf allauth).
- `profile.social_account_providers` (siehe oben).
- `OPTIONS` auf allen Routen (DRF-Metadata).

## Known differences
1. **Sprache:** Fehlermeldungen nur Englisch (upstream übersetzt per `LocaleMiddleware`).
2. **Throttle-Speicher (Review 2 M2):** Rust liest/schreibt die `login`-Historie in Djangos
   `default`-Cache über `pl_core::cache` (Schlüssel `:1:throttle_login_<ident>`, Wert = Pickle
   Protokoll 5 einer Float-Liste, Ablauf = Dauer), aktualisiert per `compare_and_set` (atomar; Django
   selbst ist nicht atomar, `get`→`set`). Cache prozesslokal (Standard) oder mit
   `PAPERRUST_CACHE=postgres` gemeinsam für alle Prozesse (Mehrprozessbetrieb). Bis 2026-11-14 lag er
   in Redis, gemeinsam mit Django; die Redis-Helfer (`add_y`, `get_raw`, `compare_and_set`,
   `throttle::allow_shared`) und der Test `tests/django_cache_redis.rs` sind entfernt.
3. **TOTP-Replay-Sperre (Review 2 M2):** `cache.add("allauth.mfa.totp.used?user=<id>&code=<code>",
   pickle("y"), 30)` über `pl_core::cache` – derselbe Schlüssel und dieselbe `cache.add`-Semantik wie
   allauth; gemeinsam über Prozesse nur mit `PAPERRUST_CACHE=postgres` (sonst je Prozess, PaperRust
   warnt im Mehrprozessbetrieb).
4. **`profile.totp.get`:** Session-Key `mfa.totp.secret` wird nicht gesetzt (paperless liest ihn
   nicht; nur allauth-eigene Formulare unter `/accounts/2fa/` würden ihn nutzen).
5. **`profile.totp.post`:** Keine Django-Message „recovery codes generated" (Cookie `messages`).
6. **`ui_settings.get` mit Mail-OAuth (Gmail/Outlook) aktiv:** seit 2026-11-09 umgesetzt
   (`gmail_oauth_url`/`outlook_oauth_url` wie httpx-oauth, `oauth_state` per `session::set_session_value`,
   Set-Cookie). Der `state` ist zufällig (Parität: Regel `oauth-state-random`).
7. **Mengen-Reihenfolge:** `inherited_permissions` (Users) und `permissions` (ui_settings) sind in
   Python Sets (Hash-Reihenfolge, pro Prozess zufällig) → Rust sortiert alphabetisch; der
   Paritäts-Diff muss diese Listen sortiert vergleichen. `groups`-ID-Listen: Rust sortiert nach
   `group_id` (upstream ohne ORDER BY).
8. **Social-Account-Name:** Rust bildet `to_str()` nur generisch (Basis-Tabelle) und für
   `openid_connect` nach; Provider mit eigenem `to_str` (GitHub, Google, …) können abweichen.
   SocialApps aus `PAPERLESS_SOCIALACCOUNT_PROVIDERS` (`APPS`) werden berücksichtigt.
9. **`isdigit`/`\W`/`lower` (Passwortvalidatoren):** Rust nutzt Unicode-Eigenschaften der
   Standardbibliothek; bei exotischen Zeichen (z. B. hochgestellte Ziffern) minimal anders.
10. **500-Fälle** (KeyError usw.) liefern die Django-Standard-500-Seite nach; Python-Exception-Texte
    erscheinen upstream nur im Log.
11. **Cachalot** (`PAPERLESS_DB_READ_CACHE_ENABLED`): Rust-Schreibzugriffe invalidieren den
    Cachalot-Cache nicht → mit aktivem Read-Cache Routen mit Schreibzugriff nicht aktivieren.
12. **JSON-Parse-Fehler** (W2 2026-11-08 behoben): Text `JSON parse error - <Python-Meldung>` über
    `pyjson` (CPython-C-Scanner nachgebaut, Quelle `crates/pl-core/src/pyjson.rs`,
    Re-Export `pl_core::pyjson`, W11). Rest: Rekursionstiefe / >4300-Ziffern-Ints angenähert.
13. **`ApplicationConfiguration` fehlt**: Rust legt die Zeile nicht an, sondern rechnet mit NULL-Werten.
14. **Präfix-Routen:** upstream nutzt `re_path("^ui_settings/")` bzw. `re_path("^profile/")` – auch
    `/api/ui_settings/<irgendwas>` landet bei `UiSettingsView`. Rust registriert nur die exakten
    Pfade; längere Pfade schreibt pl-server (`frontend::urlconf::rewrite_prefix_view`) auf die Basis um,
    sodass die Rust-View antwortet wie upstream (bis 2026-11-14 ohne diese Umschreibung an Django).
15. **Route-ids im Basiskatalog** (`cases.base.json`: `token`, `profile`, `profile.totp`,
    `ui_settings`) sind gröber als die Rust-ids (`auth.token`, `profile.get`/`profile.patch`,
    `profile.totp.get|post|delete`, `ui_settings.get|post`) – Zuordnung im Bericht.
16. **Throttle-Wartezeit:** „Expected available in N seconds" hängt von der Laufzeit der vorigen
    Anfragen ab (PBKDF2 im Debug-Build deutlich langsamer als Django) → im W2-Lauf per Regel
    `throttle-wait-seconds` akzeptiert; Status, Text und Zählung identisch.
17. **`ui_settings.post` mit Formular-Body** (W2 2026-11-08 behoben): wie DRF
    `DictField.get_value` → `parse_html_dict(prefix="settings")`; nur `settings.<key>`-Felder
    zählen, ohne sie wird `{}` gespeichert (vorher 400).
18. **Sitzung beim TOTP-GET:** Rust schreibt die Sitzung selbst (`src/session.rs`) statt über
   `SESSION_ENGINE`; der `cached_db`-Eintrag wird gelöscht statt neu befüllt (Django lädt aus der
   DB – gleiches Ergebnis, ein DB-Zugriff mehr). Gleichzeitige Schreibzugriffe von Django und Rust
   auf dieselbe Sitzung: letzter Schreiber gewinnt (wie bei zwei Django-Prozessen). Die Funktion
   doppelt pl-accounts `session.rs` (pl-accounts hängt von pl-auth-api ab), siehe CORE-REQUEST.
19. **TOTP-Aussteller (A-290, 2026-11-16):** `issuer` in der otpauth-URL ist
   `PAPERRUST_MFA_TOTP_ISSUER` (Aliase `PAPERLESS_*`), Standard `pl_core::brand::PRODUCT_NAME`
   („PaperRust“) statt upstream fest „Paperless-ngx“. Bereits eingerichtete Authenticator-Einträge
   behalten ihren alten Namen (der Code bleibt gültig, nur die Anzeige in der App).
20. **`last_login` im Benutzerobjekt (PaperRust-Erweiterung, 2026-11-16):** `/api/users/` (Liste,
    Einzelobjekt und Antworten von POST/PUT/PATCH) liefert als letztes Feld zusätzlich `last_login`
    (`auth_user.last_login`, DRF-DateTime in `PAPERLESS_TIME_ZONE` oder `null`; nur lesend – ein Wert im
    Request wird wie jedes unbekannte Feld ignoriert). Upstream `UserSerializer.Meta.fields` hat es nicht;
    gebraucht für die Spalte „zuletzt angemeldet“ der Benutzerverwaltung im PaperRust-Design
    (docs/DESIGN2-ENTWIRREN.md §2). Paritäts-Diff: Feld `last_login` bei `/api/users/` ignorieren.
21. **Geräte-Token nach Passwortwechsel widerrufen** (PaperRust-Erweiterung, Sicherheitsprüfung
    2026-11-19 F13): `PATCH /api/profile/` mit neuem Passwort und `PUT`/`PATCH /api/users/{id}/` mit
    neuem Passwort widerrufen danach alle Geräte-Token (`prt_…`, `paperrust_device_token`) des
    betroffenen Nutzers (`pl_authz::device::revoke_all_for_user`, bei `users` nach dem Commit). Der
    DRF-Token (`authtoken_token`) bleibt wie upstream bestehen; ein Wert wie `**********` (kein
    echtes Passwort) widerruft nichts.
22. **`OPTIONS /api/token/` – Labels** (2026-11-20): die `label`-Texte der Metadaten (`Username`,
    `Password`, `Token`, `MFA Code`) sind immer Englisch; upstream übersetzt DRFs `gettext_lazy`-Labels
    mit der aktiven Sprache.
23. **Passwort-Maske konstant** (2026-11-21, upstream 64bf7f845 / #14784, nach v3.4.2): `password`-Felder werden immer als `"**********"` (10 Sterne) ausgegeben; v3.4.2 sendete `"*" * max(10, len(passwort))` und verriet damit die Länge. „Nur Sterne oder leer“ = unverändert, wie bisher. Betrifft `password` in `/api/profile/`.

## Umsetzungsstand (Rust, `crates/pl-auth-api`)

| id | Status |
|---|---|
| `auth.token` | fertig (Throttle und TOTP-Replay-Sperre im Django-Cache, Review 2 M2) |
| `profile.get`, `profile.patch` | fertig (Social-Account-Namen: KD #8) |
| `profile.generate_auth_token`, `profile.disconnect_social_account` | fertig |
| `profile.totp.get`, `profile.totp.post`, `profile.totp.delete` | fertig (QR-SVG byte-gleich zu qrcode 8.2 + lxml) |
| `profile.social_account_providers` | fertig, in pl-accounts (W23) |
| `users.list`, `users.retrieve`, `users.create`, `users.update`, `users.partial_update`, `users.deactivate_totp` | fertig |
| `users.destroy` | fertig (2026-11-09, Parität tests/parity/standalone-gaps) |
| `groups.list`, `groups.retrieve`, `groups.create`, `groups.update`, `groups.partial_update`, `groups.destroy` | fertig |
| `ui_settings.get`, `ui_settings.post` | fertig (`get` mit Mail-OAuth: KD #6) |

## Tests

- Unit-Tests (`cargo test -p pl-auth-api`): 26 grün – u. a. QR: 22 Vektoren aus python-qrcode 8.2/lxml
  (`tests/parity/pl-auth-api/qr_vectors.py` → `crates/pl-auth-api/tests/qr_vectors.txt`) byte-gleich,
  HOTP (RFC 6238), Base32 nach Python-Regeln, Recovery-Codes gegen Python-HMAC-Vektor, Django-Passwort-
  validatoren, DRF-Bool/Form-Semantik, Throttle-Fenster, `to_str` der Social Accounts.
- Referenz-Parität (`cargo test -p pl-auth-api --test reference_parity -- --ignored`, Stand 2026-11-08,
  Django :8000 vs. Rust-Router in-process auf derselben DB, Token-Auth):
  - 62 Lesefälle (users/groups Liste/Detail/Filter/Sortierung/Paginierung/Fehler, profile, ui_settings,
    v9 + v10, admin/alice/bob/carol/ungültiger Token): **different = 0**.
  - 34 nicht-schreibende Mutationsfälle (Validierungsfehler users/groups/profile/ui_settings/token,
    403/404-Pfade, TOTP mit falschem Code, TOTP-Delete ohne TOTP): **different = 0**.
- Harness-Katalog `tests/parity/pl-auth-api/cases.json`: 29 Fälle inkl. Schreibfälle (`write: true`,
  nur mit `reference/reset.sh`) – noch **nicht gelaufen** (Schreibpfade gegen Django ungeprüft:
  users.create/update, groups.create/update/destroy, ui_settings.post, profile.patch, totp.post mit
  gültigem Code, token mit MFA).
