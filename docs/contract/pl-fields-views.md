# Vertrag `pl-fields-views`

Upstream: paperless-ngx **v3.4.2** (`upstream/`). Quellen (alle unter `upstream/src/`):

- `documents/views.py`: `CustomFieldViewSet`, `SavedViewViewSet`, `ShareLinkViewSet`,
  `ShareLinkBundleViewSet` (+ `rebuild`), Mixins `PassUserMixin`, `BulkPermissionMixin`,
  `PermissionsAwareDocumentCountMixin`
- `documents/serialisers.py`: `CustomFieldSerializer`, `SavedViewSerializer`,
  `SavedViewFilterRuleSerializer`, `ShareLinkSerializer`, `ShareLinkBundleSerializer`,
  `OwnedObjectSerializer`, `SetPermissionsMixin`, `SetPermissionsSerializer`
- `documents/filters.py`: `CustomFieldFilterSet`, `ShareLinkFilterSet`, `ShareLinkBundleFilterSet`,
  `PermittedObjectsFilter`
- `documents/permissions.py`: `PaperlessObjectPermissions`, `ViewDocumentsPermissions`,
  `permitted_object_ids`, `permitted_document_ids`, `get_permission_grantees`,
  `set_permissions_for_objects`, `has_perms_owner_aware`, `annotate_document_count_by_ids`
- `documents/models.py`: `CustomField`, `CustomFieldInstance`, `SavedView`, `SavedViewFilterRule`,
  `UiSettings`, `ShareLink` (SoftDeleteModel!), `ShareLinkBundle`, `PaperlessTask`
- `documents/signals/handlers.py`: `check_paths_and_prune_custom_fields`,
  `process_cf_select_update`, `cleanup_custom_field_deletion`, `before_task_publish_handler`
- `documents/tasks.py::build_share_link_bundle`
- `paperless/views.py::StandardPagination`, `paperless/settings/__init__.py` (REST_FRAMEWORK, Celery)
- Bibliotheken (gleiches Verhalten wie installierte Versionen): DRF 3.16, django-filter 26.2,
  django-auditlog 3.4.1, django-soft-delete 1.0.18, django-guardian 3.5
- Tests: `documents/tests/test_api_custom_fields.py`, `test_api_documents.py`
  (`test_saved_view*`, `test_*share_link*`), `test_share_link_bundles.py`, `test_api_permissions.py`

## Gemeinsames Verhalten (alle Routen)

- **Auth:** `IsAuthenticated` → ohne Login 401 (`WWW-Authenticate: Basic realm="api"`) bzw. 403
  (`{"detail":"Authentication credentials were not provided."}`) – Sache von `pl-authz`.
- **Globale Modellrechte** (`PaperlessObjectPermissions.perms_map`, DRF `DjangoModelPermissions`):
  GET/HEAD/OPTIONS → `documents.view_<model>`, POST → `add_<model>`, PUT/PATCH → `change_<model>`,
  DELETE → `delete_<model>`. Aktive Superuser haben alles; inaktive Nutzer nichts.
  Fehlt das Recht: **403** `{"detail":"You do not have permission to perform this action."}`.
- **Objektrechte** (`has_object_permission`, nur Detailrouten nach `get_object`):
  Objekt ohne `owner`-Attribut (CustomField) oder `owner IS NULL` → erlaubt; `owner == user` → erlaubt;
  sonst guardian-Objektrecht `<aktion>_<model>` (Nutzer- oder Gruppenrecht, Superuser immer).
  Fehlt es: bei GET → **404 `{"detail":"Not found."}`** (DRF `Http404` ohne Text); sonst, wenn das
  Objekt-`view_`-Recht fehlt → **404 `Not found.`**, sonst **403**
  (gleicher Text wie oben).
- **Sichtbarkeit** (`PermittedObjectsFilter`, SavedView/ShareLink/Bundle, auch bei Detailrouten vor
  `get_object` → sonst 404 `{"detail":"No <Model> matches the given query."}`):
  inaktiv → nichts; Superuser → alles; sonst `owner = user OR owner IS NULL OR
  CAST(id AS varchar) IN (guardian object_pk für view_<model> des Nutzers ∪ seiner Gruppen)`.
  CustomField hat **keinen** Sichtbarkeitsfilter (alle Felder für jeden mit `view_customfield`).
