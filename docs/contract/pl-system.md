# Vertrag `pl-system`

Upstream: paperless-ngx **v3.4.2** (`upstream/`). Quellen:
`src/documents/views.py` (`LogViewSet`, `StatisticsView`, `RemoteVersionView`, `TasksViewSet`,
`SystemStatusView`), `src/paperless/views.py` (`ApplicationConfigurationViewSet`),
`src/documents/serialisers.py` (`TaskSerializerV10`, `TaskSerializerV9`, `TaskSummarySerializer`,
`RunTaskSerializer`, `AcknowledgeTasksViewSerializer`), `src/paperless/serialisers.py`
(`ApplicationConfigurationSerializer`), `src/documents/filters.py` (`PaperlessTaskFilterSet`),
`src/documents/permissions.py`, `src/documents/models.py` (`PaperlessTask`),
`src/paperless/models.py` (`ApplicationConfiguration`), `src/documents/signals/handlers.py`
(`before_task_publish_handler`). Versionen der Bibliotheken laut `uv.lock`: Django 5.2.18,
DRF 3.18.3, django-filter 26.2, celery 5.6.3, kombu 5.6.2, packaging 26.0, httpx 0.28.1.

Gemeinsames Verhalten aller Routen (DRF):

- Authentifizierung: Basic, Token, Session (pl-authz). Ohne Anmeldung bei Routen mit
  `IsAuthenticated`: **401** `{"detail":"Authentication credentials were not provided."}`,
  Header `WWW-Authenticate: Token` (bzw. `Basic realm="api"`, wenn ein `Basic`-Header gesendet wurde).
- Fehlende Rechte: **403** `{"detail":"You do not have permission to perform this action."}`.
- `HttpResponseForbidden("Insufficient permissions")`: **403**, `Content-Type: text/html; charset=utf-8`,
  Body `Insufficient permissions` (kein JSON).
- `Http404` ohne Text → `{"detail":"Not found."}`; `get_object_or_404` mit gültiger Zahl, aber ohne
  Treffer → `{"detail":"No <Model> matches the given query."}`; ungültige pk (keine Zahl) → `Not found.`
- Router-Muster `^<prefix>/(?P<pk>[^/.]+)/$`: pk mit `.` trifft kein Muster → Django-404 (HTML).
- API-Version über `Accept: application/json; version=N` (erlaubt 9, 10; Standard 10; sonst
  **406** `{"detail":"Invalid version in \"Accept\" header."}`).
- JSON kompakt (`,` / `:`), UTF-8 roh, ` `/` ` escaped.
- Datum/Zeit in Serializern (`DateTimeField`): in `PAPERLESS_TIME_ZONE` umgerechnet, ISO 8601,
  Mikrosekunden nur wenn ≠ 0, `+00:00` → `Z`. In **nicht** serialisierten Antworten (z. B. `/api/status/`,
  `values()`-Dicts) rendert der DRF-Encoder das Objekt unverändert: Datenbankwerte sind UTC → `...Z`.

---

## Logs

### `logs.list` – `GET /api/logs/`

- Quelle: `LogViewSet.list`. Rechte: `IsAuthenticated` + `PaperlessAdminPermissions`
  (`is_active and is_staff`), sonst 401/403.
- Antwort 200: JSON-Liste der Schlüssel in fester Reihenfolge `paperless`, `mail`, `celery`,
  nur die, deren Datei `LOGGING_DIR/<name>.log` existiert und eine Datei ist.
- `LOGGING_DIR` = `PAPERLESS_LOGGING_DIR` oder `PAPERLESS_DATA_DIR/log` (Standard `../data/log`).

### `logs.retrieve` – `GET /api/logs/{pk}/`

- Quelle: `LogViewSet.retrieve`. Rechte wie oben (Prüfung vor allem anderen).
- `pk` nicht in {paperless, mail, celery} → 404 `{"detail":"Not found."}`; Datei fehlt → 404 dasselbe.
- Query `limit` (optional): Python-`int()`; ungültig oder `< 1` → **400**
  `{"limit":"Must be a positive integer"}` (Wert ist String, keine Liste).
- Antwort 200: Liste der Zeilen (Textmodus, universelle Zeilenenden `\r\n`/`\r` → `\n`, jede Zeile
  `rstrip()` = alle Python-Leerraumzeichen am Ende entfernt); mit `limit` nur die letzten N Zeilen.
- Datei nicht als UTF-8 lesbar → Python `UnicodeDecodeError` → 500.

## Statistik

### `statistics.get` – `GET /api/statistics/`

- Quelle: `StatisticsView.get`. Rechte: `IsAuthenticated`.
- `global = user.is_superuser or user.has_perm("paperless.view_global_statistics")` (aktiv, angemeldet).
- Dokumente: `Document.objects` (nicht gelöscht, `deleted_at IS NULL`) mit `root_document IS NULL`;
  nicht global zusätzlich `id IN permitted_document_ids(user)` (Eigentümer = user, ohne Eigentümer,
  oder guardian-Objektrecht `view_document` für Nutzer/Gruppen; Versionen über ihr Root).
- Tags/Korrespondenten/Dokumenttypen/Speicherpfade: global alle, sonst
  `restrict_queryset_to_visible(..., "view_<model>")` (gleiche Logik ohne Root).
