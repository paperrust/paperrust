# Vertrag `pl-mail`

Quelle: paperless-ngx v3.4.2 (`upstream/`), DRF 3.16.1, django-filter 26.2, imap-tools 1.15.1,
httpx-oauth 0.17.0, Django 5.2, Python 3.12 imaplib. Alle Pfade unter `upstream/src/`.

| Datei | Inhalt |
|---|---|
| `paperless_mail/views.py` | `MailAccountViewSet`, `MailRuleViewSet`, `ProcessedMailViewSet`, `OauthCallbackView`, `DeleteProcessedMailPermissions` |
| `paperless_mail/serialisers.py` | `ObfuscatedPasswordField`, `MailAccountSerializer`, `AccountField`, `MailRuleSerializer`, `ProcessedMailSerializer` |
| `paperless_mail/models.py` | `MailAccount`, `MailRule`, `ProcessedMail` (+ Choices, UniqueConstraints) |
| `paperless_mail/filters.py` | `ProcessedMailFilterSet` |
| `paperless_mail/mail.py` | `get_mailbox`, `mailbox_login`, `Pinned*MailBox`, `MailError` |
| `paperless_mail/oauth.py` | `PaperlessMailOAuth2Manager` |
| `paperless_mail/tasks.py` | `process_mail_accounts` (Celery) |
| `paperless/network.py` | `is_public_ip`, `resolve_hostname_ips` |
| `documents/serialisers.py` | `OwnedObjectSerializer`, `SetPermissionsMixin`, `SetPermissionsSerializer` |
| `documents/permissions.py` | `PaperlessObjectPermissions`, `permitted_object_ids`, `has_perms_owner_aware`, `restrict_queryset_to_visible`, `set_permissions_for_objects`, `get_permission_grantees` |
| `documents/filters.py` | `PermittedObjectsFilter` |
| `documents/views.py` | `PassUserMixin` (`full_perms`) |
| `documents/signals/handlers.py` | `before_task_publish_handler` (PaperlessTask-Zeile) |
| `paperless/views.py` | `StandardPagination` |
| Tests | `paperless_mail/tests/test_api.py`, `test_mail_oauth.py` |

Tabellen (Schema = Django-Migrationen `paperless_mail/migrations/0001…0004`):
`paperless_mail_mailaccount`, `paperless_mail_mailrule`, `paperless_mail_mailrule_assign_tags`
(`mailrule_id`, `tag_id`), `paperless_mail_processedmail`. PK überall `integer` (AutoField).
Auswahl-Felder sind `smallint` mit CHECK `>= 0`, `imap_port` ist `integer` mit CHECK `>= 0`,
`order` ist `smallint` ohne CHECK. Keine DB-seitigen ON-DELETE-Kaskaden (Django kaskadiert in Python).
Keine Signale, kein Audit-Log für Mail-Modelle; keine Transaktion um den Request (kein
`ATOMIC_REQUESTS`) – Schritte werden einzeln geschrieben.

## Gemeinsames

### Reihenfolge wie DRF `initial()`
Version (`Accept`, 406 `{"detail":"Invalid version in \"Accept\" header."}`) → Authentifizierung
(pl-authz: 401 `Authentication credentials were not provided.` mit `WWW-Authenticate`) →
globale Rechte → `get_object` → Body parsen → Serializer.

### Globale Rechte
- `PaperlessObjectPermissions.has_permission` (DjangoModelPermissions): Methode → Recht
  `GET/HEAD/OPTIONS → view_<model>`, `POST → add_<model>`, `PUT/PATCH → change_<model>`,
  `DELETE → delete_<model>` (App-Label `paperless_mail`). Fehlt → **403**
  `{"detail":"You do not have permission to perform this action."}`.
  ModelBackend: aktiver Superuser hat alles; sonst Nutzer- ∪ Gruppenrechte; inaktiv nichts.
- Objekt-Prüfung (`get_object`): `owner IS NULL` oder `owner == user` → erlaubt; sonst guardian-
  Objektrecht für die Methode (aktiver Superuser immer). Fehlt es: GET/HEAD/OPTIONS → 404; sonst
  ohne `view_<model>`-Objektrecht → 404, mit → 403 (Text wie oben).
- Guardian: `guardian_userobjectpermission` / `guardian_groupobjectpermission`
  (`object_pk` Text, Gruppen über `auth_user_groups`), `auth_permission` mit
  `content_type` = `django_content_type(app_label='paperless_mail', model=…)`.