- **404-Text** DRF `get_object_or_404`: `{"detail":"No CustomField matches the given query."}`,
  `No SavedView …`, `No ShareLink …`, `No ShareLinkBundle …`. Nicht-numerische `pk` → 404
  `{"detail":"Not found."}` (`get_object_or_404` fängt den `ValueError`).
- **API-Version** (Accept `application/json; version=N`, erlaubt 9/10, Standard 10; ungültig → 406
  `{"detail":"Invalid version in \"Accept\" header."}`).
- **Paginierung** `StandardPagination`: `page` (ungültig/zu groß → 404 `{"detail":"Invalid page."}`),
  `page_size` (Standard 25, max 100000, ≤0/ungültig → 25). Antwort
  `{"count","next","previous",["all" nur v9],"results"}`; `all` = alle IDs in Sortierreihenfolge;
  `next`/`previous` absolute URLs (Django `build_absolute_uri` + `replace_query_param`,
  `previous` auf Seite 1 ohne `page`-Parameter).
- **Sortierung** DRF `OrderingFilter`: Parameter `ordering`, kommasepariert, `-` absteigend;
  unbekannte Terme werden still verworfen; kein gültiger Term → Standardsortierung.
  `order_by()` ersetzt die Standardsortierung vollständig (kein Tiebreaker).
- **Filter** django-filter: ungültige Werte → 400 `{"<param>":["Enter a number."]}` u. ä.;
  leere Werte werden ignoriert.
- **`full_perms`** (Query, `PassUserMixin`, nur OwnedObjectSerializer = SavedView): `get_boolean`
  (`yes,y,1,t,true`, case-insensitiv). `true` → Feld `permissions` statt `user_can_change`.
- **Request-Body:** JSON (Frontend), zusätzlich form-urlencoded/multipart wie DRF-Parser.
  Body kein Objekt → 400 `{"non_field_errors":["Invalid data. Expected a dictionary, but got <type>."]}`.
- **Feldfehler** in Feldreihenfolge des Serializers; `validate()`-Fehler nur, wenn keine Feldfehler.
  Fehler aus `validate()` werden als Liste geliefert (`{"error":["…"]}`), Fehler aus
  `create()`/View als roher String (`{"error":"…"}`, `{"document_ids":"…"}`) – DRF-Eigenheit.
- **Datum/Zeit** Ausgabe: DRF `DateTimeField` in `PAPERLESS_TIME_ZONE`, ISO 8601, Mikrosekunden nur
  wenn ≠ 0, `+00:00` → `Z`. Eingabe: Django `parse_datetime` (`datetime.fromisoformat`, auch reines
  Datum), naive Werte in `PAPERLESS_TIME_ZONE`. Fehler:
  `"Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z]."`
- **Zufallsstrings** `django.utils.crypto.get_random_string`: Zeichenvorrat
  `a-zA-Z0-9` (62 Zeichen), kryptografischer Zufall (`secrets.choice`).
- Keine Transaktion pro Request (`ATOMIC_REQUESTS` nicht gesetzt) – Schritte laufen einzeln.

## DRF-Feldfehlertexte (verwendet)

| Fall | Text |
|---|---|
| fehlt | `This field is required.` |
| null | `This field may not be null.` |
| leer | `This field may not be blank.` |
| zu lang | `Ensure this field has no more than N characters.` |
| kein String | `Not a valid string.` |
| Choice | `"<wert>" is not a valid choice.` |
| Bool | `Must be a valid boolean.` |
| Integer | `A valid integer is required.` / `Ensure this value is greater than or equal to N.` / `… less than or equal to N.` |
| PK | `Invalid pk "<pk>" - object does not exist.` / `Incorrect type. Expected pk value, received <typ>.` |
| Liste | `Expected a list of items but got type "<typ>".` / `This list may not be empty.` (Kindfehler: `{"<index>":[…]}`) |
| Liste von Objekten (nested many) | `{"non_field_errors":["Expected a list of items but got type \"<typ>\"."]}` |
| Unique (CustomField.name) | `custom field with this name already exists.` |

---

## 1. Custom Fields – `/api/custom_fields/`

