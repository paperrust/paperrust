# Vertrag pl-workflows (paperless-ngx v3.4.2)

Quellen (alle `upstream/src/`): `documents/views.py` (`WorkflowTriggerViewSet`,
`WorkflowActionViewSet`, `WorkflowViewSet`), `documents/serialisers.py`
(`WorkflowTriggerSerializer`, `WorkflowActionEmailSerializer`, `WorkflowActionWebhookSerializer`,
`WorkflowActionSerializer`, `WorkflowSerializer`, `CorrespondentField`, `TagsField`,
`DocumentTypeField`, `StoragePathField`), `documents/models.py` (`WorkflowTrigger`,
`WorkflowActionEmail`, `WorkflowActionWebhook`, `WorkflowAction`, `Workflow`, `WorkflowRun`),
`documents/permissions.py::PaperlessObjectPermissions`, `documents/validators.py::url_validator`,
`documents/templating/workflows.py::validate_workflow_template` (+ `templating/environment.py`),
`documents/filters.py::CustomFieldQueryParser`, `paperless/views.py::StandardPagination`,
`paperless/urls.py`. Tests: `documents/tests/test_api_workflows.py`.
Bibliotheken (Version aus `uv.lock`): djangorestframework 3.18.3, django-multiselectfield 1.0.1,
Jinja2 3.1.6, Django 5.2.

Nur die API (CRUD). Die Ausführung der Workflows (Signale, `run_workflows`, geplante Trigger) liegt in
pl-consumer (siehe unten „Ausführung geplanter Trigger“ und „Manueller Workflow-Lauf“). Keine Celery-Aufgaben, keine Signale, kein Audit-Log (Workflow-Modelle sind nicht
bei auditlog registriert), keine Dateien.

## Routen

| id | Methode | Pfad | Upstream |
|---|---|---|---|
| `workflow_triggers.list` | GET | `/api/workflow_triggers/` | `WorkflowTriggerViewSet.list` |
| `workflow_triggers.create` | POST | `/api/workflow_triggers/` | `.create` |
| `workflow_triggers.retrieve` | GET | `/api/workflow_triggers/{id}/` | `.retrieve` |
| `workflow_triggers.update` | PUT | `/api/workflow_triggers/{id}/` | `.update` |
| `workflow_triggers.partial_update` | PATCH | `/api/workflow_triggers/{id}/` | `.partial_update` (überschrieben) |
| `workflow_triggers.destroy` | DELETE | `/api/workflow_triggers/{id}/` | `.destroy` |
| `workflow_actions.*` | dito | `/api/workflow_actions/…` | `WorkflowActionViewSet` |
| `workflows.*` | dito | `/api/workflows/…` | `WorkflowViewSet` |

Nicht in Rust: `OPTIONS` (DRF-Metadaten), `HEAD`, Format-Suffixe (`/api/workflows.json`); seit 2026-11-14 ohne
Weiterleitung an Django (Antwort von pl-server wie für nicht bediente Pfade/Methoden).

## Allgemein

- Auth: wie alle Routen (`PaperlessBasicAuthentication`, `TokenAuthentication`,
  `SessionAuthentication`); nicht angemeldet → 401 `{"detail":"Authentication credentials were not provided."}`.
- Rechte (`IsAuthenticated`, `PaperlessObjectPermissions` = `DjangoObjectPermissions`):
  Modellrecht je Methode: GET → `documents.view_<model>`, POST → `add_`, PUT/PATCH → `change_`,
  DELETE → `delete_`; `<model>` ∈ `workflowtrigger`, `workflowaction`, `workflow`.
  Fehlend → 403 `{"detail":"You do not have permission to perform this action."}` (vor dem
  Objekt-Lookup, also auch für nicht existierende ids). Aktiver Superuser hat alle Rechte, inaktiver
  Nutzer keine. Rechte aus `auth_user_user_permissions` und Gruppen (`auth_user_groups` →
  `auth_group_permissions`). Die Modelle haben **kein** `owner` → Objektprüfung immer erlaubt,
  keine Objekt-Filterung der Liste (jeder mit `view_*` sieht alles).
- Detail-Lookup `{id}`: Router-Regex `[^/.]+`. Kein gültiger Integer (`int()` wirft) → 404
  `{"detail":"Not found."}`; nicht vorhanden → 404 `{"detail":"No WorkflowTrigger matches the given query."}`
  (`WorkflowAction`, `Workflow` entsprechend).
- Liste: `StandardPagination` (`page`, `page_size` ≤ 100000, Standard 25). Antwort
  `{"count","next","previous",["all" nur API v9],"results"}`; ungültige Seite → 404 `{"detail":"Invalid page."}`.
  Keine Filter, keine Sortier-Parameter.
  - Trigger-Liste: `WorkflowTrigger.objects.all()` **ohne ORDER BY** (DB-Reihenfolge).
  - Aktions-Liste: `WorkflowAction.objects.all()` **ohne ORDER BY**.
  - Workflow-Liste: `ORDER BY "order"` (Gleichstände in DB-Reihenfolge).
