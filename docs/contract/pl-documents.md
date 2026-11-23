# Vertrag pl-documents (paperless-ngx v3.4.2)

Quellen (alle `upstream/src/`): `documents/views.py` (`DocumentViewSet`, `UnifiedSearchViewSet`,
`SelectionDataView`, `DocumentSelectionMixin`, `PassUserMixin`, `BulkPermissionMixin`),
`documents/serialisers.py` (`DocumentSerializer`, `OwnedObjectSerializer`, `NotesSerializer`,
`CustomFieldInstanceSerializer`, `ShareLinkSerializer`, `DocumentSelectionSerializer`),
`documents/models.py`, `documents/versioning.py`, `documents/permissions.py`,
`documents/filters.py` (`PermittedObjectsFilter`, `DocumentsOrderingFilter`),
`paperless/views.py::StandardPagination`, `paperless/urls.py`, `paperless/settings/__init__.py`.

Allgemein für alle Routen:

- Auth: `PaperlessBasicAuthentication`, `TokenAuthentication`, `SessionAuthentication` (in dieser
  Reihenfolge). Nicht angemeldet → **401** `{"detail":"Authentication credentials were not provided."}`
  mit `WWW-Authenticate: Token` (bzw. `Basic realm="api"`, wenn ein `Basic`-Header gesendet wurde).
  Falsches Token → 401 `{"detail":"Invalid token."}`; inaktiver Nutzer → 401
  `{"detail":"User inactive or deleted."}`. Basic mit MFA → 401 `{"detail":"MFA required"}`.
  Session-Auth erzwingt CSRF bei POST/PUT/PATCH/DELETE → 403
  `{"detail":"CSRF Failed: <grund>"}`.
- API-Version: `Accept: application/json; version=N`, erlaubt 9 und 10, Standard 10. Andere Version →
  406 `{"detail":"Invalid version in \"Accept\" header."}`.
- JSON kompakt, UTF-8 nicht escaped. Datum/Zeit: `DateTimeField` → ISO 8601 in
  `PAPERLESS_TIME_ZONE` (Standard UTC), `+00:00` → `Z`, Mikrosekunden nur wenn ≠ 0;
  `DateField` → `YYYY-MM-DD`.
- `HttpResponseForbidden("...")` (Django, nicht DRF) → 403, `Content-Type: text/html; charset=utf-8`,
  Rumpf = Klartext. `HttpResponseBadRequest` ebenso mit 400.
- `Http404`/`NotFound` (DRF) → 404 `{"detail":"Not found."}` bzw. `{"detail":"No Document matches the given query."}`
  (wenn aus `get_object_or_404`).
- "permitted" = `permitted_document_ids(user)` (`permissions.py`): aktiver Superuser → alle; inaktiv →
  keine; sonst Dokumente mit `owner = user` ODER `owner IS NULL` ODER guardian-Recht `view_document`
  (Nutzer oder eine seiner Gruppen) – eine Version wird über ihr Root-Dokument beurteilt
  (`authorizing_id = COALESCE(root_document_id, id)`, `authorizing_owner = root.owner_id`).
  `Document.objects` blendet Papierkorb (`deleted_at IS NOT NULL`) aus, `global_objects` nicht.
- `has_perms_owner_aware(user, perm, doc)`: Root des Dokuments; `owner IS NULL` oder `owner == user`
  oder guardian `ObjectPermissionChecker.has_perm` (aktiver Superuser → wahr, inaktiv → falsch,
  sonst direkte Nutzer- oder Gruppenrechte am Root).

## Dokument-Serializer (`DocumentSerializer`)

Öffentliche Rust-Schnittstelle (für pl-documents-write, pl-search, pl-files):

```rust
pub type Viewer = pl_authz::User;                       // request.user
pub async fn serialize_documents(state: &AppState, user: &pl_authz::User, ids: &[i64],
    opts: &SerializeOpts) -> Result<Vec<serde_json::Value>, pl_core::ApiError>;
pub async fn serialize_documents_db(db: &PgPool, tz: chrono_tz::Tz, user: &pl_authz::User,
    ids: &[i64], opts: &SerializeOpts) -> Result<Vec<Value>, pl_core::ApiError>;
pub struct SerializeOpts { pub fields: Option<Vec<String>>, pub truncate_content: bool,
    pub full_perms: bool, pub action: SerializeAction /* List | Retrieve | Other */,
    pub viewset_queryset: bool /* true = wie DocumentViewSet.get_queryset (Prefetch);
                                  false = TrashView, SearchResultSerializer, GlobalSearch */ }
SerializeOpts::from_query(&pl_core::QueryParams, SerializeAction)  // wie DocumentViewSet.get_serializer
pub mod db { pub fn i64_col(..); pub fn id_in_list(..); .. }        // kleine SQL-Helfer
```

Rechte-Felder (`permissions`, `user_can_change`, `is_shared_by_requester`) über
`pl_authz::owned::OwnedInfo` (eine Charge je Aufruf), Sichtbarkeit der Duplikate über
`pl_authz::objects::permitted_document_ids`. Keine eigene Authentifizierung mehr im Crate.

