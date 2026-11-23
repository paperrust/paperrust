# Vertrag: pl-consumer (Steuerung des Einlesens, Python-freier Worker)

Quelle: paperless-ngx v3.4.2 (`upstream/`, unverändert), django-auditlog/watchfiles aus `reference/venv`.
Kein HTTP-Endpunkt. Ersetzt die Celery-Aufgabe `documents.tasks.consume_file` samt Plugins, die
Signal-Empfänger von `document_consumption_finished` und `manage.py document_consumer`.

| Rust | upstream |
|---|---|
| `consumer::Consumer::consume_file` / `consume` | `documents/tasks.py::consume_file` |
| `Consumer::{preflight, asn_check, collate, barcode, workflow_trigger, run_consumer}` | `consumer.py::ConsumerPreflightPlugin`, `AsnCheckPlugin`, `double_sided.py::CollatePlugin`, `barcodes.py::BarcodePlugin`, `consumer.py::WorkflowTriggerPlugin`, `ConsumerPlugin` |
| `Consumer::{store, apply_overrides, parse_title_placeholders, create_version}` | `ConsumerPlugin._store`, `apply_overrides`, `_parse_title_placeholders`, `_create_version_from_root` |
| `Consumer::consumption_finished` | `signals/handlers.py::add_inbox_tags`, `set_correspondent`, `set_document_type`, `set_tags`, `set_storage_path`, `run_workflows_added` (Reihenfolge aus `documents/apps.py`) |
| `Consumer::update_filename_and_move_files` | `signals/handlers.py::update_filename_and_move_files` (post_save) |
| `workflows::{run_consumption, run_added, apply_*}` | `handlers.py::run_workflows`, `workflows/utils.py::get_workflows_for_trigger`, `workflows/mutations.py`, `workflows/actions.py::execute_move_to_trash_action` |
| `data::{ConsumableDocument, DocumentMetadataOverrides, ConsumeOutcome}` | `data_models.py` |
| `audit` | django-auditlog `diff.py`/`receivers.py`/`models.py` + `models.py` (`auditlog.register(Document, m2m_fields={"tags"}, exclude_fields=["content_length","modified"])`) |
| `watcher` | `management/commands/document_consumer.py` |
| `task` | Aufgabe für pl-queue (`consume_file`-kwargs als JSON, pl-queue B5) |
| `stages` (Traits), `native` (Umsetzungen) | Parser-Registry, OCR, Datum, Klassifikator, Index, ProgressManager, Barcodes, E-Mail/Webhook |

## 1. Plugins und Reihenfolge (`tasks.consume_file`)

Neues Dokument: `ConsumerPreflightPlugin → AsnCheckPlugin → CollatePlugin → BarcodePlugin →
AsnCheckPlugin (erneut, nach Barcode-ASN) → WorkflowTriggerPlugin → ConsumerPlugin`.
Neue Version (`root_document_id` gesetzt): `ConsumerPreflightPlugin → ConsumerPlugin`.

- `able_to_run`: Collate nur mit `PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED` und Unterordner
  `PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME` (Standard `double-sided`) im Pfad; Barcode nur
  mit einer der Optionen ENABLE_BARCODES/ENABLE_ASN_BARCODE/ENABLE_TAG_BARCODE/STORE_BARCODE_VALUES
  **und** MIME `application/pdf` (bzw. `image/tiff` mit BARCODE_TIFF_SUPPORT) **und** verfügbarem
  Scanner (`BarcodeScanner::available`, d. h. PDFium geladen oder – E17 – `pdftoppm` im PATH);
  alle anderen immer.
- `BarcodeConfig` wird **je Aufgabe** gelesen (upstream: je Plugin-Instanz): Umgebung
  (`PAPERLESS_CONSUMER_*BARCODE*`, inkl. `BARCODE_DPI` 300, `BARCODE_UPSCALE` 0.0, `BARCODE_MAX_PAGES`
  0) überlagert von der ersten Zeile `paperless_applicationconfiguration` (`BarcodeAppConfig`,
  `BarcodeSettings::with_app_config`): Wahrheitswerte gelten, wenn nicht NULL; Text/Zahl/Zuordnung
  nur „truthy“ (`app_config.x or settings.X`: `""`, 0, 0.0, `{}` → Umgebung).
- Ergebnis: `{"document_id"}` (Erfolg), `{"reason"}` (`StopConsumeTaskError`: Doppelseiten,
  Barcode-Trennung, Workflow „In den Papierkorb“), `{"duplicate_of","duplicate_in_trash"}` (nur mit
  `CONSUMER_DELETE_DUPLICATES`). Jede andere Ausnahme: Statusmeldung `FAILED`, `str(e)`, 100/100,
  dann Fehler der Aufgabe (`TaskError` mit `error_type` = Python-Klassenname `ConsumerError`).
- Overrides werden nach jedem Plugin weitergereicht (`overrides = plugin.metadata`).

## 2. Statusmeldungen (`ProgressManager`, Kurztexte `ConsumerStatusShortMessage`)

Dateiname der Meldungen: `overrides.filename or original_file.name`; Rechte: `owner_id` (wenn
truthy), `users_can_view = view_users + change_users`, `groups_can_view = view_groups + change_groups`.

| Fortschritt | Status | Nachricht |
|---|---|---|
| 0/100 | STARTED | `new_file` |
| 20/100 | WORKING | `parsing_document` |
| 70/100 | WORKING | `generating_thumbnail` |
| 90/100 | WORKING | `parse_date` (nur ohne Datum vom Parser) |
| 95/100 | WORKING | `save_document` |
| 100/100 | SUCCESS | `finished` (+ `document_id`) |
| 100/100 | FAILED | Kurztext (`file_not_found`, `unsupported_type`, `asn_value_out_of_range`, `asn_already_exists[_in_trash]`, `document_already_exists[_in_trash]`, `pre/post_consume_script_[not_found|error]`) bzw. `str(e)` bei Parser-/Speicherfehlern; danach (außer Duplikat) ein zweites FAILED mit dem vollen Fehlertext |

## 3. Fehlertexte (`_fail` → `ConsumerError(f"{filename}: {log_message}")`)

- `Cannot consume {original_file}: File not found.`
- `Not consuming {filename}: Given ASN {asn} is out of range [0, 4,294,967,295]`
- `Not consuming {filename}: Given ASN {asn} already exists!` (+ ` Note: existing document is in the trash.`)
- Duplikat (Warnung immer): `Consuming duplicate {filename}: {n} existing document(s) share the same content.`
  (+ ` Note: at least one existing document is in the trash.`); nur mit `CONSUMER_DELETE_DUPLICATES`:
  Datei löschen, `Not consuming {filename}: It is a duplicate of {title|original_filename|name(filename)|pk} (#{pk})`
  (+ ` Note: existing document is in the trash.`). **Ohne** die Einstellung wird das Duplikat eingelesen (wie upstream).
  Prüfung: SHA-256 der Datei gegen `checksum` **oder** `archive_checksum`, inkl. Papierkorb, `-created`.
- `Unsupported mime type {mime}`
- `Error occurred while consuming document {filename}: {e}` (ParseError), `Unexpected error while consuming document {filename}: {e}`
- `The following error occurred while storing document {filename} after parsing: {e}`
- `Configured pre-consume script {p} does not exist.`, `Error while executing pre-consume script: {e}` (post entsprechend)
- Doppelseiten: `Unsupported file type for collation of double-sided scans`, Zu viele Seiten (Text in `collate::TOO_MANY_PAGES`),
  Stopp-Texte `Received odd numbered pages of double sided scan, waiting up to 30 minutes for even numbered pages` /
  `Success. Even numbered pages of double sided scan collated with odd pages`.
- Workflow-Papierkorb beim Einlesen: Stopp `Document deleted by workflow action during consumption`.

## 4. ConsumerPlugin.run (Ablauf)

1. Arbeitskopie in `SCRATCH_DIR/paperless-ngx*/{filename}` (mtime/atime kopiert), MIME per libmagic-Nachbau
   (pl-parsers). `.pdf` + MIME in `CONSUMER_PDF_RECOVERABLE_MIME_TYPES` (`application/octet-stream`):
   `qpdf --replace-input` über pl-pdfops `repair::replace_input` mit qpdf-Exit-Semantik:
   **0** → MIME neu bestimmen (`Detected mime type after qpdf: …`), unverändertes Original nach
   `…/uo/{filename}` kopieren (wird **gespeichert** und für die **Prüfsumme** verwendet, geparst wird die
   reparierte Arbeitskopie; nach dem Commit gelöscht); **3** (Querverweise neu aufgebaut, Datei
   trotzdem ersetzt) bzw. **2** (nicht lesbar, Datei unverändert) → `check_returncode()` →
   `Error attempting to clean PDF: Command '['qpdf', '--replace-input', PosixPath('…')]' returned
   non-zero exit status N.`, MIME wird **nicht** neu bestimmt (→ meist `Unsupported mime type`).
