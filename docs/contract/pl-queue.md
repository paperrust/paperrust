# Vertrag `pl-queue` – eingebettete Aufgaben-Warteschlange

Ziel: Betrieb ohne Redis/Celery (docs/STANDALONE.md, Leitplanke 1). Seit 2026-11-14 ist die
PostgreSQL-Warteschlange (`PgQueue`, Tabelle `paperrust_task_queue`) die **einzige** Warteschlange;
Celery-Brücke und Backend-Auswahl sind entfernt. Teil A beschreibt das upstream-Verhalten (Celery),
das nachgebaut wird. Upstream-Stand
paperless-ngx **v3.4.2**, Celery 5.6.3 / kombu 5.6.2 / billiard (Quelltext in `reference/venv`
gelesen, nicht aus dem Gedächtnis). Crate: `crates/pl-queue`, Migration
`crates/pl-queue/migrations/0001_paperrust_task_queue.sql` (+ `0002_paperrust_task_queue_fence_token.sql`), DB-Tests `crates/pl-queue/tests/db.rs`,
Golden-Generator `tests/parity/pl-queue/celery_crontab_golden.py`.

## Teil A – Upstream-Verhalten (Quelle in Klammern)

### A1. Celery-Einstellungen (`src/paperless/settings/__init__.py`, `paperless/celery.py`)

| Einstellung | Wert | Bedeutung für PaperRust |
|---|---|---|
| `CELERY_WORKER_CONCURRENCY` | `PAPERLESS_TASK_WORKERS` (1) | Parallelität je Worker |
| `THREADS_PER_WORKER` | `PAPERLESS_THREADS_PER_WORKER`, sonst `max(floor(cpus / workers), 1)` | an Handler (OCR) durchgereicht |
| `CELERY_TASK_TIME_LIMIT` | `PAPERLESS_WORKER_TIMEOUT` (1800, int) | hartes Zeitlimit; Fehlertext `TimeLimitExceeded(1800,)` (billiard `__str__`) |
| `CELERY_TASK_TRACK_STARTED` | True | Zustand STARTED existiert |
| `CELERY_WORKER_MAX_TASKS_PER_CHILD` | 1 | jeder Task in frischem Prozess |
| `CELERY_TASK_ALLOW_ERROR_CB_ON_CHORD_HEADER` | True | Errback je fehlgeschlagenem Chord-Kopf (muss idempotent sein) |
| `CELERY_RESULT_EXPIRES` | 3600 | Ergebnisse 1 h |
| `CELERY_TIMEZONE` | `TIME_ZONE` (`PAPERLESS_TIME_ZONE`) | Zeitzone der crontabs |
| `CELERY_TASK_SERIALIZER` / `ACCEPT_CONTENT` | signed-pickle / `[json, x-signed-pickle]` | siehe pl-core §10 |
| `CELERY_IMPORTS` | `("documents.bulk_edit",)` | |
| `CELERY_BEAT_SCHEDULE_FILENAME` | `DATA_DIR/celerybeat-schedule.db` | ersetzt durch `paperrust_beat_state` |
| acks_late | Celery-Standard False | Nachricht vor Ausführung bestätigt |
| Queue/Priorität | keine `task_routes`, keine Prioritäten | alles in `celery`, Priorität 0 |

### A2. Alle Aufgaben (`@shared_task`)

`tracked` = in `TRACKED_TASKS` (`documents/signals/handlers.py`) → `PaperlessTask.task_type`.

| Task | Signatur | Optionen | tracked | Rückgabe |
|---|---|---|---|---|
| `documents.tasks.index_optimize` | `()` | – | – | None (No-op, Tantivy) |
| `documents.tasks.index_document` | `(document_id)` | bind, `ignore_result`, `autoretry_for=(SearchIndexLockError,)`, `max_retries=5`, `retry_backoff=60`, `retry_jitter=True` (max 600) | – | None |
| `documents.tasks.remove_document_from_index` | `(doc_id)` | wie `index_document` | – | None |
| `documents.tasks.train_classifier` | `(*, status_callback=None)` | – | `train_classifier` | Text („No automatic matching items, not training“ / „Training completed successfully“ / „Training data unchanged“) |
| `documents.tasks.consume_file` | `(input_doc: ConsumableDocument, overrides: DocumentMetadataOverrides|None)` | bind | `consume_file` | dict `{"document_id"}` / `{"reason"}` / `{"duplicate_of","duplicate_in_trash"}` / None |
| `documents.tasks.sanity_check` | `(*, raise_on_error=True)` | – | `sanity_check` | Text; bei Fehlern `SanityCheckFailedException` |
| `documents.tasks.bulk_update_documents` | `(document_ids)` | – | `bulk_update` | None |
| `documents.tasks.update_document_content_maybe_archive_file` | `(document_id, *, remote_ocr=False)` | – | `reprocess_document` | None |
| `documents.tasks.empty_trash` | `(doc_ids=None)` | – | `empty_trash` | None |
| `documents.tasks.check_scheduled_workflows` | `()` | – | `check_workflows` | None |
| `documents.tasks.llmindex_index` | `(*, iter_wrapper=identity, rebuild=False)` | – | `llm_index` | Text/None |
| `documents.tasks.apply_ai_suggestions` | `(action_id, document_id)` | bind, `autoretry_for=(LLMTimeoutError,)`, `max_retries=3`, `retry_backoff=60`, `retry_backoff_max=600`, jitter | `apply_ai_suggestions` | None |
| `documents.tasks.update_document_in_llm_index` | `(document: Document)` | – (Pickle) | – | None |
| `documents.tasks.remove_document_from_llm_index` | `(document: Document)` | – (Pickle) | – | None |
| `documents.tasks.build_share_link_bundle` | `(bundle_id)` | – | `build_share_link` | None (Fehler → re-raise) |
| `documents.tasks.cleanup_expired_share_link_bundles` | `()` | – | – | None |
| `documents.tasks.clear_expired_sessions` (upstream fix a139bdff4, nach v3.4.2) | `()` | – | – | None |
| `documents.bulk_edit.delete` | `(doc_ids)` | – | `bulk_delete` | `"OK"` |
| `documents.bulk_edit.restore_archive_serial_numbers_task` | `(backup, *args, **kwargs)` | bind | – | None |
| `documents.signals.handlers.process_cf_select_update` | `(custom_field: CustomField)` | – (Pickle) | – | None |
| `documents.workflows.webhooks.send_webhook` | `(url, data, headers, files, *, as_json=False)` | `retry_backoff=True` (Faktor 1), `autoretry_for=(httpx.HTTPStatusError,)`, `max_retries=3`, `throws=(httpx.HTTPError,)` | – | – |
| `paperless_mail.tasks.process_mail_accounts` | `(account_ids=None)` | bind; Cache-Sperre `paperless_mail_fetch_lock` (TTL 1800, nach jedem Konto erneuert) | `mail_fetch` | Text („Added N document(s).“ / „No new documents were added.“ / „Skipped: mail account processing already in progress.“) |
| `paperless_mail.mail.apply_mail_action` | `(result, rule_id, message_uid, message_subject, message_date: datetime, uid_validity=None)` | – | – | None |
| `paperless_mail.mail.error_callback` | `(request, exc, tb, rule_id, message_uid, message_subject, message_date, uid_validity=None)` | – | – | None (`ProcessedMail` FAILED, `get_or_create` → idempotent) |