Reihenfolge der Ausgabe = Reihenfolge von `ids`; unbekannte ids werden ausgelassen. Kein N+1.
Abfragen (O2, 2026-11-08, pgvalidate.md Abschnitt 9): **eine** Dokumentabfrage, die je Dokument
über korrelierte Index-Unterabfragen mitliest – Tags (`ARRAY(… ORDER BY name)`), Notizen + Nutzer
(`json_agg(… ORDER BY created)`, Zeitpunkt als Epoch-µs), Flag „hat nicht gelöschte Versionen“ –
und bei `truncate_content` nur `LEFT(content, 550)` (Python `content[0:550]` zählt Codepoints wie
PostgreSQL `left`); `content` wird nur gelesen, wenn das Feld aktiv ist. Versionen und
neueste-Version-Inhalt nur für Dokumente mit Versionen (sonst kein Roundtrip). Custom Fields bleiben
eine **eigene** Abfrage in Djangos Prefetch-Form (`ORDER BY created` hat je Dokument viele
Gleichstände – Referenz 104 Paare; nur dieselbe Abfrageform ergibt dieselbe Reihenfolge wie Django).
Danach Rechte (pl-authz `OwnedInfo`, unverändert) und Duplikate nur bei Retrieve. Werte werden aus
den Zeilen verschoben statt kopiert (doppelte ids → Kopie). Dokumentliste damit 11 → 8 Roundtrips.

Felder in Reihenfolge (`Meta.fields` ohne write-only `set_permissions`, `remove_inbox_tags`):

| Feld | Typ | Herkunft |
|---|---|---|
| `id` | int | |
| `correspondent` | int/null | FK-id |
| `document_type` | int/null | |
| `storage_path` | int/null | |
| `title` | str | |
| `content` | str | effektiver Inhalt: Inhalt der neuesten nicht gelöschten Version (`version_index DESC NULLS LAST, id DESC`), sonst eigener; nur wenn Prefetch aktiv (`content_prefetch`), sonst eigener Inhalt. `truncate_content` → erste 550 Zeichen (Codepoints) |
| `tags` | [int] | m2m, sortiert nach `Tag.name` (Meta.ordering) |
| `created` | date | `YYYY-MM-DD` |
| `created_date` | date | = `created` |
| `modified` | datetime | |
| `added` | datetime | |
| `deleted_at` | datetime/null | |
| `archive_serial_number` | int/null | |
| `original_file_name` | str/null | `original_filename` |
| `archived_file_name` | str/null | `get_public_filename(archive=True)` wenn `archive_filename` gesetzt: `str(root)` = `created.isoformat()` + (` ` + Korrespondent-Name) + (` ` + Titel, falls nicht leer), auf 251 Bytes UTF-8 gekürzt (unvollständiges Zeichen verworfen) + `.pdf`, dann `pathvalidate.sanitize_filename(replacement_text="-")` |
| `duplicate_documents` | [{id,title,deleted_at}] | nur bei `action == "retrieve"`, sonst `[]`. `global_objects` mit `checksum IN {cs, acs}` oder `archive_checksum IN {...}`, ohne sich selbst, nur Roots, `ORDER BY created DESC`, beschränkt auf permitted (inkl. Papierkorb) |
| `owner` | int/null | |
| `permissions` | {view:{users,groups},change:{users,groups}} | nur wenn `full_perms`; direkte guardian-Rechte `view_document`/`change_document`, ids aufsteigend |
| `user_can_change` | bool | nur ohne `full_perms`; owner null/gleich → true; aktiver Superuser → true; sonst direktes/Gruppen-Recht `change_document` |
| `is_shared_by_requester` | bool | nur ohne `full_perms`; `owner == user` und es existiert irgendein guardian-Recht (Nutzer oder Gruppe, beliebige Permission) auf dem Dokument |
| `notes` | [{id, note, created, user:{id,username,first_name,last_name}/null}] | nicht gelöschte Notizen, `ORDER BY created` |
| `custom_fields` | [{value, field}] | nicht gelöschte Instanzen, `ORDER BY created`; `value` je `data_type`: string→value_text, url→value_url, date→value_date (`YYYY-MM-DD`), boolean→value_bool, integer→value_int, float→value_float, monetary→value_monetary (str), documentlink→value_document_ids (JSON), select→value_select, longtext→value_long_text |
| `page_count` | int/null | Root mit Versionen → `page_count` der neuesten Version (Python-Sortierung `(version_index or 0, id)` absteigend), sonst eigener |
| `mime_type` | str | |
| `root_document` | int/null | |
| `versions` | [{id, added, version_label, checksum, is_root}] | nicht gelöschte Versionen + Root, sortiert `(version_index or 0, id)` absteigend |

`fields`-Parameter (`?fields=a,b`): Split an `,` ohne Trimmen; nur bekannte Felder bleiben; leer →
alle; nur unbekannte → `{}`. Danach `OwnedObjectSerializer.__init__`: bei `full_perms` werden
`user_can_change` und **dann** `is_shared_by_requester` entfernt – fehlt `user_can_change` schon
(KeyError), bleibt `is_shared_by_requester` erhalten (upstream-Eigenheit, nachgebaut). Ohne
`full_perms` wird `permissions` entfernt.

`full_perms` und `include_selection_data`: `documents.utils.get_boolean` – `lower() in ("yes","y","1","t","true")`. `truncate_content`: `lower() in ["true","1"]`.

## Routen

### documents.list — GET /api/documents/

`UnifiedSearchViewSet.list`. Ohne Suchparameter wie unten; mit einem der Parameter `text`,
`title_search`, `query`, `more_like_id` → Suchzweig (Abschnitt „Volltext“, Engine aus **pl-search**).

Umsetzung (`src/listing.rs::filtered_documents`): Basis `root_document_id IS NULL AND deleted_at IS
NULL` → `pl_docfilter::apply` (DjangoFilterBackend + SearchFilter) → `pl_docfilter::apply_ordering`
→ `pl_authz::objects::permitted_objects_filter` (PermittedObjectsFilter, Standard) →
`pl_core::pagination::fetch_page` (Seite + COUNT in einem Roundtrip; bei Teilstringsuche im Inhalt
– `pl_docfilter::has_content_text_filter` – `fetch_page_windowed`: `COUNT(*) OVER ()` in derselben
Abfrage, ein Scan statt zwei; Plan der Seite unverändert Seq Scan + Top-N, pgvalidate.md Abschnitt 9). Dieselbe Menge dient
`documents.retrieve` (`get_object`) und dem Suchzweig (`FilteredDocuments`).