2. Parser: `get_parser_for_file(mime, filename, path, allow_remote = overrides.remote_ocr or remote_ocr_by_default)`.
3. Pre-Consume-Skript (`DOCUMENT_SOURCE_PATH`, `DOCUMENT_WORKING_PATH`, `TASK_ID`).
4. `should_produce_archive` (pl-ocr), `parse`, Vorschaubild (`thumbnail_with_text`, siehe §6), Text, Datum (Parser, sonst
   `RegexDateParser(filename, text)` erster Treffer), Archivpfad, Seitenzahl. Läuft in `spawn_blocking`.
5. `load_classifier()` (PaperRust-Modell `DATA_DIR/paperrust_classifier.model`, `PAPERRUST_CLASSIFIER_MODEL_FILE`).
6. **Eine Transaktion**: `_store` (bzw. Version), Dokument neu laden, Empfänger von
   `document_consumption_finished`, Dateien unter `MEDIA_LOCK` ablegen, `document.save()`,
   `update_filename_and_move_files`. Fehler → Rollback **und** Löschen aller in diesem Lauf geschriebenen
   Dateien.
7. Nach dem Commit: Original + Arbeitskopie + Schattendatei `._{name}` löschen, Suchindex
   (`add_or_update`, Version → Wurzel; Sperre belegt → `index_document` mit countdown 60 über die
   Warteschlange), Post-Consume-Skript (`DOCUMENT_ID`, `DOCUMENT_TYPE`, `DOCUMENT_CREATED`, …, `TASK_ID`),
   Statusmeldung SUCCESS.

### `_store`
- `created`: `overrides.created` (Datum) › Parser-/Textdatum (lokales Datum in `PAPERLESS_TIME_ZONE`) ›
  `mtime` der Originaldatei.
- `title`: `Path(overrides.filename).stem` bzw. `original_file.stem`; `overrides.title` als
  Workflow-Vorlage (`parse_w_workflow_placeholders` mit Korrespondent/Typ/Besitzer der Overrides,
  `localtime(now)`, Dateiname, Custom Fields); Fehler → Log, Titel bleibt; auf 127 Zeichen gekürzt.
- `checksum` = SHA-256 der Arbeitskopie, `original_filename` = filename, `modified` = jetzt (auto_now).
- `apply_overrides`: Korrespondent, Typ, Tags (je Tag `add_nested_tags` = mit Vorfahren),
  Speicherpfad, ASN, version_label, Besitzer, Rechte (`set_permissions_for_objects`, merge=False, über
  pl-authz), Custom Fields (`CustomFieldInstance` je Feld, Wert in `value_<typ>`), Barcodes; dann `save()`.
  Nicht vorhandene IDs → `"{Model} matching query does not exist."` (→ Speicherfehler, Rollback).

### Empfänger `document_consumption_finished` (Reihenfolge)
1. `add_inbox_tags`: alle `is_inbox_tag`-Tags, sichtbar für den Besitzer (`restrict_queryset_to_visible`).
2. `set_correspondent`/`set_document_type`/`set_tags`/`set_storage_path` (nur wenn leer; Tags immer):
   pl-matching `match_*` über Kandidaten `permitted_object_ids(owner, view_<model>)` bzw. alle ohne
   Besitzer, Ordnung `name`; erster Treffer; Klassifikator für `MATCH_AUTO`. `save(update_fields=(feld,))`.
3. `run_workflows_added` (Trigger „Document Added“): je Workflow (aktiv, Ordnung `order`) frisch laden,
   Papierkorb → Abbruch, `document_matches_workflow` (pl-matching), Aktionen in `(order, pk)`:
   Zuweisung (Tags auf frischer Instanz, Korrespondent/Typ/Pfad/Besitzer, Custom Fields
   anlegen/aktualisieren, Titel per Workflow-Vorlage, Rechte merge=True), Entfernen (Tags inkl.
   Nachfahren, `clear()`, Rechte, Custom Fields hart löschen), E-Mail/Webhook/Passwort → `WorkflowEffects::*_in`
   (in der Transaktion, `ActionInput` mit der Dokument-Instanz im Speicher, siehe §7)
   (Standard: nur Log), Papierkorb → `document.delete()` (soft). Danach Titel[:128],
   `save(update_fields=[title, correspondent, document_type, storage_path, owner, modified])`, `WorkflowRun`.
4. `add_to_index` → nach dem Commit (siehe Known differences).

### Workflow-Trigger „Consumption Started“ (`WorkflowTriggerPlugin`)
`run_workflows(CONSUMPTION, input_doc, overrides=DocumentMetadataOverrides())`: Zuweisung/Entfernen auf
Overrides (`apply_*_to_overrides`, Tags mit Vorfahren, Titel als Vorlage), Remote-OCR-Aktion →
`overrides.remote_ocr`, KI-Aktion ignoriert, `WorkflowRun(document=None)`; Ergebnis per
`DocumentMetadataOverrides.update` in die Overrides. Meldung = `"\n".join("Applying WorkflowAction {id} from Workflow: {name}")`.
E-Mail/Webhook/Passwort → `WorkflowEffects::*_in` mit `ActionInput.overrides` = Stand der Overrides
**zum Zeitpunkt der Aktion** (nach vorherigen Zuweisungen desselben Laufs) und `mime_type` des
`ConsumableDocument`.

### BarcodePlugin (`barcodes.py`)
Arbeitsordner `{tmp}/barcode*`; TIFF → `convert_from_tiff_to_pdf` (pl-pdfops, Fehler brechen die
Aufgabe ab wie upstream); `detect` (`scan_pdf` mit dpi/upscale/max_pages; Passwort →
`File is likely password protected, not checking for barcodes: …`, sonst
`Exception during barcode scanning: …`, beides = keine Barcodes); Trennseiten (`get_separation_pages`,
nur mit ENABLE_BARCODES); Tags (wenn nicht aufgeteilt wird oder `original_path` gesetzt ist;
`get_or_create(name__iexact)`); bei Trennseiten: `separate_pages` (pl-pdfops/lopdf, Namen
`{original_file.stem}_document_{i}.pdf`, Dateizeiten vom Original) → Kopie mit Dateizeiten nach
`SCRATCH_DIR/paperless-barcode-split-*` → je Teil `consume_file` (gleiche Quelle/Mailregel,
`original_path` = Original, Overrides weitergereicht, `trigger_source` je Quelle), Original löschen,
SUCCESS `Barcode splitting complete!`, Stopp. Sonst ASN (erster Code mit Präfix, `skip_asn_if_exists`)
und gespeicherte Barcode-Werte (`DocumentBarcode`, Seite 1-basiert).

### Dateiablage
`generate_unique_filename` (pl-templating, `PAPERLESS_FILENAME_FORMAT` bzw. Speicherpfad-Vorlage;
Kollision → `_01`, `_02` …; > 1024 Zeichen → Standardname `{pk:07}{ext}`), `validate_path_in_root`,
Unterordner anlegen; Original → `ORIGINALS_DIR`, Vorschaubild → `THUMBNAIL_DIR/{pk:07}.webp`, Archiv →
`ARCHIVE_DIR` (zuerst `{stem des Originals}.pdf`), `archive_checksum`. Schreiben **atomar**
(`.{name}.{uuid}.part` + `rename`), mtime/atime übernommen.

### Audit-Log (`PAPERLESS_AUDIT_LOG_ENABLED`, Standard an)
Einträge wie django-auditlog in derselben Reihenfolge wie upstream: `create` (alle Felder ohne
`content_length`/`modified`, Rückwärtsbeziehungen als `documents.X.None`, None-DateTimes/FKs als
`["None","None"]`), m2m `tags add` je `add_nested_tags` mit tatsächlich neuen Tags (Namen nach `name`),
`CustomFieldInstance`-create, `update` je `save()` mit Unterschied DB → Instanz (inkl. der
upstream-Eigenheit, dass `created` als `str(datetime)` gegen das gespeicherte Datum verglichen wird,
solange die Instanz das `datetime` hält), `object_repr = str(document)` (`created.isoformat()` +
Korrespondent + Titel). Version: `create` + `{"Version Added": ["None", id]}` mit
`additional_data {"reason": "Version added", "version_id"}`. `actor` = None (Celery-Kontext).