- Antwortfelder (Reihenfolge):
  1. `documents_total` int – Summe der Gruppen unten
  2. `documents_inbox` int|null – `COUNT(DISTINCT doc.id)` der Dokumente mit einem sichtbaren Inbox-Tag;
     `null`, wenn kein sichtbarer Inbox-Tag existiert
  3. `inbox_tag` int|null – erster Eintrag von `inbox_tags`
  4. `inbox_tags` [int]|null – pks sichtbarer Tags mit `is_inbox_tag`, sortiert nach `name` (Tag-Meta),
     leere Liste → `null`
  5. `document_file_type_counts` [{`mime_type`, `mime_type_count`}] – `GROUP BY mime_type`,
     `ORDER BY count DESC` (bei Gleichstand Reihenfolge der DB)
  6. `character_count` int – Summe `content_length` (NULL → 0)
  7. `tag_count`, 8. `correspondent_count`, 9. `document_type_count`, 10. `storage_path_count` int
  11. `current_asn` int – `MAX(archive_serial_number)`; v3.4.2: über **alle nicht gelöschten
      Dokumente** (ohne Rechtefilter). **Rust: über die sichtbaren Dokumente** (siehe unten), Standard 0.
- **ASN-Maximum:** Der Fix „use filtered documents for statistics ASN max“ (#14710, paperless-ngx
  `dev`, nach v3.4.2) ist in v3.4.2 **nicht** enthalten. Entscheidung Nutzer 2026-11-08: Rust nutzt
  den Fix (`statistics::ASN_MAX_USES_FILTERED_DOCUMENTS = true`), siehe Known differences 1.

## Remote-Version

### `remote_version.get` – `GET /api/remote_version/`

- Quelle: `RemoteVersionView.get`. Rechte: keine (`AllowAny`, DRF-Standard); fehlgeschlagene
  Authentifizierung mit Zugangsdaten → 401 (pl-authz).
- **PaperRust (R4, 2026-11-09):** geprüft werden die **PaperRust-Releases**
  (`GET https://api.github.com/repos/paperrust/paperrust/releases/latest`,
  `pl_core::brand::RELEASES_API_URL`), verglichen nach PEP 440 mit `pl_core::brand::PAPERRUST_VERSION`
  (nicht mehr mit paperless-ngx 3.4.2). `tag_name` ohne Präfix `paperrust-` und `v`.
- Abschaltbar: `PAPERRUST_ENABLE_UPDATE_CHECK=false` (= upstream `PAPERLESS_ENABLE_UPDATE_CHECK`,
  Alias aus pl-core) → keine ausgehende Anfrage. Upstream fragt in der View trotzdem. Test-URL `PAPERRUST_REMOTE_VERSION_URL` (alt
  `PAPERLESS_RS_REMOTE_VERSION_URL`).
- Anfrage: `Accept: application/vnd.github+json`, `User-Agent` = `pl_core::brand::USER_AGENT`,
  Verbindung ≤ 2 s, gesamt ≤ 3 s, keine Weiterleitungen, Antwort ≤ 256 KiB.
- Cache im Prozess: Treffer 15 min, Fehlschlag 60 s; gleichzeitige Aufrufe ohne Cache-Treffer
  teilen sich eine Anfrage (Single-Flight).
- Antwort 200 (immer, auch ohne Netz):
  `{"version": str, "update_available": bool, "paperrust": {"current_version", "latest_version",
  "status", "releases_url"}}` – `version` = neuestes Release bzw. `"0.0.0"` (unbekannt/abgeschaltet,
  wie upstream bei Fehlern), `update_available` = neuer als die laufende Version,
  `paperrust.status` = `ok` | `unknown` | `disabled`, `latest_version` = null außer bei `ok`.
  Ungültige Antworten (kein Objekt, kein/kein PEP-440-`tag_name`) → `unknown` statt 500.

## Aufgaben (`TasksViewSet`)

Tabelle `documents_paperlesstask`. Rechte allgemein: `IsAuthenticated` + `PaperlessObjectPermissions`
(GET: `documents.view_paperlesstask`, POST: `documents.add_paperlesstask`).

**Basis-Queryset** `get_queryset()`: staff → alle; sonst `owner = user OR owner IS NULL`.
v9 zusätzlich: `task_name` (Alt-Namen `check_sanity`→`sanity_check`, `llmindex_update`→`llm_index`,
sonst unverändert) → `task_type = …`; `type` ∈ {`auto_task`→system/email_consume/folder_consume,
`scheduled_task`→scheduled, `manual_task`→manual/web_ui/api_upload} → `trigger_source IN …`
(unbekannt: ignoriert). Alle Versionen: `task_id` → `task_id = …` (exakt).

**FilterSet** `PaperlessTaskFilterSet` (Reihenfolge der Fehler-Schlüssel):
`task_type` (Mehrfach, Choices), `trigger_source` (Mehrfach), `status` (Mehrfach), `acknowledged`
(Bool: `true/false/1/0`, sonst ignoriert), `owner` (User-pk; ungültig →
`"Select a valid choice. That choice is not one of the available choices."`), `name`
(`input_data ->> 'filename'` icontains OR Task-Typ-Label enthält OR Trigger-Label enthält; die Labels
sind die **übersetzten** Labels der aktiven Sprache, `str(label).lower()`),
`result` (`result_data.reason/error_message icontains`, Zahl → `document_id`/`duplicate_of` gleich,
enthält "duplicate" → `duplicate_of IS NOT NULL`), `is_complete` (Bool → `status IN/NOT IN`
success/failure/revoked), `date_created_after` (gte), `date_created_before` (lte; DateTime nach Django,
naive Werte in `TIME_ZONE`; ungültig → `"Enter a valid date/time."`). Mehrfach-Choices ungültig →
`"Select a valid choice. <v> is not one of the available choices."`. Fehler → **400**
`{"feld": ["text"]}`. Leere Werte filtern nicht.