- Rechte: `IsAuthenticated` + `PaperlessObjectPermissions.has_permission` = Modellrecht
  `documents.view_document` (sonst 403 `{"detail":"You do not have permission to perform this action."}`).
- Basis: `Document.objects.filter(root_document__isnull=True).order_by("-created","-id")`
  (nicht gelöscht, nur Roots), danach Filter-Backends:
  1. `DjangoFilterBackend` mit `DocumentFilterSet` → **pl-docfilter** (`apply`).
  2. `SearchFilter` (`search`): Begriffe (DRF smart split an Leerzeichen/Komma, Anführungszeichen
     gruppieren), jeder Begriff muss in einem von `title`, `correspondent.name`,
     `effective_content` vorkommen (`icontains`).
  3. `DocumentsOrderingFilter` (`ordering`) → **pl-docfilter** (`apply_ordering`); erlaubte Felder
     `id,title,correspondent__name,document_type__name,storage_path__name,created,modified,added,
     archive_serial_number,num_notes,owner,page_count,custom_field_<id>`; ungültige werden
     ignoriert (Standardsortierung bleibt).
  4. `PermittedObjectsFilter`: inaktiver angemeldeter Nutzer → leer; Superuser → alles; sonst
     `id IN permitted_object_ids(view_document, parent_field=root_document)`.
- Paginierung `StandardPagination`: `page` (Standard 1, `last` erlaubt), `page_size` (Standard 25,
  max 100000, ungültig/≤0 → 25). Ungültige Seite → 404 `{"detail":"Invalid page."}`.
- Antwort 200: `{"count", "next", "previous", ["all" nur API v9: alle ids der gefilterten Menge in
  Sortierreihenfolge], "results": [DocumentSerializer...]}`; mit `include_selection_data=true`
  zusätzlich `"selection_data"` am Ende (siehe unten, Zählung über die gefilterte Menge, mit
  `Count(distinct)` – auch Korrespondenten/Typen/Pfade ohne Treffer mit 0).
  `next`/`previous`: absolute URL (`build_absolute_uri`), `page` ersetzt, Query-Parameter nach
  Schlüssel sortiert und neu kodiert (`urlencode(sorted(...), doseq=True)`); `previous` auf Seite 1
  ohne `page`.
- Serializer: `action=list`, `content_prefetch = fields is None or "content" in fields`,
  `permission_grantees` gebündelt (wirkt nur auf Effizienz).

#### Volltext (Suchzweig von `UnifiedSearchViewSet.list`)

Reihenfolge wie upstream: `_validate_tantivy_search_params` (400 `{"detail":"Specify only one of
…"}`) → `filter_queryset(get_queryset())` (Filter-/Sortierfehler 400/500) →
`pl_search::views::document_search(db, backend, user, params, page_size, &FilteredDocuments)`
(Abfragefehler 400 `{"query":[…]}`, `more_like_id` 403 Klartext) → Django-Paginator über
`len(ordered_ids)` (ungültige Seite 404 `Invalid page.`) → `SearchResultPage::slice(start, stop)` →
`SearchResultSerializer`: Dokumente ohne Versions-Prefetch (`viewset_queryset=false`: eigener
Inhalt, eigene Seitenzahl), `fields`/`truncate_content`/`full_perms` wie Liste,
`"__search_hit__": {score, highlights, note_highlights, rank}` als **letzter** Schlüssel.
Antwort `{count, next, previous, [all (v9) = alle ordered_ids], results, "corrected_query": null,
["selection_data" über ordered_ids]}`. `FilteredDocuments::ids_in` liefert die ORM-Reihenfolge
(`ordering`), `all_ids` ungeordnet. Index-Backend: `pl_search::routes::backend(state)`.

`documents.retrieve` mit Suchparameter: upstream wählt `SearchResultSerializer` und scheitert an
`hit["id"]` (TypeError) → 500; Rust ebenso.

### documents.retrieve — GET /api/documents/{id}/

- `get_object()`: dieselbe Basismenge + alle Filter-Backends (Query-Parameter filtern auch hier!),
  dann `pk=id`. Nicht gefunden / nicht permitted / Version-id → 404
  `{"detail":"No Document matches the given query."}`; nicht-numerische id → 404
  `{"detail":"Not found."}` (`get_object_or_404` fängt `ValueError`, `Http404` ohne Text).
- Rechte wie Liste (Modellrecht), dann Objektrecht: Owner → ok; kein Owner → ok; sonst guardian
  `view_document` am Objekt, fehlt es → 404 `{"detail":"Not found."}` (durch PermittedObjectsFilter
  praktisch nie erreicht).
- Antwort: DocumentSerializer mit `action=retrieve` (`duplicate_documents` gefüllt).
- `?version=<id>`: wenn `content` oder `page_count` in der Antwort: Version auflösen
  (`resolve_requested_version_for_root`, `include_deleted=True`): nicht ganzzahlig → 404
  `{"detail":"Invalid version parameter"}`; nicht gefunden/fremd → 404 `{"detail":"Not found."}`;
  sonst `content = version.content or ""`, `page_count = version.page_count`. Leerer Wert
  `?version=` → neueste Version (inkl. gelöschter).

### documents.root — GET /api/documents/{id}/root/

- Rechte: `IsAuthenticated` + Modellrecht `view_document` (403 DRF). `global_objects.get(pk)`
  (auch Papierkorb, auch Versionen), fehlt → 404 `{"detail":"Not found."}`.