- PATCH Trigger/Aktion: `"id"` im Body und `str(id) != str(pk aus URL)` → 400, Django
  `HttpResponseBadRequest("ID in body does not match URL")` (text/html, kein JSON). Nur bei
  Trigger- und Aktions-Routen (nicht bei Workflows, nicht bei PUT).
- Antworten: create → 201, retrieve/update → 200, destroy → 204 ohne Rumpf.
- Keine Transaktion um den Request (kein `ATOMIC_REQUESTS`): Teilschreibungen bei späteren Fehlern
  bleiben bestehen (siehe Nebenwirkungen).
- Body: JSON (`JSONParser`; leerer Körper → `{}`). Nicht-Objekt → 400
  `{"non_field_errors":["Invalid data. Expected a dictionary, but got <list|str|int|float|bool>."]}`,
  `null` → 400 `{"non_field_errors":["No data provided"]}`. Anderer Content-Type → 415
  `{"detail":"Unsupported media type \"<ct>\" in request."}`.
- Reihenfolge wie DRF: Version/Negotiation (406) → Authentifizierung (401/403) → Modellrecht (403) →
  Body parsen (415/400) → [PATCH Trigger/Aktion: ID-Prüfung] → Objekt (404) → Validierung (400) →
  Schreiben → Antwort.

## Darstellung (Response-Felder in Reihenfolge)

### WorkflowTrigger (`WorkflowTriggerSerializer`)

| Feld | Typ | Spalte / Herkunft |
|---|---|---|
| `id` | int | |
| `sources` | [int] | `sources` (varchar, Komma-getrennt) → Liste, Duplikate entfernt (Reihenfolge erhalten), bekannte Werte als int (1..4), unbekannte als str; `""` → `[]` |
| `type` | int | smallint (1 consumption, 2 document_added, 3 document_updated, 4 scheduled) |
| `filter_path` | str/null | |
| `filter_filename` | str/null | |
| `filter_mailrule` | int/null | `filter_mailrule_id` |
| `matching_algorithm` | int | 0..5 |
| `match` | str | |
| `is_insensitive` | bool | |
| `filter_has_tags` | [int] | m2m `documents_workflowtrigger_filter_has_tags`, ORDER BY `tag.name` |
| `filter_has_all_tags` | [int] | `…_filter_has_all_tags`, ORDER BY name |
| `filter_has_not_tags` | [int] | `…_filter_has_not_tags`, ORDER BY name |
| `filter_custom_field_query` | str/null | |
| `filter_has_any_correspondents` | [int] | ORDER BY name |
| `filter_has_not_correspondents` | [int] | ORDER BY name |
| `filter_has_any_document_types` | [int] | ORDER BY name |
| `filter_has_not_document_types` | [int] | ORDER BY name |
| `filter_has_any_storage_paths` | [int] | ORDER BY name |
| `filter_has_not_storage_paths` | [int] | ORDER BY name |
| `filter_has_correspondent` | int/null | |
| `filter_has_document_type` | int/null | |
| `filter_has_storage_path` | int/null | |
| `schedule_offset_days` | int | |
| `schedule_is_recurring` | bool | |
| `schedule_recurring_interval_days` | int | |
| `schedule_date_field` | str | `added`/`created`/`modified`/`custom_field` |
| `schedule_date_custom_field` | int/null | |

### WorkflowActionEmail: `id`, `subject`, `body`, `to`, `include_document`
### WorkflowActionWebhook: `id`, `url`, `use_params`, `as_json`, `params` (JSON/null), `body` (str/null), `headers` (JSON/null), `include_document`

### WorkflowAction (`WorkflowActionSerializer`) – `order` wird **nicht** ausgegeben

`id`, `type` (1 assignment, 2 removal, 3 email, 4 webhook, 5 password_removal, 6 move_to_trash,
7 remote_ocr, 8 apply_ai_suggestions), `assign_title` (str/null), `assign_tags` ([int], name),
`assign_correspondent`, `assign_document_type`, `assign_storage_path`, `assign_owner` (int/null),
`assign_view_users`, `assign_view_groups`, `assign_change_users`, `assign_change_groups` ([int],
Nutzer/Gruppen ohne Meta-Sortierung → DB-Reihenfolge), `assign_custom_fields` ([int], ORDER BY
`customfield.created`), `assign_custom_fields_values` (JSON/null), `remove_all_tags`, `remove_tags`,
`remove_all_correspondents`, `remove_correspondents`, `remove_all_document_types`,
`remove_document_types`, `remove_all_storage_paths`, `remove_storage_paths`,
`remove_custom_fields` ([int], created), `remove_all_custom_fields`, `remove_all_owners`,
`remove_owners`, `remove_all_permissions`, `remove_view_users`, `remove_view_groups`,
`remove_change_users`, `remove_change_groups`, `email` (Objekt/null), `webhook` (Objekt/null),
`passwords` (JSON/null, wird im Klartext ausgegeben wie upstream), `ai_suggestion_fields` (JSON/null),
`ai_create_missing`, `ai_overwrite_existing`.