**Sortierung** `ordering` (Komma-Liste, `-` für absteigend) aus
`date_created, date_done, status, task_type, duration_seconds, wait_time_seconds`; ungültige
Terme werden ignoriert; Standard `-date_created`.

**Serializer v10** (`TaskSerializerV10`), Felder in Reihenfolge:
`id`, `task_id`, `task_type`, `task_type_display`, `trigger_source`, `trigger_source_display`, `status`,
`status_display`, `date_created`, `date_started`, `date_done` (DateTime|null), `duration_seconds`,
`wait_time_seconds` (float|null), `input_data` (JSON), `result_data` (JSON|null),
`related_document_ids` ([int]: `result_data.document_id` falls wahr, sonst `duplicate_of` falls wahr,
sonst `[]`), `acknowledged` (bool), `owner` (int|null).
`*_display` = übersetztes Label (Sprache aus Cookie `<COOKIE_PREFIX>django_language` bzw.
`Accept-Language`, Django-`LocaleMiddleware`), unbekannter Wert → Rohwert.

**Serializer v9** (`TaskSerializerV9`): `id`, `task_id`, `task_name` (`sanity_check`→`check_sanity`,
`llm_index`→`llmindex_update`), `task_file_name` (`input_data.filename` oder null), `type`
(scheduled→`scheduled_task`, system/email_consume/folder_consume→`auto_task`, sonst `manual_task`),
`status` (Großbuchstaben), `date_created`, `date_done`, `result` (`"Success. New document id N created"`
| `reason` | `"Not consuming: It is a duplicate of document #N"` | `error_message` | null),
`acknowledged`, `related_document`, `duplicate_documents` (`[{"id","title","deleted_at"}]` aus
`Document.global_objects` pk=`duplicate_of`; nicht-staff nur mit `view_document`-Recht inkl. gelöschter;
`deleted_at` roh als UTC `...Z`), `owner`.

### `tasks.list` – `GET /api/tasks/`
Basis-Queryset → FilterSet → Sortierung. v10: `StandardPagination` (`count`, `next`, `previous`,
`results`; `page`, `page_size` max 100000, Standard 25; ungültige Seite → 404 `{"detail":"Invalid page."}`).
v9: unpaginierte Liste.

### `tasks.retrieve` – `GET /api/tasks/{pk}/`
Wie Liste (FilterSet und Sortierung gelten auch hier), dann pk. Nicht gefunden → 404
`"No PaperlessTask matches the given query."`. Objektrecht: Eigentümer oder kein Eigentümer → ok;
sonst guardian-Objektrecht `view_paperlesstask` (Superuser immer) → sonst **404** (`Http404` für GET).

### `tasks.acknowledge` – `POST /api/tasks/acknowledge/`
Rechte: `IsAuthenticated` + `documents.change_paperlesstask` (Modellrecht). Body: `tasks` [int]
(optional, nicht null), `all` bool (Standard false). Fehler (400): `tasks` keine Liste →
`{"tasks":["Expected a list of items but got type \"<t>\"."]}`; Element keine Ganzzahl →
`{"tasks":{"<i>":["A valid integer is required."]}}`; nicht alle im Basis-Queryset bzw. doppelt →
`{"tasks":["Some tasks in tasks don't exist or were specified twice."]}`; beides gesetzt →
`{"non_field_errors":["Set either all or tasks, not both."]}`; keins →
`{"non_field_errors":["Either all must be true or tasks must be provided."]}`; `all` ungültig →
`{"all":["Must be a valid boolean."]}`. Wirkung: `UPDATE … SET acknowledged = true` auf
Basis-Queryset (`all`: nur `acknowledged = false`; sonst `id IN tasks`). Antwort `{"result": <Anzahl>}`.

### `tasks.summary` – `GET /api/tasks/summary/`
Rechte: Nutzer mit Systemstatus-Recht (superuser/staff/`paperless.view_system_monitoring`) nur
angemeldet, sonst `view_paperlesstask`. `days` (Standard 30, auf 1..365 begrenzt; keine Ganzzahl →
**400** `{"days":"Must be a positive integer."}`). Queryset: Systemstatus-Recht → alle Aufgaben, sonst
Basis-Queryset; `date_created >= now - days`. Gruppiert nach `task_type` (ohne ORDER BY), je Zeile:
`task_type`, `total_count`, `pending_count`, `success_count`, `failure_count`,
`avg_duration_seconds`, `avg_wait_time_seconds` (float|null), `last_run` (max `date_created`),
`last_success`, `last_failure` (max `date_done` mit Status). Unpaginierte Liste.

### `tasks.status_counts` – `GET /api/tasks/status_counts/`
Rechte `view_paperlesstask`. Basis-Queryset + FilterSet **ohne** `status` und `is_complete`
(ungültige Filter → 400). Antwort `{"all","needs_attention" (failure/revoked),"in_progress"
(pending/started),"completed" (success)}`.

### `tasks.active` – `GET /api/tasks/active/`
Rechte `view_paperlesstask`. Basis-Queryset, `status IN (pending, started)`, `-date_created`, max. 50,
Serializer nach Version, unpaginiert, ohne FilterSet.

