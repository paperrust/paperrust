# Vertrag `pl-authz` – Authentifizierung, CSRF, Passwörter, Rechte

Port von paperless-ngx v3.4.2 (`upstream/`). Geprüft gegen die Quellen in `reference/venv`:
Django 5.2.18, DRF 3.18.3, django-guardian 3.5.2, django-allauth 65.20.1. Das Crate hat **keine eigenen Routen**; es liefert die Bausteine, die jeder
Handler benutzt. Alle Funktionen nennen ihre Upstream-Quelle im Kopfkommentar der Datei.

## Überblick für Fach-Agenten (öffentliche API)

| Upstream | Rust |
|---|---|
| `request.user`, `request.auth`, `successful_authenticator` | `AuthContext { user, method, token_key, http_method, www_authenticate, session }` |
| Authentifizierung (AllowAny) | Extraktor `MaybeUser` (Deref → `AuthContext`) |
| `IsAuthenticated` | Extraktor `AuthUser` (`.user()`, Deref → `AuthContext`) |
| `APIView.permission_denied` | `ctx.deny()` / `ctx.deny_with(msg)` (401/403-Regel wie DRF) |
| `(IsAuthenticated, PaperlessObjectPermissions).has_permission` | `auth.require_model_perms(&models::TAG)?` |
| `PaperlessObjectPermissions.has_object_permission` | `auth.check_object_perms(db, &models::TAG, ObjRef{id, owner_id}).await?` |
| `GlobalPermissionsByMethod`-Unterklassen | `auth.require_perms_by_method(perms::VIEW_DOCUMENTS_PERMISSIONS)?` (bzw. `check_perms_by_method` ohne `IsAuthenticated`), Konstanten `TRASH_PERMISSIONS`, `NOTE_PERMISSIONS`, `ACKNOWLEDGE_TASKS_PERMISSIONS` |
| `PaperlessAdminPermissions` | `auth.require_admin()?` |
| `has_global_statistics_permission` / `has_system_status_permission` | `perms::has_global_statistics_permission(Some(user))`, `perms::has_system_status_permission(..)` |
| `has_perms_owner_aware(user, perm, obj)` | `perms::has_perms_owner_aware(db, Some(user), "view_document", &models::DOCUMENT, ObjRef{..}).await?` (für Dokumente das **Root**-Dokument übergeben) |
| guardian `ObjectPermissionChecker.has_perm` | `perms::guardian_has_perm(db, user, &model, id, perm).await?` |
| `user.has_perm(s)` (ModelBackend) | `User::has_perm("documents.view_document")`, `has_perms(&[..])` |
| `PermittedObjectsFilter` | `objects::permitted_objects_filter(Some(user), &models::TAG, "<tabellen-alias>", &FilterOpts{..}) -> sea_query::Condition` |
| `permitted_object_ids` | `objects::permitted_object_ids(user, &model, perm, &PermOpts{include_deleted, parent_field}) -> SelectStatement` |
| `permitted_document_ids` | `objects::permitted_document_ids(user, "view_document", include_deleted)` |
| `restrict_queryset_to_visible` / `_owner_visible` | `objects::restrict_to_visible(..)` / `objects::restrict_to_owner_visible(..)` → `Condition` |
| L5: Grant-Bedingung je Zeile (guardian oder `paperrust_grant`, Umschalter) | `objects::granted_key_cond(user, &model, perm, key_expr)`; Index-Verwaltung `grants::{status, check, rebuild, command}` |
| `user_is_unrestricted` / `user_is_active_superuser` | `objects::user_is_unrestricted(..)` / `objects::user_is_active_superuser(..)` |
| `PassUserMixin` `full_perms` | `owned::parse_full_perms(query.get("full_perms"))` |
| `OwnedObjectSerializer.__init__` (Feld-Auswahl) | `owned::OwnedFields::new(full_perms, all_fields)` |
| `BulkPermissionMixin` + `get_permissions`/`get_user_can_change`/`get_is_shared_by_requester` | `owned::OwnedInfo::load(db, &model, &page_ids, fields, with_shared).await?` dann `info.write_fields(..)` oder `permissions_json`/`user_can_change`/`is_shared_by_requester` |
| `get_permission_grantees` / `get_shared_object_pks` | `owned::get_permission_grantees(..)`, `owned::get_shared_object_pks(..)` |
| `owner`-Feld (`PrimaryKeyRelatedField`) | `owned::parse_owner(db, &value).await` → `Result<Option<i64>, FieldError>` |
| `SetPermissionsSerializer` + `validate_set_permissions` | `owned::parse_set_permissions(&v)` → `owned::validate_set_permissions(db, &raw).await` → `PermissionsSpec` |
| `set_permissions_for_objects(perms, model, pks, merge=)` | `owned::set_permissions_for_objects(&mut conn, &spec, &model, &pks, merge).await` |
| `OwnedObjectSerializer.create` Owner-Default | `owned::owner_on_create(Some(user), requested)` |
| `OwnedObjectSerializer.update` Owner-/Rechte-Wächter | `owned::check_owner_update(Some(user), instance_owner, new_owner, has_set_permissions)?` (403 "Insufficient permissions.") |
| `authenticate(request, username=, password=)` (z. B. `/api/token/`) | `authz.authenticate_credentials(username, password).await?` |
| `make_password` / `check_password` | `hashers::make_password(pw)`, `hashers::verify_password(pw, enc)` (in `spawn_blocking`!) |
| `is_mfa_enabled` (allauth) | `user::is_mfa_enabled(db, user_id).await?` |
| Session-Signatur / `get_session_auth_hash` | `signing::decode_session`, `signing::encode_session`, `signing::session_auth_hash` |
| CSRF-Token maskieren/entmaskieren | `csrf::mask_cipher_secret`, `csrf::unmask_cipher_token` |
| `request.get_host()` / `is_secure()` | `csrf::get_host(cfg, headers)`, `csrf::is_secure(cfg, headers)` |

Modelle: `models::{DOCUMENT, TAG, CORRESPONDENT, DOCUMENT_TYPE, STORAGE_PATH, SAVED_VIEW,
PAPERLESS_TASK, SHARE_LINK, SHARE_LINK_BUNDLE, NOTE, CUSTOM_FIELD, WORKFLOW, MAIL_ACCOUNT,
MAIL_RULE, PROCESSED_MAIL}` (`app_label`, `model_name`, `object_name`, `table`, `soft_delete`),
`models::DOCUMENT_PARENT_FIELD = "root_document_id"`.

### Einbindung (Server)

```rust
let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&config));
router.layer(axum::Extension(authz))
```
Die Extraktoren lesen `Authz` aus den Request-Extensions und cachen den `AuthContext` dort
(mehrere Extraktoren → eine Authentifizierung). Fehler: `AuthError` (implementiert
`IntoResponse` minimal und `From<AuthError> for pl_core::ApiError`).

**Reihenfolge wie DRF `initial()`**: Negotiation/Version (406) → Authentifizierung → Rechte.
In Handlern daher `RequestContext` (pl-core) vor `AuthUser`/`MaybeUser` als Argument.

### Beispiel Liste mit Rechten