### Workflow (`WorkflowSerializer`)

`id`, `name`, `order`, `enabled`, `triggers` ([Trigger]), `actions` ([Aktion]).
- list/retrieve (Viewset-Queryset mit Prefetch): `triggers` ohne Sortierung (Join-Reihenfolge),
  `actions` ORDER BY `order`, `id`.
- create/update-Antwort (Prefetch-Cache verworfen): `triggers` und `actions` **beide ohne
  Sortierung** (`instance.actions.all()`), so wie upstream.

## Eingabe-Validierung (DRF 3.18.3)

Fehler je Serializer werden in Feldreihenfolge gesammelt (`{"feld": ["…"]}`); nur wenn keine
Feldfehler: `validate()` (Fehler → `{"non_field_errors": ["…"]}` bzw. Feldschlüssel).
Verschachtelt: `triggers`/`actions` → Objekt `{"<index>": {...}}` nur für fehlerhafte Einträge
(DRF 3.18 `LIST_SERIALIZER_ERRORS_AS_DICT = True`); Nicht-Liste →
`{"non_field_errors": ["Expected a list of items but got type \"dict\"."]}`.
Partial (PATCH): fehlende Felder werden übersprungen, auch Defaults (z. B. `sources`) entfallen;
die Partial-Eigenschaft gilt auch für verschachtelte Trigger/Aktionen in `PATCH /api/workflows/`.

Feld-Fehlertexte (DRF): `This field is required.`, `This field may not be null.`,
`This field may not be blank.`, `Not a valid string.`, `Ensure this field has no more than {n} characters.`,
`Null characters are not allowed.`, `A valid integer is required.`, `String value too large.`,
`Ensure this value is greater than or equal to {n}.`, `Ensure this value is less than or equal to {n}.`,
`Must be a valid boolean.`, `"{input}" is not a valid choice.`,
`Expected a list of items but got type "{type}".`, `Invalid pk "{pk}" - object does not exist.`,
`Incorrect type. Expected pk value, received {type}.`,
`Invalid data. Expected a dictionary, but got {type}.`, `workflow with this name already exists.`.
`{input}`/`{pk}` = Python-`str()` des JSON-Werts (`True`, `None`, `1.5`, `[1, 2]`, `{'a': 1}`),
`{type}` = Python-Typname (`str`, `int`, `float`, `bool`, `list`, `dict`, `NoneType`).

### Trigger-Felder (Eingabe)

| Feld | DRF-Feld | Regeln |
|---|---|---|
| `id` | IntegerField(required=False, allow_null) | |
| `sources` | MultipleChoiceField(1..4, allow_empty) | Default (nur nicht-partial) `{1,2,3}` → `"1,2,3"`; Eingabe → Liste ohne Duplikate in Eingabereihenfolge, gespeichert `",".join` |
| `type` | ChoiceField(1..4), **required** | |
| `filter_path`, `filter_filename` | CharField(max 256, allow_null, allow_blank) | `""` → null (in `validate`) |
| `filter_mailrule` | PK (`paperless_mail_mailrule`), allow_null | |
| `matching_algorithm` | ChoiceField(0..5) | |
| `match` | CharField(max 256, allow_blank) | |
| `is_insensitive`, `schedule_is_recurring` | BooleanField | |
| `filter_has_*` m2m | ManyRelatedField(PK) | Tag/Correspondent/DocumentType/StoragePath |
| `filter_custom_field_query` | CharField(allow_null, allow_blank) | `""` → null; sonst `CustomFieldQueryParser.parse` (Fehler unter `filter_custom_field_query`) |
| `filter_has_correspondent`/`_document_type`/`_storage_path` | PK, allow_null | |
| `schedule_offset_days` | IntegerField(−2147483648..2147483647) | |
| `schedule_recurring_interval_days` | IntegerField(1..2147483647) | |
| `schedule_date_field` | ChoiceField(added, created, modified, custom_field) | |
| `schedule_date_custom_field` | PK CustomField, allow_null | |

`validate`: effektiver Typ = `attrs.type` (bei verschachtelten Triggern gibt es kein `instance`,
bei `PATCH /api/workflow_triggers/{id}/` der gespeicherte Typ). Typ 1 (consumption) und
`filter_mailrule` nicht im Input und `filter_filename` und `filter_path` fehlend/null →
`{"non_field_errors":["File name, path or mail rule filter are required"]}`.