### `tasks.run` – `POST /api/tasks/run/`
Rechte: `IsAuthenticated` + `documents.add_paperlesstask` (403 JSON), dann nicht Superuser → 403
`Insufficient permissions` (HTML). Body `task_type` (ChoiceField aller TaskTypes;
fehlt → `{"task_type":["This field is required."]}`, ungültig →
`{"task_type":["\"<v>\" is not a valid choice."]}`). Ausführbar nur `train_classifier` (kwargs `{}`),
`sanity_check` (`{"raise_on_error": false}`), `llm_index` (`{"rebuild": false}`); sonst **400**
`{"error":"Task type '<t>' cannot be manually triggered"}`. Wirkung: Celery-Aufgabe
`documents.tasks.<name>` mit Header `trigger_source=manual`; Signal `before_task_publish` legt
`PaperlessTask` an (`get_or_create(task_id)`: task_type, trigger_source=manual, status=pending,
input_data `{}`, owner NULL). Antwort `{"task_id": "<uuid>"}`. Fehler beim Senden → 500 Text
`Error running task, check logs for more detail.`
Rust: `pl_core::tasks::send_task` in die PaperRust-Warteschlange (`paperrust_task_queue`, die PgQueue legt
die `PaperlessTask`-Zeile im selben Schritt an). Bis 2026-11-14 alternativ Celery-Protokoll v2 als JSON per
`LPUSH` an Redis (Mischbetrieb) – entfernt.

## Systemstatus

### `status.get` – `GET /api/status/`
- Quelle: `SystemStatusView.get`. Rechte: `IsAuthenticated`, dann `has_system_status_permission`
  (aktiv und superuser/staff/`paperless.view_system_monitoring`) sonst 403 `Insufficient permissions` (HTML).
- **PaperRust (R4, 2026-11-09):** Prüfungen auf PaperRust-Komponenten umgestellt, alle **parallel**
  (`tokio::join!`), jede höchstens **1 s** (`status::CHECK_TIMEOUT`; Dateisystem in
  `spawn_blocking`). Ziel < 200 ms. Eine fehlgeschlagene oder abgelaufene Prüfung liefert ihren
  Status `ERROR` samt Text, die Route bleibt 200 (upstream: DB-Fehler bei Aufgabenabfragen → 500).
- Betriebsart: seit 2026-11-14 immer **Warteschlange PostgreSQL** (kein Celery-Ping, keine
  Redis-Verbindung mehr). Cache-Backend aus `pl_core::cache::backend` (`Local` Standard, `Postgres`
  bei `PAPERRUST_CACHE=postgres`).
- Antwort (Reihenfolge; alle Schlüssel, die `src-ui/src/app/data/system-status.ts` liest, bleiben):
  - `pngx_version` = **`"3.4.2"`** (`pl_core::brand::UPSTREAM_VERSION`): die Oberfläche vergleicht
    den Wert mit ihrer `environment.version` und zeigt sonst „Versionskonflikt“. Die eigene
    Version steht in `paperrust.version`.
  - `server_os` (`platform.platform()`, einmal je Prozess ermittelt), `install_type`
    (`kubernetes` wenn `KUBERNETES_SERVICE_HOST`, sonst `docker` wenn `PNGX_CONTAINERIZED=1`, sonst `bare-metal`)
  - `storage`: `total`/`available` von `statvfs(MEDIA_ROOT)` (Fehler → `0`/`0`, siehe `paperrust`)
  - `database`: wie upstream (`type`, `url` = DB-Name, `status`, `error`, `migration_status`)
  - `tasks` – Schlüssel wie upstream, **Bedeutung PaperRust** (R2 passt die Beschriftungen der Oberfläche an):

    | Schlüssel | PaperRust | upstream |
    |---|---|---|
    | `redis_url` | **Cache**: `memory://local` (prozesslokal) bzw. `postgresql://<DB>/paperrust_cache` | `_CHANNELS_REDIS_URL` ohne Zugangsdaten |
    | `redis_status`/`redis_error` | **Cache**: lokal → `OK` ohne Verbindung; `postgres` → `SELECT 1 FROM paperrust_cache LIMIT 1` (Fehler → `ERROR` „Error reading the shared cache, check logs for more detail.“) | `PING` (Verbindung ≤ 0,5 s) |
    | `celery_status` | **PaperRust-Worker** (s. u.) | Celery-Ping |
    | `celery_url` | `postgresql://<DB>/paperrust_task_queue` | Hostname des ersten antwortenden Workers |
    | `celery_error` | Text der Worker-Prüfung | upstream-Texte |

    Worker-Prüfung (eine Abfrage auf `paperrust_task_queue` + `pg_stat_activity`):
    lebendig = Sitzung mit `LISTEN "paperrust_task_queue"` (wartender Worker) **oder** Heartbeat
    < 120 s **oder** laufende Aufgabe mit gültiger Lease. Reihenfolge der Befunde:
    abgelaufene Lease → `WARNING` „A PaperRust worker stopped responding; its tasks will be
    retried.“; nicht lebendig → `WARNING` „No PaperRust worker is connected to the task queue. This
    may be temporary.“; wartende Aufgabe mit `eta` > 60 s überfällig → `WARNING` „Tasks have been
    waiting in the queue for more than a minute.“; sonst `OK`. Abfragefehler (z. B. Tabelle fehlt)
    → `ERROR` „Error checking the task queue, check logs for more detail.“

    `index_*`, `classifier_*`, `sanity_check_*`, `llmindex_*`, `summary`: Semantik wie upstream
    (letzte abgeschlossene Aufgabe usw.), jetzt aus **einer** Abfrage (LATERAL-Joins:
    Klassifikator, Sanity-Check, LLM-Index, 30-Tage-Übersicht, `ai_enabled`/Backends aus
    `paperless_applicationconfiguration`). Fehler/Timeout → alle vier `ERROR` +
    „Error reading task results, check logs for more detail.“, Übersicht 0.
  - **`paperrust`** (neu, nur PaperRust):
    `version` (`0.1.0`), `version_string` (`PaperRust 0.1.0`), `upstream_version` (`3.4.2`),
    `queue_backend` (immer `postgres`), `django_proxy` (immer `false`; beide Schlüssel bleiben für
    bestehende Auswertungen), `checks` {
    `database` {status, error, latency_ms} – `WARNING` bei offenen Migrationen;
    `worker` {backend, status, error, connected_workers, queued, running, stale, overdue,
    last_heartbeat, last_finished};
    `cache` {backend `local`|`postgres`, status, error, latency_ms};
    `index` {status, error, last_modified};
    `classifier` {status, error, last_trained, model_file_present, model_file_modified} – Modelldatei
    ist die, die PaperRust lädt (`status::classifier_model_path`: `PAPERRUST_CLASSIFIER_MODEL_FILE`
    bzw. `DATA_DIR/paperrust_classifier.model`, C3);
    `sanity_check` {status, error, last_run};
    `ocr` {status, error, tesseract, pdfium} – nur Dateisuche (`PAPERRUST_TESSERACT_PATH` bzw.
    `tesseract` im `PATH`; `PAPERRUST_PDFIUM_PATH` bzw. Programmverzeichnis/`PATH`/Systembibliotheken),
    kein Prozessstart, 60 s zwischengespeichert; Pfade werden nicht ausgegeben;
    `ai` {status (`DISABLED` | `WARNING` ohne LLM-Backend | `OK`), error, enabled, llm_backend,
    embedding_backend, llmindex_status} – kein Netzaufruf zum LLM;
    `storage` {status (`WARNING` unter 5 % frei), error, total, available} },
    `elapsed_ms` (Laufzeit der Route bis zum Aufbau der Antwort).
  - Zeitwerte aus der DB werden roh (UTC, `Z`) gerendert.