### Sichtbarkeit (`PermittedObjectsFilter`, `permitted_object_ids`)
Inaktiv → nichts; Superuser → alles; sonst `owner_id = user OR owner_id IS NULL OR CAST(id AS text)
IN (User-Grants ∪ Gruppen-Grants für view_<model>)`.
Detail: URL-pk, den Python-`int()` nicht parst → 404 `{"detail":"Not found."}`; parsbar, aber nicht
vorhanden/nicht sichtbar → 404 `{"detail":"No <MailAccount|MailRule|ProcessedMail> matches the given query."}`.

### Owner-Felder (`OwnedObjectSerializer`)
- `owner`: int|null; Schreiben: PK auf `auth_user`, `allow_null`, nicht Pflicht.
- `user_can_change` (Standard) **oder** `permissions` (bei `?full_perms=` `yes|y|1|t|true`,
  case-insensitiv) an der Position laut Serializer. `user_can_change`: owner NULL/gleich → true;
  aktiver Superuser → true; sonst guardian `change_<model>` (direkt oder Gruppe).
  `permissions`: `{"view":{"users":[…],"groups":[…]},"change":{…}}` (nur direkte Grants, IDs
  aufsteigend).
- `set_permissions` (write-only): Objekt mit `view`/`change`, je `users`/`groups` (Liste int|null).
  Unbekannter Schlüssel → `{"set_permissions":{"<key>":"Unknown permission action."}}` (String);
  Nicht-Objekt → `{"set_permissions":{"non_field_errors":["Invalid data. Expected a dictionary, but got <pytype>."]}}`;
  Element kein int → `{"set_permissions":{"view":{"users":{"0":["A valid integer is required."]}}}}`;
  fehlende/doppelte IDs → `{"set_permissions":["Some users in don't exist or were specified twice."]}`
  bzw. `…groups…` (Reihenfolge view.users, view.groups, change.users, change.groups).
  Wirkung (merge=false): je angegebenem Schlüssel direkte Grants des Codenames für nicht genannte IDs
  löschen, genannte erhalten den Codename (bei `change` zusätzlich `view_<model>`), Konflikte ignoriert.
- Create: `owner` fehlt im Request → `owner = request.user`. Danach (owner, name) schon vorhanden
  → **400** `{"error":"Object violates owner / name unique constraint"}` (String). Insert, dann
  `set_permissions`.
- Update: ändert sich `owner` oder ist `set_permissions` gesetzt und der Nutzer ist weder
  Superuser noch Owner und das Objekt nicht unbesessen → **403** `{"detail":"Insufficient permissions."}`.
  Dann `set_permissions` anwenden (vor dem Speichern), dann (nur wenn `owner` in den validierten
  Daten) Owner/Name-Prüfung wie oben, dann alle Spalten speichern.

### DRF-Feldregeln
- Body: leer → `{}`; JSON-Objekt; Nicht-Objekt → 400 `{"non_field_errors":["Invalid data. Expected a dictionary, but got <pytype>."]}`.
- Fehler-JSON `{"<feld>":["…"]}` in Feldreihenfolge; Objekt-Validierung nur ohne Feldfehler.
- `null` auf nicht-nullbares Feld: `This field may not be null.`; fehlendes Pflichtfeld (nicht PATCH):
  `This field is required.`
- CharField: str/int/float (Zahl → `str()`), sonst `Not a valid string.`; trimmt; leer →
  `This field may not be blank.`; Validatoren (alle gesammelt, Reihenfolge): Unique (falls vorhanden),
  `Ensure this field has no more than 256 characters.`, `Null characters are not allowed.`
- IntegerField: Python-`int(str(v))` nach Entfernen von `\.0*\s*$`; bool/Liste ungültig;
  Text > 1000 Zeichen → `String value too large.`; sonst `A valid integer is required.`; Grenzen →
  `Ensure this value is less than or equal to N.` / `… greater than or equal to N.`
- ChoiceField: `str(v)` muss ein Choice-Wert sein, sonst `"<str(v)>" is not a valid choice.`
- BooleanField: `true/false`, `1/0`, `1.0/0.0`, `t y yes true on 1` / `f n no false off 0`
  (case-insensitiv); sonst `Must be a valid boolean.`
- PK-Feld: bool → `Incorrect type. Expected pk value, received bool.`; int/float (abgeschnitten)/
  Zahlentext → Lookup; sonst `Incorrect type. Expected pk value, received <pytype>.`; nicht gefunden
  (oder außerhalb `integer`) → `Invalid pk "<v>" - object does not exist.`
- Many-Relation (`assign_tags`): Nicht-Liste/Text → `Expected a list of items but got type "<pytype>".`;
  erstes ungültiges Element bricht ab (eine Meldung); `null`-Element → `Invalid pk "None" - …`.