### Aktions-Felder

`type` ChoiceField(1..8, optional), `assign_title` CharField(allow_null, allow_blank),
FKs `assign_correspondent`, `assign_document_type`, `assign_storage_path`, `assign_owner` (User) PK
allow_null; m2m als ManyRelatedField; Booleans; `assign_custom_fields_values`, `passwords`,
`ai_suggestion_fields` JSONField(allow_null); `email`/`webhook` verschachtelt (allow_null).

E-Mail: `subject` (Char max 256, required), `body` (Char required), `to` (Char required),
`include_document` (Bool), `id` (Int, allow_null).
Webhook: `url` (Char max 256, required, danach `url_validator`), `use_params`, `as_json`,
`include_document` (Bool), `params`, `headers` (JSON), `body` (Char allow_null/blank), `id`.
`url_validator` (Python `urlparse`): kein Schema → `Unable to parse URI {v}, missing scheme`;
weder netloc noch path → `Unable to parse URI {v}, missing net location or path`;
Schema ∉ {http, https} → `URI scheme '{s}' is not allowed. Allowed schemes: http, https`.

`validate` (in dieser Reihenfolge, erster Fehler gewinnt):
1. `assign_title` nicht null: `""` → null; sonst Jinja2-Prüfung (`validate_workflow_template`):
   Syntaxfehler → `{"assign_title":["Template syntax error: <jinja-meldung>"]}`,
   unbekannter Filter/Test → `{"assign_title":["Template assertion error: No filter named 'x'.\n  File \"<introspection>\", line N"]}`,
   unbekannte Platzhalter → `{"assign_title":["Template references unknown placeholders: a, b"]}`.
   Bekannte Platzhalter: `correspondent, document_type, added, added_year, added_year_short,
   added_month, added_month_name, added_month_name_short, added_day, added_time, owner_username,
   original_filename, filename, created, created_year, created_year_short, created_month,
   created_month_name, created_month_name_short, created_day, created_time, doc_title, doc_url,
   doc_id, custom_fields` + Jinja-Globals `range, dict, lipsum, cycler, joiner, namespace`.
   Filter: Jinja-Standardfilter + `datetime`, `slugify`, `localize_date`. Erweiterung `loopcontrols`.
2. `assign_custom_fields_values` truthy (Objekt) → Werte `""` → null.
3. Typ 3 und `email` nicht im Input → `Email data is required for email actions`.
4. Typ 4 und `webhook` nicht im Input → `Webhook data is required for webhook actions`.
5. Typ 5: `passwords` keine nicht-leere Liste nicht-leerer (nach strip) Strings →
   `Passwords are required for password removal actions`.
6. Typ 8: `ai_suggestion_fields` keine nicht-leere Liste aus {title, tags, correspondent,
   document_type, storage_path, created} → `At least one valid field is required for apply AI
   suggestions actions, options are: ['correspondent', 'created', 'document_type', 'storage_path', 'tags', 'title']`.
(3–6 als `non_field_errors`; nur geprüft, wenn `type` im Input steht.)

### Workflow-Felder

`name` Char(max 256, required, `UniqueValidator` → `workflow with this name already exists.`,
eigene Instanz bei Update ausgenommen), `order` Int(optional), `enabled` Bool(optional),
`triggers`, `actions` (Listen, required bei POST/PUT). `validate`:
- Remote-OCR-Aktion (7) ohne Consumption-Trigger (1) → `Remote OCR actions require a consumption started trigger`
- AI-Aktion (8) ohne Nicht-Consumption-Trigger → `Apply AI suggestions actions require a trigger other than consumption started`
Fehlen `actions`/`triggers` im Input (PATCH), werden die gespeicherten des Workflows geprüft.

## Schreiben (Nebenwirkungen)

### `/api/workflow_triggers/`, `/api/workflow_actions/` (ModelSerializer.create/update)
- create: INSERT mit den validierten Spalten (inkl. explizitem `id`, falls angegeben – bestehende
  id → IntegrityError → 500), Defaults der Modelle für fehlende Spalten; danach m2m `.set()`.
  `sources` normalisiert zu Komma-String.
- update (PUT/PATCH): Spalten aus dem Input setzen, ganze Zeile speichern, m2m aus dem Input `set()`.
- Aktion mit verschachteltem `email`/`webhook`-Objekt bei create/update über diese Routen →
  DRF-`AssertionError` (writable nested) → **500** (Django-HTML „Server Error (500)“).
  `null` ist erlaubt (setzt FK auf null).
- destroy: m2m-Zeilen, Verknüpfungen zu Workflows (`documents_workflow_triggers`/`_actions`) und
  die Zeile löschen. E-Mail/Webhook-Objekte bleiben.