## Konfiguration (`ApplicationConfigurationViewSet`)

Tabelle `paperless_applicationconfiguration` (Singleton id=1). Rechte: `IsAuthenticated` +
`DjangoModelPermissions` (GET: keine Modellrechte nötig; PUT/PATCH `paperless.change_applicationconfiguration`;
DELETE `paperless.delete_applicationconfiguration`; POST: `add_…`).
**Abweichung von v3.4.2 (upstream-Fix 53d9aa34a, #14709):** PaperRust übernimmt `PaperlessModelPermissions` –
GET/HEAD (`config.list`, `config.retrieve`) brauchen `paperless.view_applicationconfiguration`, sonst 403
`You do not have permission to perform this action.` (vor der Objektsuche). Paritätsregeln
`config-read-needs-view-perm-*` in `tests/parity/accepted.json`; Test `crates/pl-system/tests/upstream_fixes.rs`.

Felder (`fields="__all__"` → pk, deklarierte, Modellfelder):
`id`, `externally_configured_variables` (sortierte Namen aller Umgebungsvariablen mit Präfix
`PAPERLESS_`, ohne Werte), `user_args` (JSON|null), `barcode_tag_mapping` (JSON|null),
`llm_embedding_api_key`, `llm_api_key`, `remote_ocr_api_key` (null oder `"*" * max(10, len)`),
`output_type`, `pages`, `language`, `mode`, `archive_file_generation`, `image_dpi`, `unpaper_clean`,
`deskew`, `rotate_pages`, `rotate_pages_threshold`, `max_image_pixels`, `color_conversion_strategy`,
`app_title`, `app_logo` (null oder `request.build_absolute_uri(<storage url>)`; `MEDIA_URL` ist leer,
die URL ist daher relativ zum Request-Pfad), `barcodes_enabled`, `barcode_enable_tiff_support`,
`barcode_string`, `barcode_retain_split_pages`, `barcode_enable_asn`, `barcode_asn_prefix`,
`barcode_upscale`, `barcode_dpi`, `barcode_max_pages`, `barcode_enable_tag`, `barcode_tag_split`,
`barcode_store_values`, `remote_ocr_engine`, `remote_ocr_endpoint`, `remote_ocr_mode`, `ai_enabled`,
`llm_embedding_backend`, `llm_embedding_model`, `llm_embedding_endpoint`, `llm_embedding_chunk_size`,
`llm_context_size`, `llm_backend`, `llm_model`, `llm_endpoint`, `llm_output_language`,
`llm_request_timeout`.

### `config.list` – `GET /api/config/`
Unpaginierte Liste aller Zeilen (praktisch eine).

### `config.retrieve` – `GET /api/config/{pk}/`
pk Zahl ohne Treffer → 404 `No ApplicationConfiguration matches the given query.`; keine Zahl → `Not found.`

### `config.create` – `POST /api/config/`
Rechte zuerst (`add_applicationconfiguration`), dann **405 mit leerem Body** (`Response(status=405)`).

### `config.update` / `config.partial_update` – `PUT|PATCH /api/config/{pk}/` (Standalone 2026-11-09, `src/config_write.rs`)
`run_validation`: `""` → null für `user_args`, `barcode_tag_mapping`, `language`,
`llm_output_language`; API-Schlüssel `""` → null, nur `*` → Feld nicht ändern. `user_args`/
`barcode_tag_mapping` müssen JSON-Objekte sein (`"<feld> must be a JSON object."`,
`"barcode_tag_mapping values must be strings."`). `app_logo`: Endung jpg/png/gif/svg, MIME per
libmagic; SVG → `reject_dangerous_svg`; Raster → `validate_raster_image`, JPEG/PNG werden mit PIL neu
kodiert (Metadaten entfernt); altes Logo wird gelöscht. `llm_endpoint`, `llm_embedding_endpoint`,
`remote_ocr_endpoint` → `validate_outbound_http_url` (DNS-Auflösung, interne Ziele je nach
`*_ALLOW_INTERNAL_ENDPOINTS`). Nach dem Speichern ggf. `llmindex_index.apply_async(rebuild=True,
trigger_source=system)`, wenn KI-Index aktiv und (`llm_index` fehlt oder Embedding-Konfiguration geändert).

Umsetzung (docs/STANDALONE-GAPS.md): Rechte `change_applicationconfiguration` vor `get_object()`
(404), Body danach (JSON/Form/Multipart; Multipart bis 64 MiB im Handler, Route in
`STREAMING_ROUTES`). Felder in Serializer-Reihenfolge mit DRF-Semantik (Char/Choice/Integer/Float/
Boolean `allow_null`, HTML-Eingabe-Regeln, `JSONField(binary=True)`: nur Strings werden geparst, Dict →
„Value must be valid JSON.“; `user_args`/`barcode_tag_mapping` sind bei PUT Pflicht). `run_validation`
schreibt in `request.data`: bei Form/Multipart ohne Datei ist der QueryDict unveränderlich → 500 wie
upstream. Speichern immer nach pk 1 (`AbstractSingletonModel.save`, Upsert aller Spalten), `app_logo`
NULL → `""` (`FieldFile.__str__`). Logo: altes Logo zuerst gelöscht, neues über `pl_settings::logo`
(gemeinsam mit v2). `perform_update`: `llmindex_index(rebuild=True)` wenn KI-Index aktiv und (Index
fehlt oder Embedding-Konfiguration geändert); Index-Prüfung über den Dienst
`config_write::LlmIndexExists` (pl-server, Feature `ai`). Parität: tests/parity/standalone-gaps.

### `config.destroy` – `DELETE /api/config/{pk}/` (Standalone 2026-11-09)
`delete_applicationconfiguration`, `get_object()`, `DELETE` der Zeile (Logodatei bleibt), 204.

---

## Known differences

1. **ASN-Maximum (statistics):** gefiltert auf sichtbare Dokumente wie upstream-Fix #14710 (nach
   v3.4.2; Entscheidung Nutzer 2026-11-08). v3.4.2 rechnet global und verrät damit das höchste ASN
   fremder Dokumente. Konstante `statistics::ASN_MAX_USES_FILTERED_DOCUMENTS = true` (Review 2 N20).
