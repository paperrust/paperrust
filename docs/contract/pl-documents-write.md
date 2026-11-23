# Vertrag `pl-documents-write`

Upstream: paperless-ngx (Stand `upstream/`, als v3.4.2 geführt). Alle Zeilenangaben beziehen sich auf
`upstream/src/`. Abhängigkeiten der Python-Seite, deren Verhalten hier mit festgenagelt ist, wurden aus den
Wheels gelesen (nicht installiert): `django-soft-delete 1.0.23`, `drf-writable-nested 0.7.2`,
`django-auditlog 3.4.1`.

## Übersicht und Entscheidung pro Route

| id | Methode | Pfad | Upstream | Rust-Status | aktivierbar? |
|---|---|---|---|---|---|
| `documents.update` | PUT | `/api/documents/{id}/` | `documents/views.py::DocumentViewSet.update` | **fertig** (`update::update_handler`); Formular-/Multipart-Körper wie DRF (`update::document_form_data`, 2026-11-20) | **ja** (Parität 0 Unterschiede) |
| `documents.partial_update` | PATCH | `/api/documents/{id}/` | dto. (`partial=True`) | **fertig** (`update::partial_update_handler`) | **ja** |
| `documents.destroy` | DELETE | `/api/documents/{id}/` | `DocumentViewSet.destroy` | **fertig** (`update::destroy_handler`) | **ja** |
| `documents.notes.create` | POST | `/api/documents/{id}/notes/` | `DocumentViewSet.notes` (POST) | **fertig** (`notes::notes_create_handler`); Formular/Multipart wie DRF (`request.data["note"]` = letzter Wert) | **ja** |
| `documents.notes.destroy` | DELETE | `/api/documents/{id}/notes/` | `DocumentViewSet.notes` (DELETE) | **fertig** (`notes::notes_destroy_handler`) | **ja** |
| `trash.list` | GET | `/api/trash/` | `TrashView.get` | **fertig**; Parität 112/112 (`tests/trash_parity.rs`) | ja |
| `trash.action` | POST | `/api/trash/` | `TrashView.post` | **fertig** (`trash_action::trash_action_handler`) restore + empty | **ja** |
| `documents.bulk_edit` | POST | `/api/documents/bulk_edit/` | `BulkEditView.post` | **fertig** für alle Methoden; PDF (`rotate, merge, split, delete_pages, edit_pdf, remove_password`) über `hooks::set_pdf_ops` = `pdf_edit::NativePdfOps` (W13, PaperRust-Warteschlange); fehlende/ungültige `method` → 400 wie DRF (F1, 2026-11-20) | **ja** |
| `documents.reprocess` | POST | `/api/documents/reprocess/` | `ReprocessDocumentsView.post` | **fertig** für `documents=[...]` und `all=true` ohne Filter | ja |
| `documents.delete` | POST | `/api/documents/delete/` | `DeleteDocumentsView.post` | **fertig** (W23, `actions::delete_handler` → `tasks::bulk_delete_logged`) | ja |
| `documents.rotate` | POST | `/api/documents/rotate/` | `RotateDocumentsView.post` | **fertig** (W23, `actions` + `pdf_edit::Exec`; PaperRust-Warteschlange) | ja |
| `documents.merge` | POST | `/api/documents/merge/` | `MergeDocumentsView.post` | **fertig** (W23, wie rotate) | ja |
| `documents.merge_as_versions` | POST | `/api/documents/merge_as_versions/` | `MergeDocumentsAsVersionsView.post` | **fertig** (W23, `actions::merge_as_versions`) | ja |
| `documents.edit_pdf` | POST | `/api/documents/edit_pdf/` | `EditPdfDocumentsView.post` | **fertig** (W23, wie rotate) | ja |
| `documents.remove_password` | POST | `/api/documents/remove_password/` | `RemovePasswordDocumentsView.post` | **fertig** (W23, wie rotate) | ja |
| `documents.versions.destroy` | DELETE | `/api/documents/{id}/versions/{version_id}/` | `DocumentViewSet.delete_version` | **fertig** (W23, `versions.rs`) | ja |
| `documents.versions.partial_update` | PATCH | `/api/documents/{id}/versions/{version_id}/` | `DocumentViewSet.update_version_label` | **fertig** (W23, `versions.rs`); Formular/Multipart wie DRF; `version_id` nicht `\d+` → JSON-404 | ja |
| `documents.update_version` | POST | `/api/documents/{id}/update_version/` | `DocumentViewSet.update_version` | **fertig** (W23, `versions.rs`, multipart; PaperRust-Warteschlange) | ja (Streaming-Route in pl-server `upload::STREAMING_ROUTES`) |

`routes()` liefert: `documents.update`, `documents.partial_update`, `documents.destroy`,
`documents.notes.create`, `documents.notes.destroy`, `documents.bulk_edit`, `documents.reprocess`,
`trash.list`, `trash.action` und (W23) `documents.delete`, `documents.rotate`, `documents.merge`,
`documents.merge_as_versions`, `documents.edit_pdf`, `documents.remove_password`,
`documents.versions.destroy`, `documents.versions.partial_update`, `documents.update_version`. PUT/PATCH/DELETE liegen auf demselben Pfad wie `documents.retrieve`
(pl-documents) – pl-server muss die MethodRouter je Pfad zusammenführen (CORE-REQUEST 3).

### Ablauf und Nebenwirkungen (Rust = upstream-Reihenfolge)

**PUT/PATCH** (`update.rs`, upstream `views.py:1241`, `serialisers.py:1174`, drf-writable-nested 0.7.2):
1. Rechte: `IsAuthenticated` + `change_document` global; `get_object()` = gefilterte Liste
   (`pl_documents::listing::filtered_documents`, also auch Filter-Query-Parameter) + pk (Python-`int()`,
   sonst 404 `Not found.`; fehlt → `No Document matches the given query.`) + `check_object_perms`.
2. `content_doc` = neueste Version bzw. `?version=` (`Invalid version parameter` / 404). `content` bei
   Dokumenten mit Versionen → aus dem Serializer genommen, `partial=True`, später in die Version
   geschrieben (`save(update_fields=["content","modified"])`); `null` → 400 `This field may not be null.`.
3. Validierung `serializer::validate` in `Meta.fields`-Reihenfolge (`?fields=` entfernt auch
   Schreibfelder): PK-Felder (`'' → None`, `Invalid pk "x" - object does not exist.`, `Incorrect type…`),
   Tags (ManyRelatedField, erster Fehler), `title` (Trim, 128, Nullzeichen), `content`, `created`
   (alte Datetime-Strings → lokales Datum; `date.fromisoformat`-Formen inkl. ISO-Woche),
   `created_date`, ASN (0…4294967295, `document with this archive serial number already exists.`,
   danach Papierkorb-Prüfung), `owner`, `set_permissions`, `custom_fields` (je Element
   `CustomFieldInstanceSerializer.validate`: url/integer/monetary/string/select/documentlink/date; 403 bei
   fremden Link-Zielen), `remove_inbox_tags` (PUT: Standard `False`).
4. `DocumentSerializer.update`: `created_date`→`created`; entfernte documentlink-Felder →
   `remove_doclink` (Middleware-Kontext); Tags-Hierarchie; `remove_inbox_tags`; ab hier
   `set_actor(user)` (auditlog: actor + E-Mail, **ohne** remote_addr/port); Besitz-/Rechteprüfung (403
   `Insufficient permissions.`); `set_permissions_for_objects(merge=False)`; `save(update_fields=…)`
   (pre_save-Diff nur der Felder, `modified`); `tags.set` (m2m-Einträge `delete`/`add`); custom_fields:
   je Element erneute Validierung mit `partial=False`, `reflect_doclinks` + `update_or_create`
   (Aktualisierung: update_fields = Wert + `created`/`deleted_at`/`restored_at`), Fehler →
   `{"custom_fields": [{…}, {}]}` nach bereits geschriebenen Änderungen (wie upstream), übrige Instanzen
   soft-löschen (eigene transaction_id je Instanz); danach (ohne Kontext) gelöschte Instanzen hart löschen
   (auditlog DELETE, absteigend nach pk).
5. Antwort über `pl_documents::serialize_documents` (`full_perms=true`, `?fields`), danach Signal
   `document_updated` → `workflows::run_updated`, dann Index (`index.rs`).
6. Jeder `post_save` des Dokuments, jede m2m-Änderung der Tags (pre+post) und jedes
   `post_save(CustomFieldInstance)` bei Vorlagen mit `custom_fields` ruft `files::update_filename_and_move_files`
   (Sperre `MEDIA_LOCK` wie filelock, `generate_filename`/`generate_unique_filename` aus pl-templating,
   Kollision → `_01`, bereits verschobene Datei per Prüfsumme, Längenlimit 1024, Rollback, leere Ordner
   aufräumen; Versionen nur, wenn die Wurzel nicht im No-Op-Zweig zurückkehrt – upstream `return` im
   try-Block).

**Workflows „Dokument geändert“** (`workflows.rs`): Workflows/Trigger/Aktionen über
`pl_consumer::workflows::workflows_for_trigger`, Abgleich `pl_matching::document_matches_workflow`;
je Workflow `refresh_from_db`, Abbruch bei gelöscht/Papierkorb; Aktionen Zuweisung (Tags + Vorfahren
auf frischer Instanz, FK, Eigentümer, Custom Fields create/save, Titel über
`pl_templating::render_workflow_title`, Rechte `merge=True`), Entfernen (Tags/clear, FK, Rechte,
`remove_perm`, CF hart löschen), E-Mail/Webhook/Passwort über `hooks::WorkflowEffects` (Trait aus
pl-consumer, W6 registriert mit `hooks::set_workflow_effects`), Papierkorb (`softdelete::soft_delete_document`);
`save(update_fields=[title, correspondent, document_type, storage_path, owner, modified])`, `WorkflowRun`.