## 5. Eingangsordner (`watcher`)

`ConsumerFilter`: ignorierte Verzeichnisnamen in **jedem** Pfadteil (`.stfolder`, `.stversions`,
`.localized`, `@eaDir`, `.Spotlight-V100`, `.Trashes`, `__MACOSX` + `PAPERLESS_CONSUMER_IGNORE_DIRS`),
Muster per `re.search` auf den Dateinamen (`^\.DS_Store$`, `^\.DS_STORE$`, `^\._.*`, `^desktop\.ini$`,
`^Thumbs\.db$` + `PAPERLESS_CONSUMER_IGNORE_PATTERNS`), nur unterstützte Endungen
(`get_supported_file_extensions`). `FileStabilityTracker` (Stabilität = keine Ereignisse für
`PAPERLESS_CONSUMER_STABILITY_DELAY` s und gleiche mtime/Größe; leere Dateien verworfen), Vorhandenes
beim Start, `PAPERLESS_CONSUMER_RECURSIVE`, `PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS` (Tag je Ordnerteil,
`get_or_create(name__iexact)`), Sicherheits-Rescan alle 300 s, `queued`-Menge gegen Doppel-Einreihen.
Übergabe über `ConsumeSink`: `QueueSink` (`consume_file` mit `trigger_source=folder_consume`) oder
`DirectSink` (im Prozess). Fehler: `Consumption directory does not exist: {d}`,
`Consumption path is not a directory: {d}`.

Beobachtung (E16, `_watch_directory`): `PAPERLESS_CONSUMER_POLLING_INTERVAL` > 0 → Verzeichnis-
Schnappschüsse im Abstand (watchfiles `force_polling`, Log „Watching {d} using polling (interval:
{n}s)“); 0 → native Dateisystem-Ereignisse über die Crate `notify` (dieselbe, auf der watchfiles
aufbaut; inotify/ReadDirectoryChangesW/kqueue, Log „Watching {d} using native file system events“).
Wartezeit wie upstream: bei offenen Dateien `STABILITY_DELAY`, sonst bis zum nächsten Rescan
(`cap_for_rescan`, 300 s); nach dem ersten Ereignis 50 ms Sammeln (watchfiles `step`). Ereignisse:
Anlegen → added, Löschen → deleted, Umbenennen von/nach → deleted/added, sonstige Änderung →
modified, Zugriff → nichts. Verlorene Ereignisse (Überlauf, Fehler) lösen sofort einen Rescan aus.
Lassen sich keine Ereignisse einrichten (z. B. inotify-Grenze), Warnung und Rückfall auf 1-s-Polling.

Dienst-Schalter (E19, upstream `docker/rootfs/etc/s6-overlay/s6-rc.d/svc-consumer/run`):
`FolderWatcher::run` (Worker) prüft `PAPERLESS_CONSUMER_DISABLE` (jeder nicht leere Wert, auch
`false`, wie `[[ -n … ]]`) und endet dann sofort mit „Consumer is disabled, exiting“.
`FolderWatcher::run_command` (Befehl `document_consumer`) prüft das wie upstream nicht.

## 6. Bausteine (Traits in `stages`)

| Trait | Standard (`native`) | Baustein |
|---|---|---|
| `DocumentParsing` | `NativeParsing` | pl-parsers Registry + pl-ocr `RasterisedParser`/`NativeOcrmypdf` (Tesseract-CLI, PDFium); Vorschaubild PDFium, sonst `documents/resources/document.webp` |
| `DateDetector` | `RegexDates` | pl-matching `RegexDateParser` (Einstellungen wie `get_date_parser`) |
| `ClassifierSource` | `FileClassifier` | pl-classifier `load_classifier` + `CLASSIFIER_MATCH_THRESHOLD` |
| `SearchIndex` | `TantivyIndex` | pl-indexer `Indexer::add_or_update` |
| `ProgressSink` | `EventBusProgress` | pl-queue `EventBus`/`ProgressManager` |
| `BarcodeScanner` | `PdfOpsBarcodes` (mit PDFium; ohne PDFium mit `consumer::barcode_raster::PdftoppmRenderer`, E17), sonst `NoBarcodes` | pl-pdfops `barcodes::scan_pdf` (PDFium-Raster + rxing), `separate_pages` (lopdf) |
| `PdfPageOps` | `LopdfPages` | Doppelseiten mit lopdf; TIFF→PDF über pl-pdfops `convert::convert_from_tiff_to_pdf` |
| `WorkflowEffects` | `LoggingEffects` (pl-server: pl-mail `SharedEffects`) | E-Mail/Webhook/Passwort; Standard nur protokolliert |

Vorschaubild (`DocumentParsing::thumbnail_with_text`, `NativeParsing`): Text-Dateien
(`TextDocumentParser`) → pl-pdfops `make_text_thumbnail` (500×700, Schrift
`PAPERLESS_THUMBNAIL_FONT_NAME`); Mail/Tika/HTML ohne PDF-Fassung → `text_thumbnail_webp` aus dem
extrahierten Text; Bilder/PDFs → PDFium (pl-ocr); fehlt Schrift/PDFium → `document.webp`.
Scheitert PDFium an einem PDF (upstream `make_thumbnail_from_pdf_qpdf_fallback`, Lücke E15): Warnung
`Thumbnail generation with PDFium failed, attempting repair and retry.`, Reparatur mit lopdf
(pl-pdfops `repair_bytes`, qpdf-Ersatz) nach `repaired.pdf` und neuer Versuch, danach PdfCraft
(`Op::Repair` im Kindprozess, falls installiert); erst dann `Unable to make thumbnail after repair: …`
und `document.webp`. HEIC (`image/heic`, Lücke E8): vor OCR und Bild-Vorschaubild über
`office::heic_to_png_blocking` (`Op::Heic`) nach `heic-decoded.png`; OCR/Archiv laufen dann mit
`image/png`. Mail (Lücke E12): `NativeParsing.pool` (gesetzt in `Consumer::from_env`) liest
`paperless_mail_mailrule.pdf_layout` der Regel, sonst `PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`
(Standard 1), und gibt es dem Mail-Parser als `ParseContext::mail_pdf_layout`.

## 7. Workflow-Aktionen mit Außenwirkung (`WorkflowEffects`)

```rust
pub struct ActionInput { ctx: ActionContext, overrides: Option<DocumentMetadataOverrides>,
                         document: Option<DocRow>, mime_type: String }
trait WorkflowEffects {
    fn email(&self, &ActionContext); fn webhook(..); fn password_removal(..);          // synchron (pl-documents-write)
    fn email_in<'a>(&'a self, &'a mut PgConnection, &'a ActionInput) -> BoxFuture<'a, ()>; // Consumer
    fn webhook_in(..); fn password_removal_in(..);  // Standard: ruft die synchrone Methode
}
```
Der Consumer ruft `*_in` mit der Verbindung der laufenden Einlese-Transaktion bzw. (Einlese-Trigger)
einer Pool-Verbindung auf – synchron im Ablauf wie upstream, das noch nicht festgeschriebene
Dokument ist sichtbar. `build_workflow_action_context(document, overrides)`:
`overrides = Some` (Einlese-Trigger) → Titel `overrides.title or str(original_file)`, Korrespondent/
Typ/Besitzer der Overrides (`filter(pk=…).first()`), `created` der Overrides, Custom Fields aus den
Overrides (`templating::override_custom_fields`), `id = ""` (pl-templating `WorkflowDocId::Empty`);
`document = Some` („Dokument hinzugefügt“) → Instanz im Speicher (inkl. noch nicht gespeicherter
Zuweisungen dieses Workflows), Custom Fields aus der DB. `ActionContext` selbst bleibt feldgleich
(pl-documents-write baut es direkt).
| `ScriptRunner` | `CommandScripts` | `run_subprocess` |

Warteschlange: `task::handler(Arc<Consumer>)` für `pl_queue::Handlers::register("documents.tasks.consume_file", …)`;
`task::enqueue_consume` reiht mit `trigger_source` je Quelle ein.

## 8. Worker-Aufgaben auf bestehenden Dokumenten (W15)

Alle als Methoden von `Consumer` (gleiche Bausteine wie das Einlesen), Registrierung im Worker
(pl-server) per CORE-REQUEST W15.

### `update_document_content_maybe_archive_file(document_id, *, remote_ocr=False)` (`reprocess.rs`)