- DateTimeField: ISO 8601 (Django `parse_datetime`), naive Zeit in `PAPERLESS_TIME_ZONE`
  (mehrdeutig → `Invalid datetime for the timezone "<tz>".`); sonst
  `Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z].`
  Ausgabe ISO in `PAPERLESS_TIME_ZONE`, `+00:00` → `Z`, Mikrosekunden nur wenn ≠ 0.

### Paginierung (`StandardPagination`)
`page` (Std 1, `last` erlaubt), `page_size` (Std 25, max 100000, ungültig → 25). Antwort
`{"count","next","previous",["all" nur API v9],"results"}`; ungültige Seite → 404
`{"detail":"Invalid page."}`. `next`/`previous` absolute URL, Query neu kodiert (Schlüssel
sortiert, `quote_plus`), `previous` auf Seite 2 ohne `page`.

### Antwort-Header
`Content-Type: application/json` (bzw. `text/html; charset=utf-8` bei Django-`HttpResponse*`),
`Vary: Accept`, `Allow`: Liste `GET, POST, HEAD, OPTIONS`; Detail
`GET, PUT, PATCH, DELETE, HEAD, OPTIONS`; `processed_mail` `GET, HEAD, OPTIONS`; Aktionen
`test`/`process`/`bulk_delete` `POST, OPTIONS`. 204 ohne Body.

## Routen

### `mail_accounts.list` – GET `/api/mail_accounts/`
Rechte `view_mailaccount` + Sichtbarkeit; Sortierung `id ASC`; Query `page`, `page_size`,
`full_perms`. 200 paginiert.

### `mail_accounts.retrieve` – GET `/api/mail_accounts/{id}/`
200 Element; 404 wie oben.

**MailAccount-Element (Reihenfolge):** `id`, `name`, `imap_server`, `imap_port` int|null,
`imap_security` int (1 none, 2 SSL, 3 STARTTLS), `username`, `password`
(**immer `"*" * max(10, len(password))`**, nie das Passwort), `character_set`, `is_token`,
`owner`, `user_can_change` | `permissions`, `account_type` (1 IMAP, 2 Gmail OAuth, 3 Outlook
OAuth), `expiration` datetime|null. `refresh_token` wird nie ausgegeben.

### `mail_accounts.create` – POST `/api/mail_accounts/`
Rechte `add_mailaccount`. Felder: `name` (Pflicht, ≤256, eindeutig →
`mail account with this name already exists.`), `imap_server` (Pflicht, ≤256), `imap_port`
(Pflicht, IntegerField **ohne** Grenzen, nicht null), `imap_security` (1–3, Std 2), `username`
(Pflicht, ≤256), `password` (Pflicht; Leer-/Null-Prüfung, **kein** Trimmen, Rohwert gespeichert),
`character_set` (≤256, Std `UTF-8`), `is_token` (Std false), `owner`, `set_permissions`,
`account_type` (1–3, Std 1), `expiration` (datetime|null). 201 + Element.

### `mail_accounts.update` / `mail_accounts.partial_update` – PUT/PATCH `/api/mail_accounts/{id}/`
Rechte `change_mailaccount` + Objekt. Ein `password`, das nach Entfernen aller `*` leer ist, wird
ignoriert. 200 + Element.

### `mail_accounts.destroy` – DELETE `/api/mail_accounts/{id}/`
Rechte `delete_mailaccount` + Objekt. Transaktion: `documents_workflowtrigger.filter_mailrule_id`
der Regeln → NULL (`WorkflowTrigger.filter_mailrule` `on_delete=SET_NULL`, Review 2 M3), dann
ProcessedMail der Regeln, Tag-Zuordnungen der Regeln, Regeln, Konto (`accounts::ACCOUNT_DELETE_SQL`).
Andere FKs auf Mail-Tabellen gibt es laut `reference/schema/postgres.sql` nicht. Guardian-Zeilen
bleiben (wie upstream). 204.

### `mail_accounts.test` – POST `/api/mail_accounts/test/`
Nur `IsAuthenticated`. Ablauf:
1. Nicht-Objekt-Body → 500 (upstream `request.data["name"] = …` TypeError).
   `name = datetime.now().isoformat()` (lokale Zeit `PAPERLESS_TIME_ZONE`, naiv), Serializer wie
   create → bei Fehler 400 DRF-JSON (vor jeder Rechteprüfung).
2. `id` roh: fehlt/`null` → globales `add_mailaccount`, sonst **403 `text/html; charset=utf-8`
   `Insufficient permissions`**. Vorhanden → `MailAccount.get(pk=int(id))` (nicht konvertierbar /
   nicht vorhanden → 403 wie oben), dann globales `change_mailaccount` **und**
   `has_perms_owner_aware(change_mailaccount)`, sonst 403 wie oben.