```rust
async fn list(ctx: RequestContext, auth: AuthUser, State(s): State<AppState>) -> ApiResult<..> {
    auth.require_model_perms(&models::TAG)?;                       // IsAuthenticated + Modellrecht
    let mut q = Query::select().from(Alias::new("documents_tag")).to_owned();
    q.cond_where(objects::permitted_objects_filter(Some(auth.user()), &models::TAG,
                 "documents_tag", &FilterOpts::default()));        // PermittedObjectsFilter
    // ... paginieren -> ids
    let fields = OwnedFields::new(owned::parse_full_perms(ctx.query.get("full_perms")), false);
    let info = OwnedInfo::load(&s.db, &models::TAG, &ids, fields, false).await?; // 1 Abfrage/Liste
    // pro Objekt: info.write_fields(&mut map, fields, false, Some(auth.user()), id, owner_id)
}
```
Detail (`retrieve`): Objekt **durch** `permitted_objects_filter` laden (sonst 404
`No Tag matches the given query.` = `AuthError::no_match("Tag")`), dann
`auth.check_object_perms(..)`.

## 1. Authentifizierung

Quelle: `paperless/settings/__init__.py` (`REST_FRAMEWORK`, `_parse_remote_user_settings`,
`AUTO_LOGIN_USERNAME`), `paperless/auth.py`, `rest_framework/authentication.py`,
`rest_framework/request.py::_authenticate`, `django/contrib/auth/__init__.py::get_user`.

Reihenfolge (`DEFAULT_AUTHENTICATION_CLASSES`), erstes Ergebnis gewinnt, eine Exception bricht ab:

1. `PaperlessRemoteUserAuthentication` – nur mit `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API`.
   Header = META-Name `PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME` (Standard `HTTP_REMOTE_USER` →
   Header `Remote-User`). `RemoteUserBackend`: unbekannte Nutzer werden angelegt
   (`get_or_create(username=...)`, `password=''`, `is_active=true`). Nur aktive Nutzer.
2. `PaperlessBasicAuthentication`: `Authorization` als Bytes gesplittet (Python `bytes.split()`),
   Schema case-insensitiv `basic`.
   * 1 Teil → `Invalid basic header. No credentials provided.`
   * > 2 Teile → `Invalid basic header. Credentials string should not contain spaces.`
   * base64 (nicht-strikt wie `base64.b64decode`: fremde Zeichen ignoriert), UTF-8 sonst
     latin-1, `split(':', 1)` fehlgeschlagen → `Invalid basic header. Credentials not correctly base64 encoded.`
   * `authenticate(username, password)`: ModelBackend (exakter Username), dann allauth-Backend
     (nur bei nicht-leerem Passwort; `username__iexact`, danach `stored.lower() == given.lower()`,
     genau ein Treffer, sonst Dummy-Hash). Inaktive Nutzer scheitern dort →
     `Invalid username/password.` (Text `User inactive or deleted.` bleibt unerreichbar).
   * Danach MFA: `mfa_authenticator`-Zeile vorhanden → `MFA required`.
   * Fehlschlag loggt wie `handle_failed_login` (`paperless.auth`, INFO).
   * Passwort korrekt aber veraltet (`must_update`) → neu gehasht und gespeichert (Django-Setter),
     **auch im zurückgegebenen `User`** (`u.password = neu`, wie `set_password` am selben Objekt);
     sonst rechnet ein späteres `get_session_auth_hash()` mit dem alten Hash (W21).
3. `TokenAuthentication`: Schema `token` (case-insensitiv).
   * 1 Teil → `Invalid token header. No credentials provided.`
   * > 2 Teile → `Invalid token header. Token string should not contain spaces.`
   * kein UTF-8 → `Invalid token header. Token string should not contain invalid characters.`
   * unbekannt → `Invalid token.`; Nutzer inaktiv → `User inactive or deleted.`
   * `token_key` im Kontext (= `request.auth`).
4. `SessionAuthentication`: Django-`request.user`:
   * `AutoLoginMiddleware` (wenn `PAPERLESS_AUTO_LOGIN_USERNAME`, außer `POST /api/token/`):
     dieser Nutzer, falls vorhanden und aktiv (`AuthMethod::AutoLogin`).
   * sonst Session: Cookie `<COOKIE_PREFIX>sessionid`; Schlüssel ≥ 8 Zeichen; Zeile in
     `django_session` mit `expire_date > now()` (DB ist maßgeblich, `cached_db`);
     `session_data` = `signing.dumps(dict, salt="django.contrib.sessions.SessionStore",
     compress=True)`: `[.]base64url(zlib(json)):b62(ts):b64url(HMAC-SHA256)`, HMAC-Schlüssel
     `sha256("django.contrib.sessions.SessionStoresigner" + SECRET_KEY)`. Ungültig → leere Session.
   * `_auth_user_backend` muss in `AUTHENTICATION_BACKENDS` stehen; Nutzer `_auth_user_id`
     laden, nur aktiv; `_auth_user_hash` = `salted_hmac("django.contrib.auth.models.AbstractBaseUser.get_session_auth_hash",
     password, SECRET_KEY, sha256).hexdigest()` konstantzeitig vergleichen. Abweichung →
     `session.flush()` (Zeile gelöscht) und anonym.
   * Nutzer gefunden → **CSRF** (Abschnitt 2), Fehler → 403 `CSRF Failed: <reason>`.
5. `AngularApiAuthenticationOverride` – nur `PAPERLESS_DEBUG`: `Referer` beginnt mit
   `http://localhost:4200/` → erster Staff-Nutzer (nach id).

### Fehlerantworten (`APIView.handle_exception`)

* `NotAuthenticated` (`Authentication credentials were not provided.`) und
  `AuthenticationFailed` → **401** mit `WWW-Authenticate` der **ersten** Klasse:
  `Basic realm="api"`, wenn `Authorization` (kleingeschrieben) mit `basic ` beginnt, sonst
  `Token` (paperless-Sonderverhalten). Im Remote-User-API-Modus ist die erste Klasse
  `RemoteUserAuthentication` ohne Challenge → **403** ohne Header.
* `permission_denied`: ohne erfolgreichen Authenticator → `NotAuthenticated`, sonst 403
  `You do not have permission to perform this action.`
* Body immer `{"detail": "..."}`.
* Ungültige Credentials scheitern auch bei `MaybeUser` (AllowAny-Views), wie DRF.

## 2. CSRF (`SessionAuthentication.enforce_csrf` → `CsrfViewMiddleware`)

Nur bei Session-/AutoLogin-Authentifizierung und Methoden außer GET/HEAD/OPTIONS/TRACE.

1. `Origin` vorhanden: akzeptiert bei `"<http|https>://" + get_host()` (Schema per `is_secure()`),
   exakt in `CSRF_TRUSTED_ORIGINS` (ohne `*`), oder Wildcard-Eintrag (`https://*.example.com`)
   mit gleichem Schema und `is_same_domain`. Sonst
   `Origin checking failed - <origin> does not match any trusted origins.`
2. Kein `Origin` und `is_secure()`: Referer-Prüfung: fehlt → `Referer checking failed - no Referer.`;
   ohne Schema/Netloc → `... Referer is malformed.`; nicht https →
   `... Referer is insecure while host is secure.`; Host in `CSRF_TRUSTED_ORIGINS`-Hosts oder
   gleich `get_host()` → ok, sonst `Referer checking failed - <url> does not match any trusted origins.`
3. Cookie `<COOKIE_PREFIX>csrftoken`: fehlt → `CSRF cookie not set.`; Länge ≠ 32/64 →
   `CSRF cookie has incorrect length.`; Zeichen ∉ `[A-Za-z0-9]` → `CSRF cookie has invalid characters.`
   64 Zeichen → entmaskieren (`_unmask_cipher_token`).