upstream: `documents/tasks.py@v3.4.2`. `Consumer::update_document_content_maybe_archive_file(id, remote_ocr)
-> Result<ReprocessOutcome, ReprocessError>`.
1. `Document.objects.get(id=…)` (ohne Papierkorb) – fehlt → `ReprocessError::DoesNotExist`
   (`DoesNotExist: Document matching query does not exist.`, Aufgabe FAILURE wie upstream).
2. Parser `parser_for(mime, original_filename or "", source_path, allow_remote=remote_ocr or
   remote_ocr_by_default)`; keiner → `ERROR No parser found for mime type …, cannot archive document
   <str> (ID: n)`, Ergebnis `NoParser` (Aufgabe erfolgreich).
3. Im try-Block (jede Ausnahme → `ERROR Error while parsing document <str> (ID: n)`, Ergebnis
   `Failed`, Aufgabe erfolgreich, DB zurückgerollt): `should_produce_archive`, `parse(source_path)`,
   Barcodes neu lesen (`_read_barcodes_for_reprocess`: nur mit `CONSUMER_STORE_BARCODE_VALUES`
   (Umgebung + ApplicationConfiguration), nur PDF bzw. TIFF mit TIFF-Unterstützung, Fehler →
   Warnung `Could not read barcodes of document …`, Barcodes bleiben), Vorschaubild.
4. Eine Transaktion: Archiv vorhanden → `archive_checksum` (SHA-256), `archive_filename =
   generate_unique_filename(document, archive_filename=True)` (Instanz vom Anfang), `UPDATE` von
   `archive_checksum, content, archive_filename` (kein `save()` → keine Umbenennung, `modified`
   unverändert); sonst nur `content`. Audit (`log_create`, Aktion UPDATE, `object_repr = str(alt)`,
   `changes = {"content": [alt, neu][, "archive_checksum": …, "archive_filename": …]}`,
   `additional_data = {"reason": "Update document content"}`). Barcodes: alle löschen, neu anlegen,
   `modified = now`. Unter `MEDIA_LOCK`: Archiv nach `archive_path`, Vorschaubild nach
   `thumbnail_path` verschieben. `page_count`, `checksum`, Titel bleiben.
5. Danach: `INFO Updating index for document n (<archive_checksum>)`, Suchindex
   (`SearchIndex::add_or_update`), `clear_document_caches(id)` und ggf. der Wurzel. Der KI-Index
   (`llm_index_add_or_update_document`) gehört dem Worker (pl-ai).

### `check_scheduled_workflows()` (`scheduled.rs`)

upstream: `documents/tasks.py@v3.4.2`, `matching.prefilter_documents_by_workflowtrigger`.
`Consumer::check_scheduled_workflows()` (bzw. `_at(now)`), Autocommit wie die Celery-Aufgabe.
- Aktive Workflows mit Trigger Typ 4 (`get_workflows_for_trigger`, Ordnung `order, id`); je Trigger
  Typ 4: `threshold = now - schedule_offset_days`; Kandidaten (`root_document IS NULL`, ohne Papierkorb,
  Ordnung `-created`): `added`/`modified <= threshold`; `created <= threshold` als **Datum in
  `TIME_ZONE`** (Djangos `DateField.to_python`); `custom_field`: `CustomFieldInstance` (ohne Papierkorb)
  des Feldes mit `value_date` zwischen `now-365 Tage` und `threshold` (beide als Ortsdatum), dann
  `make_aware(combine(value_date, 00:00)) + offset <= now`; Offset < −365 → Warnung
  `Trigger n has large negative offset …`. Unbekanntes Datumsfeld → keine Dokumente.
- Vorfilter je Dokument: `pl_matching::workflow::prefilter_matches` (Tags any/all/not,
  Korrespondent/Typ/Speicherpfad any/eins/not, Dateiname per fnmatch, Custom-Field-Abfrage).
- Je Dokument: letzter `WorkflowRun` (Typ 4, Workflow, ohne Papierkorb, `-run_at`); nicht wiederholend
  und vorhanden → überspringen; wiederholend und `run_at > now - schedule_recurring_interval_days`
  → überspringen. Sonst `run_workflows(SCHEDULED, document, workflow_to_run=workflow)` (unten),
  danach `send_websocket_document_updated`.

### Signal-Teil von `bulk_update_documents(document_ids)` (`signals.rs`)

`Consumer::bulk_update_signals(&ids)`: für jedes Dokument aus `Document.objects.filter(id__in=ids)`
(ohne Papierkorb, `-created`): `clear_document_caches`, `document_updated` (Empfänger in der Reihenfolge
von `apps.py`: `run_workflows_updated`, `send_websocket_document_updated`; `add_or_update_document_in_llm_index`
entfällt wegen `skip_ai_index=True`), `post_save` → `update_filename_and_move_files`. Index
(`add_or_update_ids`) und `update_llm_index` bleiben im Worker (wie bisher). Einzeln nutzbar:
`Consumer::document_updated(conn, id)`, `Consumer::send_websocket_document_updated(conn, id)`.

**Gebündelter Weg (Standard seit PERF-SCALE §10, `bulk.rs`)**: `Consumer::bulk_update_signals` arbeitet
in Blöcken à 500 (Workflows „Dokument geändert“ einmal je Aufgabe; Dokumente, Namen, Tags, neueste
Versionen, Custom Fields je Block in einer Abfrage; Workflow-Prüfung und Dateiname im Speicher). Ohne
Workflow-Treffer und mit unverändertem Dateinamen bleibt nur `modified = now()`, gesammelt je Block.
Mit Treffer und nur „einfachen“ Aktionen (Tags, Korrespondent, Typ, Speicherpfad, Besitzer zuweisen/
entfernen) wird die Dokumentzeile im Speicher fortgeschrieben und einmal geschrieben; Tags, Audit-
Einträge und Workflow-Läufe entstehen wie im Einzelweg sofort und in derselben Reihenfolge. Jede
andere Aktion (Titel, Rechte, Custom Fields, Mail, Webhook, Papierkorb, KI), ein im Speicher nicht
entscheidbarer Dateiname und Versionen → ab dort exakter Einzelweg. Ändert sich der Dateiname mitten
in der Kette, wird nicht nach jedem Workflow verschoben, sondern am Ende **einmal** an den Endort
(PERF-SCALE §11; gleicher Endname, weil die eigene Datei in `generate_unique_filename` nie als
Kollision zählt und Workflow-Filter nur `original_filename` prüfen). Einzelweg: `Consumer::bulk_update_signals_single`, im
Worker per `PAPERRUST_BULK_UPDATE=single`. Gleichheit: `tests/bulk_equal.rs` (DB, Audit,
Workflow-Läufe, Aufgaben, Dateipfade und -inhalte identisch).

**Gleichzeitigkeit (2026-11-18, tests/stress/BEFUNDE.md B4/B6):** Das gesammelte `modified`-Update je
Block sperrt die Zeilen in id-Reihenfolge in einer Anweisung (`db::touch_modified_sorted`:
`UPDATE … FROM (SELECT … ORDER BY id FOR NO KEY UPDATE)`), wie der Sync-Trigger `paperrust_sync_bump`;
bei Verklemmung/Serialisierungsfehler (`40P01`/`40001`) bis zu 3 Versuche (Autocommit, nur diese
Anweisung). Steht ein Dokument mehrfach in `document_ids` (zusammengefasste Aufgaben,
pl-documents-write `coalesce`), läuft der Nachlauf bei vorhandenen Workflows „Dokument geändert“ in
`k` Runden (`bulk::rounds`): Workflows, WebSocket und Dateiname je Dokument so oft wie bei `k`
einzelnen Aufgaben; ohne solche Workflows eine Runde. Der Einzelweg (`PAPERRUST_BULK_UPDATE=single`,
`signals.rs`) entfernt Doppelte weiterhin (eine Runde).

**Gemeinsame Bausteine (B3):** `db::insert_document_tags(conn, pairs, ignore_conflicts)` ist die einzige
Stelle, die den `documents_document_tags`-INSERT baut (Einlesen, Workflows, bulk_edit); ids immer als
`i32` (Spaltentyp `integer`), damit derselbe SQL-Text nie mit anderen Parametertypen im
Statement-Cache einer Verbindung liegt. `db::lock_documents_sorted`, `db::is_retryable`,
`db::retry_backoff`, `db::MAX_ATTEMPTS` für die Schreibwege in pl-documents-write.

### `run_workflows` auf bestehenden Dokumenten (`workflows::run_workflows`)

