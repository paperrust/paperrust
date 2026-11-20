# Troubleshooting

[Deutsch](../troubleshooting.md) · [Documentation overview](README.md)

Start with PaperRust's log (stderr or service log; verbosity via `PAPERRUST_LOG`, e.g.
`PAPERRUST_LOG=debug`) and the pages *Settings → System status* and *Tasks* in the UI.

<a id="start"></a>

## PaperRust does not start

- **Invalid setting:** PaperRust aborts with a message like Django does (e.g. invalid boolean, invalid
  `PAPERRUST_BIND` address, unknown route id in `routes.toml`). The message names the variable or line.
- **`routes.toml` missing:** if `PAPERRUST_ROUTES` is set and the file is missing → abort. Without the
  variable and without a file every route is on (the file only switches single routes off).
- **"This database is not at paperless-ngx v3.4.2"** (from `paperrust manage migrate`): the database
  comes from a version that `manage migrate` does not catch up by itself – for example paperless-ngx
  2.x or a newer version with unknown changes. Way out: run `paperrust migrate --from …` once
  ([Switching from paperless-ngx](umstieg.md)); older 3.x databases are caught up by `manage migrate`.
- **Port in use:** choose another address in `PAPERRUST_BIND`.

<a id="web-ui"></a>

## UI empty, "Loading…" or 404 on `/`

- The built UI is missing: `frontend-light/dist` or `PAPERRUST_LIGHT_UI_DIR` must contain
  `index.html` and `m.html`. If it is missing, `/rs/app/` shows an error page (503). See
  [Installation → static files](installation.md#static-files).
- Sign-in page without styling: `PAPERRUST_STATICDIR` does not point at the static files
  (`scripts/build-static.sh`).
- If a route is set to `"off"` in `routes.toml`, it answers 404.
- New static files are only picked up after a restart.

<a id="consumer"></a><a id="no-files-are-added-by-the-consumer"></a>

## Files in the consumption directory are not consumed

- Is the worker running? Only `paperrust serve --worker` (or `--worker-only`) watches the folder.
- Is `PAPERRUST_CONSUMPTION_DIR` correct and readable/writable for the PaperRust user? If the folder is
  missing, the log shows an error and the server keeps running.
- With `PAPERRUST_CONSUMER_POLLING_INTERVAL=0` (default) the operating system reports new files; if
  that does not work (e.g. network drive), PaperRust looks every second. Values above 0 = polling at
  that interval. Files are taken after `PAPERRUST_CONSUMER_STABILITY_DELAY`. Ignored patterns: `PAPERRUST_CONSUMER_IGNORE_PATTERNS`.

<a id="upload"></a>

## Uploading in the UI fails

- Is a worker running (`serve --worker` or a separate `--worker-only` process)? Without a worker the
  task stays in the queue.
- Does `routes.toml` contain `"documents.post_document" = "off"`? Then the route answers 404.

<a id="ocr"></a>

## No text recognised / "OCR failed"

- Tesseract found? `PAPERRUST_TESSERACT_PATH`, language data in `PAPERRUST_TESSDATA_DIR`; every language
  in `PAPERRUST_OCR_LANGUAGE` (e.g. `eng+deu`) must exist as `*.traineddata`. Tesseract is only checked
  when a page really needs OCR.
- PDFium found? `PAPERRUST_PDFIUM_PATH`. Without PDFium there are no PDF thumbnails, no barcodes and no
  OCR of scanned PDFs.
- HEIC images (iPhone) are decoded by the helper program `paperrust-pdftool`; without it consumption
  fails with "HEIC decoding is not supported". Photos with 4:2:2/4:4:4 chroma (some Canon/Sony
  cameras) and HEIC image sequences cannot be read.
- Cleaning (`PAPERRUST_OCR_CLEAN`) and deskewing (`PAPERRUST_OCR_DESKEW`) are done by PaperRust
  itself; `clean-final` and `deskew` also replace the page image in the archive PDF.

<a id="office"></a>

## Office document without preview PDF

PaperRust converts office files (including .doc/.xls/.ppt, spreadsheets, presentations) and mails into
an archive PDF with the helper program `paperrust-pdftool` (included in the Docker image,
`PAPERRUST_OFFICE_TO_PDF=true` by default). Only if the program is missing, switched off or the
conversion fails (see the log) does the original stay as it is: it is stored and searchable, the
thumbnail shows the extracted text.

<a id="classifier"></a><a id="classifier-error-no-training-data-available"></a>

## "No automatic matching items, not training"

No tag, correspondent, document type or storage path uses the *Auto* matching algorithm. Set at least
one object to "Auto", assign a few documents, then run `paperrust manage document_create_classifier`.

<a id="search"></a>

## Search finds nothing

If the search index does not match (e.g. after changing `PAPERRUST_SEARCH_LANGUAGE` or after an
update), the worker rebuilds it at start-up (`serve --worker`). Without a worker, search returns
nothing until the index is rebuilt: `paperrust manage document_index reindex`.

<a id="redis"></a>

## Redis messages in the log

PaperRust never connects to Redis. If `PAPERLESS_REDIS` is still in the configuration (taken over
from paperless-ngx), the log only shows a hint: Redis can be shut down and the variable removed. On
the system status page the "Redis" fields describe PaperRust's cache (process-local or
`PAPERRUST_CACHE=postgres`), not a Redis service.

<a id="missing"></a>

## A function answers 404

See [what is missing without Django](installation.md#standalone-limits) (only `/admin/` and the allauth "headless" API). The start-up log lists
the missing paths. Forwarding to Django as a way out no longer exists.

<a id="mail"></a>

## Mail fetching

- Schedule `PAPERRUST_EMAIL_TASK_CRON`; right away: `paperrust manage mail_fetcher`.
- Errors per message under *Mail → Processed mail*.
- Each IMAP step has a 60 s time limit.
- OAuth (Gmail/Outlook) works without Django; so far only checked with test values, not with real accounts ([Mail](mail.md)).

<a id="collation"></a><a id="postgresql-warns-about-a-collation-version-mismatch"></a>

## PostgreSQL warns about a "collation version mismatch"

As with paperless-ngx: after an OS/ICU update run `REINDEX DATABASE paperless;` and
`ALTER DATABASE paperless REFRESH COLLATION VERSION;` (as owner or superuser).

<a id="report"></a>

## Reporting bugs

Please include version, mode (server/Docker/desktop), a log excerpt and steps to reproduce, in the
PaperRust issue tracker – not at paperless-ngx. Vulnerabilities confidentially:
[SECURITY.md](../../../SECURITY.md).
