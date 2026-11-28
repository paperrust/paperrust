# Frequently asked questions

[Deutsch](../faq.md) · [Documentation overview](README.md)

<a id="what"></a>

## What is PaperRust – and what is it not?

A document archive that grew out of paperless-ngx v3.4.2, rewritten in Rust: same REST API, same
database, same files – but a single program without Python, Redis, Celery, Tika and Gotenberg. On top
of that it has its own interface (PaperRust design) and its own features such as Automation with a
test run, built-in AI, PDF tools and ransomware protection. The classic interface in the style of
paperless-ngx can still be chosen. PaperRust is an **independent project** and not affiliated with the paperless-ngx team; please do not
report PaperRust bugs to paperless-ngx.

<a id="apps"></a>

## Do my apps and scripts keep working?

Yes, if they use the paperless-ngx REST API: paths, fields, status codes and error texts are the same
(checked with parity tests against Django). Only `/admin/` and the allauth "headless" API are
missing ([list](installation.md#standalone-limits)).

<a id="data"></a>

## Do I have to convert my data?

No. One command (`paperrust migrate --from …`) or the assistant in the desktop app takes over
everything; with PostgreSQL, if you want, directly in the same database (with an automatic backup).
SQLite and MariaDB are copied to PostgreSQL along the way. See
[Switching from paperless-ngx](umstieg.md).

<a id="back"></a>

## Can I go back to paperless-ngx?

Yes, but no longer with the same database: PaperRust adapts the schema after the switch (tags
first). Going back works via the backup from the switch or via an export from PaperRust
(`document_exporter`, paperless-ngx 3.4.2 format) imported into an empty paperless-ngx. See
[Switching → going back](migration.md#back).

<a id="where"></a>

## Where are my documents?

As in paperless-ngx below `PAPERRUST_MEDIA_ROOT` (`documents/originals`, `documents/archive`,
`documents/thumbnails`), named according to `PAPERRUST_FILENAME_FORMAT` or storage paths
([file names](storage-paths.md)). The desktop app keeps everything below `%LOCALAPPDATA%\PaperRust`
(Windows).

<a id="file-types"></a>

## Which file types are supported?

As paperless-ngx: PDF, images (PNG, JPEG, TIFF, GIF, WebP, HEIC/iPhone photos), text, Office (DOCX,
XLSX, PPTX, ODT/ODS/ODP/ODG, RTF and the legacy formats DOC, XLS, PPT), e-mails (EML). Office files and
mails are read **without Tika**; PaperRust renders the PDF version itself (paperless-ngx uses
Gotenberg) with WordCraft in the helper program `paperrust-pdftool` (see
[Office files on ingest](pdf-tools.md#office-beim-einlesen)). Without the helper the original
is stored without PDF version but stays searchable.

<a id="duplicates"></a>

## Are duplicate documents rejected?

As in paperless-ngx 3.x: duplicates are consumed and marked by default; with
`PAPERRUST_CONSUMER_DELETE_DUPLICATES` they are discarded. The document list has a duplicates filter.

<a id="resources"></a>

## How much memory does PaperRust need? Does it run on a Raspberry Pi?

Measured on a small ARM computer (6 cores, 8 GB RAM, Docker): right after
starting, about 100 MiB for PaperRust plus about 70 MiB for PostgreSQL. After consuming documents with
text recognition, PaperRust held on to about 900 MiB in this measurement; this has been improved since
(language models are freed after a period of inactivity), and a new measurement will follow. The
database needs more memory and disk space with PaperRust than with paperless-ngx. A Raspberry Pi 4/5
with a 64-bit system should work (same architecture), but has not been measured on its own. Numbers
and conditions: [release notes → performance](../../RELEASE-NOTES-0.1.0.en.md#performance).

<a id="ai"></a>

## Does PaperRust use AI, and is my data private?

AI is off by default. When enabled, document content is sent to the configured model – with a local
model (Ollama) it stays on your machine. See [AI](ai.md).

<a id="broker"></a>

## Do I need Redis/Valkey?

No. Queue, scheduler and status messages run over PostgreSQL. When switching from paperless-ngx,
the Redis/Valkey service (and `PAPERLESS_REDIS`) can be removed afterwards. For several processes (a
separate worker) there is a shared cache in PostgreSQL: `PAPERRUST_CACHE=postgres`.

<a id="docker"></a>

## Is there a Docker image?

The image can be built from the repository (`docker/Dockerfile`, amd64 and arm64); it has not been
published yet ([Installation → Docker](installation.md#docker)).

<a id="why-rust"></a>

## Why is it faster?

A compiled program with an asynchronous web server instead of a Python interpreter, plus queries
without N+1 (the document list needs a fixed number of database queries per page, no matter how many
documents). Measurements and their limits:
[release notes → performance](../../RELEASE-NOTES-0.1.0.en.md#performance).