- Root ermitteln; `has_perms_owner_aware(view_document, root)` falsch → 403 Klartext
  `Insufficient permissions`. Antwort `{"root_id": <id>}`.
- Hinweis: die Action läuft ohne `filter_backends=[]`, wertet aber keine Filter aus (eigene Abfrage).
- Nicht-numerische id bei **allen Detail-Actions** (`root`, `notes`, `share_links`, `history`):
  `Model.objects.get(pk="abc")` wirft `ValueError` → 500 mit der Django-Fehlerseite
  (`text/html`, beginnt mit Zeilenumbruch). Rust bildet das nach.

### documents.notes.list — GET /api/documents/{id}/notes/

- Rechte: `PaperlessNotePermissions` – angemeldet und Modellrechte `documents.view_note` **und**
  `documents.view_document` (sonst 403 DRF-Text bzw. 401 ohne Anmeldung).
- `Document.objects.get(pk)` (nicht gelöscht; Root oder Version), fehlt → 404 `{"detail":"Not found."}`.
- `has_perms_owner_aware(view_document)` falsch → 403 Klartext `Insufficient permissions to view notes`.
- Antwort: Liste der Notizen (Format wie Serializer-Feld `notes`) **des angefragten Dokuments**
  (bei einer Version deren eigene Notizen). Nicht paginiert.

### documents.notes.create / documents.notes.delete — POST/DELETE /api/documents/{id}/notes/

**Nur Vertrag, bleibt Django.** Nebenwirkungen, die Rust noch nicht abbildet: `Note` anlegen /
löschen (hart), Audit-Log (`LogEntry.log_create` mit Actor/IP/cid aus AuditlogMiddleware),
`doc.save(update_fields=["modified"])` löst `post_save` aus (`update_filename_and_move_files` –
Dateien können verschoben werden, `invalidate_suggestions_cache`/`invalidate_llm_suggestions_cache`
im Django-Cache), danach Suchindex `add_or_update` (Python-Schreiber).
POST: Modellrechte `add_note, view_document, change_document`; `change_document` am Objekt fehlt →
403 `Insufficient permissions to create notes`; fehlendes `note` → Ausnahme → 200
`{"error":"Error saving note, check logs for more detail."}`. Antwort: Notizliste aus dem
**vor** dem Anlegen gefüllten Prefetch-Cache (enthält die neue Notiz nicht – upstream-Eigenheit).
DELETE: `?id=` fehlt → 400 `{"id":"This field is required."}`, nicht ganzzahlig → 400
`{"id":"A valid integer is required."}`, Notiz nicht an diesem Dokument → 404
`{"detail":"No Note matches the given query."}`; 403 `Insufficient permissions to delete notes`.

### documents.share_links — GET /api/documents/{id}/share_links/

- Rechte: `IsAuthenticated` + Modellrecht `view_document`. `Document.objects.get(pk)` fehlt → 404
  `{"detail":"Not found."}`; `has_perms_owner_aware(change_document)` falsch → 403 Klartext
  `Insufficient permissions to add share link`.
- Antwort: nicht gelöschte ShareLinks des Dokuments, ohne abgelaufene (`expiration < now` raus,
  `NULL` bleibt), `ORDER BY created DESC`:
  `[{"id","created","expiration","slug","document","document_title","file_version"}]`.

### documents.history — GET /api/documents/{id}/history/

- `PAPERLESS_AUDIT_LOG_ENABLED` falsch → 400 Klartext `Audit log is disabled`.
- Rechte: `IsAuthenticated` + Modellrecht `view_document`; `Document.objects.get(pk)` fehlt → 404.
  Nutzer ohne Modellrecht `auditlog.view_logentry` oder (Root hat Owner ≠ Nutzer und kein Superuser)
  → 403 Klartext `Insufficient permissions`.
- Einträge: `auditlog_logentry` mit `content_type = documents.document` und `object_id = id`
  (`ORDER BY timestamp DESC`) → `{"id","timestamp","action","changes","actor":{id,username}/null}`;
  danach für alle nicht gelöschten CustomFieldInstances des Dokuments (`content_type =
  documents.customfieldinstance`, `object_id IN (...)`) →
  `changes = {"custom_fields":{"type":"custom_field","field": repr.split(":")[0].strip(),
  "value": repr.split(":")[1].strip()}}`. Gesamt stabil nach `timestamp` absteigend sortiert.
  `action`: 0 `create`, 1 `update`, 2 `delete`, 3 `access` (übersetzt nach Sprache der Anfrage).

### documents.next_asn — GET /api/documents/next_asn/

- Rechte: `IsAuthenticated` + Modellrecht `view_document`.
- `MAX(archive_serial_number)` über alle Dokumente **inkl. Papierkorb** (upstream `Document.objects`
  ohne Papierkorb, Known differences 11), alle inkl. Versionen, **ohne** Rechtefilter, `default=0`,
  Antwort `max + 1` als nackte Zahl.

### documents.selection_data — POST /api/documents/selection_data/

Körper (seit 2026-11-20, vorher „nicht unterstützt“ → 404): `parser_classes = (MultiPartParser, JSONParser)`
wie upstream – JSON wie bisher (Parse-Fehler 400 mit CPython-Text über `pl_core::pyjson`), Multipart über
`views::selection_form_data` (HTML-Eingabe wie DRF: `documents`/`excluded_documents` per `getlist` bzw.
`documents[0]=…`, `all` fehlt → false, `filters` = `parse_html_dict` mit Präfix `filters.`, immer ein Dict),
jeder andere Content-Type (auch `application/x-www-form-urlencoded`) → 415
`{"detail":"Unsupported media type \"…\" in request."}` (Formular über das pl-server-Body-Gate). Leerer
Körper → `{}`.