### `/api/workflows/`
- create: Workflow INSERT (`name`, `order` Default 0, `enabled` Default true); dann für jeden Trigger
  `update_or_create(id=trigger.id)` (Trigger-ids werden **übernommen** → bestehender Trigger wird
  aktualisiert und geteilt), für jede Aktion (ids **entfernt**) neu anlegen mit `order = Index`;
  E-Mail/Webhook: erneute Validierung mit nicht-partiellem Serializer, `update_or_create(id)`,
  FK an Aktion; m2m nur setzen, wenn im Input; Workflow-m2m `triggers`/`actions` auf die Liste setzen.
- update (PUT/PATCH): Workflow-Felder speichern; wie oben, aber Aktions-ids werden **übernommen**
  (`update_or_create`), nur übergebene Felder aktualisiert; fehlt `triggers`/`actions` (PATCH), bleiben
  sie unverändert. Danach `prune_triggers_and_actions`: **alle** Trigger und Aktionen ohne Workflow
  (auch eigenständig über `/api/workflow_triggers/` angelegte!) und alle E-Mail-/Webhook-Objekte ohne
  Aktion werden gelöscht.
- Erneute Validierung von `email`/`webhook` im Speichern schlägt bei PATCH mit unvollständigem
  Unterobjekt fehl → 400 mit den Fehlern **des Unterobjekts auf oberster Ebene** (z. B.
  `{"to":["This field is required."]}`), bereits geschriebene Änderungen bleiben (keine Transaktion).
- destroy: `documents_workflowrun`-Zeilen (CASCADE, hart gelöscht), Workflow-m2m-Zeilen, Workflow.
  Trigger/Aktionen bleiben (verwaist).

## Umsetzung (Rust)

| Modul | Inhalt |
|---|---|
| `schema.rs` | Tabellen/Spalten/m2m (alle 25 m2m-Tabellen gegen `reference/schema/postgres.sql` geprüft), Feldspezifikationen |
| `drf.rs`, `pyfmt.rs` | DRF-Felder (Int/Bool/Char/Choice/MultipleChoice/PK/ManyRelated/JSON), Python-`str`/`repr`/`int` |
| `serializers.rs` | `to_internal_value` + `validate` aller Serializer, `url_validator`, UniqueValidator |
| `cfquery.rs` | `CustomFieldQueryParser` (nur Prüfung) |
| `jinja/` | Port von Jinja2 3.1.6: Lexer, Parser, `loopcontrols`, Identifier-Tracking (`find_undeclared_variables`) |
| `load.rs` | Darstellung (eine Abfrage je Beziehung, `row_to_json`) |
| `write.rs` | create/update/`update_or_create`/m2m `set` (Einfügereihenfolge wie CPython-`set`)/Löschen/prune |
| `api.rs`, `lib.rs` | Handler, 18 Routen (`pl_core::route::drf_view`, `pl_authz::AuthUser`) |

Aktivierbare Routen-ids: `workflow_triggers.{list,create,retrieve,update,partial_update,destroy}`,
`workflow_actions.{…}`, `workflows.{…}` (Lesepfade und Fehlerpfade belegt, erfolgreiche Schreibpfade nur
per Rollback-Test, siehe unten).

Jinja-Eigenheit, bewusst nachgebaut: `Parser.is_tuple_end` ruft `test_any(extra_end_rules)` mit dem Tupel als
einem Argument auf → `extra_end_rules` greifen nie (z. B. `{% for in y %}` → `expected token 'in', got 'y'`).

## Tests / Paritätsstand

- Unit (ohne DB): 18 Tests, u. a. `jinja::tests::oracle_vectors` – **2500 Vorlagen** (211 handverlesen,
  1289 zufällige Bruchstücke, 1000 zufällige strukturierte Vorlagen) gegen echtes Jinja2 3.1.6 mit der
  paperless-Umgebung (`tests/parity/pl-workflows/jinja_oracle.py` → `src/jinja/oracle.json`): 0 Abweichungen.
- `tests/reference.rs` (`--ignored`, Referenz-DB + Django):
  - `render_matches_django`: Listen und alle Detailansichten der drei Endpunkte = Django.
  - `validation_matches_django`: 18 Trigger-, 16 Aktions-, 8 Workflow-Fehlerfälle (alle Custom-Field-Query-
    Fehlerpfade, Jinja-Syntax/Assertion/Platzhalter, URL-Prüfung, PATCH partial) – Django-Antwort =
    Rust-Fehlerobjekt (Django schreibt bei 400 nichts).
  - `write_flows_rollback`: Port der Schreibtests aus `test_api_workflows.py` in einer Transaktion mit
    Rollback (nested create, id-Behandlung, PATCH ohne/mit leeren Aktionen + prune, Remote-OCR-Regel,
    unvollständiges E-Mail-Unterobjekt, Trigger create/patch/delete, 500 bei verschachteltem Schreiben, delete).