Gemeinsam für „Dokument hinzugefügt“ (`run_added`), „Dokument geändert“ und „geplant“:
Versionen übersprungen; je Workflow `refresh_from_db` (fehlt → Abbruch, Papierkorb → Abbruch),
`document_matches_workflow`, Aktionen in `order, pk` (Zuweisung, Entfernen, E-Mail/Webhook/Passwort
über `WorkflowEffects::*_in` mit `trigger_type` des Laufs, Remote-OCR ignoriert,
**`APPLY_AI_SUGGESTIONS` → `RunOutcome.ai_suggestions`**), `title[:128]`, `save(update_fields=[title,
correspondent, document_type, storage_path, owner, modified])`, **`post_save` →
`files::update_filename_and_move_files`** (nur mit `RunEnv.settings`, also nicht beim Einlesen, wo
`filename` noch leer ist), `WorkflowRun` anlegen, „In den Papierkorb“ → `DocumentDeleter::soft_delete`.
`original_file` = Aufrufer (Einlesen) bzw. `source_path`.
`apply_ai_suggestions.delay_on_commit(action_id=, document_id=)`: der Aufrufer reiht nach dem Commit
über `workflows::enqueue_ai_suggestions(queue, &outcome)` ein (`documents.tasks.apply_ai_suggestions`,
kwargs `{"action_id", "document_id"}`); der Consumer tut das nach dem Commit des Einlesens, die
Worker-Aufgaben sofort (Autocommit).

### Dateiumbenennung (`files.rs`)

`files::update_filename_and_move_files(conn, &ConsumerSettings, doc_id)` = upstream-Empfänger
vollständig (wie pl-documents-write `files.rs`, hier ohne dessen Abhängigkeit): Original + Archiv,
Kollision `_01`, „schon verschoben“ per Prüfsumme, Längengrenze 1024, `validate_move` (Wurzel,
Quelle fehlt, Ziel existiert), Rollback, leere Ordner löschen, Versionen der Wurzel nachziehen.
`Consumer::update_filename_and_move_files` (Einlese-Fall mit Aufräumliste) ist jetzt `pub`.

### Bausteine (neu in `Consumer`)

| Feld | Standard | Worker (CORE-REQUEST) |
|---|---|---|
| `events: Option<EventBus>` | `from_env`: der übergebene Bus | – |
| `caches: Arc<dyn signals::DocumentCaches>` | `NoCaches` | pl-server: `signals::document_cache_keys("", id)` in Djangos `default`-Cache löschen (`pl_core::cache`, lokal oder `PAPERRUST_CACHE=postgres`; seit 2026-11-14 kein Redis) + pl-ai-Generation |
| `deleter: Arc<dyn workflows::DocumentDeleter>` | `NativeSoftDelete` | `pl_documents_write::softdelete::soft_delete_document` |

## Known differences

- **Gebündelter Nachlauf `bulk_update_documents`** (PERF-SCALE §10): Ergebnis gleich, aber
  `modified`-Zeitstempel unterscheiden sich um Millisekunden (gesammelt am Blockende bzw. am Ende
  der Workflow-Kette statt je Schritt); Cache-Schlüssel werden je Block vor statt je Dokument
  gelöscht; Zwischenstände der Dokumentzeile während einer Workflow-Kette sind für andere
  Verbindungen nicht sichtbar (nur der Endstand); Fehler-/Info-Logzeilen können bei Vorlagenfehlern
  doppelt erscheinen (Schnellprüfung + Einzelweg). Tags/Custom Fields werden für den
  Vorlagenkontext nur geladen, wenn die Vorlage `tag`, `custom_field` oder `document` enthält.
  Bei Dateinamenwechsel mitten in der Workflow-Kette nur eine Verschiebung an den Endort (§11):
  leere Zwischenordner, die upstream beim Wegverschieben aus einem *vorher schon leeren* Ordner
  entfernen würde, bleiben stehen; Warnungen zu Zwischenständen (z. B. zu langer Name nur im
  Zwischenschritt) entfallen.
- **Workflows laden** (`workflows_for_trigger`, PERF-SCALE §11): Trigger, Aktionen und M2M-Felder
  aller Workflows gebündelt (≈ 30 Abfragen statt ≈ 30 je Workflow). Gleiche Inhalte; benannte
  M2M-Listen nach Name und bei Gleichstand nach id (vorher ohne Tie-Breaker).

1. **Suchindex nach dem Commit**: upstream schreibt den Index im Signal innerhalb der Transaktion
   (bei späterem Rollback stünde ein Phantom im Index); pl-indexer liest das Dokument über den Pool und
   braucht die festgeschriebenen Daten. Gleiches Ergebnis im Erfolgsfall.
2. **Original löschen nach dem Commit** (upstream: noch in der Transaktion, vor dem Commit). Bei einem
   Commit-Fehler bleibt die Datei hier im Eingang (upstream: weg, Dokument nicht gespeichert).
3. **Fehler beim Speichern räumt Dateien auf**: upstream lässt bereits kopierte Original-/Archiv-/
   Vorschaudateien nach einem Rollback liegen; hier werden sie gelöscht. Dateien werden atomar
   (temp + rename) statt direkt geschrieben.
4. **Barcodes** über pl-pdfops: Rastern mit PDFium statt pdf2image/poppler, Lesen mit rxing statt
   zxing-cpp (pl-pdfops Known differences 1–2: Grenzfälle verzerrter Scans können abweichen),
   Teil-PDFs mit lopdf statt pikepdf (gleiche Seiten/Inhalte, **andere Bytes → andere Prüfsummen**).
   Ohne PDFium rastert `pdftoppm` (poppler, wie upstream pdf2image: `-r <BARCODE_DPI> -f n -l n
   -gray`, PGM; E17, `src/consumer/barcode_raster.rs`); fehlt auch das, läuft das Plugin nicht
   (upstream ohne poppler: Warnung, keine Barcodes). Der Scan läuft in `spawn_blocking`.
5. **qpdf-Ersatz** (pl-pdfops `repair`): nur „Querverweise neu aufgebaut“ gilt als Warnung (Exit 3);
   andere qpdf-Warnungen gelten als sauber (Exit 0) und führen hier zur MIME-Neubestimmung, wo upstream
   abbräche (pl-pdfops Known difference 8). Fehlertext mit `PosixPath`/`WindowsPath` je Plattform.
6. **Vorschaubild für Office/Mail/HTML**: upstream rendert die erste Seite des Gotenberg-PDFs; hier
   für Word/ODT/RTF/DOC, Tabellen, Präsentationen und Mails ein WordCraft-Archiv-PDF und dessen erste
   Seite (`PAPERRUST_OFFICE_TO_PDF`, docs/contract/pl-pdftools.md §8.1); für HTML-Dokumente,
   abgeschaltete oder gescheiterte Umwandlung ein Text-Vorschaubild aus dem extrahierten Text (kein
   Layout/keine Bilder). Text:
   wie upstream gezeichnet (pl-pdfops Known difference 7: WebP verlustfrei, ab_glyph statt FreeType).
   **Fehlt die Schrift** (`PAPERLESS_THUMBNAIL_FONT_NAME`, Standard Liberation unter `/usr/share`),
   nimmt PaperRust `document.webp`; upstream scheitert dann beim Einlesen (`OSError` aus
   `ImageFont.truetype`).
7. **E-Mail-/Webhook-/Passwort-/KI-Aktionen**: Standard (`LoggingEffects`) nur protokolliert; mit
   pl-mail (`SharedEffects`, pl-server) E-Mail und Webhook mit vollem Kontext in der Transaktion, das
   eigentliche Senden (SMTP; Webhook ohne Warteschlange) läuft im Hintergrund weiter (upstream:
   synchron im Signal). KI-Vorschläge werden seit W15 als `apply_ai_suggestions` eingereiht.
   **Passwort entfernen** (W1, pl-documents-write `workflow_password`, verdrahtet in pl-server):
   bei einem bestehenden Dokument im Hintergrund nach dem Festschreiben; beim Einlese-Auslöser meldet
   sich die Aktion mit der Eingangsdatei an und läuft, wenn der Consumer nach dem Commit
   `WorkflowEffects::consumption_finished(doc_id, original_file)` für genau diese Datei meldet
   (upstream verbindet sich mit dem *nächsten* `document_consumption_finished`, egal für welches
   Dokument, und läuft noch in der Einlese-Transaktion mit der Arbeitskopie als Quelle; hier
   danach mit der abgelegten Originaldatei). Wird das Einlesen nicht fertig, verfällt die
   Anmeldung nach 24 h.
   **Trigger „Document Updated“ für die Wurzel einer neuen Version** (W2): `document_updated(root)`
   (Workflows, WebSocket) und `post_save` der Wurzel laufen in `store_all` direkt vor dem Commit,
   also in der Einlese-Transaktion wie upstream; Fehler brechen das Einlesen ab.