- Rechte: `IsAuthenticated` + `ViewDocumentsPermissions` (Modellrecht `view_document`).
  Parser: JSON und Multipart.
- Request (`DocumentSelectionSerializer`): `documents` [int], `all` bool (Standard false),
  `filters` dict, `excluded_documents` [int] (Standard []).
  Fehler (400): `{"documents":["Expected a list of items but got type \"str\"."]}`,
  `{"documents":{"0":["A valid integer is required."]}}`,
  `{"non_field_errors":["excluded_documents is only supported when all is true."]}`,
  `{"non_field_errors":["documents is required unless all is true."]}`,
  `{"non_field_errors":["Some documents in documents don't exist or were specified twice."]}`
  (Existenz über `Document.objects`).
- `all=true`: ids = permitted ∩ Roots ∩ `DocumentFilterSet(filters)` (pl-docfilter) ∩ Volltext
  (pl-search, falls Suchparameter) − `excluded_documents`.
- Danach: `Document.objects.filter(pk__in=ids).count() != len(ids)` oder nicht alle permitted →
  403 Klartext `Insufficient permissions`.
- Antwort: `{"selected_correspondents":[{id,document_count}], "selected_tags", "selected_document_types",
  "selected_storage_paths", "selected_custom_fields"}` – **alle** Objekte (ohne Rechtefilter).
  Abfrage je Beziehung `Model.objects.annotate(document_count=Count(Case(When(<rel>__id__in=ids,
  then=1))))` = `LEFT JOIN` + `GROUP BY o.id`, **ohne ORDER BY** (Django ≥ 3.1 wendet
  `Meta.ordering` bei GROUP BY nicht an) → Reihenfolge des PostgreSQL-Plans (HashAggregate). Rust
  setzt dieselbe Abfrageform ab. Join ohne Soft-Delete-Filter (gelöschte Custom-Field-Instanzen
  zählen mit).
  **Reihenfolge bei großen Beständen (gemessen 100k, docs/contract/pgvalidate.md Abschnitt 8):** die
  Ausgabe eines HashAggregate hängt von der Reihenfolge der Eingabezeilen ab (Kollisionen in der
  Hash-Tabelle), also von der Reihenfolge des Seq Scans über `documents_document`. Bei Tabellen
  > `shared_buffers/4` startet PostgreSQL Seq Scans per `synchronize_seqscans` an der zuletzt
  gemeldeten Position, die von vorangegangenen Abfragen abhängt. Upstream liefert dadurch selbst
  zwischen zwei Läufen verschiedene Reihenfolgen (100k: drei verschiedene Django-Antworten in drei
  Runden der Phase-6-Messung; Rust war in Runde 1 (alle Nutzer) und Runde 2 (alice, bob) byte-gleich mit Django). Eine "genau upstream"-Reihenfolge ist deshalb nicht
  herstellbar; Rust behält die identische SQL-Form (gleicher Plan ⇒ bei gleicher Eingabereihenfolge
  gleiche Ausgabe) und Inhalte (`id`, `document_count`) sind gleich. Beim Referenzbestand (200
  Dokumente, kein synchronisierter Scan) ist die Reihenfolge stabil und gleich (Paritätslauf).
- Mit `include_selection_data=true` in der **Liste** (`_get_selection_data_for_queryset`) dagegen:
  Korrespondent/Typ/Pfad `Count(distinct, filter=...)` mit GROUP BY → Ausgabe nach `id`;
  Tags/Custom Fields über `annotate_document_count_by_ids` (ohne Aggregat) → `Meta.ordering`
  (`name` bzw. `created`), Custom-Field-Instanzen nur nicht gelöschte.
- **Abfrageform seit PERF-SCALE Q3 (2026-11-10, `src/selection_data.rs`):** eine Anfrage
  `WITH sel AS MATERIALIZED (<Auswahl>) <5 Zählungen UNION ALL>` – Auswahl = die gefilterte
  Dokumentabfrage als Unterabfrage (Liste; keine ID-Liste mehr über die Leitung) bzw.
  `id = ANY($1::int8[])` (POST, Volltexttreffer). FK-Zählungen aus `sel`, Tags/Custom Fields über
  die Zwischentabelle mit `document_id IN (SELECT id FROM sel)`; `COUNT(*)` (Paare UNIQUE, ids
  eindeutig → gleich `COUNT(DISTINCT)`). Reihenfolge explizit je Beziehung (`row_number()`):
  Liste wie oben, mit `id` als Gleichstandsregel bei Tags (`name`) und Feldern (`created`);
  **POST fest nach `id`** (upstream planabhängig, s. o.) – Paritätsregel: der Harness vergleicht
  die `selected_*`-Listen der POST-Antwort nach `id` sortiert
  (`tests/parity/harness/plharness/normalize.py`, Zähler `selection-data-post-by-id`); Inhalte
  (`id`/`document_count`) werden weiter exakt verglichen. Differenztest alt ↔ neu: 0 verschieden
  (docs/PERF-SCALE.md §9).
- **Große Auswahlen (PERF-SCALE §12, 2026-11-13):** POST mit `all=true` setzt die Auswahl als
  Unterabfrage ein (`selection_filter::resolve_all_query`) statt die ids nach Rust zu holen und
  als Liste zurückzuschicken (1 Mio. ids). Die beiden Prüfungen (`count() != len(ids)`,
  `exclude(permitted).exists()`) entfallen dort, weil die Menge per Bau nur nicht gelöschte,
  sichtbare Wurzeldokumente mit eindeutigen ids enthält – sie wären immer erfüllt. Die Zählabfrage
  läuft in einer Transaktion mit `SET LOCAL work_mem = '96MB'` (materialisierte Auswahl und
  Hash der Auswahl-ids lagerten bei 1 Mio. Dokumenten sonst in Temporärdateien aus). Teilindex
  `paperrust_doc_live_fk` (pl-store `0003`) liefert die Auswahlspalten ohne Heap-Zugriff.
  Ergebnis unverändert.