3. `set_permissions` im Body → 500 (`MailAccount(**validated_data)` TypeError). Nicht-String-
   Passwort → 500 (`.replace`).
4. Passwort nur aus `*` und Konto vorhanden → Passwort/Typ/Refresh-Token/Ablauf vom Konto.
5. `get_mailbox`: bei `PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS=false` DNS + Sperre nicht-öffentlicher
   IPs (→ MailError), Verbindung zu den gepinnten IPs, TLS mit SNI/Prüfung auf den Hostnamen.
   `NONE` Klartext, `SSL` TLS direkt, `STARTTLS` (Port 993 → Fehler vor dem Verbinden; muss in
   CAPABILITY stehen). Begrüßung `OK`/`PREAUTH`, dann `CAPABILITY` (IMAP4/IMAP4rev1 nötig).
6. Konto vorhanden + `is_token` (aus dem Request) + `expiration < now` → OAuth-Refresh des
   gespeicherten Kontos (POST Token-Endpunkt, `client_secret_post`, `Accept: application/json`;
   HTTP-Fehler/Status ≥ 400/kein JSON → MailError; fehlende `access_token`/`expires_in` → 500;
   Erfolg → Konto gespeichert, neues Passwort verwendet).
7. Login: `is_token` → `AUTHENTICATE XOAUTH2` (`user=<u>\1auth=Bearer <pw>\1\1`); ASCII-Passwort →
   `LOGIN "<u>" "<pw>"` (imaplib-Quoting); sonst `AUTHENTICATE PLAIN`; danach `SELECT "INBOX"`
   (`NO`/`READ-ONLY` → Fehler). Jede Ausnahme → MailError.
8. Immer `LOGOUT`; ohne `* BYE` → 500 (ersetzt auch Erfolg/MailError).
- Erfolg **200** `{"success":true}`; MailError **400 `text/html; charset=utf-8` `Unable to connect to server`**;
  andere Fehler (Verbindung abgelehnt, TLS, Begrüßung, STARTTLS, LOGOUT ohne BYE) **500**.

### `mail_accounts.process` – POST `/api/mail_accounts/{id}/process/`
Globales **`add_mailaccount`** (POST), `get_object` mit Objektrecht für POST (`add_mailaccount`).
Celery `paperless_mail.tasks.process_mail_accounts`, kwargs `{"account_ids":[id]}`, Header
`trigger_source="manual"`; `documents_paperlesstask` (`task_type=mail_fetch`, `trigger_source=manual`,
`status=pending`, `input_data={"account_ids":[id]}`). Rust: `celery::send_process_mail_accounts` (Modulname
historisch) über `pl_core::tasks::send_task` in die PaperRust-Warteschlange (die PgQueue legt die Zeile an;
seit 2026-11-14 kein Celery-Weg mehr). 200 `{"result":"OK"}`.

### `mail_rules.list` / `mail_rules.retrieve` – GET `/api/mail_rules/`, `/api/mail_rules/{id}/`
Rechte `view_mailrule` + Sichtbarkeit; Sortierung `order ASC` (kein Tie-Break, wie upstream).

**MailRule-Element:** `id`, `name`, `account`, `enabled`, `folder`, `filter_from`, `filter_to`,
`filter_subject`, `filter_body`, `filter_attachment_filename_include`,
`filter_attachment_filename_exclude`, `maximum_age`, `action` (1 delete, 2 move, 3 mark read,
4 flag, 5 tag), `action_parameter` str|null, `assign_title_from` (1–3), `assign_tags` [int]
(nach Tag-Name sortiert, `Tag.Meta.ordering`), `assign_correspondent_from` (1–4),
`assign_correspondent`, `assign_document_type`, `assign_owner_from_rule`, `order`,
`attachment_type` (1–2), `consumption_scope` (1–3), `pdf_layout` (0–4), `owner`,
`user_can_change` | `permissions`, `stop_processing`.

### `mail_rules.create` – POST `/api/mail_rules/`
Rechte `add_mailrule`. Felder:
- `name` Pflicht ≤256; UniqueValidator gegen **unbesessene** Regeln (`owner IS NULL`, Bedingung des
  UniqueConstraint) → `mail rule with this name already exists.` – gilt auch für Regeln mit Owner.
- `account` Pflicht; nur Konten mit `change_mailaccount`-Sichtbarkeit (Superuser alle), sonst
  `Invalid pk "<v>" - object does not exist.`; danach `validate_account` (403
  `{"detail":"Insufficient permissions."}`, praktisch unerreichbar).