2. **remote_version (R4, 2026-11-09, Auftrag Nutzer):** prüft **PaperRust**-Releases
   (`github.com/paperrust/paperrust`) gegen die PaperRust-Version statt paperless-ngx gegen 3.4.2.
   Folgen gegenüber Django: `version` ist die neueste PaperRust-Version (bzw. `"0.0.0"`),
   `update_available` bezieht sich auf PaperRust, Zusatzschlüssel `paperrust`. Abschaltbar
   (`PAPERRUST_ENABLE_UPDATE_CHECK=false` – upstream fragt trotzdem). Ungültige Antworten → `unknown` statt 500 (upstream `KeyError/InvalidVersion` → 500).
   Eigener In-Prozess-Cache (15 min) statt Django-Cache; Fehlschläge 60 s negativ gecacht
   (Review 2 N15), Single-Flight. Timeout 3 s statt 5 s.
   ~~Celery-Ping (Review 2 N19, R4)~~: mit dem Celery-Weg entfernt (2026-11-14, Modul `pickle`
   gelöscht); die Worker-Prüfung liest die PaperRust-Warteschlange.
3. **Übersetzte Labels (`*_display`, tasks):** Tabelle aus `upstream/src/locale/*/django.po`
   erzeugt (`tests/parity/pl-system/gen_labels.py`, ohne fuzzy-Einträge). `LANG_INFO`-Fallbacks
   von Django (z. B. `zh-hans`) sind nicht nachgebildet.
4. **tasks.run:** Aufgabe geht in die PaperRust-Warteschlange statt als signed-pickle-Nachricht an
   Celery; Antwort (`task_id`) und `PaperlessTask`-Zeile wie upstream.
5a. **status (R4, 2026-11-09):** neue Prüfungen und Bedeutungen siehe Abschnitt „Systemstatus“.
   Abweichungen gegenüber Django: Zusatzobjekt `paperrust` (immer); `redis_*` beschreiben den
   Cache und `celery_*` den PaperRust-Worker (seit 2026-11-14 immer, kein Celery-Ping); Fehler einzelner Prüfungen → 200 mit `ERROR` statt 500; `storage` bei
   Fehler `0/0` statt 500.
   Akzeptanzregel-Vorschlag für den Harness siehe Abschnitt „Paritätsstand“.