- `tests/http.rs` (`--ignored`): ~190 HTTP-Fälle in-process (Router + `pl_authz::Authz`) gegen Django:
  Status, JSON, `Content-Type`, `Allow` gleich (alle Rollen inkl. anonym/inaktiv, v9/v10/ungültige Version,
  Paginierung, 404-Varianten, 403, 415, `null`/Liste als Body, PATCH-ID-Prüfung, Validierungsfehler).
- Harness-Katalog `tests/parity/pl-workflows/cases.json`: Schreibfälle (`write: true`, Snapshot-Reset nötig).
  Erfolgreiche Schreibantworten wurden **nicht** gegen Django verglichen (Referenzdaten unverändert lassen).

## Ausführung geplanter Trigger (W15)

Diese Crate enthält nur die API (CRUD, Validierung). Die Laufzeit der Trigger liegt in pl-consumer:
`Consumer::check_scheduled_workflows` (Typ 4: Datumsfeld `added`/`created`/`modified`/`custom_field`,
`schedule_offset_days`, `schedule_is_recurring`, `schedule_recurring_interval_days`,
`schedule_date_custom_field`, WorkflowRun-Prüfung) und `workflows::run_workflows` für „Dokument
hinzugefügt/geändert/geplant“ – siehe docs/contract/pl-consumer.md § 8.

## Manueller Workflow-Lauf (PaperRust, Idee aus paperless-ngx `dev` 3dbeca603 / #14704)

Eigene Umsetzung, nicht in v3.4.2. JSON-Formen, Statuscodes und Fehlertexte wie in paperless-ngx `dev`,
damit eine spätere ngx-Oberfläche ohne Anpassung funktioniert. Quelle zum Nachlesen:
`git -C upstream show 3dbeca603` (`documents/serialisers.py::BulkEditSerializer._validate_parameters_run_workflow`,
`documents/bulk_edit.py::run_workflow`, `documents/tasks.py::run_workflows_for_documents`,
`documents/filters.py::WorkflowFilterSet`, `documents/views.py::DocumentOperationPermissionMixin._has_document_permissions`).

### Auslösertyp `5` „Manual“

- `WorkflowTrigger.type` nimmt zusätzlich `5` an (`schema::TRIGGER_TYPES`, `TYPE_MANUAL`). Sonst gleiche
  Felder und Prüfungen wie die übrigen Dokument-Auslöser (Inhaltsabgleich, erweiterte Filter, keine
  Pflicht für Dateiname/Pfad/Mailregel).
- Ein Workflow mit diesem Auslöser läuft **nie automatisch** (kein Signal löst Typ 5 aus); nur über
  `bulk_edit` (unten). Abgleich in `pl_matching::workflow::document_matches_workflow` wie bei
  „Dokument hinzugefügt/geändert/geplant“ (`TRIGGER_MANUAL`).
- **Kein Schemawechsel:** `type` ist `smallint` mit `CHECK (type >= 0)`; Django-`choices` sind keine
  DB-Einschränkung. Datenbanken aus paperless-ngx 3.0–3.4.2 bleiben unverändert nutzbar, es gibt keine
  eigene Migration.

### `GET /api/workflows/` – neue Filter (`crate::filters`)

| Parameter | Bedeutung | ungültiger Wert |
|---|---|---|
| `id`, `id__in` | wie `ID_KWARGS` (`in`, `exact`) | 400 `{"id": ["Enter a number."]}` |
| `enabled` | `true`/`false`/`1`/`0` (BooleanWidget) | Filter entfällt |
| `trigger_type` | Workflow hat mindestens einen Auslöser dieses Typs (Unterabfrage, keine Duplikate = `distinct=True`) | 400 `{"trigger_type": ["Enter a number."]}` |

Fehler werden gesammelt (Reihenfolge `id__in`, `id`, `enabled`, `trigger_type`). Die Filter wirken auf
`count`, `results` und `all`. Sortierung unverändert `order`. Trigger- und Aktionslisten haben keine Filter.

### `POST /api/documents/bulk_edit/` mit `method = "run_workflow"` (pl-documents-write)

Request: `{"documents": [...], "method": "run_workflow", "parameters": {"workflow": <id>, "filter_documents": <bool, optional>}}`
(auch `all`/`filters` wie bei allen bulk_edit-Methoden).

Prüfreihenfolge (`bulk_edit::validate_run_workflow`), erst danach die Dokumentrechte:

| Fall | Status | Antwort |
|---|---|---|
| `workflow` fehlt | 400 | `{"non_field_errors": ["workflow not specified"]}` |
| unbekannt / kein int (`ValueError`, `TypeError`) | 400 | `{"non_field_errors": ["Workflow does not exist"]}` |
| Nutzer ohne globales `documents.view_workflow` | 403 | `{"detail": "Insufficient permissions to run this workflow"}` |
| Workflow deaktiviert | 400 | `{"non_field_errors": ["Workflow is not enabled"]}` |
| kein Auslöser vom Typ 5 | 400 | `{"non_field_errors": ["Workflow does not have a manual trigger"]}` |
| `filter_documents` kein JSON-Bool | 400 | `{"non_field_errors": ["filter_documents must be a boolean"]}` (fehlt → `true`) |
| Dokumentrechte fehlen | 403 | `Insufficient permissions` (Text, wie alle bulk_edit-Methoden) |
| sonst | 200 | `{"result": "OK"}` |

Dokumentrechte (`perms::has_document_permissions`, `Method::RunWorkflow.needs()`): Superuser immer;
sonst globales `change_document` **und** Objekt-`change` auf allen Dokumenten **und** Besitzer aller
Wurzeldokumente (oder ohne Besitzer) – wie `set_permissions`/`delete`, weil Aktionen Besitzer/Rechte
ändern und in den Papierkorb verschieben können. Kein `add_document`/`delete_document` nötig.

Nebenwirkungen: keine Änderung in der Anfrage selbst, kein bulk_edit-Audit-Eintrag (`snapshot() = None`).
Nach dem Commit wird **eine** Aufgabe eingereiht:
`documents.tasks.run_workflows_for_documents` mit kwargs `{"document_ids": [...], "workflow_id": <id>, "filter_documents": <bool>}`
(Auslöser `manual`, nicht in `TRACKED_TASKS` → keine `PaperlessTask`-Zeile, wie upstream).

### Worker (`pl_consumer::manual`, Registrierung in `pl-server/src/worker.rs`)

`Consumer::run_workflows_for_documents(ids, workflow_id, filter_documents)`:
- Workflow per id laden (auch wenn inzwischen deaktiviert, wie upstream). Fehlt er: Warnung
  `Manual workflow run requested for missing workflow <id>`, keine Ausnahme.
- Dokumente: `id IN ids`, nur Wurzeldokumente, nicht im Papierkorb, Reihenfolge `-created`.
- je Dokument `workflows::run_workflow_manual` (Typ 5): `filter_documents = true` → Auslöser-Filter
  prüfen (nicht passende Dokumente werden übersprungen); `false` → Aktionen ohne Prüfung anwenden.
  Danach wie geplante Läufe: `WorkflowRun` (Typ 5), `apply_ai_suggestions` einreihen,
  WebSocket `document_updated` (manuelle Läufe senden kein `document_updated`-Signal).
- Fehler bei einem Dokument → Fehlerlog `Error running workflow <name> on document <id>`, die übrigen
  laufen weiter. Autocommit wie upstream.
- E-Mail/Webhook-Aktionen hängen das Dokument wie bei upstream `dev` nur für „geändert/geplant“ an
  (Typ 5 nicht in der Liste, pl-mail unverändert).

## Known differences

1. `Template references unknown placeholders: …`: upstream verbindet ein Python-`set` (Reihenfolge
   hängt von `PYTHONHASHSEED` ab, nicht deterministisch); Rust sortiert alphabetisch.
2. `URI scheme '…' is not allowed. Allowed schemes: …`: upstream verbindet ein `set` (Reihenfolge
   zufällig je Prozess); Rust gibt immer `http, https` aus.
3. Jinja2-Prüfung ist ein Rust-Port von Lexer/Parser/Identifier-Tracking von Jinja2 3.1.6
   (Meldungen 1:1 übernommen, alle Statements inkl. `extends`/`include`/`import`/`from`/`macro`/`call`/
   `filter`/`with`/`autoescape`/`raw`/`break`/`continue`). Nicht portiert: Zeilen-Statements/-Kommentare
   (in paperless aus) und die Konstantenfaltung (siehe 7).
4. `filter_custom_field_query`: relative Datumsausdrücke (`dateparser`, z. B. "in 30 days") werden
   nur für einfache englische Formen erkannt (`today`, `yesterday`, `tomorrow`, `now`,
   `N <unit>(s) ago`, `in N <unit>(s)`); andere Ausdrücke, die dateparser akzeptieren würde, lehnt
   Rust mit `Expected a date or a relative date such as "today" or "in 30 days".` ab.
   CORE-REQUEST: gemeinsamer Validator in `pl-docfilter`.
5. Nur JSON-Bodies (DRF akzeptiert auch Form/Multipart). Ungültiges JSON → Meldung von pl-core
   statt Python-`json`-Meldung.