8. **Eingangsordner**: native Ereignisse über `notify` direkt statt über watchfiles; upstream baut
   den Beobachter nach jedem Stapel neu auf (daher sein Rescan), hier läuft er durch (Rescan bleibt als
   Sicherheitsnetz). Entprellung: 50 ms Sammeln statt watchfiles' 1600 ms `debounce` (die Stabilitäts-
   prüfung entscheidet ohnehin). Ohne Ereignisse Rückfall auf 1-s-Polling (upstream: Fehler).
   `PAPERLESS_CONSUMER_DISABLE` wirkt beim Start des Beobachters im Worker (gelesen über
   `pl_core::config::var`, also auch aus der Oberfläche); eine Änderung in der Oberfläche startet den
   Beobachter neu (Schlüssel in pl-server `WATCHER_KEYS`).
   Muster sind Python-`re`, übersetzt über pl-matching `regex_compat`.
9. **Unterordner-Tags / Barcode-Tags**: neue Tags ohne Neuberechnung der `tn_*`-Spalten aller Tags
   (django-treenode) und ohne Audit-Eintrag für den Tag.
10. **`update_filename_and_move_files` beim Einlesen** ist auf den Einlese-Fall reduziert (Dateiname aus
    der finalen Vorlage; abweichend → verschieben); die Version-Synchronisation der Wurzel entfällt. Die
    Worker-Aufgaben (§ 8) nutzen die vollständige Fassung `files::update_filename_and_move_files`.
11. **Audit-Log**: Reihenfolge und Inhalt wie upstream nachgebaut; `timestamp` und Zeitwerte (`added`)
    unterscheiden sich naturgemäß. `cid`/`remote_addr` immer NULL.
12. **Mengen-Reihenfolge**: upstream `list(set(...))` beim Zusammenführen von Tag-/Rechte-Listen; hier
    aufsteigend sortiert (gespeicherte Daten gleich).
13. **`DOCUMENT_FILE_NAME`** im Post-Consume-Skript: `sanitize_filename(title + ext)` statt der vollen
    `get_public_filename`-Kürzungslogik.
14. **Pfadtrenner**: die Referenz läuft unter Windows, dort speichert upstream `filename` mit `\`
    (`str(Path)`); Docker/Linux und Rust speichern `/`. `compare.py` normalisiert das.
15. **Titel-Vorlage mit Syntaxfehler**: liefert `parse_w_workflow_placeholders` `None`
    (Syntaxfehler), scheitert upstream an `None[:127]`; hier bleibt der Dateiname als Titel.

16. **Neu-Einlesen**: `content = get_text() or ""` (upstream: `None` → `IntegrityError`, protokolliert);
    Suchindex gesperrt → Warnung statt verschobenem `index_document`. Kein Barcode-Scanner (PDFium
    fehlt) → Warnung, Barcodes bleiben (upstream liest sie).
17. **Geplante Workflows**: Kandidaten-Ordnung bei gleichem `created` nach `id` (upstream ohne
    Tie-Breaker, Postgres-Reihenfolge); Vorfilter in Rust statt SQL (gleiche Menge), die Kandidaten
    werden aber gebündelt geladen (zwei Abfragen je 1000 Dokumente statt zwei je Dokument,
    Sicherheitsprüfung final K-N2). `filter_custom_field_query` wird seit K-N1 wie upstream
    ausgewertet (`pl_docfilter::custom_field_query_matching_ids` über dieselbe Verbindung, auch im
    Vorfilter und für „Dokument hinzugefügt/geändert“); ungültige Abfrage → kein Treffer, unerwartete
    Parserfehler → Fehlerlog und kein Treffer (upstream: Ausnahme). Lücken-Ortszeit
    (Sommerzeit) bei `custom_field`: Offset vor der Lücke wie zoneinfo `fold=0`.
18. **„In den Papierkorb“ mit `NativeSoftDelete`** (ohne Worker-Adapter): Kaskade (Versionen mit
    gleicher `transaction_id`, Notizen, Freigabelinks, Feldwerte, WorkflowRuns, Barcodes hart) ohne
    django-auditlog-Einträge für die mitgelöschten Notizen/Feldwerte; der Worker setzt die
    pl-documents-write-Fassung mit Audit ein (CORE-REQUEST W15).
19. **`clear_document_caches`** nur über den eingesetzten `DocumentCaches` (Standard: nichts); ohne
    Worker-Adapter bleiben Django-Cache-Einträge (Vorschläge/Metadaten/Vorschaubild-Zeitstempel) bis
    zum Ablauf stehen.
20. **Zuweisung von Custom Fields an bestehende Instanzen** in `run_workflows` per `UPDATE` mit
    Audit-Eintrag der `CustomFieldInstance` wie upstream `instance.save()` (W7: `log_update` mit
    `{"<value_field>": [alt, neu]}`, Darstellung `"<Feld> : <Wert>"`, kein Eintrag ohne Änderung).
    Abweichung: nur das Wertfeld im Diff (bei Geldbeträgen nicht das erzeugte
    `value_monetary_amount`), keine `post_save`-Umbenennung für die Instanz.
21. **Upload-Spool aufräumen** (D-102, 2026-11-16, bewusst): Liegt die Eingabe in
    `SCRATCH_DIR/tmp…/` (Upload über API/Oberfläche) bzw. `SCRATCH_DIR/paperless-mail-…/` (Mail-Anhang), löscht
    `Consumer::consume_file` nach jedem Ende (Erfolg, Fehlschlag, Duplikat, Abbruch) die Datei und das
    dann leere Verzeichnis (`cleanup_upload_spool`). upstream löscht nur nach Erfolg das Original und
    lässt das `mkdtemp`-Verzeichnis (bei Fehlschlag auch die Datei) im Scratch-Ordner liegen. Ein
    Fehlschlag lässt sich daher nur durch erneutes Hochladen wiederholen (wie upstream, das keinen
    Wiederholen-Knopf hat). Dateien aus dem Eingangsordner oder anderswo bleiben unberührt; fremde Dateien
    im Spool-Verzeichnis verhindern das Löschen des Verzeichnisses. Alte leere `tmp…`-Ordner früherer
    Läufe werden nicht nachträglich entfernt.
22. **Umbenennen nach dem Einlesen nur über `safe_join`** (Sicherheitsprüfung 2026-11-19 F9):
    `update_filename_and_move_files` verbindet alten und neuen Namen über `files::join`/`safe_join`
    (`rename_paths`); unsicherer Name aus der DB → Datei gilt als fehlend, nichts wird verschoben
    (Warnung im Log). `working_file_name` nutzt jetzt `pl_core::safepath::sanitize_component` (gleiche
    Regeln wie vorher, Gerätenamen zusätzlich `COM0`/`LPT0`/`CONIN$`/`CONOUT$` und hochgestellte Ziffern).

23. **OCR-Sprache automatisch** (2026-11-21, `PAPERRUST_OCR_LANGUAGE_AUTO`, Standard aus; PaperRust-Erweiterung): nach der Texterkennung `langdetect` (jetzt in pl-parsers) auf den Text; bei ≥ 200 Buchstaben, Wahrscheinlichkeit ≥ 0,90, Sprache nicht eingestellt und installiert → zweite Erkennung mit dieser Sprache, deren Ergebnis zählt (Fehler → erstes Ergebnis). `native::auto_ocr_language`, Test `tests/ocr_language_auto.rs`. Messung: `results/benchmark/2026-11-21-ocr-lang`.

24. **Several consume tasks in parallel** (2026-11-22; with `PAPERLESS_TASK_WORKERS` unset/`auto`
    – the default since 2026-11-23, pl-queue Known difference 16 – or a number > 1; `1` = one at a
    time as upstream; the fixes below stay active in every case). Upstream has the same windows with `TASK_WORKERS` > 1 but leaves
    them open; PaperRust closes them:
    - **ASN**: `AsnCheckPlugin` checks before parsing; two tasks with the same ASN could both pass.
      The store transaction now starts with `claim_unique`: `pg_advisory_xact_lock` on
      `paperrust:asn:<n>`, then the same lookup again. The second task waits for the first commit and
      fails with the plugin's message/status (`asn_already_exists[_in_trash]`) instead of a raw
      unique-constraint error (upstream: `IntegrityError`).
    - **Duplicates** (only with `CONSUMER_DELETE_DUPLICATES`): same, lock on
      `paperrust:checksum:<sha256 of the original>`; a duplicate found then is rejected exactly like
      `pre_check_duplicate` (original deleted, `document_already_exists`). Without the setting
      duplicates are stored anyway (upstream), so nothing is locked.
    - **File names**: unchanged – unique names are chosen and written under `MEDIA_LOCK` (see
      `medialock`), the DB columns are unique.
    - **Tags from barcodes / sub-directories** (`db::get_or_create_tag`): lookup and insert in one
      transaction under an advisory lock on `upper(name)` (upstream `get_or_create` can create the
      tag twice under concurrency; `documents_tag` has no case-insensitive unique constraint).
    - **Search index**: unchanged – every write takes the cross-process `.tantivy.lock` (pl-indexer);
      a busy lock is retried and, when exhausted, deferred to `index_document` (countdown 60).
    - **Classifier**: `FileClassifier` keeps the last loaded model (keyed by file mtime + size) and
      shares it between tasks; upstream reads the file per document. Same predictions; one copy in
      memory instead of one per task; reloaded as soon as training replaced the file.
    - **Blocking file I/O** (working-copy copy, checksums, writing original/archive/thumbnail) runs
      through `blocking_io` (`block_in_place` on the multi-threaded runtime) so that web requests in
      the same process are moved to other runtime threads instead of waiting behind the disk.
    Tests: `tests/e2e.rs` `e2e_concurrent_same_asn`, `e2e_concurrent_duplicates`,
    `e2e_concurrent_same_filename`, `e2e_concurrent_get_or_create_tag` (database needed, `#[ignore]`);
    unit tests `native::tests::classifier_cache_shares_and_reloads`,
    `consumer::tests::blocking_io_all_runtimes`.