5. **status:** Unter Windows (nur Referenz) wird `storage` wie der Referenz-Shim über
   `GetDiskFreeSpaceExW` ermittelt (upstream hätte dort 500). `server_os` bildet CPython 3.12 `platform.platform()` nach (`src/sysinfo.rs`): Linux/Unix aus
   `uname` (`<System>-<Release>-<Maschine>`, auf Linux mit `-with-glibc<Version>` über
   `confstr(_CS_GNU_LIBC_VERSION)`); Windows `Windows-<release>-<major.minor.build>-SP<n>[.<m>]`
   über `RtlGetVersion` (dieselben Werte, die `_win32_ver` per WMI liest: Version, ProductType,
   ServicePack) und die Tabellen `_WIN32_CLIENT_RELEASES`/`_WIN32_SERVER_RELEASES`
   (z. B. `Windows-11-10.0.22631-SP0`). Rest-Abweichungen: Unix-`processor` (`uname -p`) wird nicht
   angehängt (unter Linux meist gleich `machine` oder `unknown` → ohne Wirkung); Nicht-glibc-
   Systeme (musl: Python liest `libc_ver` aus der Binärdatei), macOS (`mac_ver`) und ein
   WMI-Ausfall (Python-Fallback über `ver`) sind nicht nachgebildet.
   `unapplied_migrations` kennt nur die Migrationen der paperless-Apps aus `upstream/`
   (documents, paperless, paperless_mail, paperless_ai); Fremd-Apps gelten als angewandt.
   `index_status` prüft nur, ob `INDEX_DIR` lesbar ist und `meta.json` enthält (Python öffnet bzw.
   baut den Index). (Der frühere Nachbau des Celery-Pings über kombu-pidbox ist entfernt.)
6. **logs:** PaperRust schreibt `paperrust.log` statt `paperless.log`. Die Liste nennt daher den
   Schlüssel `paperrust` (zuerst; ein älteres `paperless.log` bleibt als `paperless` lesbar).
   `GET /api/logs/paperless/` liefert `paperrust.log`, wenn kein `paperless.log` existiert, damit
   Clients mit dem upstream-Schlüssel weiter funktionieren. Sehr große Dateien werden ganz gelesen
   (wie Python `readlines`).
7. **Formatsuffixe des Routers** (`/api/logs/paperless.json`, `/api/tasks/1.json/`, `?format=api`):
   Django rendert JSON bzw. die Browsable API; Rust kennt keine Suffixe (pk mit Punkt → 404
   `Not found.` statt Django-Antwort). In der Praxis vom Frontend nicht genutzt.
8. **Übersetzte Fehlertexte:** Django/DRF übersetzen Fehlermeldungen (`detail`, Feldfehler) nach
   Cookie/`Accept-Language`; `pl-core`/`pl-authz` liefern englische Texte (CORE-REQUEST). Labels
   (`*_display`) übersetzt pl-system selbst.
8a. **config.update (Logo):** MIME-Erkennung per Magic-Bytes statt libmagic (SVG/PNG/JPEG/GIF);
   andere Rasterformate mit erlaubter Endung (z. B. BMP als `.png`) → „Invalid logo image.“ (Pillow
   nähme sie an). SVG mit `<!DOCTYPE` abgelehnt (strenger). `json.loads` über serde (kein
   `NaN`/`Infinity`). Multipart-Bodys über 64 MiB → 400 (Django spoolt beliebig große Dateien).
9. **Fehlertext ungültiges JSON** (acknowledge/run): DRF gibt Pythons `json`-Fehlermeldung zurück
   (`JSON parse error - …`); seit W2 (2026-11-08) baut Rust sie über `pyjson` (CPython-C-Scanner,
   Quelle `crates/pl-core/src/pyjson.rs`, Re-Export `pl_core::pyjson` (W11)) wortgleich nach;
   Rest: Rekursionstiefe und >4300-Ziffern-Ganzzahlen angenähert. W2-Schreiblauf:
   `tasks.acknowledge`/`tasks.run` inkl. `documents_paperlesstask`-Zustand identisch.

10. **config `externally_configured_variables`** (überarbeitet 2026-11-20, Paritätslauf F3): upstream
    listet jeden gesetzten Namen `PAPERLESS_*` (sortiert). PaperRust liest `PAPERRUST_X`, `PAPERLESS_X`
    und den Übergangsnamen `PAPERLESS_RS_X` als dieselbe Einstellung; jede davon zählt als
    `PAPERLESS_X` und erscheint **einmal** unter diesem Namen (kein zusätzliches `PAPERRUST_X` mehr).
    Ausgelassen werden PaperRusts eigene Einstellungen (`pl_core::config::OWN_SETTINGS` und
    `PAPERRUST_*`-Schlüssel des Einstellungsschemas, z. B. `PAPERRUST_BIND`, `…_ROUTES`, `…_LOG`,
    `…_BRANDING`, `…_DATAMODEL_BACKUP` – auch wenn sie als `PAPERLESS_*` gesetzt sind) sowie
    unbekannte `PAPERRUST_*`/`PAPERLESS_RS_*` ohne upstream-Gegenstück; ein unbekanntes `PAPERLESS_*`
    erscheint wie upstream. Die Weboberfläche (`Config.svelte`) prüft deshalb `PAPERRUST_X` auch unter
    `PAPERLESS_X`. Läuft Rust mit einer anderen Umgebung als Django, weicht die Liste ab.
11. **remote_version, nur Tests:** `PAPERRUST_REMOTE_VERSION_URL` (alt
    `PAPERLESS_RS_REMOTE_VERSION_URL`) ersetzt die GitHub-URL (lokaler Stub, keine echten
    Netzaufrufe). User-Agent `pl_core::brand::USER_AGENT` (`PaperRust/<version> (+repo)`).
13. **statistics (R4, 2026-11-09, Bericht O1):** 2 SQL-Abfragen statt 6 (Zähler, Inbox-Tags und
    Inbox-Dokumente als skalare Unterabfragen in einer Anweisung, danach die MIME-Gruppen).
    Ergebnis identisch.