Celery-Neuversuch (`celery/app/autoretry.py`, `utils/time.py`): `countdown =
min(backoff_max, factor * 2**retries)`, mit Jitter `randrange(countdown + 1)`; ohne Backoff
`default_retry_delay = 180`. `retries = request.retries + 1 > max_retries` → ursprüngliche Exception
(FAILURE). Explizites `self.retry()` ohne Angabe: `max_retries = 3`.

### A3. Aufrufer (wer sendet was)

| Aufrufer (Datei::Funktion) | Task | Argumente | `trigger_source` |
|---|---|---|---|
| `bulk_edit.set_correspondent/set_storage_path/set_document_type/add_tag/remove_tag/modify_tags/modify_custom_fields/set_permissions` | `bulk_update_documents` | `kwargs={"document_ids": [...]}` | system |
| `bulk_edit.reprocess` | `update_document_content_maybe_archive_file` | `{"document_id", "remote_ocr"}` je Dokument | manual |
| `bulk_edit.rotate/delete_pages/edit_pdf/remove_password` | `consume_file` | `input_doc`, `overrides` (Pickle) | Parameter (Standard web_ui) |
| `bulk_edit.merge` | `consume_file.s(...)` + `link=[delete.si(ids)]`, `link_error=[restore_archive_serial_numbers_task.s(backup)]` (nur `delete_originals`) | Kette | Parameter |
| `bulk_edit.split/edit_pdf/remove_password` (delete_originals) | `chord(header=[consume_file.s...], body=delete.si([id])).on_error(restore_archive_serial_numbers_task.s(backup))`; sonst `group(...)` | Chord/Gruppe | Parameter |
| `bulk_edit.merge_as_versions` | `remove_document_from_index` `args=[id]`, `countdown=60`; `bulk_update_documents` | | system |
| `tasks.update_document_parent_tags` | `bulk_update_documents` | `{"document_ids"}` | system |
| `tasks.apply_ai_suggestions` | `index_document.delay(pk)`; `update_document_in_llm_index` (Pickle) | | – |
| `search/_backend.py` (Lock-Budget erschöpft) | `index_document` / `remove_document_from_index` | `args=[pk]`, `countdown=60` | – |
| `signals/handlers.check_paths_and_prune_custom_fields` | `process_cf_select_update` | `custom_field=<Instanz>` (Pickle) | – |
| `signals/handlers.add_or_update_document_in_llm_index` / `delete_document_from_llm_index` | `update_/remove_document_from_llm_index` | `document=<Instanz>` (Pickle) | – |
| `serialisers.py:…update` (StoragePath), `views.py::StoragePathViewSet.destroy` | `bulk_update_documents` | `{"document_ids"}` | system |
| `views.py::PostDocumentView.post` | `consume_file` | `input_doc`, `overrides` | web_ui / api_upload |
| `views.py::DocumentViewSet` (neue Version, ~2187) | `consume_file` | | web_ui |
| `views.py::TasksViewSet.run` (`/api/tasks/run/`, Superuser) | `train_classifier {}` / `sanity_check {"raise_on_error": False}` / `llmindex_index {"rebuild": False}` | Antwort `{"task_id": …}` | manual |
| `views.py::ShareLinkBundleViewSet.create/rebuild` | `build_share_link_bundle` | `{"bundle_id"}` | manual |
| `workflows/actions.py` (Webhook-Aktion) | `send_webhook` | `{"url","data","headers","files","as_json"}` | – |
| `paperless/views.py::ApplicationConfigurationViewSet.perform_update` | `llmindex_index` | `{"rebuild": True}` | system |
| `paperless_ai/indexing.py::queue_llm_index_update_if_needed` | `llmindex_index` | `{"rebuild": …}` (nur wenn kein LLM-Task läuft/≤5 min alt) | system |
| `paperless_mail/views.py::MailAccountViewSet.process` | `process_mail_accounts` | `{"account_ids": [pk]}` | manual |
| `paperless_mail/mail.py::queue_consumption_tasks` | `chord(header=[consume_file.s…], body=apply_mail_action.s(rule_id=…, message_uid=…, message_subject=…, message_date=…, uid_validity=…)).on_error(error_callback.s(…))` | Chord | email_consume |
| `management/commands/document_consumer.py` | `consume_file` | `ConsumableDocument(ConsumeFolder, path)`, `overrides(tag_ids)` | folder_consume |
| `barcodes.py::BarcodePlugin.run` | `consume_file` je Teil | | Parameter |
| Beat (A4) | siehe A4 | `{}` | scheduled |