**DELETE** (`destroy_handler`): Index entfernen, Soft-Delete-Kaskade (`softdelete.rs`): Versionen
(QuerySet, gleiche transaction_id), dann Rückwärtsbeziehungen in Modellreihenfolge versions (die von
`get_object()` vorab geladenen Versionen werden erneut gelöscht + gespeichert), notes (Soft-Delete +
`save()`), barcodes (hart), share_links, custom_fields (Soft-Delete + `save()`), workflow_runs, dann das
Dokument (`save(update_fields=[deleted_at, restored_at, transaction_id])`). Fehler → 400 text/html
`Error deleting document, check logs for more detail.`; 204 ohne Körper.

**Notizen**: POST – `view_document`/`change_document` owner-aware über die Wurzel, `Note.objects.create`
(auditlog CREATE der Notiz), LogEntry `{"Note Added": ["None", id]}`, `save(update_fields=["modified"])`,
Index, Antwort = Notizliste; Ausnahme → 200 `{"error": "Error saving note, check logs for more detail."}`.
DELETE – `?id` Pflicht (`{"id": "This field is required."}` / `A valid integer is required.`), 404
`No Note matches the given query.`, LogEntry `{"Note Deleted": [id, "None"]}`, Soft-Delete der Notiz.

**Papierkorb** (`trash_action.rs`): `TrashSerializer`, Rechte `delete_document` (include_deleted) → 403
text `Insufficient permissions`; `restore` (nur Wurzeln, sonst 400) mit Kaskade über `transaction_id`
(Kinder: Restore + `save()`), danach Index; `empty` = `softdelete::empty_trash` (Collector-Reihenfolge:
Notes, CFIs, Dokumente je absteigend nach pk; auditlog DELETE; `cleanup_document_deletion` mit
`EMPTY_TRASH_DIR`; LogEntries der gewählten Dokumente löschen; Fehler werden wie upstream geschluckt).

**bulk_edit neu**: `modify_custom_fields` (Liste oder Dict, Wertvalidierung je Feld, `update_or_create` je
Dokument in Ordnung `-created`, danach `reflect_doclinks`, Selbstlink übersprungen, Rückbau entfernter
Links, harte Löschung, `bulk_update_documents`; Audit-Momentaufnahme `custom_fields` = CFI-ids), `delete`
(Wurzeln + Versionen, ein `transaction_id`, vorab geladene Instanzen werden erneut gelöscht, kein
Audit-Eintrag der View, Fehler geschluckt). `set_permissions` mit `merge` war schon portiert.

**Index** (`index.rs`, `PAPERRUST_INDEX_WRITER`): `queue` (Standard, alter Name `celery` gilt weiter:
`documents.tasks.index_document` / `remove_document_from_index` über pl-queue, kein PaperlessTask), `rust` (pl-indexer synchron,
`Deferred` → Aufgabe mit `countdown=60`), `off`.

---

## Gemeinsame Bausteine

### Authentifizierung / globale Rechte

- DRF `DEFAULT_AUTHENTICATION_CLASSES` = Basic, Token, Session. Ohne Anmeldung: **401**
  `{"detail":"Authentication credentials were not provided."}`, Header `WWW-Authenticate: Basic realm="api"`.
  (Erster Authenticator ist Basic → 401 statt 403.) Liefert pl-authz; lokal nur Fallback.
- `user.has_perm("documents.x")`: aktiver Superuser → wahr; inaktiv → falsch; sonst Nutzer- oder Gruppenrecht
  (`auth_user_user_permissions`, `auth_user_groups`+`auth_group_permissions`, `auth_permission.codename`,
  `django_content_type.app_label`).

### `permitted_document_ids(user, perm, include_deleted)` (permissions.py:497, 260ff.)

Dokument wird über sein **Wurzeldokument** autorisiert (`root_document_id`), eigener Besitzer/Grants einer
Version zählen nicht.

```
authorizing_id    = COALESCE(d.root_document_id, d.id)
authorizing_owner = CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE root.owner_id END
```
- nicht angemeldet → nur `authorizing_owner IS NULL`
- inaktiv → leer (auch inaktive Superuser)
- Superuser → alle
- sonst `authorizing_owner = user.id OR authorizing_owner IS NULL OR CAST(authorizing_id AS varchar) IN
  (guardian_userobjectpermission.object_pk WHERE user_id=… AND permission.codename=perm AND
  permission.content_type = documents.document UNION ALL guardian_groupobjectpermission.object_pk WHERE
  group_id IN (Gruppen des Nutzers) …)`
- Manager: `Document.objects` (ohne gelöschte) bzw. `global_objects` bei `include_deleted`.

### Auswahl `DocumentSelectionSerializer` (serialisers.py:1591–1660)

Felder in dieser Reihenfolge (bestimmt Reihenfolge der Fehlerschlüssel): `documents`, `all`, `filters`,
`excluded_documents`, danach die Felder der Unterklasse.

| Feld | Typ | Pflicht | Fehlertexte |
|---|---|---|---|
| `documents` | Liste von Ganzzahlen | nein (Selection) / **ja** (DocumentListSerializer) | `This field is required.`, `This field may not be null.`, `Expected a list of items but got type "str".`, je Element `{"0": ["A valid integer is required."]}`; Feldvalidator: `Some documents in documents don't exist or were specified twice.` (zählt `Document.objects` = nicht gelöscht, inkl. Versionen) |
| `all` | Boolean, Standard false | nein | `Must be a valid boolean.` |
| `filters` | Objekt | nein | `Expected a dictionary of items but got type "list".` |
| `excluded_documents` | Liste Ganzzahlen, Standard `[]` | nein | wie `documents` (ohne Existenzprüfung) |

DRF-Ganzzahl: akzeptiert JSON-Zahl ohne Nachkommaanteil, `"5"`, `"5.0"`, `5.0`; `true`/`false` → Fehler.
DRF-Boolean: `true,false,1,0,"1","0","true","false","True","False","TRUE","FALSE","t","f","T","F","y","n","Y","N","yes","no","Yes","No","YES","NO","on","off","On","Off","ON","OFF"`.

`validate()` (→ `non_field_errors`):
1. `all=true` → `documents` wird `[]` (falls fehlend), fertig.
2. `excluded_documents` nicht leer → `excluded_documents is only supported when all is true.`
3. `documents` fehlt → `documents is required unless all is true.`
4. Existenzprüfung erneut.

Auflösung (`DocumentSelectionMixin._resolve_document_ids`, views.py:2895): ohne `all` → `documents` unverändert
(Reihenfolge und ggf. Versions-ids). Mit `all` → `Document.objects` ∩ `permitted_document_ids(user)` (view) ∩
`root_document IS NULL`, gefiltert mit `DocumentFilterSet(filters ohne text/title_search/query/more_like_id)`,
`.distinct()`, Tantivy-Suche für `text|title_search|query|more_like_id` (`Specify only one of …` bei
Kombination), dann `exclude(pk__in=excluded_documents)`. Reihenfolge: Standardsortierung `-created` des Modells.

### Dokumentrechte für Operationen (`DocumentOperationPermissionMixin._has_document_permissions`, views.py:2962)

- Superuser → erlaubt.
- `has_perms = has_perm("documents.change_document") und keine der ids (global_objects!) außerhalb
  permitted_document_ids(user, "change_document")`.
- Besitz aller Wurzeldokumente (`owner == user oder owner IS NULL`) zusätzlich nötig für
  `set_permissions, delete, rotate, delete_pages, edit_pdf, merge_as_versions, remove_password`, für
  `merge/split` mit `delete_originals`, für `edit_pdf` mit `update_document`.
  (Achtung upstream: bei `merge/split+delete_originals` wird `has_perms and owner` auch ohne vorheriges
  `has_perms` gerechnet – Ergebnis gleich.)
- `add_document` nötig für `split, merge` und `edit_pdf/remove_password` ohne `update_document`.
- `delete_document` nötig für `delete`, `merge_as_versions`, `merge/split` mit `delete_originals`,
  `edit_pdf/remove_password` mit `delete_original` ohne `update_document`.
- Verweigert → **403** `text/html; charset=utf-8`, Body `Insufficient permissions`.
- Ausnahme in der Methode → **400** `text/html; charset=utf-8`, Body
  `Error performing {operation_label}, check logs for more detail.` (bulk_edit: `Error performing bulk edit,
  check logs for more detail.`).

Rechteprüfung kommt **nach** der Auflösung der ids (bei `all` also nach Filter/Suche).

### Aufgaben (alle hier portierten Aufgaben; Modul `enqueue`, bis 2026-11-14 `celery`)

Seit 2026-11-14 gehen alle Aufgaben über die PaperRust-Warteschlange (`pl_core::tasks::prepare_task` +
`PreparedTask::publish`); der Celery-Versand an Redis ist entfernt. Name, kwargs, `trigger_source` und die
`documents_paperlesstask`-Zeile bleiben wie upstream. Geschichte (früherer Celery-Weg):

- Broker = `PAPERLESS_REDIS`, Schlüssel-Präfix `PAPERLESS_REDIS_PREFIX`,
  Warteschlange `celery`, `LPUSH` (kombu Redis-Transport). Worker akzeptiert `application/json`.
- Nachricht Protokoll v2: Body `[[], kwargs, {"callbacks":null,"errbacks":null,"chain":null,"chord":null}]`
  base64, Header `lang, task, id, shadow, eta, expires, group, group_index, retries, timelimit, root_id,
  parent_id, argsrepr, kwargsrepr, origin, ignore_result, replaced_task_nesting, stamped_headers, stamps` +
  `trigger_source` (aus `apply_async(headers=…)`).
- `before_task_publish_handler` (signals/handlers.py:1159) legt für überwachte Aufgaben **vor dem Senden**
  eine `documents_paperlesstask`-Zeile an: `task_id`, `task_type`, `trigger_source` (Header, sonst `manual`),
  `status='pending'`, `date_created=now`, `input_data={}`, `owner_id=NULL`, `acknowledged=false`
  (`get_or_create` auf `task_id`).
  - `documents.tasks.bulk_update_documents` → `bulk_update`
  - `documents.tasks.update_document_content_maybe_archive_file` → `reprocess_document`
  - `documents.bulk_edit.delete` → `bulk_delete`

### auditlog bei bulk_edit (BulkEditView.post, views.py:3181)