12. **pk außerhalb des Spaltenbereichs** (z. B. `/api/tasks/99999999999/`): wie Django
    `No PaperlessTask matches the given query.`; keine Zahl → `Not found.`
13. **Modelldatei im Status (C3):** upstream prüft `MODEL_FILE` (`PAPERLESS_MODEL_FILE`, Standard
    `DATA_DIR/classification_model.pickle`). PaperRust liest das Pickle nicht (eigenes Modellformat,
    pl-classifier) und prüft deshalb die Datei des PaperRust-Modells; `PAPERLESS_MODEL_FILE` verschiebt
    sie nicht (das upstream-Pickle bleibt für einen Rückweg unangetastet).

## Paritätsstand (2026-11-08, Referenz v3.4.2 auf Windows)

`crates/pl-system/tests/reference_parity.rs` (`cargo test -p pl-system -j 2 --test reference_parity
-- --ignored`): dieselben Requests an Django (:8000) und in-process an die Rust-Handler (gleiche DB,
Tokens admin/alice/bob/carol, API v9 und v10).

- `parity_with_django`: **352 GET-Fälle, different = 0** (logs, statistics, status, config, tasks
  Liste/Filter/Fehler/Sortierung/Paginierung, Detail inkl. 404-Varianten, summary-Fehler,
  status_counts, active). Normalisiert: `server_os`, `storage.available`.
- `parity_rejected_posts`: **56 POST-Fälle (acknowledge/run/config create, nur abgelehnte Requests,
  nichts geschrieben), different = 0.**
- Nicht verglichen: erfolgreiches `acknowledge`/`run` (schreibt; nur mit `reference/reset.sh`),
  `remote_version` (Netz), Celery-Ping mit echtem Worker (Referenz hat keinen Worker; beide melden
  `WARNING`), deutsche Labels (siehe unten).
- **Referenz ohne kompilierte Übersetzungen:** `upstream/src/locale/*/django.mo` fehlen (das
  Docker-Image führt `compilemessages` aus). Django liefert dort `task_type_display` usw. englisch,
  DRF-Fehlertexte aber übersetzt. Rust übersetzt die Labels wie das Docker-Image; der Test
  überspringt Deutsch, solange die `.mo`-Dateien fehlen.

Aktivierbar (Parität belegt): `logs.list`, `logs.retrieve`, `statistics.get`, `config.list`,
`config.retrieve`, `config.create`, `tasks.list`, `tasks.retrieve`, `tasks.summary`,
`tasks.status_counts`, `tasks.active`, `tasks.acknowledge` (Fehlerpfade belegt, Schreibpfad nur
Unit-getestet). Vorsicht (Stand 2026-11-08): `status.get` (Celery-Ping ungeprüft gegen echten Worker), `tasks.run`
(Senden ungeprüft gegen Worker), `remote_version.get` (Cache getrennt, sonst gleich).

### Stand R4 (2026-11-09)

- In-Prozess-Test `parity_with_django`: 352 Fälle, 6 abweichend – nur `current_asn` (Statistik,
  Entscheidung 2026-11-08, im Harness Regel `statistics-asn-filtered`); `paperrust` wird vor dem
  Vergleich entfernt. `parity_rejected_posts` 0 abweichend.
- Harness (`--crate pl-system --crate pl-auth-api --no-write`, B = `results/bin/paperrust-r4.exe`
  auf :8302, Mischbetrieb/Celery, `results/parity/routes-all.toml`): 326 verglichen,
  305 identisch, 7 akzeptiert, **14 verschieden, alle erwartet** (`results/parity/r4/2026-11-09_032925.md`):
  `status@admin` (Zusatz `$.paperrust`), 3× `remote_version` (`$.version`, Zusatz `$.paperrust`),
  10× `ui_settings*` (`app_title`/`app_logo` aus dem pl-server-Branding).
- Regel-Vorschläge (nicht eingetragen): `status-paperrust-extra` (case `status@*`, kind `extra`,
  path `\$\.paperrust.*`, target b); `remote-version-paperrust` (case `remote_version@*`, kind
  `value`/`extra`, path `\$\.(version|update_available|paperrust.*)`, target b);
  `branding-ui-defaults` (case `ui_settings*`, kind `type`/`value`, path
  `\$\.settings\.app_(title|logo)`, target b, ref docs/contract/pl-server.md).

## Entscheidungen

- 2026-11-08: ASN-Maximum in `/api/statistics/` **gefiltert** auf sichtbare Dokumente (wie upstream-Fix #14710, nach v3.4.2). Bewusste Abweichung von v3.4.2; im Paritäts-Harness als akzeptierte Regel `statistics-asn-filtered` führen.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`, Ergänzung)

- **d05e96cda (#14734) – `user_args` ohne vom Parser verwaltete Argumente.** Quelle:
  `paperless/serialisers.py::ApplicationConfigurationSerializer.validate_user_args`. Rust:
  `config_write::OCR_RESERVED_USER_ARGS` (gleiche Liste wie `pl_ocr::args`, hier ohne pl-ocr-
  Abhängigkeit); ein JSON-Objekt mit `input_file_or_options`, `output_file` oder `sidecar` →
  400 `{"user_args": ["user_args may not set arguments managed by paperless: <sortiert, ', '>."]}`,
  nichts gespeichert. Prüfung nach „must be a JSON object“. Test: `config_write::tests` (Fall aus
  `test_api_update_config_rejects_reserved_user_args` plus alle drei Schlüssel zugleich).
  Parität: v3.4.2 nimmt den Wert an – der Harness hat keinen Fall mit diesen Schlüsseln.