- `enabled` (Std true), `folder` ≤256 nicht leer (Std `INBOX`), `filter_*` ≤256 null/leer erlaubt,
  `maximum_age` 0–32767 (Std 30; `>36500` danach, unerreichbar), `action` 1–5 (Std 3),
  `action_parameter` (ohne Längengrenze, null ok, **leer verboten**, Std `""`),
  `assign_title_from` 1–3, `assign_tags` (alle Tags), `assign_correspondent_from` 1–4,
  `assign_correspondent` / `assign_document_type` (alle, null ok), `assign_owner_from_rule`,
  `order` (IntegerField ohne Grenzen, Std 0), `attachment_type` 1–2, `consumption_scope` 1–3,
  `pdf_layout` 0–4, `owner` (Std **null**, Unique-Constraint (name, owner)), `set_permissions`,
  `stop_processing` (Std false).
- Objekt-Validierung: UniqueTogether(name, owner) nur wenn kein Wert `null` ist (Update: nur
  geänderte Werte) → `{"non_field_errors":["The fields name, owner must make a unique set."]}`;
  danach `action ∈ {2,5}` ohne `action_parameter` → `{"non_field_errors":["An action parameter is required."]}`.
- 201 + Element; Tags nur gesetzt, wenn nicht leer.

### `mail_rules.update` / `mail_rules.partial_update` – PUT/PATCH `/api/mail_rules/{id}/`
Rechte `change_mailrule` + Objekt. PUT wendet Serializer-Defaults an: fehlendes `owner` → null
(Owner wird entfernt bzw. 403 für Nicht-Owner), fehlendes `action_parameter` → `""`.
`assign_tags` ersetzt die Menge. 200 + Element.

### `mail_rules.destroy` – DELETE `/api/mail_rules/{id}/`
Rechte `delete_mailrule` + Objekt. Transaktion: Workflow-Trigger-Filter → NULL (Review 2 M3), dann
ProcessedMail, Tag-Zuordnungen, Regel (`rules::RULE_DELETE_SQL`). 204.

### `processed_mail.list` / `processed_mail.retrieve` – GET `/api/processed_mail/`, `/api/processed_mail/{id}/`
Rechte `view_processedmail` + Sichtbarkeit; Std-Sortierung `processed DESC`.
Filter (auch beim Detail!): `rule` (alle Regeln; ungültig/fehlend → 400
`{"rule":["Select a valid choice. That choice is not one of the available choices."]}`, leer ignoriert),
`status` (exakt, leer ignoriert). `ordering`: kommagetrennt, `-` optional, gültig
`id owner rule folder uid subject received processed status error`, ungültige Begriffe ignoriert.
Element: `id`, `owner`, `rule`, `folder`, `uid`, `subject`, `received`, `processed`, `status`, `error`.

### `processed_mail.bulk_delete` – POST `/api/processed_mail/bulk_delete/`
Authentifiziert + globales `delete_processedmail` (sonst 403 DRF-JSON). Nicht-Objekt-Body → 500.
`mail_ids` (Std `[]`): keine Liste oder Element kein int (bool zählt als int) → **400 `text/html`
`mail_ids must be a list of integers`**. Eine vorhandene ID nicht in
`permitted_object_ids(delete_processedmail)` → **403 `text/html` `Insufficient permissions`**,
nichts gelöscht. Sonst vorhandene löschen; 200 `{"result":"OK","deleted_mail_ids":<wie gesendet>}`.

### `oauth.callback` – GET `/api/oauth/callback/` (Standalone 2026-11-09, `src/oauth_callback.rs`)
Braucht den Django-Session-Wert `oauth_state` (geschrieben von Django in
`documents/views.py::UiSettingsView`), Token-Austausch bei Google/Microsoft (nicht lokal testbar),
`update_or_create`-Semantik und `DEBUG`-abhängige Redirects; seltener Einmal-Vorgang ohne
Performance-Nutzen. Verhalten: ohne `add_mailaccount` → 400 `You do not have permission to add mail accounts`;
ohne `code`/falscher `state`/unbekannter `scope` → 400 `Invalid request, see logs for more detail`;
Erfolg 302 `<redirect>?oauth_success=1&account_id=<id>`; Token-Fehler 302 `…?oauth_success=0`.
Umsetzung: `oauth_state` aus der Django-Session (geschrieben von `ui_settings.get`, pl-auth-api),
Token-Austausch `client_secret_post` mit `Accept: application/json` (Fehler/Status ≥ 400/kein JSON →
`oauth_success=0`), neues Konto (Name `Gmail|Outlook OAuth <str(now)>`, SSL/993, `UTF-8`). Erfolgspfad
nicht gegen Google/Microsoft getestet (Fremddienst); Fehlerpfade in tests/parity/standalone-gaps.