4. Token (`_check_token`): bei **POST** zuerst das Formularfeld `csrfmiddlewaretoken`
   (`request.POST`, Lücke S6, 2026-11-18); ist es leer/fehlt es, der Header `X-CSRFToken`. Header
   fehlt → `CSRF token missing.`; Format → `CSRF token from the 'X-Csrftoken' HTTP header has
   incorrect length.` / `... has invalid characters.`; falsch → `CSRF token from the 'X-Csrftoken'
   HTTP header incorrect.` Aus dem Formular dieselben Texte mit `from POST` (`_bad_token_message`).
   Vergleich konstantzeitig nach Entmaskierung.

   Formularfeld: `csrf::form_token_middleware` liest bei `POST` mit Cookie, ohne `X-CSRFToken`-Header
   und mit `application/x-www-form-urlencoded` (ganzer Körper bis 2,5 MiB =
   `DATA_UPLOAD_MAX_MEMORY_SIZE`, letzter Wert wie `QueryDict.get`) bzw. `multipart/form-data`
   (Präfix bis 1 MiB, erster passender Nicht-Datei-Teil) das Feld und legt es als
   `csrf::FormCsrfToken` in die Request-Extensions; der Körper geht unverändert weiter (gelesenes
   Präfix + Rest als Stream). Die Extraktoren reichen es als `RequestMeta.csrf_form_token` an
   `Authz::authenticate_with` → `csrf::enforce_csrf_with_form`. pl-server hängt die Middleware um
   alle Routen (`pl_server::build_app`, erledigt 2026-11-18).

Antwort: 403 `{"detail": "CSRF Failed: <reason>"}`.

Host: `get_host()` = `X-Forwarded-Host` (nur `PAPERLESS_USE_X_FORWARD_HOST`) bzw. `Host`,
`split_domain_port` + `ALLOWED_HOSTS` (`*`, `.domain`, exakt; `localhost` und Host von
`PAPERLESS_URL` werden ergänzt). `is_secure()` = `SECURE_PROXY_SSL_HEADER`
(`PAPERLESS_PROXY_SSL_HEADER`, JSON-Paar), sonst Schema der Verbindung (Rust: http).

## 3. Passwörter (`django/contrib/auth/hashers.py`)

paperless setzt kein `PASSWORD_HASHERS` → Django-Standard: `pbkdf2_sha256` (bevorzugt,
**1 000 000** Iterationen in Django 5.2, Salt 22 Zeichen aus `[a-zA-Z0-9]`), `pbkdf2_sha1`,
Argon2, BCryptSHA256, Scrypt. `argon2-cffi` und `bcrypt` sind in v3.4.2 nicht installiert.

Seit Lücke S5 (2026-11-18) prüft `hashers::verify_password` alle fünf (Quelle
django/contrib/auth/hashers.py@5.2 `Argon2PasswordHasher`, `BCryptSHA256PasswordHasher`,
`ScryptPasswordHasher`):

* `argon2$argon2id$v=19$m=…,t=…,p=…$<salt b64>$<hash b64>` (auch `argon2i`/`argon2d` und die alte
  Form ohne `v=` = Version 0x10, wie argon2-cffi), Neuberechnung mit den Parametern des Hashes
  (Crate `argon2`).
* `bcrypt_sha256$<bcrypt-Hash $2a$/$2b$/$2y$>`: bcrypt über `hexlify(sha256(password))` (Crate
  `bcrypt`). Reines `bcrypt$` ist nicht in Djangos Standardliste → falsch.
* `scrypt$<n>$<salt>$<r>$<p>$<b64(dk 64 Byte)>` wie `hashlib.scrypt` (Crate `scrypt`); `n` muss
  Zweierpotenz > 1 sein.
* Alle drei: richtiges Passwort → `must_update` (anderer Algorithmus als der bevorzugte) → beim
  Login sofort auf `pbkdf2_sha256` umgestellt (wie Djangos Setter); falsches Passwort ohne
  `harden_runtime`. Speicher-Obergrenze 1 GiB je Prüfung (sonst falsch). Neue Hashes bleiben
  immer `pbkdf2_sha256` (Django-Reihenfolge).

* Format `pbkdf2_sha256$<iter>$<salt>$<base64(dk)>`, Prüfung durch Neuberechnung und
  konstantzeitigen Vergleich des ganzen Strings.
* `must_update`: anderer Algorithmus, andere Iterationszahl oder Salt < 128 Bit; bei falschem
  Passwort `harden_runtime` (fehlende Iterationen nachrechnen).
* `!`-Präfix = unbenutzbar; unbekannte Algorithmen → falsch.
* Unbekannter Nutzer → ein Dummy-Hash (Timing).
* PBKDF2 ist teuer: immer in `spawn_blocking` (die Async-Helfer machen das).

## 4. Nutzer (`auth_user`)

`User { id, username, password, first_name, last_name, email, is_active, is_staff,
is_superuser, last_login, date_joined, groups (aufsteigend), permissions }` – eine Abfrage inkl.
Gruppen und ModelBackend-Rechten (`"app_label.codename"`, Nutzer- ∪ Gruppenrechte; Superuser:
alle). `Debug` zeigt kein Passwort. Gleichheit = gleiche id.

## 5. Modellrechte

* `User::has_perm`: aktiv **und** (Superuser **oder** Recht vorhanden); inaktiv → nichts.
* `PaperlessObjectPermissions.perms_map`: GET/OPTIONS/HEAD → `view_`, POST → `add_`,
  PUT/PATCH → `change_`, DELETE → `delete_`; andere Methode → 405 `Method "X" not allowed.`
  (DRF prüft Rechte vor dem Dispatch).
* `GlobalPermissionsByMethod`: HEAD wie GET, fehlende Methode → verweigert.

## 6. Objektrechte (guardian + paperless)

* Tabellen `guardian_userobjectpermission` / `guardian_groupobjectpermission`
  (`object_pk` TEXT, `content_type_id`, `permission_id`, `user_id`/`group_id`), Content-Type per
  Unterabfrage auf `django_content_type(app_label, model)`.
* `permitted_object_ids`: anonym → nur `owner IS NULL`; inaktiv → nichts (auch Superuser);
  Superuser → alle (des Managers, d. h. ohne gelöschte bei Soft-Delete); sonst
  `owner = user OR owner IS NULL OR CAST(id AS varchar) IN (user-grants UNION ALL group-grants)`
  – Grants gefiltert über `auth_permission.codename` + `auth_permission.content_type`
  (wie upstream, nicht über die Grant-Spalte `content_type_id`). Mit `parent_field`
  (Dokumente: `root_document_id`) zählen Eigentümer und Grants des Root-Dokuments
  (`LEFT JOIN`, `CASE`/`COALESCE`).
* `PermittedObjectsFilter`: authentifiziert+inaktiv → `FALSE`; Superuser → keine Bedingung;
  `include_granted=false` → nur eigene/unbesessene (ohne Manager-Filter, z. B. Papierkorb);
  sonst `id IN permitted_object_ids(perm_codename or view_<model>)`.
* `has_perms_owner_aware`: `owner IS NULL` oder `owner == user` oder guardian-Grant
  (inaktiv → nein, Superuser → ja).
* `has_object_permission`: ohne Owner/eigener Owner → ok; sonst guardian-Recht der Methode;
  fehlt es: bei sicherer Methode oder ohne `view_`-Grant → 404 `Not found.`, sonst 403.