`CustomFieldViewSet(PermissionsAwareDocumentCountMixin, ModelViewSet)`; Modell `documents_customfield`
(`id, created, name UNIQUE, data_type, extra_data jsonb NULL`).
Rechte: global `documents.{view,add,change,delete}_customfield`; keine Objektrechte (kein owner).

### Response-Objekt (Reihenfolge)

| Feld | Typ |
|---|---|
| `id` | int |
| `name` | string |
| `data_type` | `string\|url\|date\|boolean\|integer\|float\|monetary\|documentlink\|select\|longtext` |
| `extra_data` | JSON (oder `null`) |
| `document_count` | int – **nur** bei list/retrieve/update/partial_update (annotiert); bei **create fehlt das Feld** (DRF `SkipField`, Instanz ohne Annotation) |

`document_count` = `COUNT(DISTINCT document_id)` aus `documents_customfieldinstance`
(nur `deleted_at IS NULL`, Default-Manager) mit `field_id = cf.id` und
`document_id IN permitted_document_ids(user)` (`documents_document.deleted_at IS NULL`;
Version wird über `root_document` autorisiert: Eigentümer/Rechte der Wurzel; Superuser → alle
nicht gelöschten Dokumente; inaktiv → keine). Bei update: Wert vor dem Speichern.

| id | Methode | Pfad | Status |
|---|---|---|---|
| `custom_fields.list` | GET | `/api/custom_fields/` | 200 |
| `custom_fields.create` | POST | `/api/custom_fields/` | 201 |
| `custom_fields.retrieve` | GET | `/api/custom_fields/{id}/` | 200 / 404 |
| `custom_fields.update` | PUT | `/api/custom_fields/{id}/` | 200 / 400 / 404 |
| `custom_fields.partial_update` | PATCH | `/api/custom_fields/{id}/` | 200 / 400 / 404 |
| `custom_fields.destroy` | DELETE | `/api/custom_fields/{id}/` | 204 / 404 |

**list:** Query `id`, `id__in` (kommaseparierte Zahlen), `name__istartswith`, `name__iendswith`,
`name__icontains`, `name__iexact`; `ordering` ∈ {`id`,`name`,`data_type`,`extra_data`,`document_count`}
(DRF-Default: alle lesbaren Serializer-Felder); Standardsortierung `name`.

**create/update – Validierung (`CustomFieldSerializer`):**

1. `name`: CharField required, max 128, nicht leer; `UniqueValidator` →
   `{"name":["custom field with this name already exists."]}`.
2. `data_type`: ChoiceField required (bei PATCH optional). **Änderbar** auch bei update (deklariertes Feld).
3. `extra_data`: JSONField, optional, `null` erlaubt.
4. `validate()` (nur ohne Feldfehler):
   - (Unerreichbar, da UniqueValidator vorher greift) `{"error":["Object violates name unique constraint"]}`.
   - Wenn `data_type` im Request = `select` **oder** bestehende Instanz `select` ist:
     `extra_data` muss im Request sein, `select_options` enthalten, Liste, nicht leer, jedes
     `label` mit Länge > 0 → sonst `{"error":["extra_data.select_options must be a valid list"]}`.
     **Folge:** PATCH nur `name` eines Select-Felds → 400.
     Optionen ohne `id` (fehlt oder `null`) bekommen `get_random_string(16)`; vorhandene IDs bleiben
     (auch `""`). Die Response gibt `extra_data` in **Eingabe-Schlüsselreihenfolge** zurück
     (neue `id` hinten angehängt), nicht in jsonb-Reihenfolge.
   - sonst wenn `data_type` im Request = `monetary`, `extra_data` im Request,
     `default_currency` ≠ null und (kein String oder Länge ∉ {0,3}) →
     `{"error":["extra_data.default_currency must be a 3-character string"]}`.
   - Python-Typfehler (z. B. `extra_data` = `null`/Liste/Zahl bei select, Option kein Objekt,
     `label` keine Zeichenkette/Liste) → **500** upstream (TypeError/AttributeError).

**Nebenwirkungen:**