### `documents.email` – POST `/api/documents/email/` und `documents.email_document` – POST `/api/documents/{id}/email/` (W23, `src/documents_email.rs`)

Quelle: src/documents/views.py@v3.4.2 `DocumentViewSet.email_documents` / `email_document`
(veraltet, API v9), src/documents/serialisers.py@v3.4.2 `EmailSerializer` (← `DocumentListSerializer`),
src/documents/mail.py@v3.4.2 `send_email`, src/documents/models.py@v3.4.2 `get_public_filename`.

- Rechte: `IsAuthenticated` + `ViewDocumentsPermissions` (POST: `documents.view_document`), danach
  Validierung, danach `documents.exclude(id__in=permitted_document_ids(user)).exists()` → 403
  `text/html` „Insufficient permissions“.
- Felder (Fehler gesammelt, Reihenfolge): `documents` (ListField[int], required; Anzahl
  vorhandener nicht gelöschter Dokumente ≠ Länge → „Some documents in documents don't exist or were
  specified twice.“; leer → „At least one document is required“), `addresses` (CharField; je
  Komma-Teil `strip` + `EmailValidator` → „Invalid email address: <teil>“; Ergebnis
  `",".join`), `subject`, `message` (CharField required), `use_archive_version` (Boolean,
  Standard `true`).
- Anhänge in `-created`-Reihenfolge: Archiv, wenn `use_archive_version` und vorhanden, sonst
  Original; `mime_type` immer das des Dokuments (wie upstream, auch beim Archiv-PDF); Name
  `get_public_filename(archive=…)` (Kontext = Wurzeldokument), Dubletten `_01`, `_02` …
  (`_get_unique_filename`). Versand über `effects::smtp` (`EMAIL_HOST`/`PORT`/`USE_TLS`/`USE_SSL`/
  Login, `DEFAULT_FROM_EMAIL`; **kein** `EMAIL_ENABLED`-Check – wie upstream wird auch mit
  `localhost:25` versucht).
- Erfolg 200 `{"message": "Email sent"}`; jeder Fehler beim Lesen/Bauen/Senden → 400 (upstream 500, Known differences 19) `text/html`
  „Error emailing documents, check logs for more detail.“ (Log `warning`).
- `email_document`: `request.data.copy().setlist("documents", [pk])` – nur mit Formular-Körper
  (QueryDict) möglich; Route `accepts_form`, Formular wird wie QueryDict gelesen (letzter Wert je
  Schlüssel). JSON-Körper → `dict` ohne `setlist` → AttributeError → Djangos 500-Seite (wie upstream,
  geprüft). Multipart → Body-Gate → 404 (nicht unterstützt; bis 2026-11-14 an Django).
- Known differences: `FileLock(MEDIA_LOCK)` um das Lesen der Anhänge fehlt (Datei-Umbenennung
  parallel kann einen Versandfehler → 500 auslösen statt zu warten); MIME-Text nicht byte-gleich
  (siehe `effects::smtp`); E-Mail-Validator wie `pl_auth_api::http::is_valid_email` (vereinfachter
  Django-`EmailValidator`, gleiche Regeln für übliche Eingaben).
- Tests: `tests/documents_email.rs` (`#[ignore]`, Referenz-DB lesend + lokale SMTP-Senke: Erfolg mit
  2 Anhängen/2 Empfängern, Formular-Detailroute, JSON-Detailroute → 500, SMTP-Fehler → 500, 403).
  Parität (W23, 2026-11-08, A = Referenz-Django :8000, B = PaperRust :8195 nur mit den W23-Routen, Rest Django; `tests/parity/pl-mail/w23b3env.sh`, Berichte `results/parity/w23/`): `tests/parity/pl-mail/cases.w23.json` 29 Fälle (Validierung, Typen, null, Rechte,
  SMTP-Fehler, Detailroute JSON/Formular): 29 identisch.

## Known differences

1. **Nur JSON-Bodies.** DRF akzeptiert auch Form/Multipart (QueryDict-Semantik); Rust → 415
   `{"detail":"Unsupported media type \"<ct>\" in request."}`. Das Frontend sendet JSON.
   (Seit 2026-11-14 ohne Django: Form/Multipart auf diesen Routen beantwortet das pl-server-Body-Gate mit
   404, bevor der Handler läuft.)