## 7. Serialisierung (`OwnedObjectSerializer`)

* `full_perms` via `get_boolean` (`yes|y|1|t|true`, case-insensitiv).
* Felder: `full_perms=true` → `permissions` statt `user_can_change`/`is_shared_by_requester`;
  sonst umgekehrt; `all_fields` → alle.
* `permissions`: `{"view":{"users":[..],"groups":[..]},"change":{"users":[..],"groups":[..]}}`,
  nur **direkte** Grants (`view_<model>` bzw. `change_<model>`), ids aufsteigend, Gruppen nicht
  expandiert; zwei Abfragen pro Seite.
* `user_can_change`: kein Owner oder Owner = Nutzer → true; anonym → false; aktiver Superuser →
  true; sonst direkter `change`-Grant des Nutzers oder einer seiner Gruppen.
* `is_shared_by_requester`: Owner = Nutzer **und** irgendein Grant (Nutzer oder Gruppe, beliebiges
  Recht) auf dem Objekt.
* Schreiben:
  * `owner`: `null`/`""` → unbesessen; bool/list/dict → `Incorrect type. Expected pk value,
    received <type>.`; nicht vorhandener Nutzer → `Invalid pk "<wert>" - object does not exist.`;
    Float wird wie Django abgeschnitten.
  * Anlegen: `owner` fehlt im Request → anfragender Nutzer.
  * Ändern: Owner-Wechsel oder `set_permissions` nur für Superuser, Owner oder bei unbesessenem
    Objekt, sonst 403 `Insufficient permissions.`
  * `set_permissions`: unbekannte Schlüssel → `{"<key>": "Unknown permission action."}` (String,
    keine Liste); kein Dict → `{"non_field_errors": ["Invalid data. Expected a dictionary, but got <type>."]}`;
    `null` → `["This field may not be null."]`; `users`/`groups`: Liste von DRF-Integern
    (`"5"`, `5.0`, `"1_000"` ok; Fehler je Index `{"0": ["A valid integer is required."]}`),
    `null` = leere Liste (alle entfernen); Existenz/Duplikate →
    `["Some users in don't exist or were specified twice."]` bzw. `groups`.
  * `set_permissions_for_objects`: Aktionen in Reihenfolge view, change; `change` vergibt auch
    `view`; ohne `merge` werden direkte Grants der Aktion (nur deren Codename) für nicht mehr
    genannte Nutzer/Gruppen gelöscht; Einfügen mit `ON CONFLICT DO NOTHING`, Batches à 1000;
    fehlende Permission-Zeile → 500 (wie `Permission.DoesNotExist`).

## Testvektoren

`crates/pl-authz/src/testvectors/gen_vectors.py` (CPython, Django-5.2-Algorithmen wörtlich
nachgebaut, ohne Django-Import) erzeugt: Session-Auth-Hash, signierte Session (komprimiert),
signiertes Objekt (unkomprimiert, bitgenau), PBKDF2-SHA256/SHA1 (1000 Iterationen), CSRF-Maske.
Aufruf: `python -I gen_vectors.py <ausgabeordner>`. Gegengeprüft mit echtem Django 5.2.18
(`reference/venv`): `signing.loads`, `SessionStore().decode`/`key_salt`, `salted_hmac`,
`check_password` (beide PBKDF2-Vektoren), `make_password`-Iterationen (1 000 000),
`_unmask_cipher_token`, `split_domain_port`, `parse_cookie` – alle identisch.

## Tests

* Lücken S5–S7 (2026-11-18, `cargo test -p pl-authz --lib`): `hashers::tests::scrypt_like_django`
  (Vektoren aus Python 3.12 `hashlib.scrypt` im Django-Format), `argon2_reference_vectors`
  (phc-winner-argon2 `test.c`: argon2i v0x13/v0x10, argon2id), `bcrypt_sha256_composition`
  (OpenBSD-Vektor „U*U“ + Zusammensetzung), `csrf::tests::form_token_like_django`,
  `form_body_parsing`, `middleware_keeps_body` (Körper bleibt vollständig, auch 3-MiB-Multipart),
  `clientip::tests::*` (Parser, `is_global` wie CPython, Proxy-Liste, Logtext).

* Unit-Tests ohne DB: Signierung/Session, Hasher, CSRF (Entmaskierung, alle Fehlertexte,
  Origin/Referer), Host-Helfer, Cookie-Parser, Header-Fehler der Basic/Token-Authentifizierung,
  401/403-Regeln, Extraktor-Antwort, SQL-Form der Sichtbarkeitsbedingungen, Owner-/
  `set_permissions`-Parser.
* DB-Paritätstests (`tests/db.rs`, `#[ignore]`, nur lesend gegen die Referenz-DB
  127.0.0.1:54329): Erwartungswerte `tests/django_expected.json` wurden mit dem **upstream**-Code
  (`tests/django_expect.py`: `permitted_object_ids`, `permitted_document_ids`,
  `get_permission_grantees`, `get_shared_object_pks`, guardian `ObjectPermissionChecker`,
  `get_all_permissions`) auf den Referenzdaten berechnet. Abgedeckt: 6 Nutzer + anonym ×
  8 Modelle × view/change, Grantees/Shared aller Objekte, guardian-Objektrecht view/change/delete
  je Nutzer und Objekt. Stand: alle grün (`cargo test -p pl-authz -j 2 -- --include-ignored`).
  Nach Änderung der Referenzdaten Fixture neu erzeugen.
* O1 (2026-11-08): `owned_info_single_query_equals_separate_helpers` (kombinierter Weg von
  `OwnedInfo::load` = getrennte Helfer, inkl. unbekannter pk und leerer Liste),
  `superuser_has_all_permissions` (Superuser-Rechte = alle `app_label.codename` ohne SQL-DISTINCT).

## Leistung (O1, 2026-11-08) – gleiche Ergebnisse, weniger Datenbankarbeit

Jede authentifizierte API-Anfrage führt genau **eine** Nutzerabfrage aus (`USER_SELECT`, Token/
Session/Basic). Kein Cache für Nutzer/Rechte: Django kann parallel Rechte ändern, jede Anfrage
sieht den aktuellen Stand wie upstream.

* `USER_SELECT`: Superuser bekommen ein einfaches `ARRAY` aller Rechte (vorher: `IN`-Unterabfrage
  je Rechtezeile, `OR u.is_superuser`); kein `DISTINCT` mehr – die Liste landet in einem
  `HashSet`, das Doppelte genauso entfernt.
* `get_permission_grantees`: Nutzer- und Gruppen-Grants in **einer** Abfrage (`UNION ALL`,
  sortiert nach Art, dann id – wie vorher je Tabelle), pks als **ein** `text[]`-Parameter
  (`object_pk = ANY($3)`) statt ein Parameter je pk: eine vorbereitete Anweisung für jede
  Seitengröße statt einer je Anzahl (Statement-Cache).
* `get_shared_object_pks` gleich gebaut; `OwnedInfo::load` mit Grantees **und** Shared-pks
  (Dokumentlisten) liest beides aus denselben Grant-Zeilen: 1 statt 3 Abfragen.

## Geräte-Token (`device`, P3b)

Spezifikation docs/SYNC.md §8. Tabelle `paperrust_device_token(id, uuid, user_id → auth_user ON
DELETE CASCADE, name, token_hash char(64) UNIQUE, created, last_used, revoked_at)`, Migration
`pl_authz.0001_paperrust_device_token` (`crates/pl-authz/migrations/`, `pl_authz::device::migrations`,
expand-only, idempotent; Rückbau `DROP TABLE paperrust_device_token`).

