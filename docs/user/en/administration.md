# Administration

[Deutsch](../administration.md) · [Documentation overview](README.md)

<a id="backup"></a>

## Backup

Back up three things – together, in the same state:

1. **Database** (PostgreSQL): `pg_dump -Fc -U paperless paperless > paperless.pgdump`.
   Consistent even while PaperRust is running (MVCC).
2. **`MEDIA_ROOT`** (`originals/`, `archive/`, `thumbnails/`): the document files.
3. **`DATA_DIR`**: search index, classifier, AI index. Can be rebuilt if necessary, but a backup saves time.

In addition or instead: the [exporter](#exporter). It writes all data and files into a folder (or a
ZIP) in the paperless-ngx format – so the backup can also be imported into paperless-ngx.

```sh
paperrust manage document_exporter /backup/export --zip
```

Desktop app: `paperrust-desktop backup [TARGET]` creates `pg_dump` and export in one step
([docs/DESKTOP.md](../../DESKTOP.md)).

<a id="migrating-restoring"></a>

### Restoring

- From `pg_dump`: create an empty database, `pg_restore --no-owner -d paperless paperless.pgdump`,
  copy back `media/` and `data/`, `paperrust manage migrate`, start.
- From an export: empty database, `paperrust manage migrate`, then
  `paperrust manage document_importer /backup/export`.
- Docker: restored `media/` and `data/` folders must belong to the container user (UID 1000 or
  `USERMAP_UID`), e.g. `chown -R 1000:1000 media data`. Otherwise PaperRust cannot write files; the integrity
  check (`document_sanity_checker`) reports it.
- Tested on 2026-11-20 on a fresh system: export, `pg_dump` and `backup restore` fully restore all documents,
  files (byte-identical), metadata, permissions and search.

<a id="updating"></a>

## Updating