- **Auswahl „alle“ ohne Filter aus den Label-Zählern (PERF-SCALE §13, 2026-11-14):** Liste mit
  `include_selection_data` ohne wirksamen Filter (`pl_docfilter::ast::parse` → `Pred::True`, auch
  `?search=` leer) und POST `all=true` ohne wirksamen Filter (keiner der Volltext-Schlüssel
  vorhanden, nachsichtige ORM-Filter → `Pred::True`; `excluded_documents` erlaubt) rechnen über
  `paperrust_label_count` (+ `_delta`, = `paperrust_label_count_live`) des eigenen Datenmodells
  (Stufen 1–3) statt über die Auswahl (`selection_data::for_all_unfiltered`). Die Zähler zählen
  nicht gelöschte Dokumente samt Versionen je Label und Sichtbarkeits-Besitzer (Besitzer der
  Wurzel, 0 = ohne). Für die Auswahl „sichtbare, nicht gelöschte Wurzeln außer Ausschlüssen“ gilt
  `Zahl = Σ Eimer B − Beitrag(lebende Versionen in B) − Beitrag(ausgeschlossene Wurzeln in B)
  + Beitrag(freigegebene fremde Wurzeln, nicht ausgeschlossen)` mit B = alle Eimer (Superuser)
  bzw. `{uid, 0}` (eingeschränkt; Sichtbarkeit der Freigaben wie die Liste,
  `docsql::Visibility::root_row`). Custom Fields: der Zähler führt nur nicht gelöschte Feldwerte
  (= Liste, `CustomFieldInstance.objects`); POST zählt wie upstream auch gelöschte Werte der
  Auswahl, die kommen direkt dazu (Teilindex `paperrust_field_value_deleted`, pl-store `0004`,
  läuft nach den Umbaustufen). Objektliste und Reihenfolge unverändert (gemeinsamer Teil
  `objects_sql`). Ohne Zähler (Datenbank vor Stufe 3), mit Filter, mit `PAPERRUST_QUERY_ENGINE=legacy`
  oder für inaktive Nutzer bleibt es beim bisherigen Weg. Gleichheit: byte-gleiche Antworten
  alt ↔ neu auf 1 Mio. Dokumenten, auch mit eingefügten Versionen, gelöschten Feldwerten und
  Freigaben (PERF-SCALE §13).

### documents.metadata — GET /api/documents/{id}/metadata/ (W23, `src/metadata.rs`)

Quelle: `DocumentViewSet.metadata`, `get_metadata`, `get_filesize`, `_resolve_request_and_root_doc`,
`_get_effective_file_doc`/`_resolve_file_doc`, `conditionals.metadata_etag`/`metadata_last_modified`,
`DocumentBarcodeSerializer`.

1. `initial()`: `IsAuthenticated` + `PaperlessObjectPermissions` (GET → `documents.view_document`).
2. `pk`: `int(pk)`; ungültig → 500 (ValueError in `metadata_etag`, wie upstream); außerhalb i64 → 404.
3. `condition()`: ETag `"<checksum>:<modified.isoformat()>"` (Python-isoformat, `+00:00`,
   Mikrosekunden nur ≠ 0), Last-Modified `modified` – beide vom effektiven Dokument
   (`resolve_effective_document_by_pk`, ohne Papierkorb; pl-files `resolve::effective_by_pk`).
   `If-None-Match`/`If-Modified-Since` → 304, `If-Match`/`If-Unmodified-Since` → 412
   (`pl_files::conditional`).
4. View: `Document.objects.get(id=pk)` (ohne Papierkorb) → 404 `{"detail":"Not found."}`; Root-Recht
   `view_document` (owner-aware) sonst **zurückgegebenes** `403 Insufficient permissions`
   (text/html, mit ETag/Last-Modified/Cache-Control wie upstream). Effektives Dokument: Version
   direkt, sonst neueste bzw. `?version=` (ungültig → 404 `Invalid version parameter`).
5. Antwort (Reihenfolge): `original_checksum, original_size, original_mime_type, media_filename,
   has_archive_version, original_metadata, archive_checksum, archive_media_filename,
   original_filename, archive_size, archive_metadata, barcodes[{page,value,format}], lang`.
   `*_metadata`: Datei fehlt → `null`, kein Parser → `[]`, sonst `parser.extract_metadata`
   (pl-parsers, Registry wie Consumer). `archive_*` nur mit Archivversion. `barcodes` nach
   `page, id`. `lang` = `langdetect.detect(content)` (Port `src/langdetect.rs`, Profile
   `data/langdetect/`, Apache-2.0), Fehler („No features in text“) → `"en"`.
6. Alle zurückgegebenen Antworten (200, 304, 403, 412) mit `Cache-Control: no-cache`; ausgelöste
   (404/500/401/403 aus DRF) ohne.

### documents.suggestions — GET /api/documents/{id}/suggestions/ (W23, `src/suggestions.rs`)

Quelle: `DocumentViewSet.suggestions`, `matching.match_*` (mit `user=request.user`),
`classifier.load_classifier`, `get_date_parser`, `NUMBER_OF_SUGGESTED_DATES`.