- Token `prt_` + 43 Zeichen base62 aus 32 OS-Zufallsbytes; gespeichert nur `sha256(token)` (hex).
- `device::create/list/revoke/lookup_user_id`; `lookup_user_id` prüft Form (ohne DB bei falscher
  Form), Hash, `revoked_at IS NULL` und aktualisiert `last_used` höchstens einmal pro Minute in
  derselben Anweisung. Kein Cache → Widerruf wirkt bei der nächsten Anfrage.
- Fehlt die Tabelle, ist jedes Geräte-Token ungültig (401), die v2-Endpunkte antworten 503.
- Ausgabe/Liste/Widerruf: `/rs/api/v2/auth/devices*` (pl-api-v2, Drossel Scope `login` wie
  `/api/token/`, MFA-Pflicht mit TOTP-`code`).

## L5: Freigabe-Index `paperrust_grant` (ARCHITECTURE-V2 §5, 2026-11-10)

Ziel: Rechte-**Lesewege** (Sichtbarkeit, `can_view`/`can_change`, Listenzählungen, Objektlisten mit
`document_count`, Auswahl, Zeitstrahl, `/ask`, Sync-Feed) lesen einen nativen, schnellen Index statt
guardian + `auth_permission` + Text-Vergleich. **Kompatibilität bleibt:** die Django-/guardian-Tabellen
sind weiter die Wahrheit (v1 `set_permissions`, Export/Rückweg zu paperless-ngx; der Mischbetrieb mit
Django-Schreibwegen ist seit 2026-11-14 entfallen); der Index ist **abgeleitet** und wird von Triggern in derselben Transaktion gepflegt.

### Upstream-Semantik (geprüft, `documents/permissions.py`, guardian 3.5.2, Django 5.2)

* Objekt-Sichtbarkeit `permitted_object_ids(user, model, perm)`: anonym → nur `owner IS NULL`;
  inaktiv → nichts (auch Superuser); aktiver Superuser → alles (des Managers); sonst
  `owner = user OR owner IS NULL OR str(key) ∈ (Nutzer-Grants ∪ Gruppen-Grants)` mit Grants gefiltert
  über `permission.codename` + `permission.content_type` (nicht über die `content_type_id` der
  guardian-Zeile). Dokumente: `key = COALESCE(root_document_id, id)`, Eigentümer des Wurzeldokuments.
* **Modellrechte** (`view_document` usw. aus `auth_user_user_permissions`/`auth_group_permissions`)
  spielen für die Objekt-Sichtbarkeit **keine** Rolle: sie entscheiden nur, ob ein Endpunkt
  überhaupt erlaubt ist (`require_model_perms`, `GlobalPermissionsByMethod`). Ein Nutzer mit
  globalem `view_document` sieht trotzdem nur eigene/besitzerlose/freigegebene Dokumente. „Sieht
  alles“ heißt genau: `is_active AND is_superuser`.
* Einzelprüfung `ObjectPermissionChecker.has_perm` (über `has_object_permission`,
  `has_perms_owner_aware`): inaktiv → nein, Superuser → ja, sonst Grant mit Codename **und** Zeilen-
  `content_type_id` = Modell (guardian filtert hier beide).
* `owner = None` ⇒ für alle sichtbar und änderbar (auch anonym bei `has_perms_owner_aware`).

### Datenmodell (Migration `pl_authz.0002_paperrust_grant`, expand-only)

```sql
paperrust_grant(perm_id int, subject bigint, object_id bigint, ct int,
                PRIMARY KEY (perm_id, subject, object_id))          -- Listen: Index-Only-Scan
INDEX paperrust_grant_object (object_id, ct) INCLUDE (perm_id, subject)  -- Einzelprüfung/Grantees/geteilt
paperrust_grant_meta(key, value)  -- enabled, built
```

* je guardian-Zeile mit **kanonischer** Ganzzahl-`object_pk` (`^(0|-?[1-9][0-9]{0,17})$`) genau
  eine Zeile: `subject = user_id` (Nutzer) bzw. `-group_id` (Gruppe), `object_id = object_pk::bigint`,
  `perm_id = permission_id`, `ct = content_type_id` der guardian-Zeile. Nicht kanonische `object_pk`
  fehlen bewusst: `CAST(id AS varchar)` kann ihnen nie gleich sein.
* **Nicht** im Index (Begründung): `owner_id` steht auf der Objektzeile und ist dort bereits über
  den L1-Teilindex `paperrust_doc_live_created … INCLUDE (owner_id)` abgedeckt – eine Kopie
  bräuchte Trigger auf jeder Dokumentänderung (Schreibkosten bei 1 Mio. Dokumenten) und machte die
  Sichtbarkeit langsamer (ein „öffentlich“-Subjekt mit 1 Mio. Zeilen statt `owner_id IS NULL`).
  Superuser/aktiv kommen aus der einen Nutzerabfrage je Anfrage (`User`), die Gruppen je Abfrage
  aus `auth_user_groups` (Django kennt keine verschachtelten Gruppen: die Tabelle *ist* die
  effektive Nutzer→Gruppen-Menge, Index `(user_id)`); Modellrechte beeinflussen die Sichtbarkeit
  nicht (s. o.). Damit braucht nur guardian Trigger; Gruppenwechsel, Eigentümerwechsel und
  Superuser-/Aktiv-Wechsel wirken sofort ohne Indexpflege. Codename/Modell eines Rechts werden zur
  Abfragezeit über `perm_id` aufgelöst (Umbenennung von Rechten ohne Pflege).
* Trigger: `AFTER INSERT/UPDATE/DELETE … FOR EACH STATEMENT` mit Transition-Tables und
  `AFTER TRUNCATE` auf beiden guardian-Tabellen (8 Stück, statisches SQL wie pl-sync P2). Rollback
  nimmt die Indexänderung mit zurück.
* Werkzeuge: `paperrust manage grants status|check|rebuild|enable|disable [--purge]`;
  SQL `paperrust_grant_check()` (fehlend/überzählig gegen `paperrust_grant_expected()`),
  `paperrust_grant_rebuild()` (sperrt guardian-Schreiber `SHARE`, eine Transaktion). Der
  **Importer** übernimmt guardian-Zeilen ohne eigenen Schritt: `document_importer`/`pl-dbmigrate`
  schreiben guardian, die Trigger füllen den Index; wurde mit abgeschalteten Triggern importiert,
  `manage grants rebuild`.

### Lesewege mit `grants` (gleiche Menge, Abbildung)