### A4. `CELERY_BEAT_SCHEDULE` (`settings/custom.py::parse_beat_schedule`)

| Name | Variable | Standard | Task | `expires` (s) |
|---|---|---|---|---|
| Check all e-mail accounts | `PAPERLESS_EMAIL_TASK_CRON` | `*/10 * * * *` → wenn **nicht gesetzt**: `m,m+10,… * * * *` mit `m = int.from_bytes(sha256(SECRET_KEY)) % 10` | `paperless_mail.tasks.process_mail_accounts` | 540 |
| Train the classifier | `PAPERLESS_TRAIN_TASK_CRON` | `5 */1 * * *` | `documents.tasks.train_classifier` | 3540 |
| Optimize the index | `PAPERLESS_INDEX_TASK_CRON` | `0 0 * * *` | `documents.tasks.index_optimize` | 82800 |
| Perform sanity check | `PAPERLESS_SANITY_TASK_CRON` | `30 0 * * sun` | `documents.tasks.sanity_check` | 601200 |
| Empty trash | `PAPERLESS_EMPTY_TRASH_TASK_CRON` | `0 1 * * *` | `documents.tasks.empty_trash` | 82800 |
| Check and run scheduled workflows | `PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON` | `5 */1 * * *` | `documents.tasks.check_scheduled_workflows` | 3540 |
| Rebuild LLM index | `PAPERLESS_LLM_INDEX_TASK_CRON` | `10 2 * * *` | `documents.tasks.llmindex_index` | 82800 |
| Cleanup expired share link bundles | `PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON` | `0 2 * * *` | `documents.tasks.cleanup_expired_share_link_bundles` | 82800 |
| Clear expired sessions (upstream fix a139bdff4 #14722, nach v3.4.2) | `PAPERLESS_SESSION_CLEANUP_TASK_CRON` | `0 3 * * *` | `documents.tasks.clear_expired_sessions` | 82800 |

* Wert `disable` → Eintrag fehlt. Format `minute hour day_month month day_week`, **genau ein
  Leerzeichen** (`value.split(" ")`, sonst Startfehler). `options.headers = {"trigger_source": "scheduled"}`.
* crontab-Semantik (`celery/schedules.py`): siehe `pl_queue::cron` – Bereiche mit Umlauf, Schritte
  per Slice, `re.match` nur am Anfang verankert, Namen erst als Monat, dann als Wochentag (in jedem
  Feld), **UND** von Tag-im-Monat und Wochentag, Wochentag 0–6 (0 = Sonntag), Fehlertexte.
* Beat (`celery/beat.py`): neuer Eintrag `last_run_at = now` (kein Sofortlauf); fällig, wenn der
  nächste Zeitpunkt nach `last_run_at` ≤ jetzt; dann `last_run_at = now` **vor** dem Senden
  (`reserve`), Sendefehler nur geloggt. Verpasste Läufe → **ein** Nachholen. `max_interval` 300 s.
  Zeitzonenwechsel → Zustand verworfen. Entfernte Einträge werden gelöscht (`merge_inplace`).

### A5. `PaperlessTask`-Lebenszyklus (`documents/signals/handlers.py`, `models.py`)

Tabelle `documents_paperlesstask`: `id, owner_id, task_id(72, unique), task_type, trigger_source,
status, date_created, date_started, date_done, duration_seconds, wait_time_seconds, input_data
(jsonb, nicht null), result_data (jsonb), acknowledged`. Nur für `TRACKED_TASKS`.

| Signal | Wirkung |
|---|---|
| `before_task_publish` | `get_or_create(task_id, defaults={task_type, trigger_source (Header, gültig sonst manual), status: pending, input_data, owner_id})`; `date_created = now`, `acknowledged = False`. Retry mit gleicher ID ändert nichts. |
| `task_prerun` | `status = started, date_started = now` |
| `task_postrun` (SUCCESS/REVOKED) | `status`, `date_done`; `duration_seconds = done - started` und `wait_time_seconds = started - created`, nur wenn `date_started`; `result_data` **nur bei dict-Rückgabe**; dict mit `duplicate_of` → `status = failure`. FAILURE/RETRY werden hier übersprungen. |
| `task_failure` | `status = failure`, `result_data = {"error_type": type(exc).__name__, "error_message": str(exc)[, "traceback": tb[:5000]]}`, `date_done`, Dauer/Wartezeit wie oben |
| `task_revoked` | `status = revoked, date_done = now, result_data = {"reason": "<Grund> Check worker logs for details."}`, Grund: `Task revoked before completion.` / `Task expired before execution.` / `Task terminated during execution.` |

`input_data`: `consume_file` → `{"filename": original_file.name, "mime_type"[, "source_path"][, "mailrule_id"][, "overrides": {nicht-None, nicht "_"…}]}`;
`mail_fetch` → `{"account_ids"}`; `apply_ai_suggestions` → `{"document_id"}`; sonst `{}`.
`owner_id` nur `consume_file` (`overrides.owner_id`). Status-Übergänge: PENDING → STARTED →
SUCCESS|FAILURE, PENDING → REVOKED; Retry bleibt `started` (nächster Start setzt `date_started` neu).
Celery-Details: hartes Zeitlimit (`Request.on_timeout`) und Totalverlust des Workers senden **kein**
`task_failure` → upstream bleibt der Eintrag `started`; Kindprozess-Tod → `WorkerLostError`
(„Worker exited prematurely: … Job: N.“) über `task_failure`. API `/api/tasks/` liest nur diese
Tabelle (pl-system).

### A6. Statusmeldungen (`documents/plugins/helpers.py`, `paperless/consumers.py`)

Channels-Gruppe `status_updates`; Consumer sendet `json.dumps(event)` (Trenner `", "`/`": "`,
`ensure_ascii`), Heartbeat alle 30 s `{"type": "heartbeat"}`.

* `{"type": "status_update", "data": {"filename", "task_id", "current_progress", "max_progress",
  "status" (STARTED|WORKING|SUCCESS|FAILED), "message", "document_id", "owner_id",
  "users_can_view", "groups_can_view"}}` (`ProgressManager.send_progress`, Consumer/Barcode/
  `consume_file` bei Ausnahme `FAILED, str(e), 100, 100`).
* `{"type": "documents_deleted", "data": {"documents": [...]}}` (an alle Angemeldeten).
* `{"type": "document_updated", "data": {"document_id", "modified", "owner_id", "users_can_view", "groups_can_view"}}`.
* Sichtbarkeit (`_can_view`): Superuser, Eigentümer, in `users_can_view` oder Gruppe in `groups_can_view`.

## Teil B – PaperRust-Umsetzung

### B1. API

```rust
pub trait TaskQueue: Send + Sync {            // objekt-sicher: Arc<dyn TaskQueue>
    fn enqueue<'a>(&'a self, task_name: &'a str, kwargs: Map<String, Value>, opts: EnqueueOptions) -> BoxFuture<'a, Result<Uuid, QueueError>>;
    fn status(&self, task_id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>>;
    fn revoke(&self, task_id: Uuid, terminate: bool) -> BoxFuture<'_, Result<bool, QueueError>>;          // Standard: Unsupported
    fn enqueue_group(&self, header: Vec<Signature>) -> BoxFuture<'_, Result<Uuid, QueueError>>;          // Standard: Unsupported
    fn enqueue_chord(&self, header: Vec<Signature>, body: Signature, on_error: Option<Signature>) -> BoxFuture<'_, Result<Uuid, QueueError>>;
}
EnqueueOptions { args, task_id, trigger_source, headers, countdown, eta, expires, queue, priority (0..=9, 0 = höchste), time_limit, link, link_error }
Signature { task, args, kwargs, immutable (.si), options }
TaskStatus { Pending, Started, Retry, Success, Failure, Revoked }   TaskInfo { task_id, task_name, status, result, error, retries, date_done }
pl_queue::from_config(&QueueConfig, &AppState) -> Arc<dyn TaskQueue>   // immer PgQueue
QueueError { Db, Query, Json, Unsupported, Invalid }
```

* ~~**`CeleryBridge`**~~ (entfernt 2026-11-14, mit `pl_queue::Backend`, `QueueError::Celery` und
  `TaskDef::pickle_only`): reichte früher über `pl_core::celery::CeleryProducer` an Celery weiter.
* **`PgQueue`**: `enqueue_in(&mut PgConnection, …)` reiht **in der Transaktion des Aufrufers** ein
  (PaperlessTask + Queue-Zeile + `pg_notify('paperrust_task_queue', queue)` atomar). Gleiche
  `task_id` erneut = No-op. Worker-API: `claim`, `mark_started`, `heartbeat`, `complete`, `fail`,
  `retry`, `revoke_job`, `release`, `reap_expired_leases`, `purge_finished`.
* **`Worker`** (`Handlers::register(name, |ctx: TaskContext| async { Ok(json!(…)) })`): beansprucht
  nur registrierte Aufgaben der konfigurierten Queues; Fehler als `TaskError::{fail, autoretry,
  retry}` (`error_type` = Python-Klassenname, `message` = `str(exc)`).
* **Beat**: `beat::parse_beat_schedule(&Env)`, `Beat::new(entries, store, queue, tz).run(shutdown)`,
  Speicher `PgBeatStore` (Tabelle) oder `MemoryBeatStore`.
* **Ereignisse**: `EventBus` (tokio `broadcast`), `ProgressManager`, `DocumentsStatusManager`,
  `WsPayload::{to_value, to_ws_text, from_value}`, `events::visible_to`, `events::notify`
  (`pg_notify('paperrust_status', json)`) + `relay_notifications` (LISTEN → Bus) für getrennte
  Prozesse. Das Relay verbindet nach Verbindungsverlust (auch `pg_terminate_backend` → FATAL
  57P01, das sqlx nicht selbst behandelt) mit Backoff 0,5 s … 30 s neu und setzt `LISTEN` erneut ab;
  der alte Listener ist dann verworfen, NOTIFY wird nicht nachgeliefert → keine Doppelzustellung,
  Meldungen in der Lücke gehen verloren. Rückkehr nur bei Shutdown oder geschlossenem Pool
  (`relay_notifications_with(.., RelayBackoff)` für Tests).

### B2. Tabelle `paperrust_task_queue` (Migrationen 0001 + 0002, expand-only, nicht automatisch)

Spalten: `id, task_id (unique), task_name, queue, priority, args, kwargs, headers, state
(queued|running|done|failed|revoked), attempts (Zustellungen seit Einreihen/Retry, für
`MAX_DELIVERIES` und Coalescing), retries, eta, expires, time_limit, locked_by, locked_until,
heartbeat_at, revoke_requested, terminate_requested, link, link_error, group_id, group_index,
created_at, started_at, finished_at, result, error` und seit 0002 (2026-11-23)
`fence_token bigint NOT NULL DEFAULT 0` (Fencing-Token je Zustellung: +1 bei jedem Beanspruchen,
Retry und Release, nie zurückgesetzt; `ADD COLUMN IF NOT EXISTS`, Rückbau `DROP COLUMN`). Dazu `paperrust_task_group` (Chord-Zähler) und `paperrust_beat_state`
(`name, task, last_run_at, total_run_count, tz`). Einspielen: `psql -f` oder bewusst
`pl_queue::migrations::apply(&pool)` (spielt 0001 und 0002 ein; macht auch der Worker-Start und
`paperrust manage migrate`). Rückbau: die drei Tabellen löschen. Beim Update alle Worker-Prozesse
zusammen neu starten (ein alter Worker erhöht `fence_token` nicht).

### B3. Ablauf

1. **Einreihen**: `record_pending` (wie `before_task_publish`) → `INSERT … ON CONFLICT (task_id) DO NOTHING`
   → NOTIFY. `eta` = `opts.eta` oder `now + countdown`; `expires` absolut.
2. **Beanspruchen**: `WITH next AS (SELECT id … WHERE state='queued' AND eta<=now AND queue=ANY AND task_name=ANY ORDER BY priority, eta, id LIMIT n FOR UPDATE SKIP LOCKED) UPDATE … SET state='running', locked_by, locked_until=now+visibility, attempts=attempts+1,
   fence_token=fence_token+1`. Der lokale Lease-Beginn (`Job::claimed_at`) wird zusammen mit diesem
   `now` genommen, vor dem Senden.
3. **Vor dem Start** (`Request.revoked`): `revoke_requested` → REVOKED „revoked“; `expires ≤ now` →
   REVOKED „expired“ (ohne `date_started`).
4. **Start**: Nur solange die Lease nachweisbar ist (vor 80 % seit `claimed_at`); dann
   `mark_started` als **eine Transaktion**: gefencter Lease-`UPDATE` (wie Heartbeat) + PaperlessTask
   `started` (TLA+-Befund F2, 2026-11-23). Gehört die Zeile inzwischen einer anderen Zustellung oder
   ist sie fertig → nichts geschrieben, Handler startet **nicht**; `terminate_requested` inzwischen
   gesetzt → REVOKED „terminated“ ohne Ausführung; DB-Fehler → Handler startet auf der Lease des
   Beanspruchens (Selbst-Fencing gilt). Handler in eigenem tokio-Task; Heartbeat `visibility/3` nach
   dem letzten erfolgreichen, nach einem Fehlschlag erneut nach `visibility/15`;
   `terminate_requested` → Abbruch → REVOKED „terminated“.
5. **Ende**: Erfolg → `done` + `mark_success` + `link`-Callbacks (Ergebnis als erstes Argument, außer
   `.si`) + Gruppenabrechnung. Fehler → `RetryMode::Auto` mit Task-Regel (A2) bzw. `Explicit`
   (Standard 3/180 s): `queued`, `eta = now + countdown`, `retries+1`, PaperlessTask bleibt
   `started`; sonst `failed` + `mark_failure` + `link_error` + Gruppenabrechnung. Panik →
   FAILURE `Panic`. Zeitlimit → FAILURE `TimeLimitExceeded` / `TimeLimitExceeded(<limit>,)`.
6. **Chord**: Kopf-Tasks tragen `group_id/group_index`; Zähler unter Zeilensperre. Alle erfolgreich →
   Body mit `[[Ergebnisse in Kopf-Reihenfolge]]`. Jeder fehlgeschlagene Kopf → `on_error` mit
   `(request, exc, traceback)`; nach dem letzten Kopf mit Fehlern zusätzlich **einmal** `on_error`
   mit `exc_type = "ChordError"`; Body läuft nicht. Widerrufene Köpfe zählen als Fehler (ohne
   eigenen Errback). Leerer Chord → Body sofort mit `[[]]`.
   Errback-Argumente (JSON statt Python-Objekte): `request = {"id","task","args","kwargs","group","retries"}`,
   `exc = {"exc_type","exc_message"}`, `traceback = str|null`; Argumente werden wie
   `Signature._merge` **vor** die eigenen gesetzt.
7. **Absturz** (keine Heartbeats): nach `locked_until` legt `reap_expired_leases` (jeder Worker,
   alle `visibility/2`) den Job zurück (`queued`); nach `PAPERRUST_QUEUE_MAX_DELIVERIES` Zustellungen
   FAILURE `WorkerLostError` „Worker exited prematurely: lease expired after N deliveries. Job: <id>.“
8. **Fencing / keine doppelte Ausführung**: Abschluss/Heartbeat/`mark_started`/Retry/Release nur mit
   gleichem `fence_token` und (`running` + eigener `locked_by` **oder** `queued` = zurückgelegt, aber
   nicht neu beansprucht). Bis 2026-11-23 war der Token `attempts`; der wiederholte sich innerhalb
   eines Prozesses nach Retry/Release (TLA+-Befund F3: verspätetes `complete()` einer alten Zustellung
   wurde für eine neuere angenommen, angehängte Coalescing-IDs gingen verloren). Der Worker bricht
   selbst ab, sobald er seine Lease nicht mehr mit Reserve nachweisen kann: eigener Timer bei 80 % der
   Lease nach dem Senden des letzten erfolgreichen Lease-Schreibens (Beanspruchen, `mark_started`,
   Heartbeat), unabhängig von Heartbeat-Versuchen (TLA+-Befund F1: vorher nur nach einem Versuch
   geprüft, bei sofort fehlschlagenden Heartbeats erst bei 100 %), oder wenn der Heartbeat „verloren“
   meldet. Garantie: höchstens
   ein Worker führt einen Job gleichzeitig aus, solange die Uhren der Worker-Rechner nicht mehr als
   20 % der Lease auseinanderliegen; „genau einmal“ gilt für Zustand und PaperlessTask, Handler
   müssen bei Neuzustellung nach Absturz idempotent sein (wie mit Celery-acks_late).
9. **Herunterfahren** (`watch`-Signal): keine neuen Jobs; laufende bekommen
   `PAPERRUST_QUEUE_SHUTDOWN_GRACE`; danach Abbruch + `release` (sofort wieder verfügbar,
   `attempts-1`).
10. **Aufräumen**: erledigte Zeilen nach `PAPERRUST_QUEUE_RESULT_EXPIRES` löschen (PaperlessTask bleibt).
11. **DB-Fehler der Schleifen** (W18, E2E-Befund 2026-11-08: fehlende `paperrust_task_queue` nach
    `DROP/CREATE DATABASE` unter laufendem Server → ~1000 Logzeilen/s): Schlägt Reap/Purge/Claim fehl,
    wartet der Worker per exponentiellem Backoff (`backoff::Backoff`, 0,5 s → 1 → 2 … → 30 s, Reset beim
    ersten Erfolg mit einer Info-Zeile „database access works again“) und loggt **eine** Zeile je
    Durchgang/Stufe (`attempt`, `retry_in_ms`). SQLSTATE `42P01` (undefined_table) → Migration 0001
    (idempotent, legt auch `paperrust_beat_state` an) wird sofort erneut eingespielt. Fehler von
    `LISTEN`-`recv()` (z. B. DB weg, Backend vom Server beendet) → Listener verwerfen, bis zur nächsten
    Backoff-Stufe nur Polling, dann neuen Listener aufbauen (sqlx verbindet nach 57P01 nicht selbst neu;
    vorher blieb der Worker danach dauerhaft ohne LISTEN). Backoff-Reset erst bei empfangener Meldung. Beat: Zustandsfehler
    einmal je Runde geloggt (nicht je Eintrag), nächster Versuch nach Backoff (nie später als der
    reguläre Termin). Test: `tests/recovery_db.rs` (eigene DB `pl_w18_*`, Tabelle wird gelöscht).

### B4. Konfiguration (`QueueConfig::from_env`)

| Variable | Standard | |
|---|---|---|
| `PAPERRUST_QUEUE_BACKEND` / `PAPERRUST_QUEUE` | – | **wirkungslos seit 2026-11-14** (immer PostgreSQL); `celery`/`redis` → Hinweis beim Start, nie Abbruch |
| `PAPERLESS_TASK_WORKERS` | 1 (upstream); `auto` = automatisch (`pl_core::resources::task_workers_from_env`; Known difference 16) | Parallelität (≥ 1) |
| `PAPERLESS_THREADS_PER_WORKER` | `max(floor(cpus/workers),1)` | an Handler |
| `PAPERLESS_WORKER_TIMEOUT` | 1800 | Zeitlimit (s) |
| `PAPERLESS_TIME_ZONE` | UTC | Beat-Zeitzone |
| `PAPERRUST_QUEUE_NAMES` | `celery` | Liste |
| `PAPERRUST_QUEUE_VISIBILITY_TIMEOUT` | 60 | Lease (s) |
| `PAPERRUST_QUEUE_MAX_DELIVERIES` | 3 | **in allen Prozessen gleich setzen** |
| `PAPERRUST_QUEUE_POLL_INTERVAL` | 1 | s (zusätzlich LISTEN) |
| `PAPERRUST_QUEUE_SHUTDOWN_GRACE` | 30 | s |
| `PAPERRUST_QUEUE_RESULT_EXPIRES` | 3600 | s |
| `PAPERRUST_WORKER_NAME` | `paperrust@<host>:<pid>` | |
| `PAPERLESS_*_TASK_CRON`, `PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON`, `PAPERLESS_SECRET_KEY` | A4 | Beat |

### B5. `consume_file` als JSON (für den späteren Rust-Worker)

`kwargs = {"input_doc": {"source": int, "original_file": "<Pfad>", "mime_type": str, "original_path": str|null, "mailrule_id": int|null, "root_document_id": int|null}, "overrides": {<Felder von DocumentMetadataOverrides>, Datum als ISO-Text}}`.
`input_data`/`owner_id` daraus wie A5 (Python macht `date.isoformat()`/`str(Path)` selbst).
(Früher über die Celery-Brücke gesperrt, weil Celery Pickle erwartet; die Brücke ist entfernt.)

## Tests

* `cargo test -p pl-queue -j 1`: 18 Unit-Tests – crontab-Parser (Mengen/Fehlertexte gegen Celery),
  **160 Golden-Fälle** nächster Lauf (20 Ausdrücke × 8 Zeitpunkte, erzeugt mit Celery 5.6.3, Europe/Zurich, inkl.
  Sommerzeit-Lücke/-Überlappung, 29. Februar, Umlaufbereiche, `sat-mon`), `is_due`,
  `parse_beat_schedule` wie `test_custom_parsers.py` (Offset `test-secret` = 6), Beat-Reservierung,
  Backoff, Tracking-Tabelle = pl-core, `input_data`/`result_data`, WebSocket-JSON/`_can_view`,
  Konfiguration.
* `tests/recovery_db.rs` (1 Test, `#[ignore]`, braucht `PAPERRUST_QUEUE_RECOVERY_DATABASE_URL` auf
  `pl_w18_*`/`pl_queue_test*`, eigener Prozess weil die Tabelle gelöscht wird): Tabelle durch eine
  Sicht ohne Spalten ersetzt (Dauerfehler 42703) → in 6 s höchstens 6 Fehlerzeilen; Sicht gelöscht
  (42P01) → Worker spielt Migration erneut ein und führt danach einen Job aus; LISTEN-Backend per
  `pg_terminate_backend` beendet → neuer Listener. Vorher (alter Code, gleiche Sicht): 94 Zeilen in 6 s
  bei Poll 0,05 s, nachher 4.
* `tests/relay_db.rs` (1 Test, `#[ignore]`, braucht `PAPERRUST_RELAY_TEST_DATABASE_URL` auf
  `pl_sec_relay*`): Relay-Listener zweimal per `pg_terminate_backend` töten → neuer LISTEN-Backend,
  jede Meldung genau einmal, genau ein Listener aktiv, sauberes Stoppen.
* `cargo test -p pl-queue`: dazu 3 Unit-Tests der Lease-Zeitlogik (`worker::LeaseTiming`: 48/20/4 s
  bei 60 s, Fence-Termin 80 % nach letztem Erfolg, F1-Regression: sofort fehlschlagende Heartbeats
  enden bei 48 s statt 60 s; keine Panik bei NaN/0/negativ).
* `tests/db.rs` (10 Tests, `#[ignore]`; 2026-11-23 auf `pl_queue_test_fence` aus `paperless_snapshot`
  gelaufen, 10/10; darunter `stale_mark_started_is_a_noop` (F2: veraltetes `mark_started` → `Lost`,
  PaperlessTask bleibt `success`; zurückgelegte Zeile wird übernommen; Terminate vor Start) und
  `old_delivery_rejected_after_reclaim_in_same_process` (F3: gleiches `(attempts, locked_by)` nach
  Retry, altes complete/fail/retry/release/heartbeat/mark_started abgelehnt, angehängte IDs bleiben;
  Release erhöht den Token); brauchen `PAPERRUST_QUEUE_TEST_DATABASE_URL` auf eine
  Datenbank `pl_queue_test*`; der Test verweigert andere Namen): PaperlessTask-Lebenszyklus inkl.
  Duplikat→failure und consume_file-`input_data`; idempotentes Einreihen; 4 Worker × 3 parallel,
  300 Jobs, jeder genau einmal, Last verteilt; Absturz (Lease-Ablauf → Neuzustellung, Fencing des
  alten Workers, `WorkerLostError` nach max. Zustellungen, abgebrochener Worker mitten im Task →
  zweiter Worker); Autoretry mit Backoff, Erschöpfung, Zeitlimit, Fehler, Ablauf, Widerruf vor Start
  und mit `terminate`; Chord ok/Fehler, `link`, `link_error` (`.si`); sauberes Herunterfahren;
  Beat-Reservierung mit 8 gleichzeitigen Prüfern → genau eine.

## Known differences

1. **Absturz → Neuversuch** (Auftrag): Celery (acks_late=False) verliert Aufgaben eines toten
   Workers, der PaperlessTask bleibt `started`. PaperRust stellt neu zu (bis `MAX_DELIVERIES`) und
   endet sonst in FAILURE `WorkerLostError`. Handler müssen deshalb idempotent sein.
2. **Hartes Zeitlimit**: upstream sendet kein `task_failure` (PaperlessTask bleibt `started`);
   PaperRust schreibt FAILURE `{"error_type": "TimeLimitExceeded", "error_message": "TimeLimitExceeded(1800,)"}`
   – sonst blieben ewig „laufende“ Einträge, ohne Prozess, der sie beenden könnte.
3. **Kein Prozess je Task** (`MAX_TASKS_PER_CHILD=1`): Aufgaben laufen als tokio-Tasks im selben
   Prozess. Handler-Panik → FAILURE `Panic` statt Prozessabsturz.
4. **Einreihen transaktional**: PaperlessTask-Fehler brechen das Einreihen ab (upstream: nur Log).
   Dadurch gibt es nie eine Aufgabe ohne Eintrag oder umgekehrt.
5. **Unbekannte Aufgaben** werden nicht beansprucht (bleiben `queued`, bis ein Worker sie kennt);
   Celery verwirft „unregistered task“ mit Log.
6. **Errback-/Chord-Argumente** sind JSON (B3.6) statt Python-`Request`/Exception/Traceback-Objekte.
   ChordError-Text: „N of M header tasks failed“ statt `Dependency <id> raised <repr>`.
7. **Prioritäten/Queues**: upstream ungenutzt; PaperRust unterstützt 0..9 (0 zuerst) und mehrere
   Queues.
8. **Retry-Zustand**: `TaskInfo.status = Retry` (Queue), der PaperlessTask bleibt wie upstream `started`.
9. **Beat-Zustand** in der DB statt `celerybeat-schedule.db`; mehrere Zeitplaner gleichzeitig
   sind erlaubt (Zeilensperre), Celery verlangt genau einen Beat-Prozess. Sommerzeit: Wanduhr-
   Semantik wie Celery (Lücke → Offset vor Umstellung, mehrdeutig → erste Zeit); exotische Fälle
   (Lauf genau in der wiederholten Stunde mit `fold=1`) nicht gegen Celery geprüft.
   `celery.backend_cleanup` (nur ohne Auto-Expire-Backend) entfällt.
10. **Ereignisse**: In-Prozess-Bus verwirft bei vollem Puffer (1024) die ältesten Meldungen für
    langsame Empfänger (upstream Channels/Redis verhält sich bei Überlauf anders); NOTIFY-Nutzlast > 8000 Byte wird verworfen.
11. **crontab-Zahlen**: Python `int()` akzeptiert Nicht-ASCII-Ziffern; Rust nur ASCII.
12. **Mail-Abrufsperre** (`cache.add(paperless_mail_fetch_lock)`) ist Sache des späteren Mail-Handlers
    (pl-mail), nicht der Warteschlange.

13. **`documents.bulk_operations.execute_bulk_operation`** (upstream `dev` 60a35edee, nach v3.4.2)
    ist in `TASKS` als getrackte Aufgabe (`bulk_update`) eingetragen; `input_data = {operation,
    document_count}` und `owner_id = user_id` setzt `paperless_task::input_data_for/owner_id_for`
    (upstream: Sonderfall in `before_task_publish_handler`). Gesendet nur über die PaperRust-
    Warteschlange (pl-documents-write `bulk_task`).

14. **Kein Celery, kein Redis** (2026-11-14): Nachrichten von paperless-ngx/Celery (Redis-Liste `celery`)
    werden nicht gelesen; beim Umstieg müssen offene Aufgaben in paperless-ngx vorher abgearbeitet sein.
    `/api/tasks/`, Task-IDs und Zustände (`PENDING`, `STARTED`, `SUCCESS`, `FAILURE`, …) bleiben gleich.
15. **„Empty trash“ beim ersten Start** – zurückgenommen 2026-11-17: Wie Celery läuft „Empty trash“
    erst zum nächsten Termin (Standard 1 Uhr nachts), nicht sofort beim ersten Start. Die kurzzeitige
    Abweichung (A-131, `last_run_at = jetzt − 400 Tage`) leerte nach einer Datenbank-Übernahme sofort die
    übernommenen alten Papierkorb-Einträge; die Umstiegsprüfung meldete sie dann als fehlend
    (Desktop-Test 2026-11-18). Verpasste Termine holt der gespeicherte Zustand (`PgBeatStore`) beim
    nächsten Start einmal nach, auch wenn der Rechner nachts aus ist. `beat::CATCH_UP_ON_FIRST_START`
    bleibt als (leere) Liste für künftige Fälle.
16. **`PAPERLESS_TASK_WORKERS=auto`** (opt-in since 2026-11-23; 2026-11-22 to 2026-11-23 it was the
    default for an unset variable): unset = 1 slot as upstream (`CELERY_WORKER_CONCURRENCY`), so a
    single document gets all cores (`THREADS_PER_WORKER` = cores). The value `auto`
    (case-insensitive; upstream `int("auto")` fails) sets the slot count to
    `pl_core::resources::default_task_workers`:
    `min(ceil(cores / t), 4, budget / (t × 100 MB))`, at least 1, with `t` = explicit
    `PAPERLESS_THREADS_PER_WORKER` or 2 and `budget` = 50 % of the memory available at start
    (4 cores → 2 slots; 3 cores with `THREADS_PER_WORKER=2` → 2; 12 cores → 4). The default for
    `THREADS_PER_WORKER` stays upstream's `max(cores // workers, 1)`, so slots × OCR threads ≤ cores.
    Cores/memory are measured once per process, so pl-queue (slots) and pl-ocr (threads) agree.
    An explicit number is used unchanged; other values fail with the ngx `int()` text. Why not the
    default: Raspberry Pi 5 (3 cores) – 255 documents 214 s (1 slot) vs. 162 s (auto), but one
    8-page scan alone 14.8 s vs. 24 s; single-document speed must not regress. The slots serve all
    tasks (mail, index, training), as with Celery concurrency > 1. The consumer is safe with
    several slots (pl-consumer Known difference 24).

17. **Start nur mit nachweisbarer Lease** (2026-11-23, TLA+-Befunde F1/F2, `verification/task-queue/`):
    Ein Job, dessen Lease vor dem Start nicht mehr nachweisbar ist (Pause zwischen Beanspruchen und
    Start ≥ 80 % der Lease) oder dessen `mark_started` meldet, dass eine andere Zustellung die Zeile
    hat, wird nicht ausgeführt und nichts geschrieben; der Aufräumer stellt ihn neu zu (höchstens
    einmal gleichzeitig, mindestens einmal insgesamt). Celery kennt keine Lease.

## Ergänzung 2026-11-18: Wartungsaufgaben (A1)

`maintenance::cleanup_expired_share_link_bundles(pool, SHARE_LINK_BUNDLE_DIR)` =
`documents.tasks.cleanup_expired_share_link_bundles` (upstream `tasks.py`): alle Bündel mit
`expiration IS NOT NULL AND expiration < now()`; je Bündel `remove_file()` (nur relative Pfade innerhalb
des Bündelordners, Fehler ignoriert), M2M-Zeilen und Bündel löschen (eigene Transaktion je Bündel,
Fehler → Warnung `Failed to delete expired share link bundle <id>: …`), am Ende
`Deleted <n> expired share link bundle(s)`. Registriert in pl-server `register_document_handlers`;
damit läuft auch der Beat-Eintrag (`SHARE_LINK_BUNDLE_CLEANUP_CRON`). DB-Test `#[ignore]` (braucht eine
beschreibbare paperless-DB, `PAPERRUST_TEST_DATABASE_URL`).

## Last-Befunde B2/B7 (2026-11-18)

- **B2:** Der Worker-Listener (`worker.rs`) und das Status-Relay (`events.rs`) lauschen über
  `pl_core::dbconn::listener` auf einer eigenen Verbindung, nicht mehr aus dem Pool der Queue.
  pl-server gibt dem Worker eine eigene `PgQueue` auf einem Nebenpool (`concurrency + 3`
  Verbindungen): Claim, Heartbeat/Lease, `complete`/`fail`/`retry` konkurrieren nicht mehr mit
  Web-Anfragen und Aufgaben-Inhalten um den Hauptpool (vorher: Heartbeat verhungert → Lease verloren →
  Aufgabe abgebrochen und erneut ausgeführt). Einreihen aus Web-Anfragen nutzt weiter den Hauptpool.
- **B7:** Die Einlese-Aufgabe vermerkt in ihrer Transaktion `headers.paperrust_document_id` an der
  eigenen Zeile von `paperrust_task_queue` (pl-consumer). Die Queue selbst wertet den Schlüssel nicht
  aus; Kopfzeilen werden bei Claim/Neuversuch unverändert weitergegeben.