2. **JSON-Parse-Fehlertext**: `JSON parse error - <serde-Meldung>` statt Python-Meldung.
3. **Python-`str()`/`repr()` exotischer Werte** (Listen, Objekte, Floats) in Fehlermeldungen
   nachgebildet, nicht byte-genau garantiert.
4. **Nicht-String-`password`**: wie upstream wird `str(value)` gespeichert (Python-`repr` für
   Listen/Objekte nachgebildet); Update damit → 500 wie upstream, Test → 400 (Known differences 19). Review 2 N28: bei `create`
   mit Zahl/Bool antwortet upstream nach dem Insert 500 (`ObfuscatedPasswordField.to_representation`
   → `len(int)` → `TypeError`), Rust 201; Zeile wird in beiden Fällen angelegt. Für Listen/Dicts 201
   wie Rust.
5. **Zeitlimit beim Verbindungstest**: upstream keines; Rust 60 s je Schritt, danach 400 (siehe 19).
6. **DB-Überlauf** (`imap_port` negativ/zu groß, `order` außerhalb smallint, `action_parameter`
   > 256): upstream IntegrityError/DataError → 500; Rust 500.
7. **500-Seite** = Django-Standardseite für DEBUG=false (DEBUG-Seite nicht nachgebildet).
8. **`expiration`-Parser**: Teilmenge von Python `datetime.fromisoformat` + Django-Regex
   (`YYYY-MM-DD`, `YYYYMMDD`, beliebiges Trennzeichen, `HH[:MM[:SS[.f]]]`, `Z|±HH[:MM]`); Wochen-Datum,
   Sekunden-Offsets u. ä. nicht.
9. **`oauth.callback`**: `update_or_create` als reines INSERT (die Suchfelder enthalten ein neues
   Token und `now()` – ein Treffer ist praktisch ausgeschlossen).
10. **Logs** ohne Kontonamen/Passwörter/Tokens (nur Konto-ID und Fehlerart).
11. **CR/LF in IMAP-Benutzername/Passwort**: imaplib sendet ungeprüft (Befehlsinjektion); Rust
    bricht mit MailError ab (→ 400). Sicherheitsbedingt.
12. **SQL** handgeschrieben (Postgres: `ANY`, `UNNEST`, `IS NOT DISTINCT FROM`), nicht sea-query;
    alle Werte gebunden. Für SQLite umzustellen.
13. **Browsable API** (`Accept: text/html`) nicht nachgebildet – immer JSON.
14. **Einzelne UTF-16-Surrogate** in JSON-Strings: Python akzeptiert sie (DRF meldet dann
    `Surrogate characters are not allowed`), serde lehnt sie als Parse-Fehler ab.
15. **TLS-Erfolgsfall des Verbindungstests** lokal nicht getestet (kein Testzertifikat); getestet sind
    die Fehlerpfade. CA-Quelle: System-Speicher (`rustls-native-certs`) +
    `PAPERLESS_EMAIL_CERTIFICATE_LOCATION`, wie `ssl.create_default_context()`.
16. **Konfiguration**: alle Werte aus `pl_core::Config` (`MailConfig::from_core`; inkl.
    `paperless.conf`). Seit 2026-11-14 ohne die Felder `redis_url`/`redis_prefix` (kein Redis). OAuth-Refresh mit 5-s-Timeout und ohne Redirects wie httpx (Review 2 N27).
17. **`expiration` mit Nicht-ASCII** (Review 2 M4): immer 400 „Datetime has wrong format…“. Pythons
    `\d`/`\s` passen auch auf Unicode-Ziffern/-Leerraum (z. B. `２０２４-01-01T10:00`); solche Werte
    akzeptiert Django, Rust nicht.
18. **`""` auf Relationsfeldern** (`owner`, `account`, `assign_correspondent`, `assign_document_type`)
    = `null` wie DRF `RelatedField.run_validation` (Review 2 N26). `assign_tags` wird mit einer
    Abfrage aufgelöst, Fehler weiterhin für den ersten fehlerhaften Eintrag (Review 2 N29).