See [Installation → Updating](installation.md#updating). In short: back up, replace the binary,
`paperrust manage migrate`, restart.

<a id="management-commands"></a><a id="manage"></a>

## Management commands: `paperrust manage`

Maintenance jobs use the same program that starts the server:

```sh
paperrust manage <command> [options]
```

In the Docker container: `docker compose exec <service> paperrust manage <command>`.

- Settings come from the environment and from `paperrust.conf` (`paperless.conf` is still read), just like for the server.
- Exit code: `0` all good, `1` error (the message starts with `CommandError:`), `2` wrong usage.
- Output is plain text lines, without progress bars.
- For people switching over: the commands have the same names as in paperless-ngx (`manage.py …`);
  common extra options such as `-v` or `--no-color` are accepted.

| Command | Purpose | State |
|---|---|---|
| `migrate [--check] [--plan] [--skip-paperrust]` | create or check the schema, apply `paperrust_*` tables | done |
| `showmigrations` | list migrations | done |
| `createsuperuser`, `manage_superuser`, `changepassword` | administrators and passwords | done |
| `document_exporter` | [export](#exporter) | done |
| `document_importer` | [import](#importer) | done |
| `document_sanity_checker` | [sanity check](#sanity-checker) | done |
| `document_retagger` | [re-apply matching](#retagger) | done |
| `document_index reindex\|optimize` | [search index](#index) | done |
| `document_create_classifier` | train the classifier | done |
| `document_thumbnails [-d ID]` | [regenerate thumbnails](#thumbnails) (needs PDFium) | done |
| `document_renamer` | [re-apply file names](#renamer) | done |
| `prune_audit_logs` | [prune history](#prune-history) | done |
| `empty_trash [--doc-ids 1,2]` | empty the trash | done (PaperRust addition) |
| `mail_fetcher` | [fetch mail now](#mail) | done |
| `document_archiver [-f] [-d ID] [--processes N]` | (re)create archive PDFs | done |
| `document_consumer [dir] [--oneshot]` | consume a directory once or watch it | done |
| `document_fuzzy_match [--ratio R] [--delete]` | find near-duplicate documents | done |
| `document_llmindex rebuild\|update\|compact\|migrate` | maintain the AI index (AI builds only) | done |
| `check [--fail-level LEVEL]` | system checks (directories, programs, settings) | done |
| `convert_mariadb_uuid` | – | not needed (MariaDB only, see below) |

There are also commands that only exist in PaperRust:

| Command | Purpose |
|---|---|
| `settings list\|get\|set\|reset\|export\|import\|rekey\|origin\|adopt\|slim-env\|check-file` | change, back up and move system settings without the UI ([Configuration](configuration.md#settings-ui)) |
| `backup init\|run\|list\|check\|restore` | encrypted backup with versions ([Ransomware protection](ransomware.md)) |
| `guard status\|lift [--note TEXT]\|scan` | ransomware guard: state, lift read-only mode, check the archive ([Ransomware protection](ransomware.md)) |
| `datamodel status\|check\|rollback --to <stage>` | show and check the state of the PaperRust data model, roll back conversion stages ([docs/DATAMODEL-V2.md](../../DATAMODEL-V2.md)) |
| `sync status\|enable\|disable [--purge]\|rebuild\|reset-identity` | sync for the desktop and mobile apps (`reset-identity` after copying a database) |
| `grants status\|check\|rebuild` | check or rebuild the sharing index (only for troubleshooting) |
| `timeline_backfill [--force]` | find deadlines in the text of all documents for the timeline (`PAPERRUST_TIMELINE_EXTRACT_DATES`) |

`paperrust manage --help` lists all commands. Help for a single command (`manage <command> --help`)
only exists for `sync` and `grants` so far; the options are listed here.

In normal operation `paperrust serve --worker` watches the consumption directory itself;
`document_consumer` is only needed for one-off runs (e.g. from cron with `--oneshot`).

<a id="exporter"></a>

### Exporter

```
paperrust manage document_exporter TARGET [-c] [-cj] [-d] [-f] [-na] [-nt] [-p] [-sm] [-z]
    [-zn NAME] [--zip-compression stored|deflated|bzip2|lzma|zstd] [--zip-compression-level N]
    [--data-only] [--passphrase PASS] [--batch-size 500]
```

| Option | Effect |
|---|---|
| `-c`, `--compare-checksums` | compare files by SHA-256 instead of size/time |
| `-cj`, `--compare-json` | write the manifest only if it changed |
| `-d`, `--delete` | delete files in the target that no longer belong to the archive |
| `-f`, `--use-filename-format` | file names according to `PAPERRUST_FILENAME_FORMAT` |
| `-na`, `-nt` | without archive files or without thumbnails |
| `-p`, `--use-folder-prefix` | sub-folders `originals/`, `archive/`, `thumbnails/`, `json/` |
| `-sm`, `--split-manifest` | one manifest per document |
| `-z`, `--zip`, `-zn` | as ZIP (default name `export-<date>`) |
| `--data-only` | database content only, no files |
| `--passphrase` | encrypt mail passwords and tokens (without: plain text, with a notice) |

The export runs in one read transaction (`REPEATABLE READ`) and is interchangeable with paperless-ngx
v3.4.2. You can choose the ZIP compression as in paperless-ngx (`--zip-compression`, also bzip2, lzma,
zstd). Difference: `--zip-name` must be a plain file name.

<a id="importer"></a>

### Importer

```
paperrust manage document_importer SOURCE [--data-only] [--passphrase PASS] [--batch-size 500]
```

`SOURCE` is an export folder or ZIP. The target should be a fresh installation (empty database after
`paperrust manage migrate`). The import runs in **one** transaction; then files are copied and the
search index is rebuilt. PaperRust is strict here: file names in the manifest must
be relative and stay inside the media folders, referenced files inside the import folder; ZIPs are
unpacked up to `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` (default 1 GiB + 100 × archive size). See
[Security](security.md).

<a id="sanity-checker"></a>

### Sanity checker

```sh
paperrust manage document_sanity_checker
```

Checks thumbnail, original (incl. SHA-256), archive file and empty content of every document and finds
orphaned files below `MEDIA_ROOT`. It ends with a summary such as "Found 2 document(s) with errors
and 1 global warning(s)." The check also runs by itself every week (see
[Tasks and schedule](#tasks)); results under *Tasks* in the UI. Incremental mode for large archives, JSON report per run and settings:
[Integrity check](integrity.md).

<a id="retagger"></a>

### Document retagger

```sh
paperrust manage document_retagger [-c] [-T] [-t] [-s] [-i] [--use-first] [-f] [--suggest] [--id-range FROM TO]
```

`-c` correspondent, `-T` tags, `-t` document type, `-s` storage path; `-i` inbox only, `-f` overwrite
existing values, `--suggest` only show. Uses the PaperRust classifier model.

<a id="classifier"></a>

### Classifier

```sh
paperrust manage document_create_classifier
```

Trains the model for automatic matching (`DATA_DIR/paperrust_classifier.model`). Without tags,
correspondents or types set to "Auto" nothing is trained ("No automatic matching items, not training").
Also runs by itself every hour.

<a id="thumbnails"></a>

### Thumbnails

```sh
paperrust manage document_thumbnails [-d ID]
```

Regenerates thumbnails (WebP). Without PDFium the default image is used.

<a id="index"></a>

### Search index

```sh
paperrust manage document_index reindex [--recreate] [--if-needed]
paperrust manage document_index optimize      # does nothing, kept for compatibility
```

The search index is updated right away on every change. It uses the same format as paperless-ngx
(tantivy). Rebuild it if it is
damaged or after changing `PAPERRUST_SEARCH_LANGUAGE`.

<a id="llm-index"></a>

### AI index

PaperRust keeps the index for the AI features (chat, suggestions based on similar documents) up to
date by itself: on every document change and once a day on schedule (`PAPERRUST_LLM_INDEX_TASK_CRON`).
When you change the embedding settings in the interface, it is rebuilt. To start it by hand, click
"Run task" next to *AI index* in the *System status* dialog. Or run `paperrust manage document_llmindex update` (or `rebuild`).
More: [AI](ai.md).

<a id="renamer"></a>

### Document renamer

```sh
paperrust manage document_renamer
```

After changing `PAPERRUST_FILENAME_FORMAT` or storage paths: renames originals and archive files of
all documents according to the current template ([file names](storage-paths.md)).

<a id="mail"></a>

### Fetching mail

```sh
paperrust manage mail_fetcher
```

Checks all mail accounts now instead of waiting for the next time set by `PAPERRUST_EMAIL_TASK_CRON`
([incoming mail](mail.md)).

<a id="prune-history"></a>

### Prune history (audit log) entries

```sh
paperrust manage prune_audit_logs
```

Deletes audit log entries of objects that no longer exist.

<a id="empty-trash"></a>

### Empty trash

```sh
paperrust manage empty_trash [--doc-ids 1,2,3]
```

Permanently deletes documents that have been in the trash for longer than `PAPERRUST_EMPTY_TRASH_DELAY`
days (or the given ids). Also runs by itself every day.

<a id="create-superuser"></a>

### Create superuser

```sh
paperrust manage createsuperuser                # interactive
DJANGO_SUPERUSER_PASSWORD=… paperrust manage createsuperuser --username admin --email a@b.ch --no-input
paperrust manage manage_superuser               # from PAPERRUST_ADMIN_USER/_MAIL/_PASSWORD
paperrust manage changepassword admin
```

Careful: the password is visible while you type. Make sure nobody is watching, or use
`DJANGO_SUPERUSER_PASSWORD` or `PAPERRUST_ADMIN_PASSWORD` instead.

<a id="archiver"></a>

### Recreate archive PDFs

```sh
paperrust manage document_archiver [-f] [-d ID] [--processes N]
```

Creates archive PDFs for documents that do not have one yet. `-f` recreates them for all documents
(e.g. after switching on PDF/A or cleaning), `-d ID` only for one document, `--processes N` runs N
jobs at the same time.

<a id="fuzzy_duplicate"></a>

### Find near-identical documents

```sh
paperrust manage document_fuzzy_match [--ratio 85] [--delete [--yes]]
```

Compares the texts of all documents and lists pairs that are at least `--ratio` percent the same
(default 85). `--delete` moves the second document of each pair to the trash and asks first (`--yes`
does not ask). Look at the list without `--delete` first!

<a id="consumer"></a>

### Consume the consumption directory by hand

```sh
paperrust manage document_consumer [directory] [--oneshot]
```

Consumes the consumption directory (or the given directory). With `--oneshot` it runs once and then
stops; otherwise the command keeps watching the directory. In normal operation
`paperrust serve --worker` does this.

<a id="check"></a>

### System check

```sh
paperrust manage check [--fail-level WARNING]
```

Checks directories, programs (Tesseract, PDFium) and settings and names missing parts with a hint.
With `--fail-level WARNING` the command already ends with an error code on warnings (for scripts).

### No longer needed

| Command (paperless-ngx) | What to use in PaperRust |
|---|---|
| `convert_mariadb_uuid` | not needed (MariaDB only) |
| `decrypt_documents` | not needed (no longer part of paperless-ngx 3.x either) |

<a id="tasks"></a>

## Tasks and schedule

PaperRust needs no extra services for background work – no Celery, no Redis. With
`paperrust serve --worker` the same program that serves the web interface also takes care of:

- consuming new files (consumption folder, upload, mail, scanner),
- mail checks, workflows and AI processing,
- the regular jobs in the table below.

**The queue lives in PostgreSQL**, in its own tables (`paperrust_*`). That means:

- When PaperRust restarts, no task is lost; open tasks are worked off afterwards.
- If a worker stops in the middle of a task, the task is handed out again – at most
  `PAPERRUST_QUEUE_MAX_DELIVERIES` times (default 3). After that it counts as failed.
- `PAPERRUST_TASK_WORKERS` sets how many tasks run at the same time (default 1); a single task may
  take at most `PAPERRUST_WORKER_TIMEOUT` seconds (default 1800).

Running and finished tasks are shown under *Tasks* and in the *System status* dialog.

**Schedule**

| Job | Default | Setting |
|---|---|---|
| Check mail | every 10 minutes | `PAPERRUST_EMAIL_TASK_CRON` |
| Train classifier | hourly (minute 5) | `PAPERRUST_TRAIN_TASK_CRON` |
| Check scheduled workflows | hourly (minute 5) | `PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON` |
| Empty trash | daily 01:00 | `PAPERRUST_EMPTY_TRASH_TASK_CRON` |
| Update AI index | daily 02:10 | `PAPERRUST_LLM_INDEX_TASK_CRON` |
| Remove expired sign-in sessions | daily 03:00 | `PAPERRUST_SESSION_CLEANUP_TASK_CRON` |
| Sanity check | Sundays 00:30 | `PAPERRUST_SANITY_TASK_CRON` |

The values are cron expressions (e.g. `0 */2 * * *` = every two hours); `disable` switches a job
off. Times are in the time zone `PAPERRUST_TIME_ZONE`.

**Several machines:** further machines can run `paperrust serve --worker-only` (background work
only, no web interface). All of them must see the same database and the same document folders. A
scheduled job still runs only once, however many workers there are.

Without `--worker` (the default; explicitly: `--no-worker`) only the web interface runs. Tasks then
wait in the queue until a worker runs somewhere.
