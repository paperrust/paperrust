# Integrity check

[Deutsch](../integrity.md) · [Administration](administration.md#sanity-checker)

PaperRust regularly checks the archive for data problems – like paperless-ngx with its sanity
checker, plus a few additions for large, long-lived archives.

## What is checked

For **every** document (including the trash and older versions):

| Check | Message (unchanged from paperless-ngx) | Level |
|---|---|---|
| Thumbnail exists and is readable | `Thumbnail of document does not exist.` / `Cannot read thumbnail file of document: …` | error |
| Original exists, is readable, SHA-256 = stored checksum | `Original of document does not exist.` / `Cannot read original file of document: …` / `Checksum mismatch. Stored: …, actual: ….` | error |
| Archive file: filename and checksum are consistent | `Document has an archive file checksum, but no archive filename.` / `Document has an archive file, but its checksum is missing.` | error |
| Archive file exists, is readable, SHA-256 matches | `Archived version of document does not exist.` / `Cannot read archive file of document: …` / `Checksum mismatch of archived document. Stored: …, actual: ….` | error |
| Content (OCR text) empty | `Document contains no OCR data` | info |

In addition, every file below `MEDIA_ROOT` that belongs to no document (except `media.lock`, the app
logo and system files such as `Thumbs.db`, `desktop.ini`, `.DS_Store`) → warning
`Orphaned file in media dir: <path>`.

## When it runs

- **Automatically** as task `sanity_check`, by default **Sundays at 00:30**
  ([`PAPERRUST_SANITY_TASK_CRON`](configuration.md#paperrust_sanity_task_cron), `disable` = off).
- **Manually** in the UI: *Settings → System status → Sanity check → Run*, or on the command line:

  ```sh
  paperrust manage document_sanity_checker               # full (like paperless-ngx)
  paperrust manage document_sanity_checker --incremental # hash changed files only
  ```

## Where the result shows up

- **System status** (*Settings → System status*): "Sanity check" is *OK* if the last run finished
  without errors, *Error* with the result text (e.g. "2 document(s) with errors found. Check logs for
  details."), *Warning* if no check has run yet.
- **Tasks** (*Tasks* in the UI, notifications): a scheduled run with errors appears as a failed task.
- **Log** `paperrust.log` (*Logs* in the UI): every single message (logger
  `paperrust.sanity_checker`, in paperless-ngx `paperless.sanity_checker`) and, added by PaperRust, a start and a finish line
  (logger `paperrust.integrity`), e.g.

  ```text
  [paperrust.integrity] Integrity check (incremental) finished in 41.2 s: 100204 document(s), 12 file(s) hashed (3.1 MiB), 197891 unchanged file(s) skipped, 0 orphaned file(s). Result: No issues detected. Report: …/data/integrity/integrity-20261108T003041.204Z.json
  ```

- **Report** (JSON, machine-readable) per run under `DATA_DIR/integrity/`, see below.

## Incremental mode (large archives)

By default every run reads **all** originals and archive files completely and computes SHA-256 – as
paperless-ngx does. With many gigabytes this takes a while. With

```sh
PAPERRUST_INTEGRITY_INCREMENTAL=true
```

PaperRust remembers path, size, modification time and checksum of every file it verified
successfully (table `paperrust_integrity_state` in the database). In the next run a file is skipped
only if **all** of these are unchanged and the confirmation is younger than
`PAPERRUST_INTEGRITY_FULL_HASH_DAYS` days (default 28). After that it is hashed completely again – so
every file is verified bit for bit at least once per period.

Always checked completely: presence of all files, thumbnails, orphaned files, empty content,
filename/checksum pairs. If a file is replaced, renamed, shortened or extended, or the stored
checksum changes (document reprocessed), it is hashed again immediately.

**Limit:** a silent change that keeps size *and* modification time (e.g. bit rot on the disk) is
found by incremental mode only at the file's next full verification, i.e. after at most
`PAPERRUST_INTEGRITY_FULL_HASH_DAYS` days. If you do not want that, set the value to `0` (every run
hashes everything) or leave the mode off.

Without `--incremental` the command line always checks completely. With the mode enabled, a full run
also refreshes the stored confirmations.

## Report

Every run (scheduled and manual) writes `DATA_DIR/integrity/integrity-<time in UTC>.json`; the newest
`PAPERRUST_INTEGRITY_REPORT_KEEP` (default 10) are kept, older ones deleted. Content:

```json
{
  "format": 1,
  "started_at": "2026-11-08T00:30:00.012Z",
  "finished_at": "2026-11-08T00:30:41.204Z",
  "duration_seconds": 41.19,
  "trigger": "task",
  "mode": "incremental",
  "full_hash_days": 28,
  "threads": 4,
  "status": "error",
  "result": "1 document(s) with errors found. Check logs for details.",
  "documents": 100204,
  "files": { "hashed": 12, "skipped": 197891, "bytes_hashed": 3250585, "orphaned": 0 },
  "counts": { "document_errors": 1, "document_warnings": 0, "document_infos": 0, "global_warnings": 0, "messages": 1 },
  "issues": [
    { "level": "ERROR", "document_id": 4711, "title": "Invoice 2019", "message": "Original of document does not exist." }
  ]
}
```

`status` is `ok` (no message), `issues` (warnings/infos only) or `error`. `trigger` is `task` (worker)
or `manage` (command line). Orphaned files have `document_id: null`.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `PAPERRUST_SANITY_TASK_CRON` | `30 0 * * sun` | schedule (as paperless-ngx), `disable` = off |
| `PAPERRUST_INTEGRITY_INCREMENTAL` | `false` | incremental mode for the scheduled run |
| `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` | `28` | every file is hashed again after at most this many days; `0` = always |
| `PAPERRUST_INTEGRITY_THREADS` | `0` (automatic, at most 4) | read files in parallel; `1` = sequentially like paperless-ngx |
| `PAPERRUST_INTEGRITY_REPORT_KEEP` | `10` | number of JSON reports kept; `0` = no reports |

Invalid values do not make the scheduled run fail: PaperRust logs a warning and checks completely.

## Runtime

Measured on a Windows laptop (debug build, SSD, virus scanner active, builds running in parallel):

| Archive | Run | Duration |
|---|---|---|
| 204 documents, 403 files of 2 MiB (806 MiB) | full, 1 thread | 1.0 s |
| | full, 4 threads (files cached) | 0.35 s (≈ 2.3 GB/s) |
| | incremental, nothing changed | 0.18 s |
| 100,204 documents, 300,000 small files (synthetic) | full, 4 threads | ≈ 20 min |

With small files, opening each file dominates (a few milliseconds per file on Windows with a virus
scanner) – this applies to both modes, because presence and thumbnails are always checked.
Incremental mode saves reading the contents: with 100,000 documents of ~1 MB original and archive
each, about 200 GB per run, i.e. between a few minutes and a few hours depending on the disk (HDD
~150 MB/s, SSD 1–2 GB/s).

## What to do about messages

- **Checksum mismatch / file missing:** restore the file from the backup (see
  [Administration](administration.md)); if the original is lost but the archive file is fine, the
  archive file can serve as the original.
- **Thumbnail missing:** `paperrust manage document_thumbnails -d <ID>` recreates it.
- **Orphaned file:** check it and delete it by hand or consume it as a new document.
- **No OCR data:** *Reprocess* the document in the UI.
