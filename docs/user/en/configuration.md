# Configuration

[Deutsch](../configuration.md) · [Documentation overview](README.md)

PaperRust reads its settings from environment variables. All variables are called `PAPERRUST_…`. The
paperless-ngx names (`PAPERLESS_…`) and the old names `PAPERLESS_RS_…` still work (see
[Variable names](#naming)), so anyone switching from paperless-ngx can simply keep their
`paperless.conf` or `docker-compose.env`. Almost everything can also be set in the web UI. Then
only the database, the `SECRET_KEY` and a few start-up values have to stay in the environment –
see [Settings in the UI](#settings-ui).

This page has two parts: first, how settings fit together and which ones exist only in PaperRust;
then the [reference](#reference) of every setting PaperRust shares with paperless-ngx.

<a id="naming"></a>

## Variable names: `PAPERRUST_*` and `PAPERLESS_*`

| Form | Example | Status |
|---|---|---|
| `PAPERRUST_<NAME>` | `PAPERRUST_OCR_LANGUAGE=deu+eng` | **primary name** of every setting |
| `PAPERLESS_<NAME>` | `PAPERLESS_OCR_LANGUAGE=deu+eng` | equivalent second name, so files from paperless-ngx keep working |
| `PAPERLESS_RS_<NAME>` | `PAPERLESS_RS_BIND` | old working name of the project; still accepted, but PaperRust warns once per name |

If both `PAPERRUST_<NAME>` and `PAPERLESS_<NAME>` are in the environment, `PAPERRUST_<NAME>` counts.
The authoritative list of names is in [docs/BRANDING.md](../../BRANDING.md) (section
"Umgebungsvariablen").

> [!NOTE]
> Django and Celery no longer exist in PaperRust; variables only meant for them (such as a former
> `PAPERRUST_DJANGO_URL`) have no effect and at most produce a hint in the log.

<a id="config-file"></a>

## Where settings come from

PaperRust looks for each value in this order and takes the first hit:

1. The process environment.
2. Values saved in the web UI (*Settings → System*, table `paperrust_setting`), see
   [Settings in the UI](#settings-ui).
3. A configuration file: the path in `PAPERRUST_CONFIGURATION_PATH` (or
   `PAPERLESS_CONFIGURATION_PATH`), otherwise the first existing file of `../paperrust.conf`,
   `../paperless.conf`, `/etc/paperrust.conf`, `/etc/paperless.conf`,
   `/usr/local/etc/paperrust.conf`, `/usr/local/etc/paperless.conf`. The file never replaces what is
   already in the environment.
4. The built-in default.

There is one exception: the options of the *Settings → Configuration* page (in the PaperRust design
*Manage → Configuration → Application*; OCR, barcodes, AI,
title and logo; table `paperless_applicationconfiguration`) take priority over the environment as
soon as they have a value in the database. This behaves exactly as in paperless-ngx.

<a id="settings-ui"></a>

## Settings in the UI

Under **Settings → System** (`/rs/app/settings/system`; in the PaperRust design *Manage →
Configuration → Server*) you set system-wide things: sending email,
login and SSO, schedules, consumption, OCR, AI, logs, time zone and much more. Superusers see the
tab. Users who only have the permission for the *Configuration* page see just its options. The
values live in the database (table `paperrust_setting`) and apply to every process connected to
that database.

For each setting the page shows its label, current value, origin and variable name. The "?" jumps
to the matching entry on this page. The badges mean:

| Badge | Meaning |
|---|---|
| **from environment** | The variable is in the environment, so the field is locked. To change it: remove or edit the variable, then restart. |
| **overrides environment** | Option of the *Configuration* page; its database value replaces the one from the environment. |
| **overrides paperrust.conf** | The database value replaces a value from the configuration file. |
| **Restart required** | The value takes effect after a restart. If PaperRust runs under a supervisor (Docker entrypoint, systemd template, desktop app), the bar at the top offers "Restart now". |
| **Rebuild index** | Rebuild the search index or the file names after the change. |
| **Secret** | The value is never shown, only "set" or "not set". An empty field leaves it as it is. |
| **no effect** | Comes from paperless-ngx and has no effect in PaperRust; can be removed. |

Most changes apply at once, for example SMTP, session lifetime, file name template or log filter;
consumption settings apply from the next document on. PaperRust restarts schedules, folder watching
and workers by itself when needed. Saving works per category. If someone else changed the same
category in the meantime, the page reports a conflict. **History** shows who changed what and when
(secrets as `***`). **Export settings/Import settings** saves the database values as JSON; secrets
are only included when you give a passphrase, and are then encrypted with it.

### Precedence

| Kind of setting | Order (first wins) | In the UI |
|---|---|---|
| Regular settings | environment → database → configuration file → default | editable as long as the environment does not set them |
| Options of the *Configuration* page (OCR, barcodes, AI, title/logo) | database (if set) → environment → configuration file → default; `0` or empty counts as "not set" | always editable; "Reset" lets the environment apply again |
| Start-up values (database, `SECRET_KEY`, paths, address …) | environment → configuration file → default | display only, with the hint "set via the environment/`paperrust.conf`" (desktop app: in its own settings) |
| Boundary to the host (proxy headers, remote user, auto-login, scripts, access to internal addresses, `DEBUG`) | environment only | display only – an administrator account in the UI must not be able to start programs on the server or get around the login |
| No effect in PaperRust (Tika, Flower, Django cache …) | – | hidden; visible with the switch "Show startup and environment-only values" |

That the environment wins for regular settings is on purpose. Whoever works with Compose files
stays in control. And a broken setting can always be repaired: set the variable and restart, or,
without a running server, `paperrust manage settings reset <VARIABLE>`.

PaperRust stores secrets (SMTP password, OAuth secrets, SSO providers …) encrypted. The key is
derived from `PAPERRUST_SECRET_KEY`. If you change the `SECRET_KEY`, run
`paperrust manage settings rekey --old-secret-key <OLD>` afterwards. Stored in plain text – as in
paperless-ngx – are the API keys of the *Configuration* page (AI, remote OCR) and the passwords of
mail accounts.

### Migration: moving the environment into the database

After switching from paperless-ngx everything runs as before: every variable still applies, and
the entries carry the badge "from environment". To slim the environment down, use the
**Origin & migration** tab (in *Settings → System*):

1. It shows all values from the environment, `paperrust.conf` (or `paperless.conf`) and the desktop app and sorts them:
   *can be moved*, *must stay in the environment* (with the reason), *no effect* or *obsolete*.
   The last two can simply be deleted.
2. **Move to database** (per row) or **Move all to the database** copies the movable values into
   the database. As long as the variable is still in the environment, the field stays locked
   ("already in the database – can be removed from the environment").
3. **Download slim configuration** returns a `docker-compose.env` that only contains what has to
   stay in the environment. Every variable left out appears as a comment.
4. **Check a configuration file**: upload `paperrust.conf`, `paperless.conf`, `docker-compose.env` or
   `docker-compose.yml`. For YAML, `environment:` and `env_file:` of the `webserver` service count,
   otherwise those of the first service with `PAPERRUST_*` or `PAPERLESS_*` variables. The file is sorted the same
   way; nothing changes until you confirm.
5. Put the slim file in place and restart the container.

The same works on the command line (in the container with
`docker compose exec -T paperrust paperrust manage …`):

```sh
paperrust manage settings origin                       # show origin and classification
paperrust manage settings adopt --all                  # all movable values into the DB
paperrust manage settings adopt PAPERRUST_EMAIL_HOST   # just one variable
paperrust manage settings slim-env < docker-compose.env > docker-compose.env.new
paperrust manage settings check-file docker-compose.yml
```

Going back to paperless-ngx: paperless-ngx does not know the `paperrust_setting` table.
`paperrust manage settings export --format env` therefore prints the values as variables.

### Slim Docker configuration

For a new installation, `docker/docker-compose.slim.yml` and `docker/docker-compose.slim.env` are
included. Their environment only contains:

| Variable | Purpose |
|---|---|
| `PAPERRUST_DBHOST` | database host; implies `PAPERRUST_DBENGINE=postgresql` (`DBNAME`/`DBUSER` are `paperless`) |
| `PAPERRUST_DBPASS` | database password (same as `POSTGRES_PASSWORD` of the `db` service) |
| `PAPERRUST_SECRET_KEY` | required; also the key for the encrypted settings |
| `PAPERRUST_URL` | optional; only needed behind a reverse proxy before the first login (CSRF), otherwise in the UI |

```sh
cd docker
cp docker-compose.slim.env .env.paperrust      # adjust (SECRET_KEY!), point env_file in the YAML to it
docker compose -f docker-compose.slim.yml up -d
docker compose -f docker-compose.slim.yml exec paperrust createsuperuser
```

Then set time zone, OCR language, email, login etc. under *Settings → System*. The detailed
templates (`docker-compose.postgres.yml` with `docker-compose.env`) still work; every variable
listed there locks the matching field in the UI.

<a id="paperrust-settings"></a>

## Settings that exist only in PaperRust

Names are given in the primary `PAPERRUST_*` form. Defaults according to the source code
(as of 2026-11-18).

### Server

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_bind"></a>`PAPERRUST_BIND` | `127.0.0.1:8080` | Address `IP:PORT` PaperRust listens on. An invalid value stops the start. An address reachable from other machines has to be set explicitly; PaperRust then logs a warning. In the Docker image the entrypoint builds the address from [`PAPERRUST_BIND_ADDR`/`PAPERRUST_PORT`](#paperrust_bind_addr) when `PAPERRUST_BIND` is missing (there `0.0.0.0:8000`). |
| <a id="paperrust_base_dir"></a>`PAPERRUST_BASE_DIR` | working directory | Root of the installation; `media/`, `data/`, `consume/` and `static/` live below it by default. |
| <a id="paperrust_routes"></a>`PAPERRUST_ROUTES` | `routes.toml` in the working directory | File that decides, per route, who serves it. If the path is set but the file is missing, PaperRust does not start. The Docker image and the desktop app ship the file. |
| <a id="paperrust_light_ui"></a>`PAPERRUST_LIGHT_UI` | `on` | `off` turns off the web UI under `/rs/app/`. |
| <a id="paperrust_light_ui_dir"></a>`PAPERRUST_LIGHT_UI_DIR` | `STATICDIR/../light` | Folder with the files of the web UI. |
| <a id="paperrust_api_v2"></a>`PAPERRUST_API_V2` | `on` | `off` turns off PaperRust's own v2 API. |
| <a id="paperrust_db_max_connections"></a>`PAPERRUST_DB_MAX_CONNECTIONS` | `10` | Size of the database pool if neither `pool.max_size` in [`PAPERRUST_DB_OPTIONS`](#paperrust_db_options) nor [`PAPERRUST_DB_POOLSIZE`](#paperrust_db_poolsize) is set. |
| <a id="paperrust_db_require_auth"></a>`PAPERRUST_DB_REQUIRE_AUTH` | – (no check) | Allowed authentication methods at the database, e.g. `scram-sha-256`. PaperRust checks this at start-up. Environment only. |
| <a id="paperrust_instance_id"></a>`PAPERRUST_INSTANCE_ID` | – | Name of this instance; shown as `instance` in `/__rs/health`. |
| <a id="paperrust_log"></a>`PAPERRUST_LOG` | `info` | How talkative the log is (`tracing` syntax), e.g. `info,sqlx=warn`. |
| <a id="paperrust_log_files"></a>`PAPERRUST_LOG_FILES` | `yes` | `no`: no log files `paperrust.log`/`mail.log` in [`PAPERRUST_LOGGING_DIR`](#paperrust_logging_dir); the "Logs" page then stays empty. Line format `[time] [LEVEL] [logger] message`; rotation see [Logging](#logging). |
| <a id="paperrust_docs_url"></a>`PAPERRUST_DOCS_URL` | – | This help is built into the program and served under `/rs/docs/` (in the UI language, also offline). If an http(s) address ending in `/` is set here (same layout as `docs/user/`), `/rs/docs/…` redirects there. |
| <a id="paperrust_share_mode"></a>`PAPERRUST_SHARE_MODE` | – | Desktop app only (it sets this itself): `local` shows "links only work on this computer" in the share dialog; `lan` builds share links with the address in the home network. Left unset on a server. |
| <a id="paperrust_share_url"></a>`PAPERRUST_SHARE_URL` | – (then `PAPERRUST_URL`) | Address used in share links with `PAPERRUST_SHARE_MODE=lan`. |
| <a id="paperrust_stop_on_stdin_eof"></a>`PAPERRUST_STOP_ON_STDIN_EOF` | `no` | `yes`: shut down cleanly as soon as standard input is closed (used by the desktop app). |
| <a id="paperrust_init"></a>`PAPERRUST_INIT` | `yes` | Docker image only: `no` skips database migration and creating the administrator at start-up (e.g. for a second process with `--worker-only`). |
| <a id="paperrust_init_db_timeout"></a>`PAPERRUST_INIT_DB_TIMEOUT` | `300` s | Docker image only: how long the start waits for the database. |
| <a id="paperrust_migrate_source"></a><a id="paperrust_migrate_target"></a>`PAPERRUST_MIGRATE_SOURCE`, `PAPERRUST_MIGRATE_TARGET` (also `PAPERLESS_MIGRATE_*`) | – | Source and target for `paperrust-migrate-db` ([switching](migration.md#sqlite-mariadb)). |
| <a id="paperrust_migrate_api_token"></a>`PAPERRUST_MIGRATE_API_TOKEN` | – | API token for the migration tool. |
| <a id="paperrust_pg_bin"></a>`PAPERRUST_PG_BIN` | – | Folder with `pg_dump` for the backup when switching. |
| <a id="paperrust_datamodel_backup"></a>`PAPERRUST_DATAMODEL_BACKUP` | `on` | Make a backup with `pg_dump` before the data model conversion steps. |
| <a id="PAPERRUST_MIGRATE_WITHOUT_BACKUP"></a><a id="paperrust_migrate_without_backup"></a>`PAPERRUST_MIGRATE_WITHOUT_BACKUP` | `false` | If `pg_dump` is missing, a data model conversion stops instead of carrying on without a backup. `true` carries on anyway – only if you have made a backup yourself. |
| <a id="PAPERRUST_DB_ACQUIRE_TIMEOUT"></a><a id="paperrust_db_acquire_timeout"></a>`PAPERRUST_DB_ACQUIRE_TIMEOUT` | `30` s | The longest time a request waits for a free database connection. Takes priority over `PAPERRUST_DB_TIMEOUT`. Takes effect after a restart. |
| <a id="PAPERRUST_DB_LONGPOLL_CONNECTIONS"></a><a id="paperrust_db_longpoll_connections"></a>`PAPERRUST_DB_LONGPOLL_CONNECTIONS` | `8` | Separate database connections for apps that wait for changes while syncing. This way waiting apps do not take connections away from other requests. |
| <a id="PAPERRUST_MEDIA_LOCK_TIMEOUT"></a><a id="paperrust_media_lock_timeout"></a>`PAPERRUST_MEDIA_LOCK_TIMEOUT` | `120` s | The longest time a write waits for the lock on the file storage (`0` = no limit). After that it stops with a message instead of hanging. |
| <a id="paperrust_remote_version_url"></a>`PAPERRUST_REMOTE_VERSION_URL` | project release address | Where the "new version available" check looks (only if [`PAPERRUST_ENABLE_UPDATE_CHECK`](#paperrust_enable_update_check) is on). |
| <a id="paperrust_django_url"></a>`PAPERRUST_DJANGO_URL` | – | **Removed.** Forwarding to Django no longer exists; PaperRust serves every route itself. If the variable is set, the log shows a hint at start-up, nothing else happens. |

### Task queue and worker

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_queue"></a>`PAPERRUST_QUEUE` (also `PAPERRUST_QUEUE_BACKEND`) | – | **No effect.** Only the built-in queue in PostgreSQL exists. An old value such as `celery` produces a hint in the log, never an abort. |
| <a id="paperrust_worker_name"></a>`PAPERRUST_WORKER_NAME` | `paperrust@<host>:<pid>` | Name of the worker in task records. |
| <a id="paperrust_queue_names"></a>`PAPERRUST_QUEUE_NAMES` | `celery` | Which queues this worker processes (`celery` is the historical name of the default queue). |
| <a id="paperrust_queue_poll_interval"></a>`PAPERRUST_QUEUE_POLL_INTERVAL` | `1` s | How often the worker looks for work if no notification (`NOTIFY`) arrives from PostgreSQL. |
| <a id="paperrust_queue_visibility_timeout"></a>`PAPERRUST_QUEUE_VISIBILITY_TIMEOUT` | `60` s | How long a running task stays reserved for one worker; a heartbeat extends it. |
| <a id="paperrust_queue_max_deliveries"></a>`PAPERRUST_QUEUE_MAX_DELIVERIES` | `3` | How often a task is handed out again after a worker crash before it fails with `WorkerLostError`. |
| <a id="paperrust_queue_result_expires"></a>`PAPERRUST_QUEUE_RESULT_EXPIRES` | `3600` s | How long finished entries stay in the queue. |
| <a id="paperrust_queue_shutdown_grace"></a>`PAPERRUST_QUEUE_SHUTDOWN_GRACE` | `30` s | How much time running tasks get when shutting down. |
| <a id="paperrust_index_writer"></a>`PAPERRUST_INDEX_WRITER` | `queue` | How the search index is updated after changes through the API: `queue` = as a background task in the queue (old name `celery` still works); `rust` = right away in the request; `off` = not at all. |
| <a id="paperrust_bulk_edit_async_min_docs"></a>`PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS` | `500` | From this many documents on, a bulk edit runs in the background (`0` = always immediately). |
| <a id="PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS"></a><a id="paperrust_workflow_run_retention_days"></a>`PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS` | `90` | How many days the [workflow history](automatik.md#history) is kept (`0` = without limit). Takes effect immediately. |
| <a id="PAPERRUST_BULK_UPDATE_COALESCE"></a><a id="paperrust_bulk_update_coalesce"></a>`PAPERRUST_BULK_UPDATE_COALESCE` | `on` | Combine the waiting follow-up work of several bulk edits into one (faster, less load). Set to `off` only for troubleshooting. |

PaperRust does not need Redis. Its cache lives inside its own process. When several processes serve
the same installation (a separate `--worker-only` process, several instances behind a load
balancer), they have to share login throttling, the lock against reused TOTP codes and the dropping
of stale cached data: set [`PAPERRUST_CACHE=postgres`](#message-broker) in every process. PaperRust
warns at start-up when it detects such a setup.

### OCR, PDF and classifier

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_tesseract_path"></a>`PAPERRUST_TESSERACT_PATH` | `tesseract` in `PATH` | Tesseract program for text recognition. |
| <a id="paperrust_tesseract_lib"></a>`PAPERRUST_TESSERACT_LIB` | standard search | Tesseract as a library (`libtesseract`): file or folder; `off` uses only the program. |
| <a id="paperrust_tessdata_dir"></a>`PAPERRUST_TESSDATA_DIR` | Tesseract default | Folder with the language data (`*.traineddata`). |
| <a id="paperrust_ocr_engine"></a>`PAPERRUST_OCR_ENGINE` | `tesseract` | Which text recognizer works: `tesseract`; `windows` (the text recognition built into Windows; if it is missing, PaperRust warns and uses Tesseract); `auto` (Windows recognition if present and able to handle all languages of `PAPERRUST_OCR_LANGUAGE`, otherwise Tesseract); `macos` (planned, not implemented yet → Tesseract); `ppocr` (PP-OCRv6, to try out: reads print as well as Tesseract and handwriting much better; needs ONNX Runtime and the model files, included in the Docker image; if anything is missing, PaperRust warns and uses Tesseract; languages in non-Latin scripts are still read by Tesseract). The archive PDF and the text layer are produced the same way in every case. See the notes below the table. |
| <a id="paperrust_ocr_orientation"></a>`PAPERRUST_OCR_ORIENTATION` | automatic | Who decides whether a page is sideways or upside down (when `PAPERRUST_OCR_ROTATE_PAGES` is on): `model` (small classifier, works for every text recognizer, also detects 90°), `engine` (Tesseract, as before), `off`. Unset: `model` if ONNX Runtime and the model are present (Docker image), otherwise `engine`. |
| <a id="paperrust_ocr_model_dir"></a>`PAPERRUST_OCR_MODEL_DIR` | default search | Folder with the OCR model files; if set, PaperRust only looks there (servers without internet put the files there themselves). Otherwise: `DATA_DIR/models/ocr`, in the Docker image `/usr/local/share/paperrust/ocr-models`, folder `ocr-models` next to the program. |
| <a id="paperrust_ocr_model_download"></a>`PAPERRUST_OCR_MODEL_DOWNLOAD` | `false` | Download missing model files from a fixed source with a fixed checksum into `DATA_DIR/models/ocr`. Without this setting PaperRust never downloads anything. |
| <a id="paperrust_ppocr_model"></a>`PAPERRUST_PPOCR_MODEL` | `small` | Model size for `ppocr`: `small`, `fast` (about 3× faster on small devices, print as good, weaker on handwriting) or `medium` (2–3× slower, slightly better on handwriting; not in the base package, needs `PAPERRUST_OCR_MODEL_DOWNLOAD` or your own files). |
| <a id="paperrust_ppocr_max_side"></a>`PAPERRUST_PPOCR_MAX_SIDE` | `1920` | Longest image side for `ppocr` (960–4096). Larger reads hardly more but costs time. |
| <a id="paperrust_ppocr_sessions"></a>`PAPERRUST_PPOCR_SESSIONS` | `1` | How many pages `ppocr` computes at the same time. More gained at most 10 % in the measurement and costs about 0.5 GB of memory each. |
| <a id="paperrust_ocr_handwriting"></a>`PAPERRUST_OCR_HANDWRITING` | `off` | Only with `ppocr` and German in the OCR languages: `auto` reads lines PP-OCR is unsure about a second time with a handwriting model (DE·HTR, included in the Docker image) and takes its text if it is more confident. Measured: real handwriting lines 44 → 84 % of words found, print unchanged; costs time only on uncertain lines. |
| <a id="paperrust_onnxruntime_path"></a>`PAPERRUST_ONNXRUNTIME_PATH` | default search | ONNX Runtime library (version 1.20 or newer) for `ppocr` and the orientation model. Set in the Docker image. |
| <a id="PAPERRUST_OCR_ENGINE_IDLE_SECS"></a><a id="paperrust_ocr_engine_idle_secs"></a>`PAPERRUST_OCR_ENGINE_IDLE_SECS` | `180` | After this many seconds without text recognition, PaperRust frees the memory of the Tesseract language models (`0` = never). Saves memory when nothing is consumed for a while; the next document then takes a little longer. Takes effect immediately. |
| <a id="paperrust_pdfium_path"></a>`PAPERRUST_PDFIUM_PATH` | standard search | PDFium library. It draws PDF pages for OCR, thumbnails and barcode detection. Without PDFium there are no PDF thumbnails and no barcodes. |
| <a id="paperrust_pdfa_font_dir"></a>`PAPERRUST_PDFA_FONT_DIR` | standard search | Folder with the Liberation fonts. If the original lacks a standard font (Helvetica/Arial, Times, Courier), PaperRust embeds a substitute with the same widths in the PDF/A archive. Without it the archive is not marked as PDF/A. Already included in the Docker image. |
| <a id="paperrust_classifier_model_file"></a>`PAPERRUST_CLASSIFIER_MODEL_FILE` | `DATA_DIR/paperrust_classifier.model` | Model file of the automatic matching (own format; the paperless-ngx model is not used). |
| <a id="paperrust_import_max_unzipped_bytes"></a>`PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` | 1 GiB + 100 × archive size | Upper limit when `document_importer` unpacks a ZIP (protection against ZIP bombs). |
| <a id="paperrust_shrink_on_ingest_mb"></a>`PAPERRUST_SHRINK_ON_INGEST_MB` | `0` (off) | Shrink scans (PDFs with hardly any text) larger than this many MB during consumption; the smaller file is kept. See [PDF tools](pdf-tools.md#verkleinern-beim-einlesen). |
| <a id="paperrust_shrink_preset"></a>`PAPERRUST_SHRINK_PRESET` | `standard` | Strength for that: `lossless`, `standard` (150 dpi, JPEG 60) or `strong` (96 dpi). |
| <a id="paperrust_pdftool_timeout"></a>`PAPERRUST_PDFTOOL_TIMEOUT` | `120` | Time limit in seconds per run of the PDF tool (redact, shrink, repair). |
| <a id="paperrust_pdftool_memory_mb"></a>`PAPERRUST_PDFTOOL_MEMORY_MB` | `1536` | Memory limit of the PDF tool in MB (`0` = no limit). |
| <a id="paperrust_pdftool_bin"></a>`PAPERRUST_PDFTOOL_BIN` | next to `paperrust` | Path to `paperrust-pdftool`. Without the program there is no redacting and shrinking; consumption continues without repair. |
| <a id="paperrust_office_to_pdf"></a>`PAPERRUST_OFFICE_TO_PDF` | `true` | Convert Word, OpenDocument and RTF files into an archive PDF with WordCraft (in `paperrust-pdftool`) on ingest; the thumbnail is then page 1. If the conversion fails, ingest continues without an archive PDF. See [PDF tools](pdf-tools.md#office-beim-einlesen). |
| <a id="paperrust_office_to_pdf_timeout"></a>`PAPERRUST_OFFICE_TO_PDF_TIMEOUT` | `0` | Time limit in seconds per office conversion (on ingest and for "Convert to PDF"); `0` = same as `PAPERRUST_PDFTOOL_TIMEOUT`. |

About Windows text recognition: it works with **one** language, namely the first one of
`PAPERRUST_OCR_LANGUAGE` for which a Windows language pack is installed. Further languages with the
same script (e.g. `eng` next to `deu`) are recognized by that pack as well. If a language with a
different script is missing, PaperRust reports an error including the command to install it.
Rotated pages are still detected by Tesseract if it is installed.

### Web UI and branding

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_branding"></a>`PAPERRUST_BRANDING` | `on` | `off`: the old Angular UI is served without PaperRust name, logo and colour, i.e. exactly as in paperless-ngx. |
| <a id="paperrust_accent"></a>`PAPERRUST_ACCENT` | `#B7410E` | Accent colour (hex). PaperRust darkens it by itself until the text on buttons is easy to read (contrast 4.5:1). |
| <a id="paperrust_default_language"></a>`PAPERRUST_DEFAULT_LANGUAGE` | – | Language of the login page and for users who have not picked a language. |
| <a id="paperrust_ui_defaults"></a>`PAPERRUST_UI_DEFAULTS` | – | Defaults for the UI settings of new users. |
| <a id="paperrust_timeline_extract_dates"></a>`PAPERRUST_TIMELINE_EXTRACT_DATES` | `no` | Timeline: look for deadlines and terms in the document text. |
| <a id="paperrust_timeline_date_fields"></a>`PAPERRUST_TIMELINE_DATE_FIELDS` | – | Timeline: which custom fields hold which kind of date (JSON). |

Name and logo are set with [`PAPERRUST_APP_TITLE`](#paperrust_app_title) and
[`PAPERRUST_APP_LOGO`](#paperrust_app_logo). More: [docs/BRANDING.md](../../BRANDING.md) and
[docs/OPERATIONS.md](../../OPERATIONS.md).

### Login

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_password_min_length"></a>`PAPERRUST_PASSWORD_MIN_LENGTH` | `8` | Minimum length of new passwords. |
| <a id="paperrust_mfa_totp_issuer"></a>`PAPERRUST_MFA_TOTP_ISSUER` | `PaperRust` | Name under which PaperRust appears in the authenticator app. |

### Integrity check and sync

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_integrity_incremental"></a>`PAPERRUST_INTEGRITY_INCREMENTAL` | `no` | `yes`: don't recompute unchanged files on every integrity check. |
| <a id="paperrust_integrity_full_hash_days"></a>`PAPERRUST_INTEGRITY_FULL_HASH_DAYS` | `28` | After this many days a file is fully checked again anyway (`0` = always). |
| <a id="paperrust_integrity_threads"></a>`PAPERRUST_INTEGRITY_THREADS` | `0` (automatic, at most 4) | How many files are read at the same time. |
| <a id="paperrust_integrity_report_keep"></a>`PAPERRUST_INTEGRITY_REPORT_KEEP` | `10` | How many check reports (JSON) are kept (`0` = none). |
| <a id="paperrust_sync_sequencer"></a>`PAPERRUST_SYNC_SEQUENCER` | `on` | Run the sync for desktop and mobile apps inside the worker. |
| <a id="paperrust_sync_tombstone_days"></a>`PAPERRUST_SYNC_TOMBSTONE_DAYS` | `180` | How long PaperRust remembers deleted entries for syncing. |

Further switches such as `PAPERRUST_SYNC_INTERVAL`, `PAPERRUST_SYNC_RETENTION_INTERVAL`,
`PAPERRUST_SYNC_BATCH`, `PAPERRUST_SEARCH_ENGINE`,
`PAPERRUST_TEXT_HIT_CACHE`, `PAPERRUST_TEXT_HIT_CACHE_WORKERS` and `PAPERRUST_BULK_UPDATE` are
meant for troubleshooting. In normal operation leave them at their defaults.

Backup and ransomware protection (`PAPERRUST_BACKUP_*`, `PAPERRUST_GUARD_*`) are described on the
[Ransomware protection](ransomware.md) page.

### AI extensions

What the features do is described under [AI](ai.md). Everything starts switched off.

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_ai_mcp_enabled"></a>`PAPERRUST_AI_MCP_ENABLED` | `no` | Turn on the built-in MCP server at `/mcp`. |
| <a id="paperrust_ai_mcp_allowed_origins"></a>`PAPERRUST_AI_MCP_ALLOWED_ORIGINS` | empty | Allowed `Origin` values, separated by commas. Empty = only requests without `Origin`. |
| <a id="paperrust_ai_mcp_rate_limit"></a>`PAPERRUST_AI_MCP_RATE_LIMIT` | `120` | Requests per minute and token on `/mcp`. |
| <a id="paperrust_ai_mcp_max_creates_per_hour"></a>`PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` | `30` | New tags, correspondents etc. per token and hour via MCP. |
| <a id="paperrust_ai_mcp_tool_groups"></a>`PAPERRUST_AI_MCP_TOOL_GROUPS` | `read,write` | Which MCP tool groups are enabled: `read`, `write`, `ingest`. |
| <a id="paperrust_ai_mcp_allow_url_fetch"></a>`PAPERRUST_AI_MCP_ALLOW_URL_FETCH` | `no` | `yes`: `add_document` may fetch files from http(s) addresses. Environment only. |
| <a id="paperrust_ai_mcp_max_upload_bytes"></a>`PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` | `1572864` (1.5 MiB) | Largest file for `add_document`. |
| <a id="paperrust_ai_mcp_max_ingests_per_hour"></a>`PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` | `30` | `add_document` calls per token and hour. |
| <a id="paperrust_ai_match_threshold"></a>`PAPERRUST_AI_MATCH_THRESHOLD` | `0.8` | From this similarity on, an existing object counts as "the same", so no duplicates are created. |
| <a id="paperrust_ai_auto_mode"></a>`PAPERRUST_AI_AUTO_MODE` | `off` | Automation: `off`, `suggest` (only suggestions to review) or `apply` (apply above the confidence threshold). |
| <a id="paperrust_ai_auto_min_confidence"></a>`PAPERRUST_AI_AUTO_MIN_CONFIDENCE` | `0.9` | Minimum confidence for `apply`. |
| <a id="paperrust_ai_auto_max_new_objects"></a>`PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` | `3` | At most this many new objects per run. |
| <a id="paperrust_ai_auto_max_documents_per_run"></a>`PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN` | `50` | Documents per automatic run. |
| <a id="paperrust_ai_auto_trigger_tag"></a>`PAPERRUST_AI_AUTO_TRIGGER_TAG` | empty | Name of a tag that triggers processing of a document. |
| <a id="paperrust_ai_trigger_tag_cron"></a>`PAPERRUST_AI_TRIGGER_TAG_CRON` | `*/5 * * * *` | When to look for that tag (`disable` = never). |
| <a id="paperrust_ai_auto_create_missing"></a>`PAPERRUST_AI_AUTO_CREATE_MISSING` | `no` | Allow the automation to create missing objects. |
| <a id="paperrust_ai_run_max_documents_per_hour"></a>`PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` | `100` | Documents per user and hour via `POST /api/x-ai/run/`. |
| <a id="PAPERRUST_AI_CUSTOM_FIELDS_ENABLED"></a><a id="paperrust_ai_custom_fields_enabled"></a>`PAPERRUST_AI_CUSTOM_FIELDS_ENABLED` | `no` | Let the AI fill custom fields (amount, invoice number …); which fields is chosen under *Settings → AI*. |
| <a id="PAPERRUST_AI_VISION_ENABLED"></a><a id="paperrust_ai_vision_enabled"></a>`PAPERRUST_AI_VISION_ENABLED` | `no` | Make AI text recognition with an image model available. |
| <a id="PAPERRUST_AI_VISION_MODEL"></a><a id="paperrust_ai_vision_model"></a>`PAPERRUST_AI_VISION_MODEL` | empty (= text model) | Name of the image model, e.g. `qwen2.5vl` or `gpt-4o-mini`. |
| <a id="PAPERRUST_AI_VISION_WHEN"></a><a id="paperrust_ai_vision_when"></a>`PAPERRUST_AI_VISION_WHEN` | `difficult` | Legacy (up to 0.1.3), only used without `PAPERRUST_AI_VISION_MODE`: `off`, `difficult` (= whole page if uncertain) or `always`. |
| <a id="PAPERRUST_AI_VISION_MODE"></a><a id="paperrust_ai_vision_mode"></a>`PAPERRUST_AI_VISION_MODE` | empty (= from `PAPERRUST_AI_VISION_WHEN`) | AI transcription: `off` (only "re-read with AI"), `uncertain_lines` (only uncertain spots), `uncertain_pages` (whole page if uncertain) or `always` (every page). Normal text recognition always runs; the selectable PDF text layer stays from it. Documents with AI text get the note "KI-Abschrift". |
| <a id="PAPERRUST_AI_VISION_SENSITIVITY"></a><a id="paperrust_ai_vision_sensitivity"></a>`PAPERRUST_AI_VISION_SENSITIVITY` | `normal` | Sensitivity of the "uncertain" modes: `sparing`, `normal` or `thorough`. |
| <a id="PAPERRUST_AI_VISION_MAX_SIDE"></a><a id="paperrust_ai_vision_max_side"></a>`PAPERRUST_AI_VISION_MAX_SIDE` | `1540` | Longest image side in pixels sent to the image model (512–4096). On a CPU about 3x faster than 2048 with hardly worse results. |
| <a id="PAPERRUST_AI_VISION_MAX_PAGES"></a><a id="paperrust_ai_vision_max_pages"></a>`PAPERRUST_AI_VISION_MAX_PAGES` | `10` | At most this many pages per document. |
| <a id="PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY"></a><a id="paperrust_ai_vision_max_pages_per_day"></a>`PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY` | `500` | At most this many pages per day (`0` = without limit) – protects against costs with cloud providers. |
| <a id="paperrust_ai_api_base"></a>`PAPERRUST_AI_API_BASE` | `http://127.0.0.1:<PAPERRUST_PORT or 8000>` | Address the MCP tools call. It sees the users' API tokens – so only ever point it at PaperRust itself. If PaperRust does not run on port 8000, enter its own address here (e.g. `http://127.0.0.1:8080`). |

### Scanner receiving (SMTP)

A small mail receiver just for scanners and printers ("scan to email"), see
[Connecting scanners](scanner.md). All values take effect after a restart. When receiving is off,
nothing listens.

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_smtp_intake"></a>`PAPERRUST_SMTP_INTAKE` | `off` | `on` turns receiving on. |
| <a id="paperrust_smtp_intake_listen"></a>`PAPERRUST_SMTP_INTAKE_LISTEN` | `0.0.0.0:2525` | Address `IP:PORT`. In Docker, also publish the port (`docker/docker-compose.smtp-intake.yml`). |
| <a id="paperrust_smtp_intake_max_mb"></a>`PAPERRUST_SMTP_INTAKE_MAX_MB` | `50` | Largest message in MB (1–1024). |
| <a id="paperrust_smtp_intake_allowed_ips"></a>`PAPERRUST_SMTP_INTAKE_ALLOWED_IPS` | empty | Allowed senders as IP or network (`192.168.1.0/24`), separated by commas or spaces. Empty = all. |
| <a id="paperrust_smtp_intake_rate_per_minute"></a>`PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE` | `30` | Messages per minute and device. |
| <a id="paperrust_smtp_intake_tls_cert"></a>`PAPERRUST_SMTP_INTAKE_TLS_CERT` / `_TLS_KEY` | empty | Certificate and key (PEM) for STARTTLS. Environment only. Without them the token travels unencrypted – then use it only inside your home network. |
| <a id="paperrust_smtp_intake_require_tls"></a>`PAPERRUST_SMTP_INTAKE_REQUIRE_TLS` | `off` | Allow login only after STARTTLS. |
| <a id="paperrust_smtp_intake_implicit_tls"></a>`PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS` | `off` | Encryption from the first byte (SMTPS, usually port 465) instead of STARTTLS. |

### Desktop app

These variables are usually set by the desktop app itself: `PAPERRUST_DESKTOP_HOME`,
`PAPERRUST_DESKTOP_LOG`, `PAPERRUST_DESKTOP_SERVER_LOG`, `PAPERRUST_DESKTOP_LOG_MAX_MB`,
`PAPERRUST_DESKTOP_PG_BIN`, `PAPERRUST_DESKTOP_SERVER_BIN`, `PAPERRUST_DESKTOP_MIGRATE_DB_BIN`,
`PAPERRUST_DESKTOP_STATIC`, `PAPERRUST_DESKTOP_ROUTES`, `PAPERRUST_DESKTOP_MODE`,
`PAPERRUST_DESKTOP_SERVER_URL`, `PAPERRUST_DESKTOP_ALLOW_HTTP`,
`PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}`, `PAPERRUST_DESKTOP_IMPORT{,_DB,_MEDIA}`,
`PAPERRUST_DESKTOP_UPDATES`, `PAPERRUST_DESKTOP_UPDATE_URL`, `PAPERRUST_DESKTOP_UPDATE_CHANNEL`,
`PAPERRUST_DESKTOP_NO_JOB`. Description: [docs/DESKTOP.md](../../DESKTOP.md).

<a id="paperrust-differences"></a>

## What PaperRust does differently

The most important points for people switching over. Details are given with each setting in the
reference.

- **Database:** PostgreSQL only ([`PAPERRUST_DBENGINE`](#paperrust_dbengine)). An installation on
  SQLite or MariaDB is moved first, see [Switching from paperless-ngx](migration.md).
- **No Redis, no Celery:** the task queue lives in PostgreSQL, the cache in the process or (with
  several processes) in PostgreSQL as well.
- **No Tika, no Gotenberg:** PaperRust reads the text of Office files and emails itself. The PDF
  version (archive PDF) is made by the bundled helper program `paperrust-pdftool`
  ([`PAPERRUST_OFFICE_TO_PDF`](pdf-tools.md#office-beim-einlesen)); without it, the original stays
  searchable and the thumbnail is drawn from the text.
- **OCR:** Tesseract and PDFium instead of OCRmyPDF, Ghostscript and unpaper. Cleaning
  (`PAPERRUST_OCR_CLEAN`) and deskewing (`PAPERRUST_OCR_DESKEW`) are done by PaperRust itself, without
  extra programs. PDF/A archives are cleaned up, colours converted and checked by PaperRust itself;
  if a PDF fails the check (e.g. fonts not embedded in the original), the PDF/A marker is left out.
- **One program:** PaperRust listens on [`PAPERRUST_BIND`](#paperrust_bind). The web server options
  of paperless-ngx (`PAPERLESS_WEBSERVER_WORKERS` etc.) no longer apply.

Full lists of differences per component: `docs/contract/*.md`, section "Known differences".

---

<a id="reference"></a>

# Reference: settings shared with paperless-ngx

Every setting in this part also exists in paperless-ngx, with the same meaning.
There they are called `PAPERLESS_<NAME>`, and that name works in PaperRust too. Most of them can be set in the web
UI (*Settings → System*). Where PaperRust behaves differently, the description says so.

How to write values: yes/no values accept `true`/`false`, `yes`/`no` and `1`/`0`. Schedules are cron
expressions with five fields (minute, hour, day, month, weekday), e.g. `0 3 * * *` for "every day at
3 am"; `disable` turns the task off. Lists are separated by commas.

<a id="required-services"></a><a id="database"></a>

## Database

PaperRust needs **PostgreSQL 14 or newer** (tested with 18). Other databases do not work.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_DBENGINE"></a><a id="paperrust_dbengine"></a><a id="PAPERLESS_DBENGINE"></a><a id="paperless_dbengine"></a>`PAPERRUST_DBENGINE` | `postgresql` | Kind of database. In PaperRust only `postgresql` is possible. In paperless-ngx SQLite was the default when nothing was set – if you come from there, move first ([switching](migration.md)). |
| <a id="PAPERRUST_DBHOST"></a><a id="paperrust_dbhost"></a><a id="PAPERLESS_DBHOST"></a><a id="paperless_dbhost"></a>`PAPERRUST_DBHOST` | – | Host name or address of the PostgreSQL server. |
| <a id="PAPERRUST_DBPORT"></a><a id="paperrust_dbport"></a><a id="PAPERLESS_DBPORT"></a><a id="paperless_dbport"></a>`PAPERRUST_DBPORT` | `5432` | Port of the database server. |
| <a id="PAPERRUST_DBNAME"></a><a id="paperrust_dbname"></a><a id="PAPERLESS_DBNAME"></a><a id="paperless_dbname"></a>`PAPERRUST_DBNAME` | `paperless` | Name of the database. |
| <a id="PAPERRUST_DBUSER"></a><a id="paperrust_dbuser"></a><a id="PAPERLESS_DBUSER"></a><a id="paperless_dbuser"></a>`PAPERRUST_DBUSER` | `paperless` | Database user. |
| <a id="PAPERRUST_DBPASS"></a><a id="paperrust_dbpass"></a><a id="PAPERLESS_DBPASS"></a><a id="paperless_dbpass"></a>`PAPERRUST_DBPASS` | `paperless` | Password of that user. |
| <a id="PAPERRUST_DB_OPTIONS"></a><a id="paperrust_db_options"></a><a id="PAPERLESS_DB_OPTIONS"></a><a id="paperless_db_options"></a>`PAPERRUST_DB_OPTIONS` | – | More connection options as `key=value`, separated by commas. Important keys: `sslmode`, `sslrootcert`, `sslcert`, `sslkey`, `connect_timeout`, `pool.max_size`, `application_name`. These take priority over the single variables below. |
| <a id="PAPERRUST_DBSSLMODE"></a><a id="paperrust_dbsslmode"></a><a id="PAPERLESS_DBSSLMODE"></a><a id="paperless_dbsslmode"></a>`PAPERRUST_DBSSLMODE` | `prefer` | Encryption of the connection (`disable`, `prefer`, `require`, `verify-ca`, `verify-full`). Older way; better use `sslmode=…` in `PAPERRUST_DB_OPTIONS`. |
| <a id="PAPERRUST_DBSSLROOTCERT"></a><a id="paperrust_dbsslrootcert"></a><a id="PAPERLESS_DBSSLROOTCERT"></a><a id="paperless_dbsslrootcert"></a>`PAPERRUST_DBSSLROOTCERT` | – | File with the server's root certificate. Older way; better `sslrootcert=…`. |
| <a id="PAPERRUST_DBSSLCERT"></a><a id="paperrust_dbsslcert"></a><a id="PAPERLESS_DBSSLCERT"></a><a id="paperless_dbsslcert"></a>`PAPERRUST_DBSSLCERT` | – | Client certificate. Older way; better `sslcert=…`. |
| <a id="PAPERRUST_DBSSLKEY"></a><a id="paperrust_dbsslkey"></a><a id="PAPERLESS_DBSSLKEY"></a><a id="paperless_dbsslkey"></a>`PAPERRUST_DBSSLKEY` | – | Key of the client certificate. Older way; better `sslkey=…`. |
| <a id="PAPERRUST_DB_TIMEOUT"></a><a id="paperrust_db_timeout"></a><a id="PAPERLESS_DB_TIMEOUT"></a><a id="paperless_db_timeout"></a>`PAPERRUST_DB_TIMEOUT` | – | Time limit in seconds for connecting. Older way; better `connect_timeout=…`. |
| <a id="PAPERRUST_DB_POOLSIZE"></a><a id="paperrust_db_poolsize"></a><a id="PAPERLESS_DB_POOLSIZE"></a><a id="paperless_db_poolsize"></a>`PAPERRUST_DB_POOLSIZE` | – (then [`PAPERRUST_DB_MAX_CONNECTIONS`](#paperrust_db_max_connections)) | Maximum number of simultaneous connections. Older way; better `pool.max_size=…`. |

paperless-ngx dropped the older variables (`DBSSL*`, `DB_TIMEOUT`, `DB_POOLSIZE`) in version 3.
PaperRust still reads them so old configurations don't break.

Example of an encrypted connection with your own certificate authority:

```sh
PAPERRUST_DB_OPTIONS="sslmode=verify-full,sslrootcert=/certs/ca.pem,connect_timeout=10"
```

<a id="message-broker"></a>

## Cache and Redis

PaperRust does not need Redis. When switching from paperless-ngx, the Redis/Valkey service can be
removed afterwards. When several PaperRust processes serve the same installation (template
`docker/docker-compose.multiprocess.yml`), they share the cache through a table in the same
PostgreSQL database.

| Variable | Default | Meaning |
|---|---|---|
| <a id="paperrust_cache"></a>`PAPERRUST_CACHE` | `local` | `local` = cache inside the process. `postgres` = shared by all processes in the table `paperrust_cache` (needed with separate web and worker processes or several instances). Takes effect after a restart. |
| <a id="PAPERRUST_REDIS"></a><a id="paperrust_redis"></a><a id="PAPERLESS_REDIS"></a><a id="paperless_redis"></a>`PAPERRUST_REDIS` | – | **No effect.** Taken over from paperless-ngx: Redis is no longer connected, the log only shows a hint. Can be removed. |
| <a id="PAPERRUST_REDIS_PREFIX"></a><a id="paperrust_redis_prefix"></a><a id="PAPERLESS_REDIS_PREFIX"></a><a id="paperless_redis_prefix"></a>`PAPERRUST_REDIS_PREFIX` | – | **No effect.** |
| <a id="PAPERRUST_CACHE_BACKEND"></a><a id="paperrust_cache_backend"></a><a id="PAPERLESS_CACHE_BACKEND"></a><a id="paperless_cache_backend"></a>`PAPERRUST_CACHE_BACKEND` | – | Only read when `PAPERRUST_CACHE` is not set: `…RedisCache` or `…DatabaseCache` act like `PAPERRUST_CACHE=postgres`, anything else like `local`. |

<a id="paths-and-folders"></a>

## Paths and folders

Relative default paths are relative to [`PAPERRUST_BASE_DIR`](#paperrust_base_dir).

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_CONSUMPTION_DIR"></a><a id="paperrust_consumption_dir"></a><a id="PAPERLESS_CONSUMPTION_DIR"></a><a id="paperless_consumption_dir"></a>`PAPERRUST_CONSUMPTION_DIR` | `consume/` | Consumption folder. Whatever lands here is consumed by PaperRust. The folder must exist; PaperRust needs read and write access. |
| <a id="PAPERRUST_DATA_DIR"></a><a id="paperrust_data_dir"></a><a id="PAPERLESS_DATA_DIR"></a><a id="paperless_data_dir"></a>`PAPERRUST_DATA_DIR` | `data/` | Search index, model of the automatic matching, logs and other working data. |
| <a id="PAPERRUST_MEDIA_ROOT"></a><a id="paperrust_media_root"></a><a id="PAPERLESS_MEDIA_ROOT"></a><a id="paperless_media_root"></a>`PAPERRUST_MEDIA_ROOT` | `media/` | The documents themselves: originals, archive versions, thumbnails. Include it in your backup! |
| <a id="PAPERRUST_SCRATCH_DIR"></a><a id="paperrust_scratch_dir"></a><a id="PAPERLESS_SCRATCH_DIR"></a><a id="paperless_scratch_dir"></a>`PAPERRUST_SCRATCH_DIR` | `/tmp/paperrust` | Folder for temporary files while consuming, editing and exporting. |
| <a id="PAPERRUST_STATICDIR"></a><a id="paperrust_staticdir"></a><a id="PAPERLESS_STATICDIR"></a><a id="paperless_staticdir"></a>`PAPERRUST_STATICDIR` | `static/` | Static files of the old Angular UI. The new UI lives next to it (see [`PAPERRUST_LIGHT_UI_DIR`](#paperrust_light_ui_dir)). |
| <a id="PAPERRUST_EMPTY_TRASH_DIR"></a><a id="paperrust_empty_trash_dir"></a><a id="PAPERLESS_EMPTY_TRASH_DIR"></a><a id="paperless_empty_trash_dir"></a>`PAPERRUST_EMPTY_TRASH_DIR` | empty | If a folder is set, PaperRust moves files there on final deletion instead of deleting them. The folder must already exist. Older name: `PAPERRUST_TRASH_DIR`. |
| <a id="PAPERRUST_FILENAME_FORMAT"></a><a id="paperrust_filename_format"></a><a id="PAPERLESS_FILENAME_FORMAT"></a><a id="paperless_filename_format"></a>`PAPERRUST_FILENAME_FORMAT` | – (off) | Template for folders and file names in the media folder, e.g. `{{ created_year }}/{{ correspondent }}/{{ title }}`. Without a template, files are just named by their number. See [File names and storage paths](storage-paths.md#file-name-handling). |
| <a id="PAPERRUST_FILENAME_FORMAT_REMOVE_NONE"></a><a id="paperrust_filename_format_remove_none"></a><a id="PAPERLESS_FILENAME_FORMAT_REMOVE_NONE"></a><a id="paperless_filename_format_remove_none"></a>`PAPERRUST_FILENAME_FORMAT_REMOVE_NONE` | `false` | Leave empty placeholders out entirely instead of writing `none` into the path. |
| <a id="PAPERRUST_LOGGING_DIR"></a><a id="paperrust_logging_dir"></a><a id="PAPERLESS_LOGGING_DIR"></a><a id="paperless_logging_dir"></a>`PAPERRUST_LOGGING_DIR` | `data/log/` | Folder for the log files. |
| <a id="PAPERRUST_MODEL_FILE"></a><a id="paperrust_model_file"></a><a id="PAPERLESS_MODEL_FILE"></a><a id="paperless_model_file"></a>`PAPERRUST_MODEL_FILE` | `data/classification_model.pickle` | Model of paperless-ngx. PaperRust does not use it but its own, see [`PAPERRUST_CLASSIFIER_MODEL_FILE`](#paperrust_classifier_model_file). |

After changing the file name template, run `document_renamer`; otherwise existing files are only
renamed the next time the document changes.

<a id="logging"></a>

## Logging

PaperRust writes `paperrust.log` (everything; an older `paperless.log` stays visible in the logs view) and `mail.log` (mail fetching) to
[`PAPERRUST_LOGGING_DIR`](#paperrust_logging_dir). How detailed is controlled by
[`PAPERRUST_LOG`](#paperrust_log).

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_LOGROTATE_MAX_SIZE"></a><a id="paperrust_logrotate_max_size"></a><a id="PAPERLESS_LOGROTATE_MAX_SIZE"></a><a id="paperless_logrotate_max_size"></a>`PAPERRUST_LOGROTATE_MAX_SIZE` | `1048576` (1 MiB) | From this size in bytes on, a new log file is started. |
| <a id="PAPERRUST_LOGROTATE_MAX_BACKUPS"></a><a id="paperrust_logrotate_max_backups"></a><a id="PAPERLESS_LOGROTATE_MAX_BACKUPS"></a><a id="paperless_logrotate_max_backups"></a>`PAPERRUST_LOGROTATE_MAX_BACKUPS` | `20` | How many old log files are kept. |

<a id="hosting-and-security"></a><a id="hosting--security"></a>

## Hosting and security

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_SECRET_KEY"></a><a id="paperrust_secret_key"></a><a id="PAPERLESS_SECRET_KEY"></a><a id="paperless_secret_key"></a>`PAPERRUST_SECRET_KEY` | – (required) | Secret key for sessions, signatures and the encrypted settings. Without it – or with the placeholder `change-me` – PaperRust does not start. Keep the previous value when switching over, otherwise every logged-in user is logged out. |
| <a id="PAPERRUST_URL"></a><a id="paperrust_url"></a><a id="PAPERLESS_URL"></a><a id="paperless_url"></a>`PAPERRUST_URL` | – | Public address without a trailing `/` and without a path, e.g. `https://documents.example.org`. Allowed hosts, CORS and CSRF (the three variables below) follow from it. Also used for links in emails and for OAuth. |
| <a id="PAPERRUST_CSRF_TRUSTED_ORIGINS"></a><a id="paperrust_csrf_trusted_origins"></a><a id="PAPERLESS_CSRF_TRUSTED_ORIGINS"></a><a id="paperless_csrf_trusted_origins"></a>`PAPERRUST_CSRF_TRUSTED_ORIGINS` | – (from `PAPERRUST_URL`) | Addresses from which writing requests (forms, POST) are accepted. Only needed if PaperRust can be reached under several addresses. |
| <a id="PAPERRUST_ALLOWED_HOSTS"></a><a id="paperrust_allowed_hosts"></a><a id="PAPERLESS_ALLOWED_HOSTS"></a><a id="paperless_allowed_hosts"></a>`PAPERRUST_ALLOWED_HOSTS` | `*`, as long as `PAPERRUST_URL` is also missing | Host names PaperRust answers to. If only `PAPERRUST_URL` is set, its host name applies. Unless the list is `*`, `localhost`, `127.0.0.1` and `[::1]` are always allowed (access from the same machine, health check). Otherwise an empty list adds nothing; `*` allows any host. |
| <a id="PAPERRUST_CORS_ALLOWED_HOSTS"></a><a id="paperrust_cors_allowed_hosts"></a><a id="PAPERLESS_CORS_ALLOWED_HOSTS"></a><a id="paperless_cors_allowed_hosts"></a>`PAPERRUST_CORS_ALLOWED_HOSTS` | `http://localhost:8000` | Other websites allowed to call the API from the browser. |
| <a id="PAPERRUST_TRUSTED_PROXIES"></a><a id="paperrust_trusted_proxies"></a><a id="PAPERLESS_TRUSTED_PROXIES"></a><a id="paperless_trusted_proxies"></a>`PAPERRUST_TRUSTED_PROXIES` | – | Addresses of reverse proxies whose forwarded client address PaperRust believes. Important for correct addresses in the log (e.g. for fail2ban). Environment only. |
| <a id="PAPERRUST_ALLAUTH_TRUSTED_PROXY_COUNT"></a><a id="paperrust_allauth_trusted_proxy_count"></a><a id="PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT"></a><a id="paperless_allauth_trusted_proxy_count"></a>`PAPERRUST_ALLAUTH_TRUSTED_PROXY_COUNT` | number of entries in `PAPERRUST_TRUSTED_PROXIES` | How many proxy hops are in `X-Forwarded-For` (hops, not addresses). Decides which address counts as the client, e.g. for login throttling. Environment only. |
| <a id="PAPERRUST_ALLAUTH_TRUSTED_CLIENT_IP_HEADER"></a><a id="paperrust_allauth_trusted_client_ip_header"></a><a id="PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER"></a><a id="paperless_allauth_trusted_client_ip_header"></a>`PAPERRUST_ALLAUTH_TRUSTED_CLIENT_IP_HEADER` | – | Header in which the proxy sends the real client address, e.g. `X-Real-IP` or `CF-Connecting-IP`. Takes priority over the proxy count. Environment only. |
| <a id="PAPERRUST_FORCE_SCRIPT_NAME"></a><a id="paperrust_force_script_name"></a><a id="PAPERLESS_FORCE_SCRIPT_NAME"></a><a id="paperless_force_script_name"></a>`PAPERRUST_FORCE_SCRIPT_NAME` | – (at `/`) | Sub-path if PaperRust does not run at the root, e.g. `/documents` (no trailing `/`). |
| <a id="PAPERRUST_STATIC_URL"></a><a id="paperrust_static_url"></a><a id="PAPERLESS_STATIC_URL"></a><a id="paperless_static_url"></a>`PAPERRUST_STATIC_URL` | `/static/` | Address of the static files of the old UI (with trailing `/`). Set it to match a sub-path, e.g. `/documents/static/`. |
| <a id="PAPERRUST_AUTO_LOGIN_USERNAME"></a><a id="paperrust_auto_login_username"></a><a id="PAPERLESS_AUTO_LOGIN_USERNAME"></a><a id="paperless_auto_login_username"></a>`PAPERRUST_AUTO_LOGIN_USERNAME` | – (off) | Every visitor is automatically logged in as this user. Only for a machine nobody else can reach – never make it reachable from the internet. Environment only. |
| <a id="PAPERRUST_ADMIN_USER"></a><a id="paperrust_admin_user"></a><a id="PAPERLESS_ADMIN_USER"></a><a id="paperless_admin_user"></a>`PAPERRUST_ADMIN_USER` | – | Creates a superuser with this name at start-up if it does not exist yet. Needs `PAPERRUST_ADMIN_PASSWORD`. Existing users stay unchanged. |
| <a id="PAPERRUST_ADMIN_MAIL"></a><a id="paperrust_admin_mail"></a><a id="PAPERLESS_ADMIN_MAIL"></a><a id="paperless_admin_mail"></a>`PAPERRUST_ADMIN_MAIL` | `root@localhost` | Email address of that superuser. |
| <a id="PAPERRUST_ADMIN_PASSWORD"></a><a id="paperrust_admin_password"></a><a id="PAPERLESS_ADMIN_PASSWORD"></a><a id="paperless_admin_password"></a>`PAPERRUST_ADMIN_PASSWORD` | – | Password of that superuser. Remove it from the configuration after the first start. |
| <a id="PAPERRUST_COOKIE_PREFIX"></a><a id="paperrust_cookie_prefix"></a><a id="PAPERLESS_COOKIE_PREFIX"></a><a id="paperless_cookie_prefix"></a>`PAPERRUST_COOKIE_PREFIX` | empty | Prefix for cookie names. Needed when two installations run under the same host name. After changing it you have to log in again. |
| <a id="PAPERRUST_ENABLE_HTTP_REMOTE_USER"></a><a id="paperrust_enable_http_remote_user"></a><a id="PAPERLESS_ENABLE_HTTP_REMOTE_USER"></a><a id="paperless_enable_http_remote_user"></a>`PAPERRUST_ENABLE_HTTP_REMOTE_USER` | `false` | Log in through an SSO proxy in front that sends the user name in a header. See the warning below. Environment only. |
| <a id="PAPERRUST_ENABLE_HTTP_REMOTE_USER_API"></a><a id="paperrust_enable_http_remote_user_api"></a><a id="PAPERLESS_ENABLE_HTTP_REMOTE_USER_API"></a><a id="paperless_enable_http_remote_user_api"></a>`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API` | `false` | The same for API calls. Environment only. |
| <a id="PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME"></a><a id="paperrust_http_remote_user_header_name"></a><a id="PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME"></a><a id="paperless_http_remote_user_header_name"></a>`PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME` | `HTTP_REMOTE_USER` | Name of that header, written with `HTTP_` in front and underscores (`X-Auth-User` becomes `HTTP_X_AUTH_USER`). Environment only. |
| <a id="PAPERRUST_LOGOUT_REDIRECT_URL"></a><a id="paperrust_logout_redirect_url"></a><a id="PAPERLESS_LOGOUT_REDIRECT_URL"></a><a id="paperless_logout_redirect_url"></a>`PAPERRUST_LOGOUT_REDIRECT_URL` | – | Page you land on after logging out, e.g. the logout page of the SSO service. |
| <a id="PAPERRUST_USE_X_FORWARD_HOST"></a><a id="paperrust_use_x_forward_host"></a><a id="PAPERLESS_USE_X_FORWARD_HOST"></a><a id="paperless_use_x_forward_host"></a>`PAPERRUST_USE_X_FORWARD_HOST` | `false` | Take the host name from the proxy's `X-Forwarded-Host` header. Environment only. |
| <a id="PAPERRUST_USE_X_FORWARD_PORT"></a><a id="paperrust_use_x_forward_port"></a><a id="PAPERLESS_USE_X_FORWARD_PORT"></a><a id="paperless_use_x_forward_port"></a>`PAPERRUST_USE_X_FORWARD_PORT` | `false` | Take the port from `X-Forwarded-Port`. Environment only. |
| <a id="PAPERRUST_PROXY_SSL_HEADER"></a><a id="paperrust_proxy_ssl_header"></a><a id="PAPERLESS_PROXY_SSL_HEADER"></a><a id="paperless_proxy_ssl_header"></a>`PAPERRUST_PROXY_SSL_HEADER` | – | How PaperRust recognizes that the connection to the proxy was encrypted, as a JSON list, e.g. `["HTTP_X_FORWARDED_PROTO", "https"]`. Only set it if the proxy always overwrites this header from outside. Environment only. |
| <a id="PAPERRUST_EMAIL_CERTIFICATE_LOCATION"></a><a id="paperrust_email_certificate_location"></a><a id="PAPERLESS_EMAIL_CERTIFICATE_LOCATION"></a><a id="paperless_email_certificate_location"></a>`PAPERRUST_EMAIL_CERTIFICATE_LOCATION` | – | Extra certificate file (e.g. self-signed) that PaperRust trusts when fetching from mail servers. |
| <a id="PAPERRUST_DEBUG"></a><a id="paperrust_debug"></a><a id="PAPERLESS_DEBUG"></a><a id="paperless_debug"></a>`PAPERRUST_DEBUG` | `false` | Debug mode. Always keep it off in production. Environment only. |

> [!WARNING]
> With `PAPERRUST_ENABLE_HTTP_REMOTE_USER`, PaperRust believes anyone who sends the header. That is
> only safe if PaperRust can be reached exclusively through the proxy and the proxy always removes a
> header sent from outside. Otherwise anyone can pose as any user.

<a id="authentication"></a><a id="authentication--sso"></a>

### Login and SSO

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_ACCOUNT_ALLOW_SIGNUPS"></a><a id="paperrust_account_allow_signups"></a><a id="PAPERLESS_ACCOUNT_ALLOW_SIGNUPS"></a><a id="paperless_account_allow_signups"></a>`PAPERRUST_ACCOUNT_ALLOW_SIGNUPS` | `false` | Visitors may create an account themselves. |
| <a id="PAPERRUST_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperrust_account_default_groups"></a><a id="PAPERLESS_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperless_account_default_groups"></a>`PAPERRUST_ACCOUNT_DEFAULT_GROUPS` | – | Groups that self-created accounts are put in. The groups must already exist. |
| <a id="PAPERRUST_SOCIALACCOUNT_PROVIDERS"></a><a id="paperrust_socialaccount_providers"></a><a id="PAPERLESS_SOCIALACCOUNT_PROVIDERS"></a><a id="paperless_socialaccount_providers"></a>`PAPERRUST_SOCIALACCOUNT_PROVIDERS` | – (no provider) | Login through external providers (OpenID Connect, e.g. Authentik, Keycloak, as well as GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea), as JSON. Example below. |
| <a id="PAPERRUST_SOCIAL_AUTO_SIGNUP"></a><a id="paperrust_social_auto_signup"></a><a id="PAPERLESS_SOCIAL_AUTO_SIGNUP"></a><a id="paperless_social_auto_signup"></a>`PAPERRUST_SOCIAL_AUTO_SIGNUP` | `false` | On the first login through a provider, create the account right away without a form. |
| <a id="PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS"></a><a id="paperrust_socialaccount_allow_signups"></a><a id="PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS"></a><a id="paperless_socialaccount_allow_signups"></a>`PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS` | `true` | Allow new accounts through a provider. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS"></a><a id="paperrust_social_account_sync_groups"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS"></a><a id="paperless_social_account_sync_groups"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS` | `false` | Take over the groups from the provider on every login. Groups with the same names must exist in PaperRust. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM"></a><a id="paperrust_social_account_sync_groups_claim"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM"></a><a id="paperless_social_account_sync_groups_claim"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM` | `groups` | In which field (claim) the provider sends the groups. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP"></a><a id="paperrust_social_account_sync_superuser_group"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP"></a><a id="paperless_social_account_sync_superuser_group"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP` | – | Whoever is in this group at the provider becomes superuser. If the group is missing at the next login, the right is taken away again. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP"></a><a id="paperrust_social_account_sync_staff_group"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP"></a><a id="paperless_social_account_sync_staff_group"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP` | – | The same for staff status (admin rights without superuser). |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR"></a><a id="paperrust_social_account_mfa_trusted_amr"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR"></a><a id="paperless_social_account_mfa_trusted_amr"></a>`PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR` | `mfa` | If the provider reports one of these values in the `amr` field, the second factor counts as done and PaperRust does not ask for it again. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperrust_social_account_default_groups"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperless_social_account_default_groups"></a>`PAPERRUST_SOCIAL_ACCOUNT_DEFAULT_GROUPS` | – | Groups for accounts newly created through a provider. The groups must exist. |
| <a id="PAPERRUST_ACCOUNT_DEFAULT_HTTP_PROTOCOL"></a><a id="paperrust_account_default_http_protocol"></a><a id="PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL"></a><a id="paperless_account_default_http_protocol"></a>`PAPERRUST_ACCOUNT_DEFAULT_HTTP_PROTOCOL` | `https` | Protocol in generated links, e.g. for the provider's return address. |
| <a id="PAPERRUST_ACCOUNT_EMAIL_VERIFICATION"></a><a id="paperrust_account_email_verification"></a><a id="PAPERLESS_ACCOUNT_EMAIL_VERIFICATION"></a><a id="paperless_account_email_verification"></a>`PAPERRUST_ACCOUNT_EMAIL_VERIFICATION` | `optional` (`none` without mail server) | Whether new accounts must confirm their email address: `mandatory`, `optional` or `none`. `mandatory` needs a working mail server and an email address on every account, otherwise the login waits for the confirmation. |
| <a id="PAPERRUST_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS"></a><a id="paperrust_account_email_unknown_accounts"></a><a id="PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS"></a><a id="paperless_account_email_unknown_accounts"></a>`PAPERRUST_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` | `true` | Also send an email to addresses without an account, e.g. when resetting a password. `false` does not reveal that the address is unknown, but also sends no email. |
| <a id="PAPERRUST_DISABLE_REGULAR_LOGIN"></a><a id="paperrust_disable_regular_login"></a><a id="PAPERLESS_DISABLE_REGULAR_LOGIN"></a><a id="paperless_disable_regular_login"></a>`PAPERRUST_DISABLE_REGULAR_LOGIN` | `false` | Turn off login with user name and password in the UI, so only SSO remains. The API still accepts passwords and tokens. |
| <a id="PAPERRUST_REDIRECT_LOGIN_TO_SSO"></a><a id="paperrust_redirect_login_to_sso"></a><a id="PAPERLESS_REDIRECT_LOGIN_TO_SSO"></a><a id="paperless_redirect_login_to_sso"></a>`PAPERRUST_REDIRECT_LOGIN_TO_SSO` | `false` | The login page immediately redirects to the first SSO provider. |
| <a id="PAPERRUST_ACCOUNT_SESSION_REMEMBER"></a><a id="paperrust_account_session_remember"></a><a id="PAPERLESS_ACCOUNT_SESSION_REMEMBER"></a><a id="paperless_account_session_remember"></a>`PAPERRUST_ACCOUNT_SESSION_REMEMBER` | `true` | `false`: the login ends when the browser is closed. Otherwise `PAPERRUST_SESSION_COOKIE_AGE` applies. |
| <a id="PAPERRUST_SESSION_COOKIE_AGE"></a><a id="paperrust_session_cookie_age"></a><a id="PAPERLESS_SESSION_COOKIE_AGE"></a><a id="paperless_session_cookie_age"></a>`PAPERRUST_SESSION_COOKIE_AGE` | `1814400` (3 weeks) | How long you stay logged in, in seconds. |
| <a id="PAPERRUST_SESSION_CLEANUP_TASK_CRON"></a><a id="paperrust_session_cleanup_task_cron"></a><a id="PAPERLESS_SESSION_CLEANUP_TASK_CRON"></a><a id="paperless_session_cleanup_task_cron"></a>`PAPERRUST_SESSION_CLEANUP_TASK_CRON` | `0 3 * * *` (daily 3 am) | When expired logins are deleted from the database. |
| <a id="PAPERRUST_TOKEN_THROTTLE_RATE"></a><a id="paperrust_token_throttle_rate"></a><a id="PAPERLESS_TOKEN_THROTTLE_RATE"></a><a id="paperless_token_throttle_rate"></a>`PAPERRUST_TOKEN_THROTTLE_RATE` | `5/min` | How often per period an API token may be fetched with a password (protection against password guessing), e.g. `20/hour`. |

Example for an OpenID Connect provider (return address to enter at the provider:
`https://<PAPERRUST_URL>/accounts/oidc/<provider_id>/login/callback/`):

```sh
PAPERRUST_APPS="allauth.socialaccount.providers.openid_connect"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"openid_connect": {"APPS": [{"provider_id": "authentik", "name": "Authentik", "client_id": "<id>", "secret": "<secret>", "settings": {"server_url": "https://auth.example.org/application/o/paperrust/.well-known/openid-configuration"}}]}}'
```

<a id="ocr"></a><a id="ocr-settings"></a>

## OCR

PaperRust recognizes text with Tesseract (on Windows optionally with the built-in Windows text
recognition, see [`PAPERRUST_OCR_ENGINE`](#paperrust_ocr_engine)) and draws pages with PDFium. The
options of this group can also be set on the *Settings → Configuration* page; values set there take
priority over the environment.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_OCR_LANGUAGE"></a><a id="paperrust_ocr_language"></a><a id="PAPERLESS_OCR_LANGUAGE"></a><a id="paperless_ocr_language"></a>`PAPERRUST_OCR_LANGUAGE` | `deu+eng` | Language(s) of the documents as three-letter Tesseract code, several with `+`, e.g. `fra+eng`. The language data must be installed. Default `deu+eng` (paperless-ngx: `eng`): a second language costs only 3–7 % time, while `eng` alone loses umlauts and ß in German text. If the German data is missing for the default, PaperRust uses `eng`. |
| <a id="PAPERRUST_OCR_LANGUAGE_AUTO"></a><a id="paperrust_ocr_language_auto"></a>`PAPERRUST_OCR_LANGUAGE_AUTO` | `false` | Automatic language: after recognition PaperRust detects the language of the text. If it differs from the setting (e.g. French with `deu+eng`) and the language data is installed, the document is recognized once more with that language. Costs time only for such documents (a second recognition); acts only from about 200 letters and on a confident detection. Measured: French with `deu+eng` 93 % of the words, with `fra` 100 %. |
| <a id="PAPERRUST_OCR_MODE"></a><a id="paperrust_ocr_mode"></a><a id="PAPERLESS_OCR_MODE"></a><a id="paperless_ocr_mode"></a>`PAPERRUST_OCR_MODE` | `auto` | When to recognize: `auto` = only pages without text; `redo` = replace an existing text layer with a new one; `force` = treat all pages as images and recognize again (changes the pages); `off` = never, just take existing text. |
| <a id="PAPERRUST_ARCHIVE_FILE_GENERATION"></a><a id="paperrust_archive_file_generation"></a><a id="PAPERLESS_ARCHIVE_FILE_GENERATION"></a><a id="paperless_archive_file_generation"></a>`PAPERRUST_ARCHIVE_FILE_GENERATION` | `auto` | When an additional searchable archive version (PDF/A) is made: `auto` = for scans and images; `always`; `never`. |
| <a id="PAPERRUST_OCR_CLEAN"></a><a id="paperrust_ocr_clean"></a><a id="PAPERLESS_OCR_CLEAN"></a><a id="paperless_ocr_clean"></a>`PAPERRUST_OCR_CLEAN` | `clean` | Clean up scans before recognition: specks, dark borders/scanner edges, punch holes and faint gray areas become white (built-in replacement for `unpaper`). `clean`: only the image used for recognition, the archive keeps the original. `clean-final`: the cleaned image also goes into the archive PDF. `none`: off. With OCR mode `redo`, `clean-final` becomes `clean`. |
| <a id="PAPERRUST_OCR_DESKEW"></a><a id="paperrust_ocr_deskew"></a><a id="PAPERLESS_OCR_DESKEW"></a><a id="paperless_ocr_deskew"></a>`PAPERRUST_OCR_DESKEW` | `true` | Straighten pages that went in crooked (up to ±15°). The page is rotated before recognition; the straightened image also goes into the archive PDF. Pages that are already straight (below 0.1°) stay unchanged. Has no effect with OCR mode `redo`. |
| <a id="PAPERRUST_OCR_ROTATE_PAGES"></a><a id="paperrust_ocr_rotate_pages"></a><a id="PAPERLESS_OCR_ROTATE_PAGES"></a><a id="paperless_ocr_rotate_pages"></a>`PAPERRUST_OCR_ROTATE_PAGES` | `true` | Turn pages that are upside down or sideways the right way up. Needs the Tesseract data `osd`. |
| <a id="PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD"></a><a id="paperrust_ocr_rotate_pages_threshold"></a><a id="PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD"></a><a id="paperless_ocr_rotate_pages_threshold"></a>`PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD` | `12` | How sure the detection has to be before a page is turned. Higher = more careful (about 15), lower = more eager (about 2). |
| <a id="PAPERRUST_OCR_OUTPUT_TYPE"></a><a id="paperrust_ocr_output_type"></a><a id="PAPERLESS_OCR_OUTPUT_TYPE"></a><a id="paperless_ocr_output_type"></a>`PAPERRUST_OCR_OUTPUT_TYPE` | `pdfa` | Format of the archive version: `pdf`, `pdfa` (means PDF/A-2b), `pdfa-1`, `pdfa-2` or `pdfa-3`. PaperRust writes PDF/A without Ghostscript: embedded text-layer font, sRGB colour profile, XMP metadata, JavaScript/forbidden actions/additional actions removed, annotations made printable, embedded files removed for PDF/A-1/-2, transparency flattened for PDF/A-1. Afterwards it checks the file itself against the main PDF/A rules. If it fails (usually: fonts of the original are not embedded), the file stays a normal PDF without the PDF/A marker; the log names the reason. |
| <a id="PAPERRUST_OCR_PAGES"></a><a id="paperrust_ocr_pages"></a><a id="PAPERLESS_OCR_PAGES"></a><a id="paperless_ocr_pages"></a>`PAPERRUST_OCR_PAGES` | `0` (all) | Only recognize the first N pages. Saves time with long documents. |
| <a id="PAPERRUST_OCR_IMAGE_DPI"></a><a id="paperrust_ocr_image_dpi"></a><a id="PAPERLESS_OCR_IMAGE_DPI"></a><a id="paperless_ocr_image_dpi"></a>`PAPERRUST_OCR_IMAGE_DPI` | – (estimated) | Resolution for images that carry no such information. Without a value, PaperRust estimates it from the image size. |
| <a id="PAPERRUST_OCR_MAX_IMAGE_PIXELS"></a><a id="paperrust_ocr_max_image_pixels"></a><a id="PAPERLESS_OCR_MAX_IMAGE_PIXELS"></a><a id="paperless_ocr_max_image_pixels"></a>`PAPERRUST_OCR_MAX_IMAGE_PIXELS` | – (standard limit) | Largest image area in pixels that is recognized. Above it there is a warning; from twice the area the image is skipped. `0` removes the limit. Protects against deliberately huge files. |
| <a id="PAPERRUST_OCR_COLOR_CONVERSION_STRATEGY"></a><a id="paperrust_ocr_color_conversion_strategy"></a><a id="PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY"></a><a id="paperless_ocr_color_conversion_strategy"></a>`PAPERRUST_OCR_COLOR_CONVERSION_STRATEGY` | `RGB` | Colour conversion for PDF/A (`CMYK`, `Gray`, `LeaveColorUnchanged`, `RGB`, `UseDeviceIndependentColor`), applied to images and vector colours of the archive version. `Gray` = everything gray, `RGB` = CMYK becomes RGB, `UseDeviceIndependentColor` = images with a colour profile, `LeaveColorUnchanged` = only CMYK becomes RGB (PDF/A requires it). `CMYK` is not possible for PDF/A (no CMYK profile) and then acts like `RGB` (warning in the log). |
| <a id="PAPERRUST_OCR_USER_ARGS"></a><a id="paperrust_ocr_user_args"></a><a id="PAPERLESS_OCR_USER_ARGS"></a><a id="paperless_ocr_user_args"></a>`PAPERRUST_OCR_USER_ARGS` | – | Extra OCRmyPDF options as a JSON object, e.g. `{"optimize": 2, "jpeg_quality": 60}`. Effective: `optimize` (0–3, default 1: lossless, 1-bit images as JBIG2; 2/3: additionally JPEG quality and colour reduction), `jpeg_quality`, `png_quality`, `pdfa_image_compression` (`jpeg`/`lossless`/`auto`), `title`, `author`, `subject`, `keywords`, `mode`, `tesseract_pagesegmode`, `tesseract_oem`, `tesseract_thresholding`, `tesseract_config`, `user_words`, `user_patterns`, `tesseract_timeout`, `tesseract_non_ocr_timeout`, `tesseract_downsample_large_images`/`tesseract_downsample_above`, `max_ocr_image_mpixels`, `remove_vectors`, `ocr_engine` (`none` = no text recognition) plus the options PaperRust sets itself. No effect (one warning per option): `jbig2_lossy` (deprecated in OCRmyPDF too), `unpaper_args`, `remove_background`, `fast_web_view`, plugins. |

Other OCR differences: PaperRust reads HEIC images through `paperrust-pdftool` (except 4:2:2/4:4:4
photos and image sequences), and the text selection in the archive PDF may sit slightly off. Full list: `docs/contract/pl-ocr.md`, "Known differences".

<a id="software_tweaks"></a><a id="software-tweaks"></a>

## Performance, schedules and search

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_TASK_WORKERS"></a><a id="paperrust_task_workers"></a><a id="PAPERLESS_TASK_WORKERS"></a><a id="paperless_task_workers"></a>`PAPERRUST_TASK_WORKERS` | `auto` | How many background tasks (consumption, mail, index, training) run at the same time – for a batch import, how many documents are consumed at once. Default `auto`: PaperRust picks the value from cores and memory – cores ÷ 2 (rounded up; with `PAPERRUST_THREADS_PER_WORKER` set: cores ÷ that value), at most 4, fewer when little memory is free. The cores are shared: a document that runs alone gets all cores and is done as fast as with `1`; when several run, they share the cores page by page. Large batches get faster (NAS with 3 cores: 30 scans in 55 s instead of 95 s). `1` restores the paperless-ngx behaviour: one document after the other. Any other number is used unchanged. |
| <a id="PAPERRUST_THREADS_PER_WORKER"></a><a id="paperrust_threads_per_worker"></a><a id="PAPERLESS_THREADS_PER_WORKER"></a><a id="paperless_threads_per_worker"></a>`PAPERRUST_THREADS_PER_WORKER` | all cores | The most pages of one document recognized at the same time. All documents together share as many simultaneous pages as the machine has cores (fewer when little memory is free; in a container its CPU and memory limits count) – a document running alone uses all cores, several at once share them. A smaller value limits each document. |
| <a id="PAPERRUST_WORKER_TIMEOUT"></a><a id="paperrust_worker_timeout"></a><a id="PAPERLESS_WORKER_TIMEOUT"></a><a id="paperless_worker_timeout"></a>`PAPERRUST_WORKER_TIMEOUT` | `1800` s | After this time a task is aborted. Raise it on slow machines with large documents. |
| <a id="PAPERRUST_TIME_ZONE"></a><a id="paperrust_time_zone"></a><a id="PAPERLESS_TIME_ZONE"></a><a id="paperless_time_zone"></a>`PAPERRUST_TIME_ZONE` | `UTC` | Time zone, e.g. `Europe/Berlin`. Affects dates and schedules. |
| <a id="PAPERRUST_CLASSIFIER_MATCH_THRESHOLD"></a><a id="paperrust_classifier_match_threshold"></a><a id="PAPERLESS_CLASSIFIER_MATCH_THRESHOLD"></a><a id="paperless_classifier_match_threshold"></a>`PAPERRUST_CLASSIFIER_MATCH_THRESHOLD` | `0.3` | From this confidence (0.0 to 1.0) on, automatic matching sets correspondent, document type or storage path. |
| <a id="PAPERRUST_DATE_PARSER_LANGUAGES"></a><a id="paperrust_date_parser_languages"></a><a id="PAPERLESS_DATE_PARSER_LANGUAGES"></a><a id="paperless_date_parser_languages"></a>`PAPERRUST_DATE_PARSER_LANGUAGES` | – (from `PAPERRUST_OCR_LANGUAGE`) | Languages in which dates are searched, as two-letter codes, e.g. `de+en`. Helps with month names like "März". |
| <a id="PAPERRUST_EMAIL_TASK_CRON"></a><a id="paperrust_email_task_cron"></a><a id="PAPERLESS_EMAIL_TASK_CRON"></a><a id="paperless_email_task_cron"></a>`PAPERRUST_EMAIL_TASK_CRON` | every 10 minutes | When mail accounts are fetched. PaperRust shifts the exact minute a little per installation so not all servers fetch at once. |
| <a id="PAPERRUST_TRAIN_TASK_CRON"></a><a id="paperrust_train_task_cron"></a><a id="PAPERLESS_TRAIN_TASK_CRON"></a><a id="paperless_train_task_cron"></a>`PAPERRUST_TRAIN_TASK_CRON` | `5 */1 * * *` (hourly) | When automatic matching learns again. |
| <a id="PAPERRUST_INDEX_TASK_CRON"></a><a id="paperrust_index_task_cron"></a><a id="PAPERLESS_INDEX_TASK_CRON"></a><a id="paperless_index_task_cron"></a>`PAPERRUST_INDEX_TASK_CRON` | `0 0 * * *` (daily at midnight) | When the search index is tidied up. |
| <a id="PAPERRUST_SANITY_TASK_CRON"></a><a id="paperrust_sanity_task_cron"></a><a id="PAPERLESS_SANITY_TASK_CRON"></a><a id="paperless_sanity_task_cron"></a>`PAPERRUST_SANITY_TASK_CRON` | `30 0 * * sun` (Sundays 0:30) | When the consistency check runs (missing files, wrong checksums). |
| <a id="PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON"></a><a id="paperrust_workflow_scheduled_task_cron"></a><a id="PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON"></a><a id="paperless_workflow_scheduled_task_cron"></a>`PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON` | `5 */1 * * *` (hourly) | When scheduled workflows are checked. |
| <a id="PAPERRUST_SEARCH_LANGUAGE"></a><a id="paperrust_search_language"></a><a id="PAPERLESS_SEARCH_LANGUAGE"></a><a id="paperless_search_language"></a>`PAPERRUST_SEARCH_LANGUAGE` | – (from `PAPERRUST_OCR_LANGUAGE`) | Language of the full-text search, so that e.g. "invoices" also finds "invoice". After a change, PaperRust rebuilds the index at the next start. |
| <a id="PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD"></a><a id="paperrust_advanced_fuzzy_search_threshold"></a><a id="PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD"></a><a id="paperless_advanced_fuzzy_search_threshold"></a>`PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD` | – (off) | Threshold for approximate hits in the full-text search, e.g. `0.5`. Approximate hits come after exact ones. |
| <a id="PAPERRUST_ENABLE_COMPRESSION"></a><a id="paperrust_enable_compression"></a><a id="PAPERLESS_ENABLE_COMPRESSION"></a><a id="paperless_enable_compression"></a>`PAPERRUST_ENABLE_COMPRESSION` | `true` | Send responses compressed. If a reverse proxy in front already does that, you can turn it off here. |
| <a id="PAPERRUST_MAX_IMAGE_PIXELS"></a><a id="paperrust_max_image_pixels"></a><a id="PAPERLESS_MAX_IMAGE_PIXELS"></a><a id="paperless_max_image_pixels"></a>`PAPERRUST_MAX_IMAGE_PIXELS` | – (standard limit) | Largest image area in pixels PaperRust opens at all (for thumbnails etc.). `0` removes the limit. |
| <a id="PAPERRUST_APPS"></a><a id="paperrust_apps"></a><a id="PAPERLESS_APPS"></a><a id="paperless_apps"></a>`PAPERRUST_APPS` | – | Extra modules, separated by commas. In PaperRust only used to turn on SSO providers, e.g. `allauth.socialaccount.providers.openid_connect`. |

<a id="consume_config"></a><a id="document-consumption"></a>

## Document consumption

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_DELETE_DUPLICATES"></a><a id="paperrust_consumer_delete_duplicates"></a><a id="PAPERLESS_CONSUMER_DELETE_DUPLICATES"></a><a id="paperless_consumer_delete_duplicates"></a>`PAPERRUST_CONSUMER_DELETE_DUPLICATES` | `false` | Reject a file that already exists (same checksum) and delete it from the consumption folder. Otherwise duplicates are consumed normally. |
| <a id="PAPERRUST_CONSUMER_RECURSIVE"></a><a id="paperrust_consumer_recursive"></a><a id="PAPERLESS_CONSUMER_RECURSIVE"></a><a id="paperless_consumer_recursive"></a>`PAPERRUST_CONSUMER_RECURSIVE` | `false` | Also watch subfolders of the consumption folder. |
| <a id="PAPERRUST_CONSUMER_SUBDIRS_AS_TAGS"></a><a id="paperrust_consumer_subdirs_as_tags"></a><a id="PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS"></a><a id="paperless_consumer_subdirs_as_tags"></a>`PAPERRUST_CONSUMER_SUBDIRS_AS_TAGS` | `false` | Use the subfolder names as tags; missing tags are created. Needs `PAPERRUST_CONSUMER_RECURSIVE`. `consume/Taxes/2025/receipt.pdf` gets the tags "Taxes" and "2025". |
| <a id="PAPERRUST_CONSUMER_IGNORE_PATTERNS"></a><a id="paperrust_consumer_ignore_patterns"></a><a id="PAPERLESS_CONSUMER_IGNORE_PATTERNS"></a><a id="paperless_consumer_ignore_patterns"></a>`PAPERRUST_CONSUMER_IGNORE_PATTERNS` | `[]` | Extra patterns (regular expressions as a JSON list) for file names that should be left alone. Common system files such as `.DS_Store`, `desktop.ini` or `Thumbs.db` are skipped anyway. |
| <a id="PAPERRUST_CONSUMER_IGNORE_DIRS"></a><a id="paperrust_consumer_ignore_dirs"></a><a id="PAPERLESS_CONSUMER_IGNORE_DIRS"></a><a id="paperless_consumer_ignore_dirs"></a>`PAPERRUST_CONSUMER_IGNORE_DIRS` | `[]` | Extra folder names (JSON list) that are skipped with all their contents. Some system folders (e.g. Synology's `@eaDir`) are built in. |
| <a id="PAPERRUST_PRE_CONSUME_SCRIPT"></a><a id="paperrust_pre_consume_script"></a><a id="PAPERLESS_PRE_CONSUME_SCRIPT"></a><a id="paperless_pre_consume_script"></a>`PAPERRUST_PRE_CONSUME_SCRIPT` | – | Program that runs before each file is consumed. See [Advanced topics](advanced.md#pre-consume-script). Environment only. |
| <a id="PAPERRUST_POST_CONSUME_SCRIPT"></a><a id="paperrust_post_consume_script"></a><a id="PAPERLESS_POST_CONSUME_SCRIPT"></a><a id="paperless_post_consume_script"></a>`PAPERRUST_POST_CONSUME_SCRIPT` | – | Program that runs after consumption. See [Advanced topics](advanced.md#post-consume-script). Environment only. |
| <a id="PAPERRUST_FILENAME_DATE_ORDER"></a><a id="paperrust_filename_date_order"></a><a id="PAPERLESS_FILENAME_DATE_ORDER"></a><a id="paperless_filename_date_order"></a>`PAPERRUST_FILENAME_DATE_ORDER` | – (off) | Also look for a date in the file name, in this order, e.g. `YMD` for `2025-03-14_invoice.pdf`. A date in the file name takes priority over one in the text. |
| <a id="PAPERRUST_DATE_ORDER"></a><a id="paperrust_date_order"></a><a id="PAPERLESS_DATE_ORDER"></a><a id="paperless_date_order"></a>`PAPERRUST_DATE_ORDER` | `DMY` | In which order day, month and year appear in the text. For US documents use `MDY`. |
| <a id="PAPERRUST_NUMBER_OF_SUGGESTED_DATES"></a><a id="paperrust_number_of_suggested_dates"></a><a id="PAPERLESS_NUMBER_OF_SUGGESTED_DATES"></a><a id="paperless_number_of_suggested_dates"></a>`PAPERRUST_NUMBER_OF_SUGGESTED_DATES` | `3` | How many other dates found in the document are offered as suggestions while editing. `0` = none. |
| <a id="PAPERRUST_IGNORE_DATES"></a><a id="paperrust_ignore_dates"></a><a id="PAPERLESS_IGNORE_DATES"></a><a id="paperless_ignore_dates"></a>`PAPERRUST_IGNORE_DATES` | – | Dates that should never count as the document date, e.g. your own birthday. Separated by commas, in the format `YYYY-MM-DD`. |
| <a id="PAPERRUST_THUMBNAIL_FONT_NAME"></a><a id="paperrust_thumbnail_font_name"></a><a id="PAPERLESS_THUMBNAIL_FONT_NAME"></a><a id="paperless_thumbnail_font_name"></a>`PAPERRUST_THUMBNAIL_FONT_NAME` | `/usr/share/fonts/liberation/LiberationSerif-Regular.ttf` | Font file for thumbnails of text and Office files. If it is missing, there is a plain default image. |
| <a id="PAPERRUST_CONSUMER_POLLING_INTERVAL"></a><a id="paperrust_consumer_polling_interval"></a><a id="PAPERLESS_CONSUMER_POLLING_INTERVAL"></a><a id="paperless_consumer_polling_interval"></a>`PAPERRUST_CONSUMER_POLLING_INTERVAL` | `0` | How often PaperRust looks for new files in the consumption folder, in seconds. `0` means, as in paperless-ngx: wait for notifications from the operating system; if that does not work (e.g. network drive), PaperRust checks every second. Larger values = check at this interval, which takes load off slow drives. |
| <a id="PAPERRUST_CONSUMER_STABILITY_DELAY"></a><a id="paperrust_consumer_stability_delay"></a><a id="PAPERLESS_CONSUMER_STABILITY_DELAY"></a><a id="paperless_consumer_stability_delay"></a>`PAPERRUST_CONSUMER_STABILITY_DELAY` | `5` s | How long a file must stay unchanged before PaperRust reads it. Prevents half-copied files from being consumed. |

<a id="workflow-webhooks"></a>

## Workflow webhooks

These three limits decide where workflows may send webhooks. They only apply through the
environment, so an account in the UI cannot trigger requests into the internal network.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES"></a><a id="paperrust_webhooks_allowed_schemes"></a><a id="PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES"></a><a id="paperless_webhooks_allowed_schemes"></a>`PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES` | `http,https` | Allowed address schemes. |
| <a id="PAPERRUST_WEBHOOKS_ALLOWED_PORTS"></a><a id="paperrust_webhooks_allowed_ports"></a><a id="PAPERLESS_WEBHOOKS_ALLOWED_PORTS"></a><a id="paperless_webhooks_allowed_ports"></a>`PAPERRUST_WEBHOOKS_ALLOWED_PORTS` | – (all) | Allowed ports, e.g. `443`. |
| <a id="PAPERRUST_WEBHOOKS_ALLOW_INTERNAL_REQUESTS"></a><a id="paperrust_webhooks_allow_internal_requests"></a><a id="PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS"></a><a id="paperless_webhooks_allow_internal_requests"></a>`PAPERRUST_WEBHOOKS_ALLOW_INTERNAL_REQUESTS` | `true` | `false` blocks targets in your own network (e.g. `localhost`, `192.168.…`). |

<a id="incoming_mail"></a><a id="incoming-mail"></a>

## Incoming mail

The mail accounts themselves are set up in the web UI, see [Email](mail.md). Only the settings for
all accounts are listed here.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS"></a><a id="paperrust_email_allow_internal_hosts"></a><a id="PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS"></a><a id="paperless_email_allow_internal_hosts"></a>`PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS` | `true` | `false` blocks mail accounts whose server is in your own network. Environment only. |

<a id="email_oauth"></a><a id="email-oauth"></a>

### Login to Gmail and Outlook (OAuth)

Gmail and Outlook accounts with OAuth need your own app at the provider. Its return address is
`<base address>/api/oauth/callback/`.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_OAUTH_CALLBACK_BASE_URL"></a><a id="paperrust_oauth_callback_base_url"></a><a id="PAPERLESS_OAUTH_CALLBACK_BASE_URL"></a><a id="paperless_oauth_callback_base_url"></a>`PAPERRUST_OAUTH_CALLBACK_BASE_URL` | – (then `PAPERRUST_URL`) | Base address for the return address. One of the two must be set. |
| <a id="PAPERRUST_GMAIL_OAUTH_CLIENT_ID"></a><a id="paperrust_gmail_oauth_client_id"></a><a id="PAPERLESS_GMAIL_OAUTH_CLIENT_ID"></a><a id="paperless_gmail_oauth_client_id"></a>`PAPERRUST_GMAIL_OAUTH_CLIENT_ID` | – | Client ID of the Google app. |
| <a id="PAPERRUST_GMAIL_OAUTH_CLIENT_SECRET"></a><a id="paperrust_gmail_oauth_client_secret"></a><a id="PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET"></a><a id="paperless_gmail_oauth_client_secret"></a>`PAPERRUST_GMAIL_OAUTH_CLIENT_SECRET` | – | Secret of the Google app. |
| <a id="PAPERRUST_OUTLOOK_OAUTH_CLIENT_ID"></a><a id="paperrust_outlook_oauth_client_id"></a><a id="PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID"></a><a id="paperless_outlook_oauth_client_id"></a>`PAPERRUST_OUTLOOK_OAUTH_CLIENT_ID` | – | Client ID of the Microsoft app. |
| <a id="PAPERRUST_OUTLOOK_OAUTH_CLIENT_SECRET"></a><a id="paperrust_outlook_oauth_client_secret"></a><a id="PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET"></a><a id="paperless_outlook_oauth_client_secret"></a>`PAPERRUST_OUTLOOK_OAUTH_CLIENT_SECRET` | – | Secret of the Microsoft app. |

<a id="encrypted_emails"></a><a id="encrypted-emails"></a>

### Encrypted emails

With `PAPERLESS_ENABLE_GPG_DECRYPTOR=true` PaperRust decrypts GPG-encrypted emails before consuming
them, with keys from `PAPERLESS_EMAIL_GNUPG_HOME` (otherwise the GnuPG default folder). This needs
the `gpg` program (not in the Docker image). If it is missing or decryption fails, the email is not
processed and an error is written to the log. See [Encrypted emails](advanced.md).

<a id="barcodes"></a>

## Barcodes

Barcodes can split a stack of scans into several documents, set an archive serial number (ASN) or
assign tags. Detection needs PDFium. These options can also be set on the *Settings → Configuration*
page.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_ENABLE_BARCODES"></a><a id="paperrust_consumer_enable_barcodes"></a><a id="PAPERLESS_CONSUMER_ENABLE_BARCODES"></a><a id="paperless_consumer_enable_barcodes"></a>`PAPERRUST_CONSUMER_ENABLE_BARCODES` | `false` | Look for separator sheets and split the file at each one. |
| <a id="PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT"></a><a id="paperrust_consumer_barcode_tiff_support"></a><a id="PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT"></a><a id="paperless_consumer_barcode_tiff_support"></a>`PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT` | `false` | Also search TIFF files for barcodes. |
| <a id="PAPERRUST_CONSUMER_BARCODE_STRING"></a><a id="paperrust_consumer_barcode_string"></a><a id="PAPERLESS_CONSUMER_BARCODE_STRING"></a><a id="paperless_consumer_barcode_string"></a>`PAPERRUST_CONSUMER_BARCODE_STRING` | `PATCHT` | Content of the barcode on the separator sheet. |
| <a id="PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"></a><a id="paperrust_consumer_barcode_retain_split_pages"></a><a id="PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"></a><a id="paperless_consumer_barcode_retain_split_pages"></a>`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES` | `false` | Keep the separator sheet (as the first page of the following document) instead of throwing it away. |
| <a id="PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE"></a><a id="paperrust_consumer_enable_asn_barcode"></a><a id="PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE"></a><a id="paperless_consumer_enable_asn_barcode"></a>`PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE` | `false` | Take the archive serial number from a barcode, e.g. `ASN00123` → 123. An ASN barcode also splits the document. |
| <a id="PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX"></a><a id="paperrust_consumer_asn_barcode_prefix"></a><a id="PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX"></a><a id="paperless_consumer_asn_barcode_prefix"></a>`PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX` | `ASN` | Prefix that marks an ASN barcode. |
| <a id="PAPERRUST_CONSUMER_BARCODE_UPSCALE"></a><a id="paperrust_consumer_barcode_upscale"></a><a id="PAPERLESS_CONSUMER_BARCODE_UPSCALE"></a><a id="paperless_consumer_barcode_upscale"></a>`PAPERRUST_CONSUMER_BARCODE_UPSCALE` | `0.0` | Enlarge pages before searching, e.g. `1.5`. Helps with small barcodes; only takes effect above 1.0 and costs memory. |
| <a id="PAPERRUST_CONSUMER_BARCODE_DPI"></a><a id="paperrust_consumer_barcode_dpi"></a><a id="PAPERLESS_CONSUMER_BARCODE_DPI"></a><a id="paperless_consumer_barcode_dpi"></a>`PAPERRUST_CONSUMER_BARCODE_DPI` | `300` | Resolution at which pages are drawn for the search. |
| <a id="PAPERRUST_CONSUMER_BARCODE_MAX_PAGES"></a><a id="paperrust_consumer_barcode_max_pages"></a><a id="PAPERLESS_CONSUMER_BARCODE_MAX_PAGES"></a><a id="paperless_consumer_barcode_max_pages"></a>`PAPERRUST_CONSUMER_BARCODE_MAX_PAGES` | `0` (all) | Only search the first N pages. |
| <a id="PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE"></a><a id="paperrust_consumer_enable_tag_barcode"></a><a id="PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE"></a><a id="paperless_consumer_enable_tag_barcode"></a>`PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE` | `false` | Assign tags from barcodes (and create missing ones) when the content matches a pattern in `PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING`. One barcode may hold several tags separated by commas. |
| <a id="PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING"></a><a id="paperrust_consumer_tag_barcode_mapping"></a><a id="PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING"></a><a id="paperless_consumer_tag_barcode_mapping"></a>`PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING` | `{"TAG:(.*)": "\\g<1>"}` | Mapping "pattern → tag name" as JSON. The default turns `TAG:Taxes` into the tag "Taxes". |
| <a id="PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT"></a><a id="paperrust_consumer_tag_barcode_split"></a><a id="PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT"></a><a id="paperless_consumer_tag_barcode_split"></a>`PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT` | `false` | Also start a new document at tag barcodes. |
| <a id="PAPERRUST_CONSUMER_STORE_BARCODE_VALUES"></a><a id="paperrust_consumer_store_barcode_values"></a><a id="PAPERLESS_CONSUMER_STORE_BARCODE_VALUES"></a><a id="paperless_consumer_store_barcode_values"></a>`PAPERRUST_CONSUMER_STORE_BARCODE_VALUES` | `false` | Store the content of all barcodes found with the document, so you can search for it. |

Example: barcodes `DEPT:Accounting` should give the tag "Accounting", `TAG:…` keeps working as
before:

```sh
PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING='{"TAG:(.*)": "\\g<1>", "DEPT:(.*)": "\\g<1>"}'
```

<a id="audit-trail"></a>

## Audit trail

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_AUDIT_LOG_ENABLED"></a><a id="paperrust_audit_log_enabled"></a><a id="PAPERLESS_AUDIT_LOG_ENABLED"></a><a id="paperless_audit_log_enabled"></a>`PAPERRUST_AUDIT_LOG_ENABLED` | `true` | Record who changed what and when on documents, document types, correspondents and tags. Turning it off does not delete existing entries. |

<a id="collate"></a><a id="collate-double-sided-documents"></a>

## Collating double-sided scans

For scanners that can only scan one side: first scan all front sides, then turn the stack over and
scan all back sides. Both files go, one after the other, into their own subfolder of the
consumption folder; PaperRust interleaves the pages into one document.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED"></a><a id="paperrust_consumer_enable_collate_double_sided"></a><a id="PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED"></a><a id="paperless_consumer_enable_collate_double_sided"></a>`PAPERRUST_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED` | `false` | Turn collating on. |
| <a id="PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME"></a><a id="paperrust_consumer_collate_double_sided_subdir_name"></a><a id="PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME"></a><a id="paperless_consumer_collate_double_sided_subdir_name"></a>`PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME` | `double-sided` | Name of the subfolder. You create it yourself. |
| <a id="PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT"></a><a id="paperrust_consumer_collate_double_sided_tiff_support"></a><a id="PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT"></a><a id="paperless_consumer_collate_double_sided_tiff_support"></a>`PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT` | `false` | Also collate TIFF files. |

The back-side file must have as many pages as the front-side file and arrive no later than
30 minutes after it.

<a id="trash"></a>

## Trash

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_EMPTY_TRASH_DELAY"></a><a id="paperrust_empty_trash_delay"></a><a id="PAPERLESS_EMPTY_TRASH_DELAY"></a><a id="paperless_empty_trash_delay"></a>`PAPERRUST_EMPTY_TRASH_DELAY` | `30` days | How long deleted documents stay in the trash before they are gone for good (at least 1). |
| <a id="PAPERRUST_EMPTY_TRASH_TASK_CRON"></a><a id="paperrust_empty_trash_task_cron"></a><a id="PAPERLESS_EMPTY_TRASH_TASK_CRON"></a><a id="paperless_empty_trash_task_cron"></a>`PAPERRUST_EMPTY_TRASH_TASK_CRON` | `0 1 * * *` (daily 1 am) | When expired entries are removed from the trash. |

<a id="share-links"></a>

## Share links

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_SHARE_LINK_BUNDLE_CLEANUP_CRON"></a><a id="paperrust_share_link_bundle_cleanup_cron"></a><a id="PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON"></a><a id="paperless_share_link_bundle_cleanup_cron"></a>`PAPERRUST_SHARE_LINK_BUNDLE_CLEANUP_CRON` | `0 2 * * *` (daily 2 am) | When expired share bundles and their ZIP files are removed. |

<a id="binaries"></a>

## Programs

PaperRust needs only a few helper programs: Tesseract
([`PAPERRUST_TESSERACT_PATH`](#paperrust_tesseract_path),
[`PAPERRUST_TESSDATA_DIR`](#paperrust_tessdata_dir)), the PDFium library
([`PAPERRUST_PDFIUM_PATH`](#paperrust_pdfium_path)) and its own PDF tool
([`PAPERRUST_PDFTOOL_BIN`](#paperrust_pdftool_bin)). The program paths of paperless-ngx
(`PAPERLESS_CONVERT_BINARY`, `PAPERLESS_GS_BINARY`) are [ignored](#ignored).

<a id="docker"></a><a id="docker-specific-options"></a>

## Docker image

These variables are only evaluated by the start-up of the PaperRust Docker image, not by the program
itself. The image is currently built from the repository, see [Installation](installation.md#docker).

| Variable | Default | Meaning |
|---|---|---|
| <a id="USERMAP_UID"></a><a id="usermap_uid"></a>`USERMAP_UID` | `1000` | User ID PaperRust runs as in the container (check on the host with `id -u`). Only takes effect if the container starts as root (`user: root`). `PUID` works as a fallback. |
| <a id="USERMAP_GID"></a><a id="usermap_gid"></a>`USERMAP_GID` | `1000` | Matching group ID (`id -g`). `PGID` works as a fallback. |
| <a id="PAPERRUST_OCR_LANGUAGES"></a><a id="paperrust_ocr_languages"></a><a id="PAPERLESS_OCR_LANGUAGES"></a><a id="paperless_ocr_languages"></a>`PAPERRUST_OCR_LANGUAGES` | – | Extra Tesseract languages installed at start-up, e.g. `tur ces`. Only works if the container starts as root. Better: choose the languages when building (`--build-arg TESSERACT_LANGS="eng deu fra ita spa"`, which is also the default). |
| <a id="PAPERRUST_BIND_ADDR"></a><a id="paperrust_bind_addr"></a><a id="PAPERLESS_BIND_ADDR"></a><a id="paperless_bind_addr"></a>`PAPERRUST_BIND_ADDR` | `0.0.0.0` | Address PaperRust listens on in the container – only if [`PAPERRUST_BIND`](#paperrust_bind) is not set. |
| <a id="PAPERRUST_PORT"></a><a id="paperrust_port"></a><a id="PAPERLESS_PORT"></a><a id="paperless_port"></a>`PAPERRUST_PORT` | `8000` | Matching port. |

<a id="frontend-settings"></a>

## Name, logo and update check

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_APP_TITLE"></a><a id="paperrust_app_title"></a><a id="PAPERLESS_APP_TITLE"></a><a id="paperless_app_title"></a>`PAPERRUST_APP_TITLE` | – ("PaperRust") | Your own name for the application, e.g. "Family archive". |
| <a id="PAPERRUST_APP_LOGO"></a><a id="paperrust_app_logo"></a><a id="PAPERLESS_APP_LOGO"></a><a id="paperless_app_logo"></a>`PAPERRUST_APP_LOGO` | – | Your own logo, as a path below `media/` with "logo" in its name, e.g. `/logo/family.svg`. The easiest way is to upload it on the *Settings → Configuration* page. The logo also shows on the login page, i.e. to everyone. |
| <a id="PAPERRUST_ENABLE_UPDATE_CHECK"></a><a id="paperrust_enable_update_check"></a><a id="PAPERLESS_ENABLE_UPDATE_CHECK"></a><a id="paperless_enable_update_check"></a>`PAPERRUST_ENABLE_UPDATE_CHECK` | `default` | Whether PaperRust looks for new versions. Normally you switch this in the UI; the variable is only taken over once, as long as nothing was chosen there. Source: [`PAPERRUST_REMOTE_VERSION_URL`](#paperrust_remote_version_url). |

<a id="email-sending"></a>

## Sending email

PaperRust sends emails for password resets, from workflows and when you send documents. Everything
else works without a mail server.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_EMAIL_HOST"></a><a id="paperrust_email_host"></a><a id="PAPERLESS_EMAIL_HOST"></a><a id="paperless_email_host"></a>`PAPERRUST_EMAIL_HOST` | `localhost` | Mail server for outgoing mail (SMTP). |
| <a id="PAPERRUST_EMAIL_PORT"></a><a id="paperrust_email_port"></a><a id="PAPERLESS_EMAIL_PORT"></a><a id="paperless_email_port"></a>`PAPERRUST_EMAIL_PORT` | `25` | Port of the mail server; 587 (STARTTLS) or 465 (SSL) are common. |
| <a id="PAPERRUST_EMAIL_HOST_USER"></a><a id="paperrust_email_host_user"></a><a id="PAPERLESS_EMAIL_HOST_USER"></a><a id="paperless_email_host_user"></a>`PAPERRUST_EMAIL_HOST_USER` | empty | User name at the mail server. |
| <a id="PAPERRUST_EMAIL_HOST_PASSWORD"></a><a id="paperrust_email_host_password"></a><a id="PAPERLESS_EMAIL_HOST_PASSWORD"></a><a id="paperless_email_host_password"></a>`PAPERRUST_EMAIL_HOST_PASSWORD` | empty | Password at the mail server. Stored encrypted when set in the UI. |
| <a id="PAPERRUST_EMAIL_FROM"></a><a id="paperrust_email_from"></a><a id="PAPERLESS_EMAIL_FROM"></a><a id="paperless_email_from"></a>`PAPERRUST_EMAIL_FROM` | – (then `PAPERRUST_EMAIL_HOST_USER`) | Sender address. |
| <a id="PAPERRUST_EMAIL_USE_TLS"></a><a id="paperrust_email_use_tls"></a><a id="PAPERLESS_EMAIL_USE_TLS"></a><a id="paperless_email_use_tls"></a>`PAPERRUST_EMAIL_USE_TLS` | `false` | Encrypt the connection with STARTTLS (usually port 587). |
| <a id="PAPERRUST_EMAIL_USE_SSL"></a><a id="paperrust_email_use_ssl"></a><a id="PAPERLESS_EMAIL_USE_SSL"></a><a id="paperless_email_use_ssl"></a>`PAPERRUST_EMAIL_USE_SSL` | `false` | Connect encrypted from the start (usually port 465). Not together with `USE_TLS`. |
| <a id="PAPERRUST_EMAIL_SUBJECT_PREFIX"></a><a id="paperrust_email_subject_prefix"></a>`PAPERRUST_EMAIL_SUBJECT_PREFIX` | `[PaperRust] ` | Put in front of the subject of e-mails that PaperRust sends itself (e.g. password reset). |

<a id="remote-ocr"></a>

## Text recognition in the cloud

Instead of Tesseract, a cloud service can recognize the text. Currently only Azure AI Document
Intelligence is supported. The documents leave your own machine for this.
The service returns the text and a searchable PDF that becomes the archive version. The API key is only
sent to the configured address and never appears in a log. With `workflow_only` a document only goes
to the cloud when a workflow with the "Remote OCR" action applies or "Remote OCR" is chosen when
reprocessing; otherwise Tesseract recognizes the text.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_REMOTE_OCR_ENGINE"></a><a id="paperrust_remote_ocr_engine"></a><a id="PAPERLESS_REMOTE_OCR_ENGINE"></a><a id="paperless_remote_ocr_engine"></a>`PAPERRUST_REMOTE_OCR_ENGINE` | – (off) | Service; currently only `azureai`. |
| <a id="PAPERRUST_REMOTE_OCR_API_KEY"></a><a id="paperrust_remote_ocr_api_key"></a><a id="PAPERLESS_REMOTE_OCR_API_KEY"></a><a id="paperless_remote_ocr_api_key"></a>`PAPERRUST_REMOTE_OCR_API_KEY` | – | API key of the service. |
| <a id="PAPERRUST_REMOTE_OCR_ENDPOINT"></a><a id="paperrust_remote_ocr_endpoint"></a><a id="PAPERLESS_REMOTE_OCR_ENDPOINT"></a><a id="paperless_remote_ocr_endpoint"></a>`PAPERRUST_REMOTE_OCR_ENDPOINT` | – | Address of the service; required for Azure. |
| <a id="PAPERRUST_REMOTE_OCR_MODE"></a><a id="paperrust_remote_ocr_mode"></a><a id="PAPERLESS_REMOTE_OCR_MODE"></a><a id="paperless_remote_ocr_mode"></a>`PAPERRUST_REMOTE_OCR_MODE` | `always` | `always` = every suitable document goes to the service; `workflow_only` = only when a workflow or "Reprocess" asks for it. |
| <a id="PAPERRUST_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperrust_remote_ocr_allow_internal_endpoints"></a><a id="PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperless_remote_ocr_allow_internal_endpoints"></a>`PAPERRUST_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS` | `true` | `false` blocks addresses in your own network. Environment only. |

<a id="ai"></a>

## AI

What the AI features do is described under [AI](ai.md). PaperRust's own
[extensions](#paperrust-settings) are listed in the first part of this page.

| Variable | Default | Meaning |
|---|---|---|
| <a id="PAPERRUST_AI_ENABLED"></a><a id="paperrust_ai_enabled"></a><a id="PAPERLESS_AI_ENABLED"></a><a id="paperless_ai_enabled"></a>`PAPERRUST_AI_ENABLED` | `false` | Main switch for all AI features. |
| <a id="PAPERRUST_AI_LLM_BACKEND"></a><a id="paperrust_ai_llm_backend"></a><a id="PAPERLESS_AI_LLM_BACKEND"></a><a id="paperless_ai_llm_backend"></a>`PAPERRUST_AI_LLM_BACKEND` | – | Provider of the language model: `openai-like` (any OpenAI-compatible interface) or `ollama` (runs on your own machine). |
| <a id="PAPERRUST_AI_LLM_MODEL"></a><a id="paperrust_ai_llm_model"></a><a id="PAPERLESS_AI_LLM_MODEL"></a><a id="paperless_ai_llm_model"></a>`PAPERRUST_AI_LLM_MODEL` | – (`gpt-3.5-turbo` or `llama3.1`) | Name of the model. |
| <a id="PAPERRUST_AI_LLM_API_KEY"></a><a id="paperrust_ai_llm_api_key"></a><a id="PAPERLESS_AI_LLM_API_KEY"></a><a id="paperless_ai_llm_api_key"></a>`PAPERRUST_AI_LLM_API_KEY` | – | API key; usually needed for OpenAI-compatible services. |
| <a id="PAPERRUST_AI_LLM_ENDPOINT"></a><a id="paperrust_ai_llm_endpoint"></a><a id="PAPERLESS_AI_LLM_ENDPOINT"></a><a id="paperless_ai_llm_endpoint"></a>`PAPERRUST_AI_LLM_ENDPOINT` | – | Address of the provider; required for Ollama, e.g. `http://ollama:11434`. |
| <a id="PAPERRUST_AI_LLM_OUTPUT_LANGUAGE"></a><a id="paperrust_ai_llm_output_language"></a><a id="PAPERLESS_AI_LLM_OUTPUT_LANGUAGE"></a><a id="paperless_ai_llm_output_language"></a>`PAPERRUST_AI_LLM_OUTPUT_LANGUAGE` | – (UI language) | Language of the suggestions. |
| <a id="PAPERRUST_AI_LLM_CONTEXT_SIZE"></a><a id="paperrust_ai_llm_context_size"></a><a id="PAPERLESS_AI_LLM_CONTEXT_SIZE"></a><a id="paperless_ai_llm_context_size"></a>`PAPERRUST_AI_LLM_CONTEXT_SIZE` | `8192` | How much text goes to the model at once (in tokens). For Ollama the value is also sent as `num_ctx`. |
| <a id="PAPERRUST_AI_LLM_REQUEST_TIMEOUT"></a><a id="paperrust_ai_llm_request_timeout"></a><a id="PAPERLESS_AI_LLM_REQUEST_TIMEOUT"></a><a id="paperless_ai_llm_request_timeout"></a>`PAPERRUST_AI_LLM_REQUEST_TIMEOUT` | `120` s | Time limit for one request to the provider. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_BACKEND"></a><a id="paperrust_ai_llm_embedding_backend"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_BACKEND"></a><a id="paperless_ai_llm_embedding_backend"></a>`PAPERRUST_AI_LLM_EMBEDDING_BACKEND` | – | Provider for embeddings (the number vectors behind the meaning-based search and the document chat): `openai-like`, `huggingface` or `ollama`. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_MODEL"></a><a id="paperrust_ai_llm_embedding_model"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_MODEL"></a><a id="paperless_ai_llm_embedding_model"></a>`PAPERRUST_AI_LLM_EMBEDDING_MODEL` | – (per provider, e.g. `text-embedding-3-small`) | Model for embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_API_KEY"></a><a id="paperrust_ai_llm_embedding_api_key"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_API_KEY"></a><a id="paperless_ai_llm_embedding_api_key"></a>`PAPERRUST_AI_LLM_EMBEDDING_API_KEY` | – (then `PAPERRUST_AI_LLM_API_KEY`) | API key for embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT"></a><a id="paperrust_ai_llm_embedding_endpoint"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT"></a><a id="paperless_ai_llm_embedding_endpoint"></a>`PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT` | – (then `PAPERRUST_AI_LLM_ENDPOINT`) | Address for embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_CHUNK_SIZE"></a><a id="paperrust_ai_llm_embedding_chunk_size"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE"></a><a id="paperless_ai_llm_embedding_chunk_size"></a>`PAPERRUST_AI_LLM_EMBEDDING_CHUNK_SIZE` | `1024` | How large the pieces are that the document text is cut into. Make it smaller if the model rejects long inputs. |
| <a id="PAPERRUST_AI_LLM_EXTRA_PARAMS"></a><a id="paperrust_ai_llm_extra_params"></a><a id="PAPERLESS_AI_LLM_EXTRA_PARAMS"></a><a id="paperless_ai_llm_extra_params"></a>`PAPERRUST_AI_LLM_EXTRA_PARAMS` | empty | More fields as a JSON object, added unchanged to every request, e.g. `{"temperature": 0.2}`. They override PaperRust's own values. |
| <a id="PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperrust_ai_llm_allow_internal_endpoints"></a><a id="PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperless_ai_llm_allow_internal_endpoints"></a>`PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS` | `true` | `false` blocks addresses in your own network. Environment only. |
| <a id="PAPERRUST_LLM_INDEX_TASK_CRON"></a><a id="paperrust_llm_index_task_cron"></a><a id="PAPERLESS_LLM_INDEX_TASK_CRON"></a><a id="paperless_llm_index_task_cron"></a>`PAPERRUST_LLM_INDEX_TASK_CRON` | `10 2 * * *` (daily 2:10 am) | When the AI index is updated. Only runs if AI is on and an embedding provider is set. |

<a id="ignored"></a><a id="optional-services"></a><a id="tika"></a><a id="email-parsing"></a>

## Not needed or ignored

You can leave these paperless-ngx variables in place; PaperRust does not mind them. Most have no
effect; rows marked "effective" apply again (they are listed here because in paperless-ngx they were
often set together with Tika or Celery). [Origin & migration](#settings-ui) helps to tidy up.

| Variable | Reason |
|---|---|
| <a id="PAPERRUST_TIKA_ENABLED"></a><a id="paperrust_tika_enabled"></a><a id="PAPERLESS_TIKA_ENABLED"></a><a id="paperless_tika_enabled"></a>`PAPERLESS_TIKA_ENABLED` | PaperRust reads Office files and emails itself; Tika is not needed. |
| <a id="PAPERRUST_TIKA_ENDPOINT"></a><a id="paperrust_tika_endpoint"></a><a id="PAPERLESS_TIKA_ENDPOINT"></a><a id="paperless_tika_endpoint"></a>`PAPERLESS_TIKA_ENDPOINT` | as above, no Tika. |
| <a id="PAPERRUST_TIKA_GOTENBERG_ENDPOINT"></a><a id="paperrust_tika_gotenberg_endpoint"></a><a id="PAPERLESS_TIKA_GOTENBERG_ENDPOINT"></a><a id="paperless_tika_gotenberg_endpoint"></a>`PAPERLESS_TIKA_GOTENBERG_ENDPOINT` | no Gotenberg; the PDF version of Office files and emails is made by `paperrust-pdftool`. |
| <a id="PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT"></a><a id="paperrust_email_parse_default_layout"></a><a id="PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT"></a><a id="paperless_email_parse_default_layout"></a>`PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT` | **effective again**: order inside the PDF of a consumed e-mail without mail rule (1 = text, then HTML (default), 2 = HTML, then text, 3 = HTML only, 4 = text only); with a mail rule its "PDF layout" applies. See [Office files on ingest](pdf-tools.md#office-beim-einlesen). |
| <a id="PAPERRUST_DB_READ_CACHE_ENABLED"></a><a id="paperrust_db_read_cache_enabled"></a><a id="PAPERLESS_DB_READ_CACHE_ENABLED"></a><a id="paperless_db_read_cache_enabled"></a>`PAPERLESS_DB_READ_CACHE_ENABLED` | Django's read cache; PaperRust has none. |
| <a id="PAPERRUST_READ_CACHE_TTL"></a><a id="paperrust_read_cache_ttl"></a><a id="PAPERLESS_READ_CACHE_TTL"></a><a id="paperless_read_cache_ttl"></a>`PAPERLESS_READ_CACHE_TTL` | part of Django's read cache. |
| <a id="PAPERRUST_READ_CACHE_REDIS_URL"></a><a id="paperrust_read_cache_redis_url"></a><a id="PAPERLESS_READ_CACHE_REDIS_URL"></a><a id="paperless_read_cache_redis_url"></a>`PAPERLESS_READ_CACHE_REDIS_URL` | part of Django's read cache. |
| <a id="PAPERRUST_CHANNELS_BACKEND"></a><a id="paperrust_channels_backend"></a><a id="PAPERLESS_CHANNELS_BACKEND"></a><a id="paperless_channels_backend"></a>`PAPERLESS_CHANNELS_BACKEND` | Django Channels; PaperRust sends status messages itself. |
| <a id="PAPERRUST_MATCH_REGEX_TIMEOUT_SECONDS"></a><a id="paperrust_match_regex_timeout_seconds"></a><a id="PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS"></a><a id="paperless_match_regex_timeout_seconds"></a>`PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS` | **effective**: time limit per matching regex as in paperless-ngx (special syntax of the Python `regex` module such as `{e<=1}` stays invalid). |
| <a id="PAPERRUST_ENABLE_GPG_DECRYPTOR"></a><a id="paperrust_enable_gpg_decryptor"></a><a id="PAPERLESS_ENABLE_GPG_DECRYPTOR"></a><a id="paperless_enable_gpg_decryptor"></a>`PAPERLESS_ENABLE_GPG_DECRYPTOR` | **effective**: see [Encrypted emails](#encrypted-emails) (needs the `gpg` program). |
| <a id="PAPERRUST_EMAIL_GNUPG_HOME"></a><a id="paperrust_email_gnupg_home"></a><a id="PAPERLESS_EMAIL_GNUPG_HOME"></a><a id="paperless_email_gnupg_home"></a>`PAPERLESS_EMAIL_GNUPG_HOME` | **effective**: GnuPG folder for decrypting mail. |
| <a id="PAPERRUST_CONVERT_BINARY"></a><a id="paperrust_convert_binary"></a><a id="PAPERLESS_CONVERT_BINARY"></a><a id="paperless_convert_binary"></a>`PAPERLESS_CONVERT_BINARY` | ImageMagick is not used. |
| <a id="PAPERRUST_CONVERT_MEMORY_LIMIT"></a><a id="paperrust_convert_memory_limit"></a><a id="PAPERLESS_CONVERT_MEMORY_LIMIT"></a><a id="paperless_convert_memory_limit"></a>`PAPERLESS_CONVERT_MEMORY_LIMIT` | ImageMagick is not used. |
| <a id="PAPERRUST_CONVERT_TMPDIR"></a><a id="paperrust_convert_tmpdir"></a><a id="PAPERLESS_CONVERT_TMPDIR"></a><a id="paperless_convert_tmpdir"></a>`PAPERLESS_CONVERT_TMPDIR` | ImageMagick is not used. |
| <a id="PAPERRUST_GS_BINARY"></a><a id="paperrust_gs_binary"></a><a id="PAPERLESS_GS_BINARY"></a><a id="paperless_gs_binary"></a>`PAPERLESS_GS_BINARY` | Ghostscript is not used. |
| <a id="PAPERRUST_OCR_SKIP_ARCHIVE_FILE"></a><a id="paperrust_ocr_skip_archive_file"></a><a id="PAPERLESS_OCR_SKIP_ARCHIVE_FILE"></a><a id="paperless_ocr_skip_archive_file"></a>`PAPERLESS_OCR_SKIP_ARCHIVE_FILE` | old name; today [`PAPERRUST_ARCHIVE_FILE_GENERATION`](#paperrust_archive_file_generation). |
| <a id="PAPERRUST_ENABLE_NLTK"></a><a id="paperrust_enable_nltk"></a><a id="PAPERLESS_ENABLE_NLTK"></a><a id="paperless_enable_nltk"></a>`PAPERLESS_ENABLE_NLTK` | already dropped in paperless-ngx 3.2. |
| <a id="PAPERRUST_NLTK_DIR"></a><a id="paperrust_nltk_dir"></a><a id="PAPERLESS_NLTK_DIR"></a><a id="paperless_nltk_dir"></a>`PAPERLESS_NLTK_DIR` | already dropped in paperless-ngx 3.2; an old NLTK folder can go. |
| <a id="PAPERRUST_WEBSERVER_WORKERS"></a><a id="paperrust_webserver_workers"></a><a id="PAPERLESS_WEBSERVER_WORKERS"></a><a id="paperless_webserver_workers"></a>`PAPERLESS_WEBSERVER_WORKERS` | PaperRust is a single process with several threads (limit with `TOKIO_WORKER_THREADS`); the Docker image points this out at start-up. |
| <a id="PAPERRUST_CONSUMER_DISABLE"></a><a id="paperrust_consumer_disable"></a><a id="PAPERLESS_CONSUMER_DISABLE"></a><a id="paperless_consumer_disable"></a>`PAPERLESS_CONSUMER_DISABLE` | **effective**: any non-empty value turns off watching the consumption folder in the worker (the `document_consumer` command stays usable). |
| <a id="PAPERRUST_ENABLE_FLOWER"></a><a id="paperrust_enable_flower"></a><a id="PAPERLESS_ENABLE_FLOWER"></a><a id="paperless_enable_flower"></a>`PAPERLESS_ENABLE_FLOWER` | Flower monitors Celery; no Celery, no Flower. The web UI shows running tasks. |
| <a id="PAPERRUST_SUPERVISORD_WORKING_DIR"></a><a id="paperrust_supervisord_working_dir"></a><a id="PAPERLESS_SUPERVISORD_WORKING_DIR"></a><a id="paperless_supervisord_working_dir"></a>`PAPERLESS_SUPERVISORD_WORKING_DIR` | already without effect in paperless-ngx. |