- **Audit-Log** (`auditlog.register(CustomField)`, `PAPERLESS_AUDIT_LOG_ENABLED`, Standard an):
  Zeile in `auditlog_logentry` bei create (action 0), update (1, nur wenn sich ein Feld ändert),
  delete (2). `object_repr` = `"<name> : <data_type>"`, `object_pk` = str(id), `object_id` = id,
  `content_type` = `documents.customfield`, `serialized_data` NULL, `additional_data` NULL,
  `changes_text` `""`, `cid` = Header `X-Correlation-ID` (sonst NULL),
  `remote_addr` = erstes Element `X-Forwarded-For` (Port entfernt) sonst Client-IP,
  `remote_port` = `X-Forwarded-Port` (int) sonst NULL, `actor`/`actor_email` = Nutzer **nur bei
  Session-Login** (AuditlogMiddleware sieht Token/Basic-Nutzer nicht), `timestamp` = jetzt.
  `changes` (jsonb, `AUDITLOG_STORE_JSON_CHANGES=False` → Strings): `{feld: [alt, neu]}`;
  Werte als `str()`: `None`→`"None"`, datetime naiv in UTC (`"YYYY-MM-DD HH:MM:SS[.ffffff]"`),
  JSON per `json.dumps(sort_keys=True)` (`null` → `"null"`).
  create/delete enthalten zusätzlich die Rückwärtsrelation `fields`
  (`"documents.CustomFieldInstance.None"` ↔ `"None"`).
- **post_save** `check_paths_and_prune_custom_fields`: wenn `data_type == select` und das Feld
  Instanzen hat (`deleted_at IS NULL`) und `extra_data` truthy → Celery
  `process_cf_select_update(custom_field=<Modellinstanz>)` (**signed-pickle, Python-Objekt**).
  Die Aufgabe setzt `value_select = NULL` für entfernte Optionen und benennt Dateien um.
  → **nicht JSON-fähig**; PaperRust sendet stattdessen `custom_field_id` an den Rust-Worker
  (siehe Known differences).
  Bei create nie (noch keine Instanzen).
- **delete** (Django-Collector, keine DB-Kaskaden):
  1. `documents_customfieldinstance` des Felds (alle, auch soft-gelöschte) **hart** löschen, je
     Instanz Audit-Log `delete` (CFI ist registriert),
  2. `documents_workflowtrigger.schedule_date_custom_field_id` → NULL,
  3. M2M-Zeilen `documents_workflowaction_assign_custom_fields` / `…_remove_custom_fields` löschen,
  4. Feld löschen + Audit-Log `delete`,
  5. post_delete `cleanup_custom_field_deletion`: `custom_field_<id>` aus allen
     `documents_savedview.display_fields` entfernen (erstes Vorkommen, `list.remove`) und
     `sort_field = 'custom_field_<id>'` → `'created'`.

## 2. Saved Views – `/api/saved_views/`

`SavedViewViewSet(BulkPermissionMixin, PassUserMixin, ModelViewSet)`; Tabellen
`documents_savedview` (`id, name, icon, sort_field, sort_reverse, page_size, display_mode,
display_fields jsonb, owner_id`), `documents_savedviewfilterrule (id, rule_type, value, saved_view_id)`.
Rechte: global `documents.*_savedview`, Sichtbarkeit `PermittedObjectsFilter` (view_savedview),
Objektrechte wie oben.

| id | Methode | Pfad |
|---|---|---|
| `saved_views.list` | GET | `/api/saved_views/` |
| `saved_views.create` | POST | `/api/saved_views/` |
| `saved_views.retrieve` | GET | `/api/saved_views/{id}/` |
| `saved_views.update` | PUT | `/api/saved_views/{id}/` |
| `saved_views.partial_update` | PATCH | `/api/saved_views/{id}/` |
| `saved_views.destroy` | DELETE | `/api/saved_views/{id}/` (204) |

**Response (Reihenfolge):** `id`, `name`, `icon`, `sort_field` (string|null), `sort_reverse` (bool),
`filter_rules` (`[{"rule_type":int,"value":string|null}]`, Reihenfolge: DB/`id`), `page_size`
(int|null), `display_mode` (string|null), `display_fields` (JSON|null), `owner` (int|null), dann
`user_can_change` (bool) **oder** bei `full_perms=true` `permissions`
(`{"view":{"users":[…],"groups":[…]},"change":{…}}`, IDs aufsteigend, nur direkte guardian-Rechte).
**v9** zusätzlich am Ende `show_on_dashboard`, `show_in_sidebar` (bool) aus
`UiSettings.settings.saved_views.{dashboard,sidebar}_views_visible_ids` des anfragenden Nutzers
(fehlt UiSettings → false).