25. **Duplicate check of shrunk files** (2026-11-23; PaperRust-only, upstream has no shrinking).
    Upstream `ConsumerPreflightPlugin.pre_check_duplicate` compares the checksum of the *delivered*
    file with `checksum`/`archive_checksum` of the existing documents, and `Document.checksum` is
    the checksum of the *stored* file (`_store`: `unmodified_original` or `working_copy`). With
    `PAPERRUST_SHRINK_ON_INGEST_MB` the stored file is the shrunk copy (`run_consumer`,
    `o.shrunk` → `unmodified_original = None`), so its `checksum` differs from the delivered file and
    uploading the same original again was **not** detected as a duplicate (also not by the
    `claim_unique` re-check, whose lock key is the delivered file's checksum).
    Fix: `documents_document.checksum` stays the checksum of the stored file (the sanity checker
    compares them). Additionally, when the stored file is a shrunk copy with another checksum,
    `store_all` writes the delivered file's checksum to the PaperRust table
    `paperrust_source_checksum (document_id PK, checksum)` in the same transaction
    (`source_checksum::record`, right after `_store`/version). `db::find_duplicates` (preflight and
    `claim_unique`) matches `checksum = c OR archive_checksum = c` **UNION** documents with a
    `paperrust_source_checksum` row for `c` (joined to `documents_document`, so trash counts as in
    upstream and hard-deleted documents drop out); order and messages unchanged. The advisory-lock
    protocol is unchanged (one key, `paperrust:checksum:<checksum of the delivered file>`, taken before
    the lookup, held until COMMIT): concurrent tasks with the same original serialize on it and the
    later one sees the committed source row. Table: migration `pl_consumer.0002_paperrust_source_checksum`
    (expand-only, idempotent, in `workflow_ext::migrations::ALL`, which `paperrust manage migrate`
    applies), also created on first use (`source_checksum::ensure_pool`, under an advisory lock). No
    foreign key (from data model stage 1 on `documents_document` is a view); stale rows of hard-deleted
    documents are ignored by the join and never match a new document (ids from a sequence).
    Not changed, as upstream: a **pre-consume script** that modifies the working copy also makes the
    stored `checksum` differ from the delivered file (upstream `pre_check_duplicate` has the same gap),
    so the same original is not recognized as a duplicate then; no source row is written for it.
    Limits: the table is not part of `document_exporter` yet (after export/import, shrunk documents are
    found only by the checksum of the stored file); a file whose bytes equal another document's shrunk
    copy *is* a duplicate of it (the stored checksum matches, as in upstream).
    Tests: unit `source_checksum::tests::*`, `db::tests::duplicates_sql_matches_upstream_and_source_checksum`;
    DB `tests/e2e.rs` `e2e_shrunk_original_is_duplicate` (real shrinking via `paperrust-pdftool`,
    sequential re-upload → `Duplicate`, two identical originals at once → one stored, one duplicate;
    fails without the fix). Formal model: `verification/parallel-ingest` (`Shrink`/`RecordSource`,
    `Broken_shrink_no_source`).

## Tests

- `cargo test -p pl-consumer -j 1` (lib, 23 Unit-Tests ohne DB): Overrides-`update`, JSON (B5),
  Ergebnis-Dicts, Einstellungen (inkl. Barcode-Vorrang ApplicationConfiguration vor Umgebung),
  Audit-Diffs (gegen den Referenz-Eintrag id 52), `created`-Darstellung, Prüfsumme, atomares Schreiben,
  qpdf-Ersatz (Exit 0/2/3, Fehlertext, `repr(Path)`), Doppelseiten-Zusammenführung (lopdf) inkl.
  Fehlertext, Eingangsfilter, Stabilitätsprüfung, Schnappschuss-Vergleich, Hilfsfunktionen.
- `tests/e2e.rs` (`#[ignore]`, braucht `PAPERRUST_CONSUMER_TEST_DATABASE_URL` auf `pl_consumer_test*`,
  frisch über `tests/parity/pl-consumer/reset_db.sh`): PDF mit Text, gescanntes PNG (Tesseract 5.5.4 +
  PDFium → Text, Archiv-PDF, `archive_checksum`), E-Mail (Datum aus der Mail), Duplikat (mit/ohne
  `DELETE_DUPLICATES`), Overrides (Titel-Vorlage, Korrespondent, Typ, Tags mit Vorfahren, Besitzer,
  Rechte, Datum, ASN, Custom Fields), ASN-Fehler, MATCH_ANY/MATCH_REGEX + Speicherpfad-Vorlage,
  Datum aus dem Inhalt, `PAPERLESS_FILENAME_FORMAT` + Kollision `_01`, Seed-Workflow „Document Added“,
  Datei fehlt, nicht unterstützter Typ, Suchindex (pl-search findet die Dokumente), Statusmeldungen,
  Beobachter `--oneshot` mit Unterordner-Tags und Ignorierregeln. (2026-11-08 nicht neu gelaufen.)
- Parität (`tests/parity/pl-consumer/`): `reset_db.sh` → `run_upstream.sh` (unverändertes upstream,
  DB `…_up`, Temp-Datenverzeichnis, Overlay `pl_consumer_settings.py`: Celery eager, Channels/Cache im
  Speicher; für Barcodes nur `pdf2image.convert_from_path` durch pypdfium2 mit gleicher DPI ersetzt wie
  bei pl-pdfops) → `cargo test -p pl-consumer --test parity -- --ignored` → `compare.py`. DB-Namen über
  `PL_CONSUMER_DB` (Standard `pl_consumer_test`; W9 lief mit `pl_w9_test`/`pl_w9_test_up`).
  16 Fälle (`cases.json`): Text, Overrides, ANY+Speicherpfad, REGEX, Workflow „Document Added“
  (Zuweisung inkl. Custom Fields, Entfernen, WorkflowRun), Besitzer-gefilterte Zuordnung, Duplikat
  eingelesen/gelöscht, ASN existiert, `FILENAME_FORMAT` + Kollision, PDF mit OCR aus, **Barcode-Trennung
  PATCHT** (2 Teil-Dokumente), **ASN aus Code39**, **TIFF → PDF + Trennung** (2 Teil-Dokumente),
  **Tag-Barcodes + gespeicherte Barcode-Werte nur über ApplicationConfiguration**. Verglichen:
  Aufgabenergebnis/Fehlertext, alle neu entstandenen Dokumente paarweise (`new_ids`), alle
  Document-Felder, Tags, Custom Fields, Objektrechte, WorkflowRuns, `DocumentBarcode`,
  **vollständige Audit-Log-Folge** (ohne Zeitstempel), Dateien. Teil-PDFs: Prüfsummen normalisiert
  (lopdf ≠ pikepdf-Bytes), Seitenzahl/Inhalt verglichen.
  Ergebnis 2026-11-08: **0 Unterschiede**; bekannt nur `content` (+ Audit-Inhalt) bei `pdf_ocr_off`
  (upstream ohne pdftotext) sowie bei `barcode_asn`/`barcode_tags_app_config`
  (PDF-Textebene: Zeilenenden/Leerzeilen zwischen Seiten, Textextraktion von pl-parsers – nicht
  Barcode-Logik). E-Mail/Bilder kann upstream in der Referenz nicht einlesen → nur Rust-E2E.
- **W15-Parität** (`tests/parity/pl-consumer/w15/`): `reset_db.sh` (DBs `pl_w15_ref`/`pl_w15_rs` aus
  `paperless_snapshot` + gemeinsames `setup.sql`) → `run_upstream.sh` (upstream-Funktionen direkt,
  `apply_ai_suggestions.delay_on_commit` abgefangen, Prüf-Parser für das Archiv, weil die Referenz ohne
  tesseract/ghostscript keins erzeugt) → `PAPERRUST_W15_DATABASE_URL=…/pl_w15_rs cargo test -p pl-consumer
  --test w15_parity -- --ignored` → `compare.py`. Schritte: Neu-Einlesen Text / PDF / mit Archiv /
  fehlendes Dokument, `check_scheduled_workflows` zweimal (added einmalig fällig/nicht fällig, created
  +30, modified wiederholend mit altem/jungem Lauf, Feld-Datum −7 fällig/zu spät/zu alt, Papierkorb mit
  Kaskade), `bulk_update_documents` (Workflow „Dokument geändert“ + Speicherpfad-Umbenennung,
  Standard-Umbenennung). Verglichen: Schrittergebnisse, eingereihte Aufgaben, Dokumentfelder, Tags,
  WorkflowRuns (inkl. Papierkorb), Feldwerte, Barcodes, Audit-Log der Dokumente, PaperlessTask-Anzahl,
  Dateien (Original/Archiv SHA-256, Vorschaubild). Ergebnis siehe `out/report.md`.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **759f9cd22 (#14741) – Vorschaubild für Bilder ohne Archiv-PDF.** `NativeParsing::thumbnail`:
  Tesseract-Parser + kein Archiv + Bild-MIME (`pl_ocr::imageinfo::is_image`) →
  `pl_ocr::thumbnail::make_thumbnail_from_image` (vorher: Bild ist kein PDF → Standardbild). Betrifft
  Verarbeitung ohne Archiv (OCR übersprungen) und `document_thumbnails` (übergibt nie ein Archiv).
  Known difference: upstream wirft bei einem Kodierfehler `ParseError`; Rust nimmt mit Warnung das
  Standardbild, weil die image-Crate manche Formate (HEIC) nicht liest, die Pillow kann – sonst würde
  die Verarbeitung scheitern, wo upstream gelingt. Test: pl-ocr
  `thumbnail::tests::image_without_archive_has_image_thumbnail`.

## Last-Befunde B1/B7/B8 (2026-11-18)

Quelle: tests/stress/BEFUNDE.md.

- **B1 – Sperrreihenfolge MEDIA_LOCK (`medialock.rs`).** Eine gemeinsame `MediaLock` für pl-consumer
  und pl-documents-write. Regel überall: **erst Dokumentzeilen sperren (Transaktion bzw.
  Sicherungspunkt), dann MEDIA_LOCK; unter MEDIA_LOCK nie auf eine fremde Zeilensperre warten.**
  `files::update_filename_and_move_files` und `update_filename_with_context` öffnen dafür eine eigene
  Transaktion (in einer Aufrufer-Transaktion einen Sicherungspunkt), sperren Dokument + Stammdokument
  mit `SELECT … FOR UPDATE` (id-Reihenfolge, `medialock::lock_document_rows`) und holen erst dann
  MEDIA_LOCK. Einlesen (`store_all`) und Neuverarbeiten (`reprocess`) hielten die Regel schon
  (unter MEDIA_LOCK nur Lesen und Dateien). MEDIA_LOCK hat zusätzlich eine prozessinterne Warteschlange
  und ein Zeitlimit `PAPERRUST_MEDIA_LOCK_TIMEOUT` (Sekunden, Standard 120, `0` = ohne) mit Fehler
  „timed out after N s waiting for the media lock …“ statt endlosem Warten.
  *Known difference:* upstream wartet ohne Zeitlimit (filelock `timeout=-1`) und holt in
  `update_filename_and_move_files` erst die Dateisperre, dann schreibt es die Zeile (dieselbe
  theoretische Verklemmung, dort wegen Autocommit selten). Schlägt das Festschreiben des
  Sicherungspunkts fehl, wird nur protokolliert (wie upstream bei Fehlern im try-Block).
- **B7 – erneut zugestellte Einlese-Aufgabe.** `store_all` schreibt in der Einlese-Transaktion
  `paperrust_task_queue.headers.paperrust_document_id = <neues Dokument>` (nur mit Aufgaben-id und
  vorhandener Tabelle). `consume_file` prüft vor allen Plugins: gibt es zu dieser Aufgabe schon ein
  (noch vorhandenes) Dokument, wird nicht erneut eingelesen, sondern: Original löschen (falls der
  Abbruch davor lag), Suchindex nachziehen, Erfolg mit dieser Dokument-id melden. Das
  Nach-Einlese-Skript läuft dabei nicht erneut (unbekannt, ob es schon lief).
  *Known difference:* upstream (Celery `acks_late`) hätte dieselbe Lücke und meldet „File not found“.
- **B8:** `add_to_index`: auch ein Fehler des Index (nicht nur belegte Sperre) reiht
  `index_document` ein (belegte Sperre: countdown 60 wie upstream; Fehler: countdown 5), statt die Änderung nur zu protokollieren. Scheitert auch das
  Einreihen (Datenbank weg, tests/chaos c1, Gesamtprüfung 2026-11-18), versucht ein Hintergrund-Task
  es bis gut 30 min lang erneut (5 s … 15 min) und reiht dann ohne countdown ein; vorher blieb das
  Dokument bis zum nächsten Neustart unauffindbar. Upstream (Celery `delay_on_commit`) hat diesen Fall nicht.
- **s3-Nachlauf:** `run_consumer` wiederholt `store_all` bei Verklemmung/Serialisierungsfehler
  (40P01/40001, erkannt über den SQLSTATE der sqlx-Fehler) bis zu 3-mal; die abgelegten Dateien des
  gescheiterten Versuchs werden vorher entfernt. `update_filename_and_move_files` sperrt das
  Stammdokument vor der Version.

## Ausfall-Befunde C1/C4 (tests/chaos, 2026-11-18)

- **C1-2 Upload ging bei DB-Ausfall verloren:** Scheiterte das Speichern, weil die Datenbank weg war
  (Verbindung abgerissen, Pool-Zeitüberschreitung), löschte `consume_file` die Spool-Datei (D-102) und
  meldete die Aufgabe fehlgeschlagen; die Buchführung scheiterte ebenfalls, der Aufräumer stellte die
  Aufgabe nach Ablauf der Lease erneut zu → „File not found“, Dokument verloren. Jetzt: Fehlschlag bei
  nicht erreichbarer DB (Fehlertext `is_db_unavailable_text` oder DB antwortet nicht binnen 5 s) bzw.
  nicht erreichbarem Remote-OCR-Dienst (`is_service_unreachable_text`) → `ConsumeError::Transient`,
  Datei bleibt liegen, Fortschritt „Working“ mit Meldung „… temporarily not reachable – the file is kept
  …“, Aufgabe `retry` in 15/30/60 s (höchstens 3×, danach endgültig fehlgeschlagen und Spool aufgeräumt).
  Gelingt das Einreihen nicht (DB noch weg), stellt der Aufräumer die Aufgabe zu (`max_deliveries`).
  Doppelt eingelesen wird nichts: die B7-Prüfung (`task_document`) erkennt ein schon gespeichertes Dokument.
  upstream: Ausnahme der Aufgabe, keine Wiederholung (Celery-Aufgabe verloren).
- **Remote-OCR nicht erreichbar:** upstream liest das Dokument mit leerem Text ein; PaperRust ließ es
  scheitern und verwarf den Upload. Jetzt wie oben vorübergehend (Neuversuch), HTTP-Fehlerantworten des
  Dienstes bleiben endgültige Fehler.
- Nachweis: `python tests/chaos/chaos.py c1 c4`, Unit-Tests `consumer::tests::transient_failure_texts`,
  `task::tests::transient_backoff`.