| Stelle | legacy | grants |
|---|---|---|
| `objects::permitted_object_ids` (Listen aller Arten, `permitted_document_ids`, `restrict_to_*`, `PermittedObjectsFilter`, Objektlisten mit `document_count`, Auswahl `all=true`, Sync-Feed andere Arten, Zeitstrahl, `/ask`/MCP) | `CAST(key AS varchar) IN (guardian user ∪ group)` | `key IN (SELECT object_id FROM paperrust_grant WHERE perm_id = ANY(ARRAY(<id von codename+Modell>)) AND subject = ANY(ARRAY(-gruppen) ‖ uid))` |
| `docsql::Visibility::root_row` (Dokumentliste/Zählung/Seite, `visible_ids_sql`, Duplikate) | wie oben auf `D.id` | wie oben auf `D.id` (über `objects::granted_key_cond`) |
| `perms::guardian_has_perm` (`check_object_perms`, `has_perms_owner_aware`) | `EXISTS` je guardian-Tabelle | `EXISTS (… object_id = $id AND ct = Modell AND perm … AND subject = ANY(…))` |
| `owned::OwnedInfo`/`get_permission_grantees`/`get_shared_object_pks` | `UNION ALL` guardian | eine Abfrage auf `paperrust_grant ⋈ auth_permission` (Art aus dem Vorzeichen) |
| `shared_by__id` (`pl_docfilter::perms::document_is_shared`) | `EXISTS` guardian, Zeilen-ct = Dokument | `EXISTS (… object_id = D.id AND ct = Dokument)` |
| pl-store v2 `perm::grants` (Access je Seite) | guardian | Index |
| pl-search (globale Suche, Volltext-Sichtbarkeit, `more_like_id`) | eigenes SQL | eigenes SQL auf dem Index |
| pl-mail (Konten/Regeln: Sichtbarkeit, Checker, Grantees, `user_can_change`) | eigenes SQL | eigenes SQL auf dem Index |

Bleiben auf guardian (Schreib- bzw. Hintergrundwege, keine Leseabfragen eines Nutzers): v1
`set_permissions_for_objects` (schreibt guardian, Index folgt per Trigger), Workflow-/Consumer-
Rechtezuweisung, `pl-indexer` (Grantees für den Suchindex), Löschkaskaden von Nutzern/Gruppen.

Gleichwertigkeit: (1) `perm_id = ANY(ids von (codename, Modell-ct))` ≡ `permission__codename,
permission__content_type`; (2) `subject = ANY(-{g | (uid,g) ∈ auth_user_groups} ∪ {uid})` ≡
`user = uid ∪ group_id IN user.groups`; (3) `key = object_id` ⇔ `CAST(key AS varchar) = object_pk`
für kanonische `object_pk`, für nicht kanonische ist beides falsch; (4) Checker zusätzlich `ct`.
Eigentümer-, Superuser-, Aktiv- und Papierkorbregeln sind unverändert derselbe Code.

### Umschalter und Bereitschaft

`PAPERRUST_PERMISSION_ENGINE=legacy|grants` (Klasse A, live; pl-settings, docs/SETTINGS.md 1.10). *(Umschalter seit Datenmodell-Stufe 7 entfernt: mit bereitem Index immer `grants`, ohne – nur auf Datenbanken ohne PaperRust-Migrationen – guardian direkt, docs/DATAMODEL-V2.md §6)*
`grants` wirkt nur, wenn der Index **bereit** ist (Tabelle da, `enabled`, `built`, 8 Trigger aktiv);
sonst still `legacy`. Der Server prüft das alle 5 s im Hintergrund (`grants::spawn_refresher`
aus `Authz::new`); ein Verbindungsfehler ändert den Zustand nicht. `manage grants disable` setzt
nur `enabled = false` (Trigger pflegen weiter, Rückkehr ohne Rebuild); `--purge` wartet die
Prüfspanne ab und entfernt dann alles.

### Tests

* Unit: `grants::tests` (Umschalter, SQL-Form), `docsql::tests::visibility_on_outer_row` (beide Wege).
* **upstream-Erwartungen** `crates/pl-authz/tests/db.rs` (mit dem upstream-Code berechnet) zusätzlich mit
  `PL_AUTHZ_ENGINE=grants` auf einer DB-Kopie mit Migration: 6/6 grün (wie `legacy`).
* **Differenz- und Triggertest** `crates/pl-server/tests/grants_diff.rs` (`#[ignore]`, nur DB-Kopie,
  `PL_L5_DB`): Phase A vergleicht `legacy` gegen `grants` über alle Nutzer + anonym × 12 Modelle
  mit Eigentümer × view/change/delete (Dokumente mit/ohne Root-Regel, mit/ohne Papierkorb),
  docsql-Sichtbarkeit, `shared_by`, pl-search (Sichtbarkeit aller Dokumente, `more_like_id`), pl-mail
  (Sichtbarkeit, Checker, Grantees, `user_can_change`), guardian-Checker je Objekt (alle Objekte mit
  guardian-Zeile + Stichprobe) und `OwnedInfo`-Grantees/geteilt. Phase B: zufällige Änderungen
  (v1 `set_permissions` mit/ohne `merge`, auch zurückgerollt; guardian-Zeilen löschen/ändern inkl.
  nicht kanonischer `object_pk` und fremdem Recht; Zeilen mit abweichendem Content-Type;
  Gruppenmitgliedschaft; Eigentümerwechsel inkl. NULL; Superuser-/Aktiv-Wechsel; Versionen
  an-/abhängen; Papierkorb; Gruppen-Grants leeren; `TRUNCATE` zurückgerollt und echt) – nach
  **jeder** Runde `paperrust_grant_check()` (Trigger-Stand = Neuberechnung), alle N Runden wieder
  Phase A; am Ende Trigger-Stand = `rebuild()` zeilengleich.

### Ergebnisse (2026-11-10)

| Prüfung | Ergebnis |
|---|---|
| Differenz/Trigger `paperless_snapshot`-Kopie (6 Nutzer, 3 Gruppen) | 1 000 Runden, 2 486 Änderungen, 1 001 Index-Prüfungen, 101 volle Vergleiche: 30 401 Mengenvergleiche (641 646 Zeilen), 440 208 Checker-, 24 456 Grantee-, 14 746 Mail-, 62 430 Suchvergleiche – **0 verschieden** (`results/l5/diff_snap.txt`) |
| Differenz/Trigger 100k-Kopie (`pl_pgvalidate`, 62 633 guardian-Zeilen) | 300 Runden, 726 Änderungen, 301 Index-Prüfungen, 13 volle Vergleiche: 3 913 Mengenvergleiche (36,3 Mio. Zeilen), 123 462 Checker-Vergleiche – **0 verschieden** (`results/l5/diff_100k.txt`) |
| upstream-Erwartungen `tests/db.rs` mit `grants` | 6/6 |
| Paritätslauf lesend (eigene W2-Kopie `wl5`, A Django :8150, B PaperRust :8151, Standard `grants`, Index bereit) | **2 024 verglichen, 2 001 identisch, 0 verschieden**, 23 akzeptiert, 580 übersprungen, 0 Lecks (`results/parity/l5/2026-11-10_175550.md`) |
| Paritätslauf schreibend (`wl5`, `--write-only`) | **565 verglichen, 557 identisch, 0 verschieden**, 8 akzeptiert, 3 übersprungen, 2 Fehler = Reset-Zeitüberschreitung der Testumgebung (paralleler 1-Mio.-Kopiervorgang); Nachlauf der beiden Fälle 3/3 identisch (`results/parity/l5-write/2026-11-10_180613.md`, `…_190746.md`, `…_191454.md`) |
| `manage grants` | `check` erkennt absichtlich erzeugte Abweichung (Trigger aus, 3 Zeilen gelöscht → 3 extra, Exit 1), `rebuild` behebt sie, `disable`/`enable`, `disable --purge` (entfernt alle 14 Funktionen, 8 Trigger, Tabellen) |

Erstaufbau: 100k-Kopie 62 633 Zeilen in < 1 s (10 MB); 1-Mio.-Kopie 596 180 Zeilen in 8 s
(`rebuild` 9,7 s, 105 MB).

### Messung vorher/nachher (SQL, gleiche Indizes inkl. L1-Teilindex, `VACUUM ANALYZE`, warm)