Nur wenn `PAPERLESS_AUDIT_LOG_ENABLED` (Standard `true`) und das Feld in `MODIFIED_FIELD_BY_METHOD` nicht
`None` ist:

| Methode | Feld | Momentaufnahme |
|---|---|---|
| set_correspondent / set_document_type / set_storage_path | `correspondent` / `document_type` / `storage_path` | `values_list(pk, field)` → id oder null |
| add_tag / remove_tag / modify_tags | `tags` | sortierte Liste der Tag-ids je Dokument (leer = `[]`) |
| modify_custom_fields | `custom_fields` | Liste der CFI-ids nach pk |
| delete | `deleted_at` | – (nicht portiert) |
| reprocess | `checksum` | Checksumme (vorher = nachher, da asynchron) |
| set_permissions, rotate, … | – | kein Eintrag |

Je Dokument aus `Document.objects.filter(pk__in=ids)` (Standardsortierung `-created`; gelöschte fehlen) ein
`auditlog_logentry`:

| Spalte | Wert |
|---|---|
| content_type_id | `django_content_type` (`documents`,`document`) |
| object_pk | `str(id)` |
| object_id | id |
| object_repr | `str(doc)` = `created.isoformat()` + (` ` + Korrespondent-Name) + (` ` + Titel, wenn nicht leer) |
| serialized_data | NULL |
| action | 1 (UPDATE) |
| changes_text | `''` |
| changes | `{"<feld>": [alt, neu]}` (JSON-Rohwerte, keine Strings) |
| actor_id | Nutzer |
| actor_email | **NULL** (actor wird explizit gesetzt → `_set_actor` überspringt E-Mail) |
| cid | Header `X-Correlation-ID` oder NULL |
| remote_addr | erstes Element von `X-Forwarded-For` (Port entfernt) sonst `REMOTE_ADDR` |
| remote_port | `int(X-Forwarded-Port)` oder NULL |
| timestamp | jetzt |
| additional_data | `{"reason": "Bulk edit: <method.__name__>"}` (z. B. `Bulk edit: reprocess` auch für `redo_ocr`) |

Die Momentaufnahmen werden über die **aufgelösten** ids gebildet (`{doc_id: []}` für Tags auch bei Versions-ids).

---

## Route `documents.bulk_edit` – POST `/api/documents/bulk_edit/`

Quelle: views.py:3125 `BulkEditView`, serialisers.py:1824 `BulkEditSerializer`, bulk_edit.py.
Rechte: `IsAuthenticated`, Parser nur JSON (`parser_classes = (JSONParser,)`; anderer Content-Type → 415
`{"detail":"Unsupported media type \"…\" in request."}`).

Request-Felder: Auswahl (siehe oben) + `method` (Choice), `parameters` (Objekt, Standard `{}`), `from_webui`
(Boolean, Standard false).

`method`-Werte: `set_correspondent, set_document_type, set_storage_path, add_tag, remove_tag, modify_tags,
modify_custom_fields, set_permissions` und Legacy `delete, reprocess, rotate, merge, edit_pdf, remove_password,
split, delete_pages`. Fehler `"\"x\" is not a valid choice."`. (`redo_ocr` steht **nicht** in der Choice-Liste
→ ungültig, obwohl `validate_method` es kennt.) Legacy-Methoden erzeugen nur eine Log-Warnung.

Validierung (`validate`, Reihenfolge, Fehler als `non_field_errors` wenn nicht anders angegeben):
1. Auswahl-Validierung.
2. `all=true` und Methode ∈ {merge, split, delete_pages, edit_pdf, remove_password} →
   `This method does not support all=true.`
3. `source_mode` in parameters → muss `latest_version` oder `explicit_selection` sein → `Invalid source_mode`.
4. je Methode:
   - set_correspondent: Schlüssel fehlt → `correspondent not specified`; null ok; sonst unbekannt →
     `Correspondent does not exist`.
   - set_document_type: `document_type not specified` / `Document type does not exist`.
   - set_storage_path: `storage path not specified` / `Storage path does not exist`.
   - add_tag / remove_tag: `tag not specified` / `Tag does not exist` (null → `Tag does not exist`).
   - modify_tags: `add_tags not specified`, `remove_tags not specified`; Liste prüfen: `{name} must be a list`,
     `{name} must be a list of integers` (JSON-bool zählt als Ganzzahl!), `Some tags in {name} don't exist or
     were specified twice.`
   - modify_custom_fields: `add_custom_fields not specified`, `remove_custom_fields not specified`; Liste oder
     Objekt `{id: wert}`; Fehler `{name} must be a list of integers or a dict of id:value pairs`, `… see the
     log for details` (Schlüssel nicht int-bar), `Some custom fields in {name} don't exist or were specified
     twice.`; Werte über `CustomFieldInstanceSerializer` → `{"add_custom_fields": {"<id>": {…}}}`.
   - set_permissions: `set_permissions not specified`; `set_permissions` (wenn nicht null) über
     `SetPermissionsSerializer` (`{"<aktion>": "Unknown permission action."}`, `Expected a dictionary of
     items…`, Listenfehler); Nutzer/Gruppen müssen existieren (`Some users in don't exist or were specified
     twice.` / `Some groups in …`); `owner` nicht null → existierender Nutzer sonst `Specified owner cannot be
     found`; `merge` Standard false.
   - rotate: `invalid rotation degrees` (fehlt/keine Ganzzahl), `degrees must be a multiple of 90`.
   - split: nur ein Dokument (`Split method only supports one document`), `pages not specified`, `invalid
     pages specified`, `document page count is unknown`, `delete_originals must be a boolean`.
   - delete_pages: `Delete pages method only supports one document`, `pages not specified`, `pages must be a
     list`, `pages must be a list of integers`, `pages must be positive integers`.
   - merge: `delete_originals must be a boolean`, `archive_fallback must be a boolean`.
   - edit_pdf: `Edit PDF method only supports one document`, `operations not specified`,
     `{"operations": …}`, `update_document must be a boolean`, `include_metadata must be a boolean`,
     `doc index is out of bounds`, `update_document only allowed with a single output document`,
     `Page {n} is out of bounds for document with {m} pages.`
   - remove_password: `password not specified`, `password must be a string`.
   - reprocess: `remote_ocr must be a boolean`.

Ablauf: ids auflösen → bei `METHOD_NAMES_REQUIRING_USER` Nutzer, bei
`…_TRIGGER_SOURCE` `trigger_source` (`web_ui` bei `from_webui`, sonst `api_upload`) anhängen → Rechte →
Momentaufnahme alt → Methode → Momentaufnahme neu + LogEntries → **200** `{"result": "OK"}`.

Methoden im Detail (bulk_edit.py):
- `set_correspondent(ids, c)`: betroffene = `Document.objects` mit `id IN ids AND NOT (correspondent = c)`;
  bei `c = null` übersetzt Django `~Q(correspondent=None)` zu `correspondent_id IS NOT NULL`; bei `c` ≠ null zu
  `NOT (correspondent_id = c AND correspondent_id IS NOT NULL)` → schließt NULL-Zeilen ein. `UPDATE … SET
  correspondent_id = c` (ohne `modified`!). Celery `bulk_update_documents(document_ids=betroffene)` **immer**
  (auch leer). Gleich für document_type, storage_path.
- `add_tag(ids, t)`: Tag + Vorfahren (`tn_ancestors_pks`); je Tag Dokumente ohne diesen Tag; Zeilen anlegen;
  Celery nur wenn betroffen (Menge, Reihenfolge undefiniert).
- `remove_tag(ids, t)`: Tag + Nachfahren (`tn_descendants_pks`); Zeilen löschen; Celery nur wenn betroffen.
- `modify_tags(ids, add, remove)`: betroffene = existierende (nicht gelöschte) Dokumente der ids;
  add + Vorfahren, remove + Nachfahren; erst löschen, dann fehlende Paare anlegen; Celery wenn betroffene.
- `set_permissions(ids, set_permissions, owner, merge)`: `merge` → `owner` nur wo `owner IS NULL`, sonst für
  alle; dann guardian (permissions.py:178 `set_permissions_for_objects`): je Aktion mit `users`/`groups`;
  ohne merge vorhandene direkte Grants (`codename = <aktion>_document`) anderer Nutzer/Gruppen löschen; neue
  Zeilen für `<aktion>_document` (bei `change` auch `view_document`) mit `ignore_conflicts`. Celery immer.
- `reprocess(ids, remote_ocr)`: je id Quelle = id (wenn Version) sonst neueste Version (`version_index DESC,
  id DESC`… siehe `versions_newest_first`) oder id; `DISTINCT`; je Quelle Celery
  `update_document_content_maybe_archive_file(document_id=…, remote_ocr=…)`, Header `trigger_source=manual`.

### Große Metadaten-Änderungen im Hintergrund (upstream `dev` 60a35edee / #14711, eigene Umsetzung)

Quelle der Idee: `src/documents/bulk_operations.py::execute_bulk_operation`,
`views.py::BulkEditView._queue_bulk_operation` (upstream `dev`, nach v3.4.2). Rust: `bulk_task.rs`.

- **Wann:** Methode aus `QUEUED_OPERATIONS` (`set_correspondent`, `set_document_type`,
  `set_storage_path`, `add_tag`, `remove_tag`, `modify_tags`, `modify_custom_fields`,
  `set_permissions`; `Method::queueable`) **und** Auswahl ≥ `PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS`
  Dokumente (Standard **500**, `0` = nie) **und** PaperRust-Warteschlange aktiv (Django v3.4.2
  kennt die Aufgabe nicht) **und** Schlüsselwortargumente gültig (sonst liefert der sofortige Weg
  wie bisher 400). Sonst unverändert sofort.
- **Request:** Validierung, Auswahl, Rechteprüfung wie bisher; dann Aufgabe
  `documents.bulk_operations.execute_bulk_operation` mit kwargs `operation`, `document_ids`,
  `parameters` (unverändert wie gesendet), `user_id`, dazu PaperRust `actor` (Middleware-Akteur
  ja/nein) und `audit_meta` (`remote_addr`, `remote_port`, `cid`). Header `trigger_source` =
  `web_ui` bei `from_webui`, sonst `api_upload`. Antwort **200 `{"result": "OK", "task_id": "<uuid>"}`**
  (upstream-Schema: `task_id` optional – alte Clients lesen nur `result`).