6. JSON-Ganzzahlen außerhalb i64/u64 (Python: beliebig groß) werden von serde_json als f64 gelesen.
7. Jinja-Konstantenfaltung (`optimizeconst`/`Optimizer`) ist nicht portiert: Unterausdrücke, die upstream
   zur Übersetzungszeit zu Konstanten gefaltet werden (z. B. `{{ false and ('a'|foo) }}`), prüft Rust
   trotzdem auf unbekannte Filter/Tests → Rust 400 (`No filter named 'foo'.`), upstream 201/200. In den
   2500 Orakel-Vorlagen kam der Fall nicht vor.
8. Jinja: Bezeichner-Zeichenklassen (Unicode-`\w`, `isidentifier`) und `\N{name}`-Escapes nur angenähert
   (Namen werden nicht gegen die Unicode-Datenbank geprüft).
9. `url_validator`: Sonderfälle von Python `urlsplit` (Prüfung von IPv6-Hosts in `[...]`, NFKC-Prüfung des
   netloc) nur teilweise; dort akzeptiert Rust evtl., wo upstream `Unable to parse URI …` meldet.
10. Reihenfolge ohne `ORDER BY` (Trigger-/Aktionslisten, Trigger eines Workflows, Nutzer/Gruppen in m2m,
    Aktionen in create/update-Antworten): gleiche SQL-Form wie Django, Reihenfolge bestimmt PostgreSQL.
    Die Einfügereihenfolge der m2m-Zeilen bildet die CPython-`set`-Iteration nach (`write::py_set_order`).
11. Fälle, die upstream mit 500 enden (Python-`TypeError`/`AttributeError`/`AssertionError`, z. B. `"id" in 5`
    beim PATCH, Nicht-String-Operator in der Custom-Field-Query, `AssertionError` im Jinja-Compiler,
    verschachteltes `email`/`webhook` über `/api/workflow_actions/`): Rust ebenfalls 500 (pl-core-Fehlerseite).
12. **Manueller Workflow-Lauf (PaperRust-Erweiterung, nicht in v3.4.2):** Auslösertyp `5` wird angenommen
    (v3.4.2: 400 `"5" is not a valid choice.`), `GET /api/workflows/` versteht `id`/`id__in`/`enabled`/`trigger_type`
    (v3.4.2 ignoriert sie), `bulk_edit` kennt `run_workflow` (v3.4.2: 400 `"run_workflow" is not a valid choice.`).
    Paritätsfälle gegen v3.4.2 mit diesen Eingaben weichen bewusst ab. Formen/Texte wie paperless-ngx `dev` 3dbeca603.
13. Ein Trigger vom Typ 5 bleibt in einer Datenbank, die wieder mit paperless-ngx 3.4.2 betrieben wird, liegen,
    läuft dort aber nie; der Workflow-Dialog von 3.4.2 zeigt den Typ leer und lehnt Speichern ab, bis ein anderer
    Typ gewählt ist (Django-Serializer `ChoiceField`). Daten gehen nicht verloren.
14. `run_workflows_for_documents` gibt es nur im PaperRust-Worker (upstream v3.4.2 kennt die Aufgabe nicht;
    die Celery-Brücke, über die ein Python-Worker sie hätte bekommen können, ist seit 2026-11-14 entfernt).

- **JSON-Parse-Fehlertexte** (W2 2026-11-08): `pyjson` (CPython-C-Scanner nachgebaut, Quelle
  `crates/pl-core/src/pyjson.rs`, Re-Export `pl_core::pyjson` (W11)) statt serde-Meldung.
- **W2-Schreiblauf 2026-11-08:** alle Schreib-Fälle (`cases.json` + `cases-w2.json` mit
  Komplett-Abzug aller `documents_workflow*`-Tabellen per SQL) identisch; offen nur Form-Bodies (415).

## CORE-REQUEST

1. **pl-docfilter**: gemeinsamer Validator für `custom_field_query`, z. B.
   `pub async fn validate_custom_field_query(conn: &mut PgConnection, prefix: &str, query: &str) -> Result<(), serde_json::Value /* DRF-Detail */>`,
   und Abhängigkeit pl-workflows → pl-docfilter erlauben (oder Validator nach pl-core). Bis dahin lokaler
   Port `pl_workflows::cfquery` (darf von pl-docfilter übernommen werden).
2. **pl-core**: Request-Body wie DRF-Parser (`JSONParser`, `FormParser`, `MultiPartParser`) inkl.
   Python-`json`-Fehlermeldung. Bis dahin lokal `api::parse_body` (nur JSON, sonst 415).
3. **pl-authz**: `models::WORKFLOW_TRIGGER` / `models::WORKFLOW_ACTION` (`documents.workflowtrigger`,
   `documents.workflowaction`) ergänzen; bis dahin lokal `api::M_TRIGGER` / `api::M_ACTION`.