Test `grants_bench` (gleiche Datei), Minimum aus 5 (100k) bzw. 3 (1 Mio.) Läufen, Summe über die
eingeschränkten Nutzer (100k: alice, bob, carol; 1 Mio.: user120, user018, user003 – je 1–2
Gruppen, ~1 300 direkte Grants); Rohdaten `results/l5/bench_*.csv`.

| Abfrage | 100k legacy → grants (ms) | 1 Mio. legacy → grants (ms) |
|---|---:|---:|
| Dokumentliste `COUNT` | 85 → 70 (−18 %) | 1 294 → 841 (**−35 %**) |
| Seite 1 (`LIMIT 50`) | 16,6 → 13,5 (−19 %) | 22,0 → 15,8 (−28 %) |
| Seite `OFFSET 5000` | 24,2 → 19,6 (−19 %) | 54,1 → 39,3 (−27 %) |
| Auswahl `all=true` | 81 → 70 (−13 %) | 842 → 688 (−18 %) |
| `is_tagged=1` / `title_content` | ±4 % | −9 % / −10 % |
| `OwnedInfo` (Grantees + geteilt, 10 Seiten × 50) | 234 → 122 (−48 %) | 3 067 → 36 (**−99 %**) |
| Einzelprüfung 500 × `change_document` | 1 377 → 1 074 (−22 %) | 1 433 → 988 (−31 %) |
| `permitted_document_ids` (Root-Join) | 949 → 878 (−8 %) | 6 346 → 6 018 (−5 %) |
| `document_count` Tags/Korrespondenten/Typen | ±8 % | −4 … +3 % |

Superuser (admin) unverändert (kein Rechte-Teil), außer `OwnedInfo` 1 131 → 176 ms (1 Mio.).
Die `document_count`-Abfragen werden vom Root-Self-Join in `permitted_document_ids` (über alle
Dokumente) bestimmt, nicht von den Grants – Folgearbeit (z. B. Wurzel-Sichtbarkeit direkt auf
`documents_document` wie docsql).

Hinweis Planer: direkt nach einem Massen-Insert ohne `VACUUM` (Sichtbarkeitskarte leer) plant
PostgreSQL für den Index statt Index-Only-Scan einen Seq Scan (100k: Seite 6 → 21 ms). Deshalb
`VACUUM (ANALYZE) paperrust_grant` nach `rebuild`/`enable`; nach der Migration übernimmt das
Autovacuum (Insert-Schwelle).

### Stand

**Standard `PAPERRUST_PERMISSION_ENGINE=grants`** (seit diesem Stand; ohne bereiten Index gilt
`legacy`). Rückfall: `PAPERRUST_PERMISSION_ENGINE=legacy` (live) oder `manage grants disable`. *(Umschalter seit Datenmodell-Stufe 7 entfernt: mit bereitem Index immer `grants`, ohne – nur auf Datenbanken ohne PaperRust-Migrationen – guardian direkt, docs/DATAMODEL-V2.md §6)*


## Known differences

1. **Übersetzungen**: Fehlertexte nur Englisch. Django übersetzt DRF-/paperless-Texte gemäß
   `Accept-Language`/Sprach-Cookie (`LocaleMiddleware`). → CORE-REQUEST i18n.
2. **`csrfmiddlewaretoken`-Formularfeld** (S6): wird gelesen (pl-server hängt
   `csrf::form_token_middleware` ein). Abweichungen: steht **auch** der Header
   da, gilt der Header (Django: Formularfeld zuerst) – die Middleware liest den Körper dann nicht;
   bei `multipart/form-data` nur die ersten 1 MiB (HTML-Formulare setzen das versteckte Feld
   vorn; Django parst den ganzen Körper).
3. **Session-Nebenwirkungen**: Rust erzeugt/rotiert keine Sessions (Remote-User-Middleware,
   AutoLogin `auth.login`, CSRF-Rotation), schreibt keinen Django-Cache. Bei Hash-Abweichung
   wird die DB-Zeile gelöscht (wie `flush`), der Cache-Eintrag nicht. `SessionState` liefert dem
   Server die Information für `Vary: Cookie`/Cookie-Löschung (`delete_cookie_header`).
4. ~~Remote-User-Middleware nicht portiert~~ – erledigt: Nicht-API-Pfade mit Session-Persistenz
   (`PersistentRemoteUserMiddleware`, An- und Abmelden) in pl-accounts `auth.rs`; der API-Pfad
   (`PaperlessRemoteUserAuthentication`) hier.
5. **Argon2/BCryptSHA256-Hashes** (S5, bewusst): PaperRust prüft sie und stellt sie beim Login auf
   pbkdf2_sha256 um; paperless 3.4.2 antwortet mangels `argon2-cffi`/`bcrypt` mit 500. Nutzen:
   Umstieg aus Datenbanken anderer Django-Werkzeuge. Scrypt wie Django.
6. **Fehlerhafte Eingaben, die in Django 500 auslösen**, ergeben hier eine definierte Antwort:
   ungültiges `_auth_user_id` → anonym; kaputter
   pbkdf2-String → falsch; Session über das guardian-Backend → anonym; DEBUG-Override ohne
   Staff-Nutzer → authentifiziert ohne Nutzer (403 bei `IsAuthenticated`).
7. **Anonyme guardian-Grants** (guardians `AnonymousUser`-Datensatz) werden nicht ausgewertet;
   `has_perms_owner_aware(None, ..)` ist nur bei Objekten ohne Owner wahr.
8. **Login-Fehlerlog** (S7): Client-IP wie `IpWare(proxy_list=TRUSTED_PROXIES).get_client_ip`
   (`clientip.rs`, `is_global`/`is_loopback` wie CPython 3.12.10): „from IP `…`.“ /
   „from private IP `…`.“ / „Unable to determine IP address.“ Gelesen werden nur META-Schlüssel, die
   Django hat (`HTTP_X_FORWARDED_FOR`, `HTTP_CLIENT_IP`, `HTTP_X_REAL_IP`, `HTTP_X_FORWARDED`,
   `HTTP_X_CLUSTER_CLIENT_IP`, `HTTP_FORWARDED_FOR`, `HTTP_FORWARDED`, `HTTP_CF_CONNECTING_IP`,
   `REMOTE_ADDR` aus `ConnectInfo`); IPv6 mit Zonen-Index (`%eth0`) wird übergangen.
   `Authz::authenticate_credentials` (ohne Anfrage-Daten) loggt ohne IP; `/api/token/` (pl-auth-api)
   und `/rs/api/v2/devices` (pl-api-v2) nutzen `authenticate_credentials_from` mit Kopfzeilen und
   TCP-Gegenstelle.
9. **`user_can_change`** nutzt auch bei Einzelobjekten den Batch-Weg; Unterschied zu guardian nur
   für inaktive Nutzer, die sich nicht authentifizieren können.
10. **Cookie-/Header-Zusammenführung** wie Django-ASGI (`,` bzw. `; `), Dekodierung latin-1.
11. Session-Ablauf: `expire_date > now()` mit der Uhr von PostgreSQL statt Python.
12. `py_b64decode` der Session-Payload ist strikt (Session-Daten sind von Django signiert, daher
    ohne Wirkung).