- **PaperlessTask:** Typ `bulk_update`, `input_data = {"operation", "document_count"}`,
  `owner_id = user_id` (pl-queue `paperless_task::input_data_for/owner_id_for`).
- **Aufgabe:** Nutzer laden (fehlt → `DoesNotExist`), inaktiv → `PermissionDenied
  "Insufficient permissions to execute bulk operation"`, Rechte erneut prüfen (gleicher Text),
  Parameter erneut validieren (`ValidationError`), dann derselbe Weg wie der Request
  (`views::run_method`: Momentaufnahme → Methode → `bulk_update_documents` einreihen → Audit →
  Commit). Ergebnis `{"operation", "document_count"}`.

## Route `documents.reprocess` – POST `/api/documents/reprocess/`

Quelle: views.py:3379 `ReprocessDocumentsView`, serialisers.py:1820. Felder: Auswahl + `remote_ocr`
(Boolean, Standard false). Rechte: wie oben mit `method=reprocess` (kein Besitz nötig). Antwort 200
`{"result":"OK"}`. Kein auditlog (nur BulkEditView schreibt Einträge). Fehler in der Methode → 400
`Error performing document reprocess, check logs for more detail.`

## Eigene Aktionsrouten (W23, `actions.rs`)

Quelle: views.py:2962–3456 (`DocumentOperationPermissionMixin`, `*DocumentsView`), serialisers.py:1591–1822.
Alle: `permission_classes = (IsAuthenticated,)`, `parser_classes = (JSONParser,)` (leerer Körper = `{}`,
Nicht-Dict → `{"non_field_errors": ["Invalid data. Expected a dictionary, but got <typ>."]}`), Allow
`POST, OPTIONS`. Ablauf `serializer.is_valid` (Feldfehler **unter dem Feldnamen**, Reihenfolge der
Serializer-Felder: Basisfelder zuerst) → `validate()` (`non_field_errors`) → `_resolve_document_ids`
→ `_has_document_permissions` (403 `Insufficient permissions`, text/html) → `method(documents,
**parameters)` → `{"result": "OK"}`; Ausnahme in der Methode → 400 `Error performing <label>, check logs
for more detail.` (text/html). Kein auditlog-Feld-Schnappschuss (nur `BulkEditView`).

| Route | Serializer-Felder (Reihenfolge) | `validate()` | Methode / Rechte-Zusatz | Label |
|---|---|---|---|---|
| delete | `documents`?, `all`, `filters`, `excluded_documents` (Auswahl) | Auswahl | `bulk_edit.delete` (direkt, Fehler geschluckt) – Besitz + `delete_document` | `document delete` |
| rotate | Auswahl, `degrees` (Integer, Pflicht, `validate_degrees`: `degrees must be a multiple of 90`), `source_mode`, `from_webui` | Auswahl | `rotate(degrees, source_mode, user, trigger_source)` – Besitz | `document rotate` |
| merge | `documents` (Pflicht), `metadata_document_id` (Integer, null erlaubt), `delete_originals`, `archive_fallback`, `source_mode`, `from_webui` | – | `merge(...)` – `add_document`; mit `delete_originals` Besitz + `delete_document` | `document merge` |
| merge_as_versions | `documents`, `root_document_id` (Pflicht), `version_label` (Char, blank/null, max 64, strip → None) | `At least two documents are required.`, `version_label can only be used when merging one source document.`, `root_document_id must be one of the selected documents.`, `Only top-level documents can be merged as versions.`, `Documents with existing versions cannot be merged into another document.` | `merge_as_versions(root_document_id, version_label?, user)` – Besitz + `delete_document` | `document merge as versions` |
| edit_pdf | `documents`, `operations` (Liste `PdfEditOperationSerializer`, nicht leer), `delete_original`, `update_document`, `include_metadata` (Standard true), `source_mode`, `from_webui` | `Edit PDF method only supports one document`, `doc index is out of bounds`, `update_document only allowed with a single output document`, `documents[0]` (leer → IndexError 500), `Page <n> is out of bounds for document with <p> pages.` | `edit_pdf(...)` – Besitz; ohne `update_document` `add_document`; `delete_original` ohne `update_document` → `delete_document` | `PDF edit` |
| remove_password | `documents`, `password` (Char, Pflicht, getrimmt, nicht leer), `update_document`, `delete_original`, `include_metadata`, `source_mode`, `from_webui` | – | `remove_password(...)` – wie edit_pdf | `password removal` |

`source_mode`: `CharField` (getrimmt; null → `This field may not be null.`; kein String → `Not a valid
string.`), dann `validate_source_mode` (`Invalid source_mode`; gültig wie bei bulk_edit inkl.
`documents.bulk_edit`). `trigger_source` = `web_ui` bei `from_webui`, sonst `api_upload`
(`merge_as_versions` ohne). Die PDF-Methoden laufen über `pdf_edit::Exec` (gleiche Ausführung wie die
bulk_edit-Methoden) und brauchen die PaperRust-Warteschlange (ohne eingehängte Warteschlange, nur in
Crate-Tests: „nicht unterstützt“ → 404).

`merge_as_versions` (bulk_edit.py:656): in einer Transaktion `SELECT … FOR UPDATE ORDER BY id`,
`next_version_index = max(version_index der Versionen des Ziels, global) or 0`, Quellen (Eingabe-
reihenfolge) bekommen `root_document`, fortlaufenden `version_index`, ASN NULL, ggf. `version_label`
(`bulk_update`, ohne Signale/auto_now); Ziel `modified = now` und – falls ohne ASN – die erste ASN einer
Quelle (Log `took archive serial number`), übrige ASNs gehen verloren (Warnung). auditlog (falls an):
LogEntry am Ziel `{"Merged As Versions": ["None", [quellen]]}`, `additional_data {"reason": "Merged as
versions", "version_ids": [...]}`, Akteur = Nutzer (explizit, ohne E-Mail). Danach Index `remove`
(Quellen), `bulk_update_documents([ziel])` (trigger `system`), WebSocket `documents_deleted`.

## Versionen (W23, `versions.rs`)

Quelle: views.py:2135–2395. `DocumentViewSet.permission_classes = (IsAuthenticated,
PaperlessObjectPermissions)` → globales Modellrecht nach Methode (POST `add_document`, PATCH
`change_document`, DELETE `delete_document`; sonst DRF-403 JSON). Allow der Action `versions/<id>`:
`PATCH, DELETE, OPTIONS`. `version_id` nur `\d+` (sonst kein URL-Treffer → 404).

* **DELETE `versions/{version_id}/`**: Wurzel = `get_root_document(Document.objects.get(pk))` (fehlt →
  404 `{"detail":"Not found."}`, nicht-numerische pk → 500); `has_perms_owner_aware(delete_document,
  Wurzel)` sonst 403 `Insufficient permissions`; Version = `Document.objects.get(pk=version_id)` und
  Wurzel selbst oder eine ihrer Versionen, sonst 404; Wurzel selbst → 400 `Cannot delete the
  root/original version. Delete the document instead.`; Index `remove(version)`, `version.delete()`
  (Soft-Delete-Kaskade), Wurzel `modified = now`, Index `add_or_update(Wurzel)`, LogEntry
  `{"Version Deleted": ["None", id]}` + `{"reason": "Version deleted", "version_id": id}` (Akteur
  explizit), `document_updated` (Workflows + Hook). Antwort `{"result": "OK", "current_version_id":
  <neueste lebende Version nach version_index DESC NULLS LAST, id DESC, sonst Wurzel>}`.
* **PATCH `versions/{version_id}/`**: zuerst `DocumentVersionLabelSerializer` (`version_label` Pflicht,
  blank/null erlaubt, max 64, strip → leer = None), dann Wurzel/Rechte (`change_document`)/Version wie
  oben; `version.save(update_fields=["version_label"])` (auditlog-Diff der Version + Dateinamen-
  Empfänger), Wurzel `modified = now`, bei Änderung LogEntry `{"Version Label": [alt, neu]}` +
  `{"reason": "Version label updated", "version_id": id}`, `document_updated`. Antwort `{"id", "added"
  (DRF-JSONEncoder: ISO in UTC mit `Z`, Mikrosekunden nur wenn ≠ 0), "version_label", "checksum",
  "is_root"}`.
* **POST `update_version/`** (`parser_classes=[MultiPartParser]`; anderer Content-Type mit Körper → 415,
  leerer Körper → leere Daten): `DocumentVersionSerializer` (`document` FileField + `validate_document`
  wie `post_document`: leer → `The submitted file is empty.`, MIME nicht unterstützt → `File type <mime>
  not supported` außer `application/octet-stream` + `.pdf`; `version_label` Char blank/null max 64),
  dann Dokument (404) → Wurzel → `change_document` global **und** owner-aware (403 Text). Datei nach
  `mkdtemp(SCRATCH_DIR)/sanitize_filename(name)` (ohne NFC), mtime = jetzt, `consume_file(input_doc=
  ConsumableDocument(ApiUpload, datei, root_document_id=Wurzel), overrides(version_label (falls nicht
  leer), owner_id=actor_id=Nutzer))` mit `trigger_source=web_ui`. Antwort: Task-ID als JSON-String.
  Fehler dabei → 500 `Error updating document, check logs for more detail.` (text/html).

## PDF-Methoden von bulk_edit (W13, `pdf_edit.rs`)

Quelle: views.py:2944 `DocumentOperationPermissionMixin`, :3125 `BulkEditView`, serialisers.py:1824
`BulkEditSerializer` (+ `PdfEditOperationSerializer` :1739), bulk_edit.py:241–868, data_models.py
`DocumentMetadataOverrides.from_document`, versioning.py `get_root_document`/`get_latest_version_for_root`.
`merge_as_versions` ist in v3.4.2 **keine** bulk_edit-Methode (nur `/api/documents/merge_as_versions/`, Django).