19. **Keine 500 bei Fehleingaben/Verbindungsfehlern** (A-230/B-313, 2026-11-16, bewusst):
    - `mail_accounts.test`: jeder Fehlschlag (Verbindung abgewiesen, DNS, Zeitlimit, TLS, Port 993
      mit STARTTLS, OAuth-Refresh-Fehler) → 400 `Unable to connect to server` (Text wie upstream bei
      `MailError`); upstream → 500 außer bei `MailError`. Grund nur im Log (ohne Zugangsdaten).
      Nicht-Objekt-Body → 400 `{"non_field_errors":["Invalid data. Expected a dictionary, but got
      list."]}`; fehlendes/nicht-String-`password` bzw. fehlender/zu großer `imap_port` → 400 mit
      Feldfehler; `set_permissions` wird beim Test ignoriert (upstream TypeError → 500).
    - `documents.email`: Versand gescheitert (kein SMTP, nicht erreichbar) → **400**
      `Error emailing documents, check logs for more detail.` (upstream gleicher Text mit 500).
    - `documents.email_document` (veraltet) mit JSON-Objekt-Body: wird wie der Formular-Körper
      verarbeitet (upstream AttributeError → 500); Nicht-Objekt → 400.

20. **Webhook-Aktion im Workflow wird immer im Hintergrund eingereiht** (tests/stress/BEFUNDE.md B2,
    2026-11-18): `SharedEffects::webhook_in` (Consumer-/Änderungsweg, Aufrufer hält eine Verbindung
    bzw. Transaktion) wartet nicht mehr auf `send_webhook.apply_async`, sondern startet das Einreihen
    als eigene Aufgabe. Vorher holte das Einreihen eine zweite Verbindung aus demselben Pool, während
    der Aufrufer eine hielt (Pool-Erschöpfung unter Last). Upstream reiht ebenfalls außerhalb der
    Transaktion ein (Celery), sichtbar ist nur, dass die Aufgabe Millisekunden später in der
    Warteschlange steht.

20a. **Kontotest schickt gespeicherte Zugangsdaten nur an das gespeicherte Ziel** (Sicherheitsprüfung
    2026-11-19): `POST /api/mail_accounts/test/` mit `id` und Passwort nur aus `*` übernimmt das
    gespeicherte Passwort (und Ablauf) nur, wenn `imap_server` (ohne Groß-/Kleinschreibung),
    `imap_port`, `imap_security` und `username` dem gespeicherten Konto entsprechen. Sonst **400**
    `{"password": ["The stored password can only be used with the saved server, port, security and
    username. Please enter the password again."]}`, ohne Verbindungsaufbau. Ebenso wird das
    OAuth-Token des gespeicherten Kontos nur bei gleichem Ziel erneuert. upstream verwendet das
    gespeicherte Passwort für jeden Host – ein Nutzer mit `change_mailaccount` hätte es so an einen
    eigenen Server schicken und mitlesen können. Test: `accounts::tests::stored_password_only_for_saved_target`.

21. **Anhang des Workflow-Mails nur über `safe_join`** (Sicherheitsprüfung 2026-11-19 F9):
    `effects::context::for_row` bildet `source_path` über `pl_consumer::files::join`. Unsicherer
    DB-Dateiname → Pfad, der nie existiert; der Versand scheitert wie bei einer fehlenden Datei
    (upstream öffnet den Pfad ungeprüft).

## Paritätsstand

- **Lesend gegen Django verglichen** (`crates/pl-mail/tests/parity_django.rs`, `#[ignore]`, nur GET):
  Referenz-DB + Django 127.0.0.1:8000, Nutzer `admin`/`user1`/`user2`/`noperm`, API v10 und v9,
  23 URLs (Listen, `full_perms`, Paginierung inkl. ungültiger Seite, Details sichtbar/fremd/
  nicht-numerisch/fehlend, `processed_mail` mit `rule`/`status`/`ordering`, Detail mit Filter):
  **184 Vergleiche, 0 Abweichungen** (Status, Content-Type, Body byte-genau). Die Anmeldung wird
  dabei in-process gesetzt (401/403 der Auth-Schicht liefert pl-authz).
- **Schreibende Routen** (create/update/destroy, `test`, `process`, `bulk_delete`): Fälle in
  `tests/parity/pl-mail/cases.json` (`write: true` → brauchen `reference/reset.sh`); **noch nicht
  gegen Django gelaufen**.
- Verbindungstest: 16 Tests gegen lokalen Fake-IMAP-Server (`crates/pl-mail/tests/imap_fake.rs`).

## Ausfall-Befund C4 (tests/chaos, 2026-11-18)

21. **Webhook bei nicht erreichbarem Empfänger wird wiederholt:** `ConnectError` (Port zu, Netz weg)
    fällt jetzt unter dieselbe Neuversuch-Regel wie Serverfehler (`max_retries=3`, Backoff mit Jitter),
    auch in `send_with_retries`. Ausnahme: die SSRF-Sperre interner Adressen („non-public“) bleibt
    endgültig. upstream: nur `HTTPStatusError` wird wiederholt, ein kurz neu startender Empfänger
    verlor den Webhook. Test: `effects_local::webhook_retries_until_receiver_is_back`.