13. **Geräte-Token (PaperRust-Erweiterung, P3b):** `Authorization: Bearer prt_…` authentifiziert
    zusätzlich nach `TokenAuthentication` (Methode `AuthMethod::DeviceToken`, im Audit-Log wie
    Token: Akteur NULL). Django kennt das Schema nicht (galt im früheren Mischbetrieb nur auf Rust-Routen;
    seit 2026-11-14 gibt es nur noch Rust-Routen). Andere `Bearer`-Werte
    bleiben wie in DRF unbeachtet; ein `Bearer prt_…`, das nicht (mehr) gültig ist, ergibt 401
    `Invalid token.` (bzw. `User inactive or deleted.`).
14. **Drosselung von HTTP-Basic (PaperRust-Erweiterung, Sicherheitsprüfung 2026-11-18, Befund S-2):**
    upstream begrenzt `PaperlessBasicAuthentication` nicht, die Anmeldeseite aber schon (allauth
    `login_failed` 10/m/ip, 5/300s/key) – über Basic ließ sich das Passwort beliebig oft raten.
    `basic_throttle`: höchstens 10 Fehlversuche je Client-IP in 60 s und 5 je Benutzername in 300 s;
    danach 401 `Too many failed login attempts.` ohne Passwortprüfung (auch für das richtige
    Passwort, bis das Fenster abläuft). Zähler je Prozess im Speicher (max. 50 000 Schlüssel).
    Tests: `basic_throttle::tests`, `tests/security/test_auth_flows.py::ZThrottling::test_basic_auth_bruteforce`.
    Ergänzungen Sicherheitsprüfung 2026-11-19 F7:
    - **Speichergrenze:** Bei 50 000 Schlüsseln werden zuerst abgelaufene Einträge verworfen
      (je Schlüssel eigenes Fenster), reicht das nicht, die ältesten 10 % (nach letztem
      Fehlversuch). Nie mehr alle Zähler auf einmal – sonst setzte eine Flut neuer Namen die
      Sperre eines angegriffenen Kontos zurück. Tests `key_flood_keeps_recent_lock`,
      `expired_ip_keys_go_first`.
    - **Client-IP der Drossel** (`clientip::throttle_client_ip`): ohne `PAPERLESS_TRUSTED_PROXIES`
      immer die TCP-Gegenstelle (Kopfzeilen wirkungslos); mit Liste nur dann `X-Forwarded-For`,
      wenn die Gegenstelle selbst ein vertrauter Proxy ist – und dort die rechteste Adresse, die
      kein vertrauter Proxy ist. Einträge der Liste: Einzeladresse, CIDR (`10.0.0.0/8`) oder wie
      ipware ein Text-Präfix (`10.0.0.`). Hinter einem Proxy ohne Liste teilen sich alle Clients
      den IP-Zähler der Proxy-Adresse (sicherer Fall; die Grenze je Benutzername gilt weiter). Das
      Login-Fehlerlog nutzt weiter `client_ip` (ipware-Parität, Known differences 8). Test
      `clientip::tests::throttle_ip_ignores_spoofed_forwarded_for`.
    - **Konto-Sperre ist gewollt:** Die Grenze je Benutzername (5 in 300 s) gilt unabhängig von der
      IP. Wer einen Benutzernamen kennt, kann damit dessen Basic-Anmeldung für bis zu 5 Minuten
      sperren (auch mit richtigem Passwort). Das ist dieselbe Abwägung wie allauths
      `login_failed`-Grenze je Konto auf der Anmeldeseite und bewusst so: Schutz vor verteiltem
      Passwort-Raten geht vor. Sitzung (Anmeldeseite), DRF-Token und Geräte-Token sind von dieser
      Basic-Sperre nicht betroffen.
15. **Gleich viele Hash-Läufe bei Basic/`/api/token/`** (Sicherheitsprüfung 2026-11-19 F5):
    `authenticate_credentials_from` rechnet im allauth-Schritt immer einen Hash – trifft er
    dieselbe Zeile wie ModelBackend, einen Dummy-Hash statt nichts. So brauchen bekannte und
    unbekannte Namen mit falschem Passwort je 2 Hash-Läufe (Antwortzeit verrät den Namen nicht),
    wie der Formularweg (`pl-accounts/src/auth.rs`, der dieselbe Zeile zweimal prüft). Ein
    Rehash beim zweiten Mal entfällt weiter. Test `authn::tests::same_hash_runs_for_known_and_unknown_names`.
16. **Geräte-Token-Widerruf nach Passwortwechsel** (Sicherheitsprüfung 2026-11-19 F13):
    `device::revoke_all_for_user(db, user_id)` setzt `revoked_at` für alle aktiven Geräte des
    Nutzers (fehlende Tabelle → 0). Aufgerufen von pl-accounts (Passwort-Reset) und pl-auth-api
    (`PATCH /api/profile/`, `PUT|PATCH /api/users/{id}/` mit neuem Passwort). DB-Test
    `tests/device_revoke.rs` (`#[ignore]`, Wegwerf-DB auf der Referenz-PostgreSQL).

## CORE-REQUEST

1. `sha1 = "0.10"` in `[workspace.dependencies]` (pbkdf2_sha1-Hasher); bis dahin
   `sha1 = "=0.10.6"` in `crates/pl-authz/Cargo.toml`.
2. `[profile.dev.package.sha2]`, `[profile.dev.package.pbkdf2]`, `[profile.dev.package.hmac]`
   mit `opt-level = 3`: PBKDF2 mit 1 000 000 Iterationen dauert im unoptimierten Debug-Build
   viele Sekunden pro Login.
3. i18n der DRF-Fehlertexte (siehe Known differences 1).
5. **P3b:** `pl_authz::device::migrations::ALL` in `pl_cli::migrate::paperrust_migrations()`
   aufnehmen (`v.extend_from_slice(pl_authz::device::migrations::ALL);`), damit
   `paperrust manage migrate` die Tabelle anlegt (bis dahin nur über `migrations::apply`).
4. `AppState` könnte `Authz` direkt halten (Typ-erased oder pl-core definiert den Trait) –
   derzeit über `axum::Extension(Authz)` gelöst (Server-Agent).
6. **S6 (2026-11-18, erledigt):** pl-server hängt vor den API-Routen
   `axum::middleware::from_fn(pl_authz::csrf::form_token_middleware)` ein (außen um die Routen,
   damit die Extraktoren die Extension sehen). Ohne das bleibt das Formularfeld wirkungslos.
7. **S7 (2026-11-18, erledigt):** pl-auth-api (`/api/token/`, `token.rs`) und pl-api-v2 (`devices.rs`) rufen
   `authz.authenticate_credentials_from(&username, &password, &headers, &pl_authz::RequestMeta {
   remote_addr: <ConnectInfo>, csrf_form_token: None })` auf, damit auch dort die Client-IP im Log
   steht.
8. **S5 (2026-11-18, erledigt: in `[workspace.dependencies]`):** `argon2 = "0.5.3"` (ohne Standard-Features, `alloc`), `bcrypt = "0.17.1"`,
   `scrypt = "0.11.0"` (ohne Standard-Features) in `[workspace.dependencies]`; bis dahin mit fester
   Version in `crates/pl-authz/Cargo.toml` (alle aus dem lokalen Registry-Cache, rein Rust).

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **00096e025 (#14735) – `ALLOWED_HOSTS`-Standard.** `AuthConfig::from_env` nutzt
  `pl_core::config::initial_allowed_hosts` / `allowed_host_from_url_hostname` (Regeln im Vertrag
  von pl-core). Fehlertext jetzt „has no valid hostname“. Test:
  `config::tests::url_host_normalised_like_upstream_00096e025`.