`user_can_change`: owner NULL oder owner==user → true; aktiver Superuser → true; sonst direktes
`change_savedview`-Recht des Nutzers oder einer seiner Gruppen.

**list:** `ordering` ∈ {`name`}; Standard `name`. Keine weiteren Filter.

**Validierung:** `name` CharField required max 128; `icon` Choice (Liste in `SavedView.Icon`),
Standard `funnel`; `sort_field` Char max 128, null/leer erlaubt; `sort_reverse` Bool;
`filter_rules` nested many, **required bei create/PUT**, Elemente: `rule_type` Choice 0–50
(int oder String), `value` Char max 255 null/leer erlaubt; `page_size` Int ≥1 ≤2147483647, null;
`display_mode` Choice `table|smallCards|largeCards`, null/leer; `display_fields` JSON null;
`owner` PK auf `auth_user` null; `set_permissions` (write-only) `{view?:{users?,groups?},change?:…}`,
unbekannte Aktion → `{"set_permissions":{"<key>":"Unknown permission action."}}`, nicht
existierende/doppelte IDs → `{"set_permissions":["Some users in don't exist or were specified twice."]}`
(bzw. `groups`).
`validate()`: jedes `display_fields`-Element: enthält `custom_field_` → Zahl extrahieren, Feld muss
existieren; sonst muss es in `title, created, added, tag, correspondent, documenttype, storagepath,
note, owner, shared, asn, pagecount, custom_field_%d` sein → `{"non_field_errors":["Invalid field: <f>"]}`.
Nicht-String-Elemente / `custom_field_` ohne Zahl → 500 upstream.
v9: `show_on_dashboard`/`show_in_sidebar` werden vorab als Bool geparst
(`{"show_on_dashboard":["Must be a valid boolean."]}`).

**create:** `owner` = Nutzer, wenn `owner` nicht im Request; dann
`validate_unique_together`: existiert SavedView mit gleichem owner (NULL-gleich) und name →
**400 `{"error":"Object violates owner / name unique constraint"}`** (String, keine Liste).
Reihenfolge: INSERT View → set_permissions → INSERT filter_rules → v9 UiSettings.
**update:** wenn (`owner` im Request und ≠ aktuell) oder `set_permissions` im Request und Nutzer
weder Superuser noch Eigentümer noch View unowned → **403 `{"detail":"Insufficient permissions."}`**.
Dann set_permissions anwenden, unique-Prüfung (nur wenn `owner` im Request), `display_fields: []`
→ NULL (nur update), UPDATE; wenn `filter_rules` im Request: alle Regeln löschen und neu anlegen.
v9: `UiSettings` get_or_create, IDs setzen/entfernen, sortiert speichern.

**set_permissions** (`set_permissions_for_objects`, merge=False): pro Aktion mit `users`/`groups`:
bestehende guardian-Zeilen dieser Codenames (`view_savedview` bzw. `change_savedview`) für nicht
genannte IDs löschen; für genannte IDs Zeilen anlegen (`change` legt zusätzlich `view` an),
Konflikte ignorieren.

**destroy:** Filterregeln + View löschen. guardian-Zeilen bleiben (wie upstream).
Kein Audit-Log, keine Signale.

## 3. Share Links – `/api/share_links/`

`ShareLinkViewSet` (Create/Retrieve/Destroy/List, **kein** PUT/PATCH → 405 wie DRF, von pl-server für nicht bediente Methoden unter `/api/` erzeugt).
Tabelle `documents_sharelink` (`id, created, expiration, slug UNIQUE, document_id, owner_id,
file_version, deleted_at, restored_at, transaction_id`), Manager blendet `deleted_at IS NOT NULL` aus.

| id | Methode | Pfad |
|---|---|---|
| `share_links.list` | GET | `/api/share_links/` |
| `share_links.create` | POST | `/api/share_links/` (201) |
| `share_links.retrieve` | GET | `/api/share_links/{id}/` |
| `share_links.destroy` | DELETE | `/api/share_links/{id}/` (204) |