**Wann Rust:** `NativePdfOps::bulk_edit` antwortet, wenn `Arc<dyn TaskQueue>` eingehängt ist (pl-server
`setup_queue`, immer). Bis 2026-11-14 blieb es mit der Celery-Brücke bei Django (`consume_file` braucht
upstream ein signed-pickle `ConsumableDocument`).

**Ablauf** (wie upstream): JSON-Dict → Felder (`documents, all, filters, excluded_documents, parameters,
from_webui`, gesammelt) → Auswahl-Validierung → `all=true` bei merge/split/delete_pages/edit_pdf/
remove_password → `This method does not support all=true.` → `source_mode` (nur wenn vorhanden; gültig:
`latest_version`, `explicit_selection`, `documents.bulk_edit`, `null` = Python-3.12-`__dict__.values()`)
→ Methoden-Validierung (Texte siehe oben) → ids auflösen → Rechte → kwargs-Prüfung (unbekannter Schlüssel
in `parameters` → `TypeError` → 400) → Methode → 200 `{"result":"OK"}`; Ausnahme → 400 `Error performing
bulk edit, check logs for more detail.`. `documents=[]` bei split/edit_pdf → `IndexError` in der
Validierung → **500**; bei delete_pages → `IndexError` in der Methode → 400. Kein auditlog
(`MODIFIED_FIELD_BY_METHOD` = None).

`edit_pdf.operations` = `ListField(child=PdfEditOperationSerializer(), allow_empty=False)`; Fehler
`{"operations": e.detail}`: `["This field may not be null."]`, `["Expected a list of items but got type
\"str\"."]`, `["This list may not be empty."]` bzw. `{"<index>": {"page": […], "rotate": […], "doc": […]}}`
(`This field is required.`, `A valid integer is required.`, `Ensure this value is greater than or equal to
1.`/`0.`, `rotate must be a multiple of 90`), Nicht-Dict-Element → `{"non_field_errors": ["Invalid data.
Expected a dictionary, but got int."]}`, `null` → `["This field may not be null."]`.

**Rechte:** Besitz aller Wurzeln für rotate, delete_pages, edit_pdf, remove_password und merge/split mit
`delete_originals`; `add_document` für merge, split, edit_pdf/remove_password ohne (wahres)
`update_document`; `delete_document` für merge/split mit `delete_originals`, edit_pdf/remove_password mit
`delete_original` ohne `update_document`. Unvalidierte Parameter (`edit_pdf.delete_original`, alle
Schalter von remove_password, `merge.metadata_document_id`) zählen nach Python-Wahrheitswert.

**Ausführung** (`Exec`, Autocommit wie upstream): Dokumente über den Default-Manager, Paar
(Wurzel, Quelle) = `_resolve_root_and_source_doc` (Wurzel auch gelöscht, Quelle = neueste nicht gelöschte
Version nach `version_index DESC NULLS LAST, id DESC` bzw. die gewählte Version); PDF-Arbeit mit pl-pdfops
in `spawn_blocking`, Ausgabe in je einem neuen `SCRATCH_DIR/tmpXXXXXXXX/` mit upstream-Dateinamen.
Overrides = `from_document` (Titel, FK-ids, Besitzer, Tag-ids nach `Tag.name`, `created`, direkte
view/change-Nutzer/Gruppen aufsteigend, Custom Fields `{field_id: value}`) + Methodenregeln (pl-pdfops
§4: Titel `(merged)`/`(split n)`, `owner_id`/`actor_id`, `skip_asn_if_exists`, ASN-Übergabe).
Aufgaben (PgQueue): `consume_file.s(input_doc, overrides).set(headers={"trigger_source": web_ui|api_upload})`
mit `ConsumableDocument(source=ConsumeFolder=1, original_file, root_document_id, mime_type)`;
merge+delete_originals: `release_archive_serial_numbers(affected)` → `apply_async(link=[delete.si(ids)],
link_error=[restore_archive_serial_numbers_task.s({id: asn})])` (Sendefehler → ASNs zurück);
split/edit_pdf+delete_original(s): ASN des gewählten Dokuments freigeben → `chord(header, delete.si([id]))
.on_error(restore.s(backup))`; remove_password+delete_original: `chord(header, delete.si([id]))` ohne
on_error; sonst `group(header)`; rotate/delete_pages/update_document: Einzelaufgabe mit `root_document_id`.
Fehlerbehandlung je Methode wie upstream (rotate/split/delete_pages nur Log; merge: nicht lesbare
Quellen übersprungen, `No documents were merged`; edit_pdf/remove_password → 400).

**Worker-Aufgaben** (`tasks.rs`, pl-server registriert sie): `documents.bulk_edit.delete(doc_ids)`
(Soft-Delete Wurzeln + Versionen mit einer `transaction_id`, Index entfernen, `documents_deleted`; Fehler
nur Log, Ergebnis `"OK"`), `restore_archive_serial_numbers_task(backup, *args)` (backup = erstes Argument;
als Errback stellt die Warteschlange wie Celery `(request, exc, traceback)` voran → upstream bekommt den
Request als `backup` und scheitert mit `AttributeError` – nachgebildet, d. h. die ASN-Wiederherstellung
nach fehlgeschlagenem Einlesen findet wie upstream **nicht** statt), `documents.tasks.empty_trash
(doc_ids=None)` (ohne ids: `deleted_at < now − EMPTY_TRASH_DELAY·24 h`; harte Löschung über
`softdelete::empty_trash`; Beat „Empty trash“ wird damit eingeplant).

**`document_updated` → LLM-Index:** `hooks::set_document_updated_hook`; PUT/PATCH ruft ihn nach
`run_workflows_updated` (Reihenfolge `documents/apps.py`). pl-server (Feature `ai`): wenn
`AIConfig.llm_index_enabled` (AI_ENABLED + Embedding-Backend) → `update_document_in_llm_index`
(`{"document_id": root}`, ohne trigger_source) in die PgQueue.
bulk_edit-Methoden lösen das Signal upstream nur über `bulk_update_documents` aus (`skip_ai_index`, dort
`update_llm_index` im Worker).

## Route `trash.list` – GET `/api/trash/`

Quelle: views.py:5590 `TrashView.get`. Rechte: `IsAuthenticated` + `TrashPermissions` (GET →
`documents.view_document`, sonst 403 `{"detail":"You do not have permission to perform this action."}`).

Abfrage: `Document.deleted_objects` (`deleted_at IS NOT NULL`) ohne Versionen, deren Wurzel ebenfalls im
Papierkorb liegt (`root_document_id NOT IN (gelöschte ids)` – SQL-`NOT IN` mit NULL-Wurzel ist wahr, da die
Unterabfrage keine NULLs enthält). Filter `_TrashPermittedObjectsFilter`: inaktiver Nutzer → leer; Superuser →
alles; sonst **nur besessene oder besitzerlose** (über `authorizing_owner`, also Besitzer der Wurzel bei
Versionen) – explizite Freigaben zählen nicht. Sortierung `ORDER BY created DESC` (Meta.ordering, keine
eindeutige Zweitsortierung – Gleichstände wie PostgreSQL liefert).

Paginierung `StandardPagination` (`page`, `page_size` ≤ 100000, Standard 25): `count`, `next`, `previous`,
(v9: `all` = alle ids), `results`. Ungültige Seite → 404 `{"detail":"Invalid page."}`.

Serialisierung: `DocumentSerializer` mit `full_perms` aus `?full_perms` (Standard false), **ohne**
`fields`/`truncate_content` (die setzt nur DocumentViewSet), ohne Prefetch → `content` ist der eigene Inhalt
(nicht der der neuesten Version), `duplicate_documents` = `[]` (View hat kein `action == "retrieve"`),
`page_count`/`versions` über eigene Abfragen (gelöschte Versionen sind per `Document.objects` ausgeblendet),
keine Seiten-Charge für `permissions` (TrashView hat kein `BulkPermissionMixin`).
Umgesetzt mit `pl_documents::serialize_documents(&state, user, &ids, &SerializeOpts { fields: None,
truncate_content: false, full_perms, action: SerializeAction::Other, viewset_queryset: false })`
(`src/trash.rs::trash_list_handler`). Parität (`tests/trash_parity.rs`, 7 Nutzer inkl. anonym/
ungültiges Token × v9/v10 × 8 Pfade mit Paginierung/`full_perms`): 112 verglichen, 112 identisch.

## Known differences

- **Review 1 N5:** `source_mode` = Dict … ist upstream „gültig“ und scheitert danach mit `TypeError`
  (400 HTML); Rust antwortet 400 JSON `Invalid source_mode`. upstream-Bug, bewusst nicht nachgebaut.
- **Review 1 N8:** JSON-Ganzzahlen > i64 → Rust „must be a list of integers“, Python „Some tags … don't exist“.
- **DRF `BooleanField`**: Strings kleingeschrieben verglichen (`"tRuE"` gültig, DRF 3.18.3).

1. **Transaktion (bulk_edit):** Datenänderungen + PaperlessTask + LogEntries in einer Transaktion, Versand
   nach Commit (upstream Autocommit). Versand über `pl_core::tasks` (W14): `prepare_task` → in der
   Transaktion `enqueue::record` nur bei `records_in_caller()` → nach Commit `publish` (PgQueue-Dispatcher,
   der die Zeile selbst anlegt). PDF-Methoden (Chords/link)
   gehen direkt an `TaskQueue`. PUT/PATCH/DELETE/Notizen/Papierkorb laufen wie upstream im
   Autocommit auf einer Verbindung (Teil-Schreibungen bei späteren Fehlern wie upstream).
2. **Celery `task-sent`-Ereignis** wird nicht gesendet (kein Celery; Aufgaben in der PaperRust-Warteschlange).
3. ~~JSON-Parse-Fehlertext~~ behoben: `ApiError::json_body_parse_error(body, e)` (Python-`json`-Text aus pl-core).
4. **auditlog `cid`** nur aus `X-Correlation-ID`.
   auditlog-Akteur (`actor_id`/`actor_email` der Middleware): gesetzt bei Session, AutoLogin, RemoteUser
   (`pl_core::client::audit_actor` + `user::request_user_source`, wie pl-objects), NULL bei Token, Basic,
   DebugAngular (DRF-Authentifizierung nach der Middleware).