1. `initial()` wie oben; `condition()` ohne ETag/Last-Modified (siehe Known differences 8).
2. `get_object_or_404(Document.objects, pk=pk)`: Nicht-Zahl → 500, fehlt/Papierkorb → 404
   `{"detail":"No Document matches the given query."}`.
   `has_perms_owner_aware(user, "change_document", doc)` (Dokument selbst) sonst 403
   `Insufficient permissions`.
3. Klassifikator: PaperRust-Modell (`PAPERRUST_CLASSIFIER_MODEL_FILE` bzw.
   `DATA_DIR/paperrust_classifier.model`, Schwelle `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD`), wie der
   Consumer; fehlt/kaputt → kein Klassifikator.
4. Daten: `NUMBER_OF_SUGGESTED_DATES` (`PAPERLESS_NUMBER_OF_SUGGESTED_DATES`, Standard 3) > 0 →
   `RegexDateParser.parse(doc.filename, doc.content)` (Konfiguration wie `get_date_parser()`:
   `DATE_PARSER_LANGUAGES` oder aus `OcrConfig().language`, `TIME_ZONE`, `IGNORE_DATES`,
   `FILENAME_DATE_ORDER`, `DATE_ORDER`), die ersten N, als Menge sortiert, `%Y-%m-%d`.
5. Kandidaten je Typ: für `request.user` sichtbar (`permitted_object_ids`), Reihenfolge `name`;
   Treffer = `matches(o, doc)` oder AUTO und vorhergesagt (`suggestion_content` = effektiver Inhalt).
6. Antwort `{"correspondents":[…],"tags":[…],"document_types":[…],"storage_paths":[…],"dates":[…]}`,
   `Cache-Control: no-cache`.

### Nicht in diesem Crate / bleibt Django

| Route | Grund |
|---|---|
| GET `/api/documents/{id}/ai_suggestions/` | pl-ai |
| POST `/api/documents/email/`, `/{id}/email/` | Mailversand – W23 B3 |
| POST `/{id}/update_version/`, DELETE/PATCH `/{id}/versions/{vid}/` | pl-documents-write |
| `preview`, `thumb`, `download` | pl-files |
| PUT/PATCH/DELETE `/{id}/` | pl-documents-write |
| POST/DELETE `/{id}/notes/` | siehe oben |

## Known differences

1. `history.action` immer englisch (`create/update/delete/access`). Django übersetzt nach
   Sprache der Anfrage; Kataloge dafür gibt es nur in django-auditlog für `ja` und `ko`
   (paperless selbst übersetzt diese Wörter nicht) – Abweichung also nur bei japanischer/koreanischer
   Oberfläche. Umsetzung, sobald `ctx.language` genutzt wird.
2. `archived_file_name`: `pathvalidate.sanitize_filename` nachgebaut (ungültige Zeichen
   `\ / : * ? " < > |` und Steuerzeichen → `-`, Windows-reservierte Namen, Abschneiden auf 255 Bytes).
   Der Kopf beginnt immer mit einem Datum, reservierte Namen können daher nicht auftreten.
3. Gleichstand bei Sortierung (z. B. Tags/Korrespondenten gleichen Namens verschiedener Owner,
   `ordering=custom_field_*` ohne Tiebreaker) ist in PostgreSQL ohne eindeutigen Schlüssel nicht
   deterministisch – beide Seiten sortieren gleich, die Reihenfolge bei Gleichstand kann abweichen.
4. `selection_data` mit Multipart: Dateiteile werden übersprungen; Werte kommen als Strings in dieselbe
   Validierung wie JSON (DRF wandelt sie ebenso über die Felder).
5. `PAPERLESS_DATE_PARSER_LANGUAGES` steht nicht in `pl_core::Config` → `FilterContext` mit `None`
   (= alle Sprachen, Standard von upstream). Gesetzte Variable wird ignoriert (CORE-REQUEST).
6. Fehlertexte nur Englisch (DRF/Django übersetzen nach `Accept-Language`) – wie alle Crates.
7. Fehlende Dokumente eines Suchtreffers (Index ≠ DB, schon durch die ORM-Schnittmenge praktisch
   ausgeschlossen) → 500 wie upstream (`KeyError`).

8. **metadata/suggestions ohne Django-Cache** (W23): `get/set/refresh_metadata_cache` und
   `get/set/refresh_suggestions_cache` werden nicht gelesen/geschrieben (nur Performance; Django
   prüft beim Lesen ohnehin die Prüfsummen bzw. die Klassifikator-Version). `suggestions` liefert
   daher kein ETag/Last-Modified: upstream nimmt dafür `classifier_hash`/`classifier_modified` aus
   dem Django-Cache, die nur Djangos Pickle-Klassifikator setzt (ohne `MODEL_FILE` – wie in der
   Referenz – auch upstream keins).
9. **Klassifikator** = PaperRust-Modell (pl-classifier), nicht Djangos `classification_model.pickle`.
   Vorhersagen weichen bei AUTO-Objekten ab, solange nicht beide Modelle auf denselben Daten
   trainiert sind (siehe pl-classifier).
10. **`lang`** (metadata): langdetect zieht upstream bei jedem Aufruf einen zufälligen Seed; der
    Port ist fest geseedet (deterministisch). Bei eindeutigen Texten gleich, bei sehr kurzen /
    gemischten Texten kann upstream selbst zwischen Aufrufen wechseln.
11. **`next_asn` zählt den Papierkorb mit** (A-109, 2026-11-16, bewusst): `MAX(archive_serial_number)`
    über alle Dokumente inkl. gelöschter (`Document.global_objects`). upstream nimmt `Document.objects`
    und schlägt dann eine ASN vor, die ein Papierkorb-Dokument noch belegt; Speichern scheitert mit
    „Document with this Archive Serial Number already exists in the trash.“