**Response (Reihenfolge):** `id`, `created`, `expiration` (datetime|null), `slug` (50 Zeichen),
`document` (int), `document_title` (string), `file_version` (`archive|original`).
Kein `owner`/`permissions` (nicht in `Meta.fields`).

**list:** Filter `created`/`expiration` mit `__year, __month, __day, __date__gt, __date__gte, __gt,
__gte, __date__lt, __date__lte, __lt, __lte`; `ordering` ∈ {`created`,`expiration`,`document__title`};
Standard `created`.

**create:** Felder `expiration` (optional, null), `document` (PK auf nicht gelöschte Dokumente;
`blank=True` → **nicht required**; fehlt es → IntegrityError → 500 upstream), `file_version`
(Choice, Standard `archive`). `validate_document`: Nutzer braucht globales
`documents.view_document` **und** `has_perms_owner_aware(view_document)` auf dem (Wurzel-)Dokument
→ sonst **403 `{"detail":"Insufficient permissions."}`** (geht vor andere Feldfehler).
`slug = get_random_string(50)`, `owner` = Nutzer, `created` = jetzt.

**destroy:** Soft-Delete: `deleted_at = now, restored_at = NULL, transaction_id = uuid4`.

## 4. Share Link Bundles – `/api/share_link_bundles/`

`ShareLinkBundleViewSet` (`http_method_names` get/post/delete/head/options).
Tabellen `documents_sharelinkbundle` (`id, created, expiration, slug, owner_id, file_version, status,
size_bytes, last_error jsonb, file_path, built_at`), `documents_sharelinkbundle_documents`.

| id | Methode | Pfad |
|---|---|---|
| `share_link_bundles.list` | GET | `/api/share_link_bundles/` |
| `share_link_bundles.create` | POST | `/api/share_link_bundles/` (201) |
| `share_link_bundles.retrieve` | GET | `/api/share_link_bundles/{id}/` |
| `share_link_bundles.destroy` | DELETE | `/api/share_link_bundles/{id}/` (204) |
| `share_link_bundles.rebuild` | POST | `/api/share_link_bundles/{id}/rebuild/` (200) |

**Response (Reihenfolge):** `id`, `created`, `expiration`, `slug`, `file_version`, `status`
(`pending|processing|ready|failed`), `size_bytes` (int|null), `last_error` (JSON|null),
`built_at` (datetime|null), `documents` (IDs der **nicht gelöschten** Dokumente, Reihenfolge
`Document.Meta.ordering = -created`), `document_count` (`COUNT(DISTINCT documents)` über die
Through-Tabelle inkl. gelöschter Dokumente; ist das 0 → Anzahl nicht gelöschter).

**list:** Filter `created`/`expiration` (DATETIME_KWARGS), `status` (exact), `documents`
(kommaseparierte IDs; nicht-numerisch → leere Liste; `DISTINCT`); `ordering` ∈
{`created`,`expiration`,`status`}; Standard `-created`.

**create:** Rechte: global `add_sharelinkbundle` **und** `documents.view_document`.
Felder: `document_ids` (Liste Int ≥1, nicht leer, required, Duplikate →
`{"document_ids":["Duplicate document identifiers are not allowed."]}`), `expiration_days`
(Int ≥1, null, optional), `file_version` (Choice, Standard `archive`).
Dann: fehlende (oder gelöschte) Dokumente → **400 `{"document_ids":"Documents not found: 1, 2"}`**
(sortiert, String); erstes (kleinste ID) nicht in `permitted_document_ids(user)` →
**400 `{"document_ids":"Insufficient permissions to share document <id>."}`**.
INSERT Bundle (`slug` = `get_random_string(50)`, `expiration` = jetzt + Tage oder NULL,
`status = pending`, `owner` = Nutzer, `file_path = ''`), Dokumente verknüpfen, Status-Felder
zurücksetzen (Datei entfernen), **Celery `documents.tasks.build_share_link_bundle(bundle_id=<id>)`**
mit Header `trigger_source = "manual"` → **JSON-fähig** (Worker akzeptiert `application/json`).
`before_task_publish_handler` legt `documents_paperlesstask` an (`task_type = build_share_link`,
`trigger_source = manual`, `status = pending`, `input_data = {}`, `owner_id = NULL`).
Response `document_count` = Anzahl der IDs.