5. `add_tag`: `document_ids` aufsteigend (upstream Python-`set`).
6. Typfehler in `parameters` → 500 wie upstream.
7. **Windows-Referenz:** upstream speichert unter Windows `str(Path)` mit `\` in `filename`; für einen
   bereits mit `/` gespeicherten Pfad mit Unterordnern erkennt upstream dann eine „Verschiebung“ auf sich
   selbst (`Cannot rename file since target path … already exists`) und lässt `modified` unverändert.
   Rust folgt dem Linux-Verhalten (`/`, kein Schein-Move). Der Harness vergleicht `filename` mit `/`
   normalisiert; der Restore-Fall stellt dafür die Windows-Schreibweise her.
8. **Nachgebaut seit 2026-11-18:** WebSocket `document_updated` nach PUT/PATCH und nach Versions-
   Änderungen (`hooks::send_websocket_document_updated`, W3; Reihenfolge wie `apps.py`: Workflows →
   WebSocket → LLM-Index); Aktion „KI-Vorschläge anwenden“ beim Auslöser „Geändert“ reiht
   `apply_ai_suggestions(action_id, document_id)` ein (W4, nach dem Lauf; die Verbindung hat keine
   Transaktion, also wie `delay_on_commit`); E-Mail/Webhook laufen über `email_in`/`webhook_in` mit
   dem Dokument im Speicher und derselben Verbindung (W6, kein Warten auf ein Festschreiben mehr);
   „Passwort entfernen“ (W1) über `workflow_password` (siehe unten). **Weiter nicht nachgebaut:**
   WebSocket `documents_deleted` bei PUT/PATCH, Cache-Invalidierung (`clear_document_caches`).
20. **Workflow-Aktion „Passwort entfernen“ (W1, `workflow_password.rs`):** wie
   `execute_password_removal_action`: Passwörter der Aktion (getrimmt, leere weg) der Reihe nach mit
   `bulk_edit.remove_password([doc], update_document=True, user=document.owner)` (`pdf_edit::Exec`, jetzt mit
   `user_id: Option<i64>` wie upstream `user=None`); erstes passendes → neue Version (`consume_file` mit
   `root_document_id`), Log `Unlocked document …`; keins → `could not unlock … with any configured value`.
   Unterschiede: läuft im Hintergrund und wartet, bis das Dokument festgeschrieben ist (bis 120 s), statt
   synchron mit `source_paths_by_id` = Arbeitskopie; beim Einlese-Auslöser (upstream: Empfänger an
   das *nächste* `document_consumption_finished`) meldet sich die Aktion mit der Eingangsdatei an und
   läuft, wenn der Consumer nach dem Commit „Einlesen fertig“ für genau diese Datei meldet
   (`WorkflowEffects::consumption_finished`, `workflow_password::consumption_finished`); Anmeldungen
   ohne fertiges Einlesen verfallen nach 24 h. Registriert von pl-server (`install_password_removal`)
   für Consumer und „Dokument geändert“.
9. **Filter-Query-Parameter** bei PUT/PATCH/DELETE wirken wie upstream (`get_object` über die gefilterte
   Liste); `more_like_id`/Volltext dort delegiert pl-documents.
10. `generate_filename` mit Kontextfehler (z. B. unbekannte Select-Option): upstream 500, Rust protokolliert
    und verschiebt nicht.
11. `CustomFieldInstanceSerializer` mit `id`/`pk` eines bestehenden CFI im Element: upstream `AttributeError`
    (500), Rust ebenso 500.
13. **Windows-Uhr der Referenz:** `timezone.now()` hat dort ~1 ms Auflösung; mehrere im selben Request
    angelegte Custom-Field-Instanzen bekommen gleiche `created`-Werte, die Reihenfolge in der Antwort
    (`Meta.ordering = created`) ist upstream dann zufällig. Rust (µs) liefert Anlege-Reihenfolge (wie
    upstream unter Linux). Der Harness sortiert `custom_fields` in Antworten nach `field`.
14. **PaperlessTask bei `consume_file` mit Datums-Custom-Field (upstream-Bug):** `from_document` legt
    `datetime.date`-Werte in `overrides.custom_fields` ab; `_extract_input_data` wandelt nur oberste
    Datumswerte um → `TypeError: Object of type date is not JSON serializable`, upstream loggt
    `Creating PaperlessTask failed` und legt **keine** `documents_paperlesstask`-Zeile an (die Aufgabe
    läuft trotzdem). PgQueue legt die Zeile an (Datum als ISO-Text). Bewusst nicht nachgebaut (Aufgaben
    wären sonst in `/api/tasks/` unsichtbar); im Harness als `accept` markiert (4 Fälle).
15. **PDF-Dateien:** lopdf schreibt andere Bytes als pikepdf (pl-pdfops Known differences 3/4); verglichen
    werden Seitenzahl, Drehung und Text je Seite.
16. ~~PDF-Methoden nur mit PgQueue~~ – seit 2026-11-14 gibt es nur noch die PgQueue (keine Celery-Brücke).
17. **Pfade (Review H1, wave3):** `files::join` ist auf die Wurzel begrenzt (wie
    `pl_files::serve::safe_join`, `\` und `/` als Trenner): `..` über die Wurzel, absolute Namen,
    Laufwerke/Präfixe und NUL ergeben einen ungültigen Pfad (`UNSAFE_MARKER`, jede Dateioperation
    scheitert). `validate_move` prüft Quelle **und** Ziel mit `validate_path_in_root`;
    `cleanup_document_deletion` bricht bei einem Pfad außerhalb von `ORIGINALS_DIR`/`ARCHIVE_DIR` ab
    (Log, nichts gelöscht/verschoben); PDF-Methoden lesen nur über `checked_source_path`/
    `checked_archive_path` (sonst wie ein nicht lesbares Original: rotate/split/delete_pages Log,
    merge überspringt, edit_pdf/remove_password 400); Workflow-Anhänge ebenso. Upstream
    (`Path.resolve()`) hat diese Grenze nicht – bewusst strenger.
12. Gleichstände in `-created`-Ordnungen (Versionen derselben Wurzel) löst Rust mit `id ASC` auf (so wie
    PostgreSQL es in der Referenz liefert).

- **W23 Aktionsrouten/Versionen:**
  1. `rotate/merge/edit_pdf/remove_password/update_version` über die PaperRust-Warteschlange
     (`consume_file` als JSON; upstream signed-pickle an Celery).
  2. `update_version`: Dateiname nach Djangos `sanitize_file_name` ohne `html.unescape` der Entitäten
     (`&amp;` bleibt stehen) – nur bei absichtlich kodierten Namen sichtbar.
  3. ~~`update_version` gepuffert~~ – steht in pl-server `upload::STREAMING_ROUTES`.
  4. Suchindex: `delete`/`merge_as_versions`/Versionen über `crate::index` (Modus `PAPERRUST_INDEX_WRITER`);
     im Standardmodus `queue` als Aufgaben statt synchron wie upstream.

- **Hintergrund-Massenänderung (upstream `dev` 60a35edee):**
  1. Nur ab 500 Dokumenten (einstellbar) im Hintergrund; upstream immer. Grund: alte Oberflächen
     (Angular v3.4.2, frontend-light) laden nach `OK` sofort neu – bei kleinen Auswahlen sehen sie
     die Änderung so weiterhin sofort.
  2. Den Nachlauf `bulk_update_documents` reiht die Aufgabe als **eigene** Aufgabe ein (upstream ruft
     ihn in derselben Aufgabe auf). Folge: in `/api/tasks/` stehen zwei Einträge; die erste Aufgabe ist
     fertig, sobald die Metadaten geschrieben sind.
  3. Keine WebSocket-Meldung `bulk_operation_status` an den Auslöser (upstream: Channels-Gruppe
     `bulk_operations_<user>`); die alten Oberflächen werten sie nicht aus. Nachrüsten braucht einen
     neuen Ereignistyp in pl-queue und einen Nutzerfilter in pl-server `ws.rs`.
  4. Audit-Einträge der Aufgabe behalten Adresse/Port/cid und den Akteur des Requests (upstream:
     Akteur = Nutzer, keine Adresse).
  5. Upstream prüft in `has_document_operation_permissions` zusätzlich `user.is_active`; PaperRust
     prüft das nur in der Aufgabe (im Request ist ein inaktiver Nutzer gar nicht angemeldet).
  6. Mit großer Auswahl ist die Liste direkt nach `OK` noch alt (Änderung folgt Sekunden später).

21. **Gleichzeitige Massenänderungen (2026-11-18, tests/stress/BEFUNDE.md B3–B6, bewusst):**
    - `add_tag` legt die Zuordnungen mit `ON CONFLICT DO NOTHING` an (upstream `bulk_create` ohne
      `ignore_conflicts`). Ergebnis gleich („Tag gesetzt“); upstream scheitert bei zwei gleichzeitigen
      `add_tag` derselben Dokumente gelegentlich mit `IntegrityError` (500/400).
    - Jede schreibende bulk_edit-Methode (alle außer `reprocess`, `run_workflow`) sperrt zu Beginn der
      Transaktion die ausgewählten Dokumentzeilen in id-Reihenfolge (`bulk_edit::lock_documents`, vor
      der Audit-Momentaufnahme). Gleichzeitige Änderungen derselben Dokumente laufen nacheinander statt
      sich zu verklemmen; die Momentaufnahme „alt“ zeigt den festgeschriebenen Stand. `touch_modified`
      (`docs.rs`) und das `modified`-Update in `cfi::reflect_doclinks` sperren ebenso sortiert.
    - Bricht PostgreSQL die bulk_edit-Transaktion mit `40P01`/`40001` ab, wird sie bis zu 3-mal
      vollständig wiederholt (`views::run_method`, Wartezeit 50/100 ms); erst danach 400 wie upstream.
    - **Zusammenfassen der Nacharbeit** (`coalesce.rs`): wartet schon eine nicht gestartete
      `bulk_update_documents`-Aufgabe (Zustand `queued`, `attempts = 0`, fällig, nur kwarg
      `document_ids`, zusammen ≤ 20 000 Einträge, Zeile nicht gesperrt), hängt die Änderung ihre ids
      in derselben Transaktion dort an, statt eine neue Aufgabe einzureihen (upstream: immer eine neue
      Aufgabe). Die ids bleiben mit Wiederholungen stehen; pl-consumer lässt Workflows „Dokument
      geändert“ (inkl. Webhooks/E-Mails), WebSocket und Dateinamenprüfung je Dokument so oft laufen,
      wie upstream Aufgaben gelaufen wären; Suchindex und Cache-Löschen nur einmal (wirken nur auf den
      Endstand). Folgen: weniger `bulk_update`-Einträge in `/api/tasks/` (keine `PaperlessTask`-Zeile
      für angehängte Änderungen); Reihenfolge der Workflow-Läufe bei mehrfach enthaltenen Dokumenten
      dokumentweise (D1, D1, D2) statt aufgabenweise (D1, D2, D1). Abschalten:
      `PAPERRUST_BULK_UPDATE_COALESCE=off`. Wird zwischen Anhängen und Ausführen ein Workflow „Dokument
      geändert“ erst eingeschaltet, laufen die Wiederholungen trotzdem (Zählung steht in der Aufgabe);
      wird der letzte ausgeschaltet, läuft eine Runde (ohne Workflows wirkungslos).
    - Hintergrundweg (`bulk_task`): scheitert die Aufgabe, endet sie wie bisher mit Fehlerstatus und
      Grund (`PaperlessTask` FAILURE) und meldet das zusätzlich dem Auslöser per WebSocket
      (`status_update`, Status `FAILED`, `filename` = „Bulk edit: <methode> (<n> documents)“,
      `owner_id` = Nutzer; Text ohne Interna). Vorher sah der Nutzer nach `200 OK` nichts.
    - Datenbankfehler tragen intern den SQLSTATE (`ApiError::Database`); Antwort unverändert
      (500 bzw. 400 „Error performing bulk edit …“).
18. **Ohne Django/Celery** (2026-11-14, überarbeitet 2026-11-20 nach dem Paritätslauf): Fälle, die früher
    an Django delegiert wurden, werden jetzt in Rust beantwortet:
    - **Formular-/Multipart-Körper** (`application/x-www-form-urlencoded`, `multipart/form-data`) bei
      PUT/PATCH Dokument, Notiz anlegen, Papierkorb-Aktion und Versionslabel: geparst wie DRFs
      `FormParser`/`MultiPartParser` (`form.rs`, `QueryDict`: letzter Wert je Schlüssel) und auf das
      reduziert, was die Felder bei HTML-Eingabe lesen (`Field.get_value`, `ListField`/`ManyRelatedField`
      `getlist`, `parse_html_list`, `parse_html_dict`; Details in `update::document_form_data`,
      `trash_action::form_data`, `versions::label_form_data`). Dateiteile werden übersprungen (keiner
      dieser Views liest Dateien). Folge wie upstream: PUT per Formular ohne `tags` leert die Tags,
      `set_permissions.view.users=…` meldet „Unknown permission action.“ (upstream-Verhalten von
      `SetPermissionsSerializer.to_internal_value` mit einem `MultiValueDict`).
    - **bulk_edit ohne/mit ungültiger `method`** → 400 wie DRF, alle Feldfehler in Deklarationsreihenfolge
      (`documents, all, filters, excluded_documents, method, parameters, from_webui`), z. B.
      `{"method": ["\"nope\" is not a valid choice."]}`, `{"method": ["This field is required."]}`,
      `null` → „This field may not be null.“, Nicht-Strings über Pythons `str()` (`"5"`, `"True"`).
    - **`version_id` nicht `\d+`** (`/api/documents/1/versions/abc/`): upstream passt kein URL-Muster
      (Fall-through auf die Index-Seite bzw. 302 zur Anmeldung); PaperRust antwortet wie jeder `/api/`-Pfad
      ohne Route: 404 `{"detail": "Not found."}` (Known difference).
    - **PDF-Methoden/`update_version` ohne PaperRust-Warteschlange**: pl-server hängt sie immer an; fehlt
      sie (Fehlkonfiguration), 500 statt früher 404.
    Alle Aufgaben gehen in die PaperRust-Warteschlange; Modul `celery` heißt `enqueue`.
    `PAPERRUST_INDEX_WRITER`: `queue` (Standard; `celery` als alter Name), `rust`, `off`.
19. **`bulk_edit` mit Nicht-Objekt-Body** (B-313, 2026-11-16, bewusst): `[]`, Zahl, String usw. → 400
    `{"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]}` (`null` → „No data
    provided“). upstream: `request.data.get("method")` → AttributeError → 500.