12. **langdetect verschoben** (2026-11-21): Modul und Profile liegen jetzt in `pl-parsers` (`pl_parsers::langdetect`, Daten `crates/pl-parsers/data/langdetect/`), `pl_documents::langdetect` verweist darauf – Verhalten unverändert; Grund: pl-consumer braucht es für die OCR-Sprachautomatik (sonst Abhängigkeitskreis).
13. **Notes carry `format`** (2026-11-22, matches paperless-ngx 3.5.1, upstream #14626): `NotesSerializer`
    fields `id, note, format, created, user` in the `notes` field of documents (`NOTES_SQL`) and in
    `GET /api/documents/{id}/notes/` (`load_notes`). Needs `documents_note.format`
    (PaperRust migration `pl_documents_write.0001_paperrust_note_format`). The v3.4.2 parity set
    (`tests/parity.rs`, `notes`) differs in this field.
14. **Feld `folder` (nur PaperRust, Ordneransicht, 2026-11-22):** `serialize_documents_db` hängt **nach** den
    upstream-Feldern das Lesefeld `folder` an (Ordner-id oder `null` = „Unsortiert“, aus
    `paperrust_document_folder`). Nur wenn die Instanz mindestens einen Ordner hat oder `fields=folder`
    ausdrücklich angefragt wird; ohne Ordner (und ohne Tabelle) bleiben die Antworten byte-gleich zu
    paperless-ngx. Eine zusätzliche Abfrage je Seite (ein Index-Zugriff). Schreiben nur über
    `POST /rs/api/v2/folders/move-documents`, nicht über v1 (v1 ist ngx-Vertrag).

## Parität

In-process-Vergleich `crates/pl-documents/tests/parity.rs` (`#[ignore]`, Aufruf siehe Dateikopf):
gleiche Anfrage an Django (127.0.0.1:8000) und an die Rust-Handler **mit `pl_authz::Authz`** (wie
der Server), Vergleich von Status, Content-Type und JSON-Text (Schlüsselreihenfolge). Sätze über
`PARITY_SETS=routes,docfilter,search`.

* `routes`: Nutzer admin (Superuser), alice, bob, carol (ohne Modellrechte), dave (inaktiv),
  ungültiges Token, anonym; Basic admin / Basic falsches Passwort auf 3 Pfaden; API v10 und v9;
  ~135 Pfade (Liste mit Paginierung/`fields`/`truncate_content`/`full_perms`/Sortierung/`search`/
  `id__in`/`id=abc`/`tags__id__all`/`ordering=custom_field_999`/`include_selection_data` mit Filter,
  Detail inkl. `?version=` und Filterparameter, `root`, `notes`, `share_links`, `history` für Roots,
  Versionen, Papierkorb, Duplikate, geteilte und bob-Dokumente, `next_asn`, 11 `selection_data`-Rümpfe,
  nicht-numerische ids).
* `docfilter`: alle 104 Fälle aus `tests/parity/pl-docfilter/cases.base.json` (Variablen über Django
  aufgelöst; admin + alice; Filter, alle Sortierungen inkl. `custom_field_*`, Formular-/DSL-Fehler).
* `search`: alle `/api/documents/`-Fälle aus `tests/parity/pl-search/cases.json` und `cases.base.json`
  (text, title_search, query inkl. Feld-/Notiz-/Wildcard-/Bereichssuche, Sortierung score/-score/
  created/title, Fehler 400, `page=last`, `include_selection_data`, more_like_id gültig/unbekannt/
  ungültig, Suche + Filter).

Trash: `crates/pl-documents-write/tests/trash_parity.rs` (gleiches Verfahren).

### Ergebnis (2026-11-08, Referenz v3.4.2, Europe/Zurich)

| Satz | verglichen | identisch | nur Gleichstand | verschieden |
|---|---|---|---|---|
| routes | 1398 | 1398 | 0 | 0 |
| docfilter | 188 | 188 | 0 | 0 |
| search | 47 | 46 | 0 | 1 |
| trash.list (pl-documents-write) | 112 | 112 | 0 | 0 |

Die eine Abweichung: `?more_like_id=1&page=1&page_size=10` als admin – gleiche `count` (15), aber
andere Reihenfolge der MLT-Treffer (Django `[2, 109, 73, …]`, Rust beginnt mit 109). Die Reihenfolge
kommt unverändert aus `pl_search::SearchBackend::more_like_this_ids` (Engine, nicht dieses Crate) –
an pl-search gemeldet (siehe Bericht). Als alice identisch.

Zusätzlich gegen Django geprüft (curl): `GET /api/documents/1/?query=x` → 500; der Rust-Handler antwortet im Code ebenso mit 500 (nicht im Paritätslauf).

Nicht durch Parität abgedeckt: Session-Auth/CSRF (würde Sitzungen in der Referenz-DB anlegen; liegt
vollständig in pl-authz), Delegationsfälle von `selection_data` (früher in Django, seit 2026-11-14 404),
`PAPERLESS_AUDIT_LOG_ENABLED=false`.

### W23 metadata/suggestions (2026-11-08)

`tests/parity/pl-documents/cases.w23.json` (+ passende Basisfälle), A = Referenz-Django :8000, B =
PaperRust :8194 (`tests/parity/pl-documents/w23env.sh`, nur `documents.metadata`/`documents.suggestions`
in Rust, Rest Proxy, Referenz-DB nur lesend): `documents.metadata.*` 52/52 identisch,
`documents.suggestions.*` 40/40 identisch (Benutzer admin/user1/noperm/anon, Versionen, `?version=`,
Papierkorb, 304/412, PNG/Text/PDF). Berichte `results/parity/w23/`.