**rebuild:** global `add_sharelinkbundle` + Sichtbarkeit + Objektrecht (POST → `add_sharelinkbundle`).
`status == processing` → **400 `{"detail":"Bundle is already being processed."}`**.
Sonst Datei entfernen, `status = pending, last_error = NULL, size_bytes = NULL, built_at = NULL,
file_path = ''`, Celery wie oben, 200 mit Bundle.

**destroy:** Datei unter `MEDIA_ROOT/documents/share_link_bundles/<file_path>` entfernen
(nur relative Pfade innerhalb des Ordners), M2M-Zeilen + Bundle löschen.

PUT/PATCH → 405 `{"detail":"Method \"PUT\" not allowed."}` (pl-server, wie DRF).

**Versand (W14):** über `pl_core::tasks::send_task` in die PgQueue (PaperlessTask legt die PgQueue an).
Seit 2026-11-14 gibt es keinen Celery-Weg mehr; ohne eingehängten Dispatcher (nur Crate-Tests) liefert
`send_task` einen Fehler.

**Worker-Aufgabe (W14, `bundle_task.rs`, nur Rust-Worker):** `build_share_link_bundle(bundle_id)` wie
`documents/tasks.py`: Bundle fehlt → Warnung „Share link bundle <id> no longer exists.“, Erfolg ohne Ergebnis;
sonst `remove_file()`, `status = processing` + Felder leeren, Dokumente `bundle.documents.all()` (ohne
gelöschte) nach pk, ZIP_DEFLATED (`pl_files::zipw`) in `SCRATCH_DIR/tmpXXXXXXXX.zip`, Namen wie
`_filename_only` (`get_public_filename`, Zähler bei Kollision), `file_version = archive` → Archiv-PDF falls
vorhanden (ArchiveOnlyStrategy), sonst Original (OriginalsOnlyStrategy); verschieben nach
`SHARE_LINK_BUNDLE_DIR/<slug>.zip`, `status = ready`, `size_bytes`, `built_at = now`. Fehler →
`status = failed`, `last_error = {bundle_id, exception_type, message, timestamp}` und Aufgabe FAILURE.

---

## Known differences / offene Punkte

1. **`custom_fields.update`/`partial_update`** lösen bei Select-Feldern mit Instanzen
   (`deleted_at IS NULL`) und truthy `extra_data` `process_cf_select_update(custom_field=<Instanz>)`
   aus (signed-pickle, Python-Objekt → nicht JSON-fähig). PaperRust schreibt immer selbst und reiht
   nach dem Commit `process_cf_select_update(custom_field_id=<id>)` in die PaperRust-Warteschlange ein
   (Rust-Worker, docs/STANDALONE-GAPS.md §4.3); Einreihfehler → nur Log. Seit 2026-11-14 keine
   Delegation an Django mehr (vorher ohne Dispatcher, d. h. im Celery-Mischbetrieb).
2. **500-Fälle** upstream (Python-Typfehler in `validate()`, fehlendes `document` bei Share-Link,
   `display_fields` mit Nicht-Strings): Rust antwortet ebenfalls 500 mit der Django-500-Seite
   aus `pl_core::ApiError::internal`, **ohne** `Allow`/`Vary: Accept` (Djangos Handler, nicht DRF;
   `shim::finish` beachtet `pl_core::DjangoHandlerResponse`, W2-Lauf 2026-11-08). Es wird vorher
   nichts geschrieben. Share-Link ohne `document`: upstream scheitert erst das INSERT und
   verbraucht dabei eine id der Identity-Sequenz – Rust zieht ebenfalls `nextval(...)`, damit
   spätere ids gleich bleiben (W2-Fall `w2.sl.create.validation`).
3. **Audit-Log:** `changes` für die Rückwärtsrelation `fields` und die CFI-Löschzeilen bei
   `custom_fields.destroy` sind aus dem auditlog-Quelltext nachgebaut und im W2-Schreiblauf per
   SQL gegen Django geprüft (`auditlog_logentry`, Fälle `w2.cf.*`). Reihenfolge der
   CFI-Löschzeilen wie Djangos `Collector` (pk absteigend). Akteur nur bei Session-Login (wie
   AuditlogMiddleware).