20. **Notes as Markdown – matches paperless-ngx 3.5.1, not 3.4.2** (2026-11-22, deliberate step beyond
    3.4.2 parity; upstream #14626, commits b63ea7754 + 694619c33). Source: v3.5.1
    `src/documents/serialisers.py` (`NotesSerializer`, `NoteCreateSerializer`), `views.py`
    (`DocumentViewSet.notes`), `models.py` (`Note.Format`), migration `documents.0030_note_format`.
    - Schema: `documents_note.format varchar(16) NOT NULL DEFAULT 'plain'` via PaperRust migration
      `pl_documents_write.0001_paperrust_note_format` (adds the column if missing; on databases from
      paperless-ngx 3.5 the column exists without a default – the migration sets it). No CHECK (upstream
      has none). `django_migrations` unchanged.
    - `POST /api/documents/{id}/notes/` order as in 3.5.1: view permission → `change_document`
      permission (403 „Insufficient permissions to create notes“) → validation → insert. Validation
      `NoteCreateSerializer`: `note` CharField (required, trimmed, not blank/null, no NUL characters,
      `Not a valid string.` for lists/objects/booleans), `format` ChoiceField `plain`/`markdown`
      (optional, default `plain`, `"x" is not a valid choice.`, `This field may not be null.`).
      Errors 400 `{field: [msg]}` in declaration order (`note`, `format`); non-object body →
      `{"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]}`.
      v3.4.2 answered a missing/null `note` with 200 `{"error": "Error saving note, …"}` and stored
      the text untrimmed – now as 3.5.1. The stored note is the **trimmed** text (DRF CharField).
    - Response/auditlog: note objects are `id, note, format, created, user` (3.5.1 field order); the
      auditlog create diff of a note lists `format` after `note`.
    - Parity tests against the v3.4.2 reference (`tests/write_parity.rs`, `w23_parity.rs`) differ in
      the `format` field and the validation cases above – accepted difference.

## Umsetzung (Rust)

| Modul | Inhalt |
|---|---|
| `update.rs` | PUT/PATCH (View + `DocumentSerializer.update`), DELETE, `log_ctx` |
| `serializer.rs` | Eingabevalidierung DocumentSerializer / CustomFieldInstanceSerializer, `uri_validator`, Datum |
| `docs.rs` | Dokument-Instanzen: `save(update_fields)` mit auditlog + Dateiname, Tags add/remove/clear/set |
| `cfi.rs` | CustomFieldInstance: update_or_create, create, save, soft/hard delete, restore, doclinks |
| `softdelete.rs` | Soft-Delete-Kaskade, Restore, `empty_trash` |
| `files.rs` | `update_filename_and_move_files`, `cleanup_document_deletion`, `MediaLock` |
| `workflows.rs` | Trigger „Dokument geändert“ |
| `auditlog.rs` | django-auditlog: Feldwerte, Diff, Kontexte (Middleware / set_actor / danach) |
| `notes.rs`, `trash_action.rs` | Notizen POST/DELETE, Papierkorb-Aktionen |
| `hooks.rs` | Schnittstellen `PdfOps`, `WorkflowEffects` (W6), `DocumentUpdatedHook` (pl-ai) |
| `pdf_edit.rs` | PDF-Methoden von bulk_edit (`NativePdfOps`, Validierung, Rechte, Ausführung, ASN-Freigabe) |
| `tasks.rs` | Worker-Aufgaben `bulk_edit.delete`, `restore_archive_serial_numbers_task`, `empty_trash` |
| `index.rs` | Index nachführen (`queue` / `rust` = pl-indexer / `off`) |
| `actions.rs` | W23: eigene Aktionsrouten delete/rotate/merge/merge_as_versions/edit_pdf/remove_password |
| `versions.rs` | W23: Versionen löschen/umbenennen, neue Version hochladen |
| `drf.rs`, `selection.rs`, `perms.rs`, `bulk_edit.rs`, `audit.rs`, `enqueue.rs` (bis 2026-11-14 `celery.rs`), `trash.rs`, `views.rs` | wie bisher (+ modify_custom_fields, delete) |

## Paritäts-Tests

Eigene Referenz-Kopie (`tests/parity/pl-documents-write/`): `w4env.sh` (DB `pl_w4_ref` aus
`paperless_snapshot`, Daten `work/data` aus `reference/snapshot/data.tar`, Django 127.0.0.1:8130,
Redis-Präfix `w4`), `reset.sh`, `django-start.sh`/`django-stop.sh` (nur per PID), Fälle
`write_cases.json` (75, inkl. `setup_sql` für zusätzliche Workflows), Treiber
`crates/pl-documents-write/tests/write_parity.rs` (`#[ignore]`): je Fall Reset → Django → Erfassen →
Reset → Rust in-process (Router + `pl_authz::Authz`, `ConnectInfo` 127.0.0.1) → Erfassen → Vergleich von
Status, Content-Type, Körper, Tabellen `documents_document`, `_document_tags`, `_customfieldinstance`,
`_note`, `_sharelink`, `_workflowrun`, `_documentbarcode`, guardian (ohne id), `auditlog_logentry`
(ohne id), `documents_paperlesstask` (ohne id) und Dateibaum unter MEDIA_ROOT (SHA-256, ohne `*.lock`).
Normalisierung: Zeitstempel ab Testbeginn → `<NOW>`, UUIDs → `<uuidN>` in Reihenfolge des Auftretens,
`filename`-Backslashes → `/`. Bericht: `out/report.json`.

**Ergebnis (2026-11-08, 80 Fälle):** Lauf 2 79/80 identisch; der einzige Unterschied war die
Reihenfolge gleichzeitig angelegter Custom Fields (Windows-Uhr, Known difference 13) – mit der
Sortierung im Harness erneut geprüft (siehe `out/report.json`). Abgedeckt: PATCH/PUT (Titel, Tags-
Hierarchie, Inbox, FK, created/created_date, ASN, Rechte/Eigentümer, Validierungsfehler, Custom Fields
aller Typen inkl. documentlink-Symmetrie, Versionen/`?version=`, `?fields=`, Datei verschieben inkl.
Kollision), DELETE (inkl. Versionen), Notizen POST/DELETE, Papierkorb restore/empty, bulk_edit
`modify_custom_fields`/`delete`/`set_permissions merge`, Workflows „Dokument geändert“ (Zuweisung,
Entfernen, Papierkorb, E-Mail/Webhook der Referenz-Workflows), Rechte-/Fehlerfälle (401/403/404).
Nicht abgedeckt: Formular-Körper (nur Unit-Tests, `form.rs`/`update.rs`/`trash_action.rs`), `EMPTY_TRASH_DIR`, `FILENAME_FORMAT` per Umgebung,
Barcodes (0 in der Referenz), Rust-Index-Modus `rust`.

### PDF-Methoden (W13)

Eigene Kopie `tests/parity/pl-documents-write/w13/`: `w13env.sh` (DB `pl_w13_ref` aus `paperless_snapshot`,
Daten `w13/work/data`, `SCRATCH_DIR` `w13/work/data/scratch`, Django 127.0.0.1:8150,
`PAPERRUST_QUEUE_BACKEND=postgres`), `reset.sh`, `django-start.sh`/`django-stop.sh` (nur per PID),
Overlay `overlay/pl_w13_settings.py` (Celery-Broker/-Backend im Speicher; `before_task_publish` schreibt
jede veröffentlichte Nachricht mit kwargs, Header, Callbacks/Errbacks/Chord nach `W13_TASK_LOG`; keine
Ausführung), Fälle `pdf_cases.json` (91: je Methode Erfolg, Validierungsfehler, fehlende Rechte/Besitz,
verschlüsselte PDFs über `setup_files` = `samples/password-is-test.pdf`, Versionen/`source_mode`,
ASN-Übergabe/Freigabe, kwargs-`TypeError`, 500 bei leerer Liste), Treiber
`crates/pl-documents-write/tests/pdf_parity.rs` (`#[ignore]`). Verglichen: Status, Content-Type, Körper,
`documents_document`, `auditlog_logentry`, `documents_paperlesstask`, Aufgabenplan (Einzelaufgabe mit
link/link_error, Gruppe, Chord mit Body/on_error; `consume_file`-kwargs mit `ConsumableDocument` und allen
Overrides, Dateiname statt Scratch-Pfad, `trigger_source`) und erzeugte PDFs (Seiten, Drehung, Text je
Seite, gelesen mit pl-pdfops/lopdf für beide Seiten).

**Ergebnis (2026-11-08):** Lauf 1: 87/91 identisch; die 4 Unterschiede = Known difference 14 (fehlende
PaperlessTask-Zeile upstream), danach mit `accept` markiert. Lauf 2 (nach H1-Pfadhärtung, Audit-Akteur,
JSON-Fehlertext): **91/91 identisch** (Laufzeit ~30 min). Worker-Aufgaben: `tests/tasks_db.rs`
(`#[ignore]`, gegen `pl_w13_ref`): restore (direkt + Errback-Form), `bulk_edit.delete` mit Versionen,
`empty_trash(doc_ids)` und abgelaufene.

### Aktionsrouten und Versionen (W23)

Eigene Kopie `tests/parity/pl-documents-write/w23/`: `w23env.sh` (DB `pl_w23_ref` aus `paperless_snapshot`,
Daten `w23/work/data`, Django 127.0.0.1:8190 mit dem W13-Overlay, `PAPERRUST_QUEUE_BACKEND=postgres`,
`PAPERRUST_INDEX_WRITER=rust`), `reset.sh`, `django-start.sh`/`django-stop.sh` (nur per PID), Fälle
`cases.json` (113, erzeugt von `gen_cases.py`), Treiber `crates/pl-documents-write/tests/w23_parity.rs`
(`#[ignore]`, Verfahren wie `pdf_parity.rs`, zusätzlich `documents_note`/`documents_customfieldinstance`,
beliebige Methode/Pfad, JSON-, Roh- und Multipart-Körper, Dispatcher auf die PgQueue).
Abgedeckt: alle sechs Aktionsrouten (Erfolg, Feld-/Serializer-Fehler in DRF-Reihenfolge, Rechte/Besitz,
Versionen, `source_mode`, Papierkorb, 401/403/500), `versions/<id>/` DELETE/PATCH (inkl. Wurzel, fremde
Wurzel, 404, Rechte), `update_version/` (Erfolg mit/ohne Label, PNG, über eine Versions-pk, alle
Validierungsfehler, recoverable PDF, 404/403, 415, leerer Körper).

**Ergebnis (2026-11-08):** Lauf 1 **111/113 identisch**; die 2 Unterschiede (`pw_ok`,
`pw_update_document`) = Known difference 14 (Dokument 21 hat ein Datums-Custom-Field), danach mit
`accept` markiert und erneut 2/2 identisch. Protokoll `w23/out/run1-full.log`.

## Last-Befunde B1/B2/B10 (2026-11-18)

Quelle: tests/stress/BEFUNDE.md.

- **B1:** `files::MediaLock` ist jetzt `pl_consumer::medialock::MediaLock`; `update_one`
  (`update_filename_and_move_files`) sperrt zuerst Dokument- und Stammdokumentzeile in eigener
  Transaktion bzw. Sicherungspunkt, dann MEDIA_LOCK (Regel und Known difference: pl-consumer.md
  „Last-Befunde“). Damit auch Löschen im Stapel (`docs::save` mit Medien in der Transaktion) ohne
  Verklemmung. Test: `tests/lock_order_db.rs` (`b1_row_then_media_lock_no_deadlock`, ohne die
  Zeilensperre rot).
- **B2:** keine zweite Pool-Verbindung, solange ein Handler eine hält: PATCH/PUT (`update.rs`) hält
  die Verbindung nur für Prüfen/Speichern und für Workflows/WebSocket, nicht beim Serialisieren
  (`serialize_documents` über `state`), Einreihen, Index und `document_updated`-Empfängern;
  `get_object` gibt die Verbindung vor der Rechteprüfung zurück. Ebenso `versions.rs`
  (DELETE/PATCH `versions/{id}/`): Rechteprüfung, Suchindex, KI-Aufgaben und Hook ohne gehaltene
  Verbindung. Reihenfolge der Nebenwirkungen leicht anders (WebSocket vor dem Einreihen der
  KI-Aufgaben; beim Löschen einer Version Index des Stammdokuments nach dem Audit-Eintrag) – ohne
  sichtbaren Unterschied in der Antwort.
- **B10:** schreibende Handler laufen zu Ende, auch wenn der Client abbricht – systematisch über die
  pl-server-Middleware `detach` (pl-server.md).
- **B2 (Gesamtprüfung):** `selection::resolve` gibt bei `all=true` die gehaltene Verbindung vor
  `resolve_all` (holt eigene Verbindungen) zurück und holt danach eine neue (bulk_edit, reprocess,
  delete, rotate, PDF-Methoden); Notizen (`doc_and_perm`) und PDF-Werkzeuge (`target_any`) geben die
  Verbindung vor der Rechteprüfung zurück; `overrides_from_document` liest die Berechtigten vor dem
  Holen der eigenen Verbindung.
- **Suchindex-Aufgaben nicht doppelt einreihen (Gesamtprüfung Last-Suite, bewusst):** `index::add_or_update`
  (Modus `queue`) reiht `documents.tasks.index_document(id)` nicht erneut ein, wenn für dasselbe Dokument
  schon eine noch nicht gestartete Aufgabe wartet (`queued`, `attempts = 0`, fällig, ohne Ablauf/Widerruf).
  Die wartende Aufgabe liest den Stand erst bei der Ausführung, die Änderung des Aufrufers ist dann
  schon festgeschrieben. *Known difference:* upstream reiht je Änderung eine Aufgabe ein; unter Last
  (s2, 30 Nutzer) wuchs die Warteschlange sonst schneller, als sie abgearbeitet wurde.
- **s3-Nachlauf (Verklemmung PATCH ↔ Einlesen einer Version, 40P01):** PATCH/PUT schreibt
  `apply_update` (Zeile, Tags, Felder, Rechte) und danach die Workflows „Dokument geändert“ je in einer
  Transaktion, die **zuerst die Dokumentzeile sperrt** (`FOR UPDATE`), und wiederholt sie bei 40P01/40001
  bis zu 3-mal (`begin_root_tx`, `retry_conflict`). Vorher fügte PATCH im Autocommit eine
  Tag-Zuordnung ein (Eindeutigkeitsindex) und sperrte erst danach über den Sync-Trigger die
  Dokumentzeile, während das Einlesen die Zeile hielt und dieselbe Zuordnung einfügen wollte. Inhalt
  einer Version (`?version=` bzw. neueste Version), Versionsbezeichnung und Löschen einer Version
  sperren **erst das Stammdokument, dann die Version** (Reihenfolge wie beim Einlesen), ebenfalls mit
  Wiederholung. *Known difference:* upstream schreibt ohne umgebende Transaktion; scheitert ein
  späterer Schritt (z. B. Validierungsfehler in `custom_fields` nach dem Speichern), bleibt hier nichts
  halb gespeichert.

## Ausfall-Befund C1 (tests/chaos, 2026-11-18)

Bricht die Datenbank während `bulk_edit` weg, antwortete die Route mit 400 „Error performing bulk edit,
check logs for more detail.“ (Transaktion zurückgerollt, nichts geändert). Jetzt 503 „The database is
temporarily not reachable…“ (pl-core `ApiError::db_unavailable`). `ApiError::Database` wird über
`From<sqlx::Error> for pl_core::ApiError` gerendert (503 bei nicht erreichbarer DB, sonst 500 wie bisher).