4. **Reihenfolge bei Gleichstand** (z. B. `ordering=name` mit gleichen Namen, Bundle-`documents`
   nach `-created`): ORDER BY ohne Tiebreaker wie Django; PostgreSQL kann bei Gleichstand
   unterschiedlich liefern.
5. **JSON-Parse-Fehlertexte:** `pl_core::pyjson` (bis W11 `src/pyjson.rs`) bildet CPythons `json`-C-Scanner nach
   (Meldung, Zeile/Spalte/Zeichen, `NaN`/`Infinity` über DRFs `strict_constant`, UTF-8-Fehler);
   71 Goldvektoren aus CPython 3.12 (`tests/parity/pl-core/pyjson_vectors.py`).
   Restabweichung: Rekursionstiefe (upstream RecursionError → 500) und Ganzzahlen > 4300
   Ziffern (upstream ValueError-Text) nur angenähert.
6. **Request-Formate:** JSON und `application/x-www-form-urlencoded`; `multipart/form-data`
   → 415 (DRF würde es parsen). Verschachtelte Felder (`filter_rules`, `extra_data`,
   `set_permissions`) nur sinnvoll per JSON (wie das Frontend).
7. **DateTimeField-Eingabe `""`** (`expiration`) wird als `null` behandelt; DRF tut das nur bei
   Formulardaten, bei JSON wäre es ein Formatfehler.
8. Celery-`task-sent`-Events gibt es nicht (Aufgaben in der PaperRust-Warteschlange).
9. **OPTIONS** (DRF-Metadaten) nicht in Rust; seit 2026-11-14 keine Weiterleitung an Django mehr, die Antwort
   kommt von pl-server wie für jede nicht bediente Methode.
10. `remove_file` (Bundle rebuild/destroy) nutzt `Config.share_link_bundle_dir` und kanonisiert
    Pfade; Symlinks außerhalb des Ordners werden wie upstream (`resolve()` + `relative_to`)
    nicht gelöscht.

- **W14 `build_share_link_bundle` (Rust-Worker):** Ausnahmetypen/-texte nur für die üblichen Fälle wie Python
  (`FileNotFoundError: [Errno 2] No such file or directory: '<pfad>'`, `PermissionError`); andere I/O-Fehler
  `OSError` mit Rust-Text, DB-Fehler `OperationalError`. Temp-Dateiname `tmp` + 8 Hex-Zeichen statt
  `mkstemp`-Zufallsname. Kein Traceback im PaperlessTask.

## Paritäts-Stand

- **Lesend, live gegen die Referenz (2026-11-08):** `crates/pl-fields-views/tests/live_parity.rs`
  (`#[ignore]`, in-process Router mit `pl-authz`, nur GET): 31 Pfade (Listen mit Filtern/
  Sortierung/Paginierung/Fehlern, Details, 404) × 6 Nutzer (admin, alice, bob, carol, dave
  inaktiv, anonym) × v10/v9 = **360 Vergleiche, 0 Abweichungen** (Status + Body bytegenau).
  Einschränkung: die Seed-Daten enthalten **keine Share-Link-Bundles** (nur leere Listen/404
  verglichen).
- **Schreibend (W2, 2026-11-08):** `cases.json` + `cases-w2.json` (DB-Zustand per SQL nach
  jedem Ablauf: Felder, Instanzen, Saved Views + Regeln, Share Links, Bundles, guardian,
  Audit-Log, Tasks) gegen die private Kopie (`tests/parity/w2-*.sh`, Django :8100 / Rust :8101).
  Gefunden und behoben: `filter_rules`-Fehler als Dict `{index: …}` (DRF 3.18
  `LIST_SERIALIZER_ERRORS_AS_DICT`), Reihenfolge der CFI-Audit-Einträge, JSON-Fehlertexte,
  `Allow`/`Vary` an Djangos 500-Seite, verbrauchte Sequenz-id bei Share-Link-500. Ergebnis im
  W2-Bericht (results/parity/w2/).
