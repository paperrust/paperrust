# PaperRust 0.1.0 – release notes

[Deutsch](RELEASE-NOTES-0.1.0.md) · As of 2026-11-20 (draft, approval according to
[docs/RELEASE-GATE.md](RELEASE-GATE.md) is pending)

The first version of PaperRust. This page says what PaperRust is, what it can do, what is new
compared to paperless-ngx – and, honestly, what does not work (yet).

> **Please stay healthily sceptical.** PaperRust 0.1.0 is a first version. The core is thoroughly tested –
> but not with your documents. Before switching: make a **full backup** of paperless-ngx (export, or database
> and media folder), **try PaperRust with a copy first**, and only turn paperless-ngx off once everything is
> right. Don't migrate blindly. The way back to paperless-ngx stays open.

## What is PaperRust?

PaperRust is an archive for your documents: scan paper or drop files, PaperRust recognises the
text, assigns sender, document type and tags, and makes everything searchable.

PaperRust grew out of [paperless-ngx](https://github.com/paperless-ngx/paperless-ngx) 3.4.2 and was
rewritten in Rust. It understands the same database, the same files and the same REST API. But it
is **a single program** plus PostgreSQL – without Python, Redis, Celery, Tika and Gotenberg. PaperRust
is an independent project, not affiliated with the paperless-ngx team, and like paperless-ngx it is
licensed under the GPL-3.0.

## Main features

- **Adding documents** from the consumption folder, upload (also drag and drop), mailboxes, scanners
  (scan button via AirScan/eSCL, "scan to e-mail" from the printer) and the REST API.
- **File types:** PDF, images (PNG, JPEG, TIFF, GIF, WebP, **HEIC** from iPhones), text, Office (DOCX,
  XLSX, PPTX, ODT/ODS/ODP/ODG, RTF, also the old **DOC/XLS/PPT**), e-mails (EML) – Office files and
  mails get an **archive PDF** (mail PDF with selectable layout).
- **Text recognition** with Tesseract, with built-in **cleaning** and **deskewing** of crooked scans,
  archive as **PDF/A**, shrinking of the archive file, optionally Windows text recognition or Azure
  cloud recognition.
- **Organising:** correspondents, document types, nested tags, storage paths (with a builder), custom
  fields, ASNs (also several number ranges), saved views; automatic matching that learns from your
  decisions.
- **Searching:** full text with field, date and wildcard search, similar documents, global search.
- **Sharing:** share links (also for several documents), sending by e-mail, permissions per document.
- **File versions** of a document, trash, change history.
- **Several users** with roles, groups, two-factor login, login via OpenID Connect, GitHub, GitLab,
  Google, Microsoft, Nextcloud, Discord, Gitea, e-mail verification.
- **Desktop app for Windows and Linux** (x86_64) with everything included (PostgreSQL, Tesseract,
  PDFium) – on Windows also as a window onto a server, with an offline copy of selected documents.
  macOS is planned.
- **User interface in 10 languages** (de, en, fr, it, es, nl, pt-BR, pl, cs, sv), manual built in
  (`/rs/docs/`, also offline).

## New compared to paperless-ngx

- **One program instead of five services:** `paperrust serve --worker` replaces web server, Celery
  worker, scheduler and consumption folder; queue and cache live in PostgreSQL. One Docker image of
  0.65 GB instead of four images with 6.5 GB.
- **Own user interface (PaperRust design):** workspace with list, preview and details side by side,
  inbox with "Done", dashboard made of widgets, timeline with deadlines, command bar (Ctrl + K) that
  also **finds settings**, **create tags and correspondents right in the field**, a **tour** in twelve
  steps, light/dark. The classic interface in the style of paperless-ngx can still be chosen.
- **Automation:** workflows as a readable sentence, cards "When – Only if – Then – Otherwise",
  templates, suggestions from your own archive, a **dry run** with before/after before applying, a
  **history** per workflow, an **otherwise branch**, triggers "Manual" and "E-mail arrives"
  ([Automation](user/en/automatik.md)).
- **AI built in** (off until you switch it on; locally with Ollama or with OpenAI-compatible
  services): suggestions with a **review list** (accept, change, reject per field, undo), filling in
  custom fields, **AI text recognition** with a vision model, own prompts, processing the existing
  archive, AI condition in workflows, workflow draft from a description, chat with sources, **MCP
  server** for external agents. Replaces paperless-ai and paperless-gpt
  ([AI](user/en/ai.md#replaces)).
- **PDF tools:** real **redaction** (the content is removed, not covered, with a check),
  **shrinking**, **convert to PDF**, repairing broken PDFs while consuming – isolated in a separate
  program with time and memory limits ([PDF tools](user/en/pdf-tools.md)). When **sharing**, PaperRust
  shows which version is redacted and preselects it for new links and bundles.
- **Ransomware protection:** a guard that switches to read-only on suspicious changes; encrypted
  backups with versions, also on S3 with Object Lock; scheduled integrity check of all files
  ([Ransomware](user/en/ransomware.md)).
- **Settings in the user interface** instead of only environment variables, with origin display,
  history, export/import and moving over from `docker-compose.env`.
- **Roles** (Read only, Edit, Manage, Administrator or your own) and "Visible to" per document.
- **Scanner connection** without the NAS password on the device, printing **ASN labels**, splitting
  batches with separator sheets.
- **Windows desktop app without login:** locally the app signs its owner in automatically (a key per
  start, readable only by this Windows account); access from other devices still needs a password
  ([Installation](user/en/installation.md#desktop-login)).
- **New management commands:** `settings`, `backup`, `guard`, `datamodel`, `sync`, `timeline_backfill`,
  plus all the known ones (`document_exporter`, `document_archiver`, `document_fuzzy_match`,
  `document_consumer`, `check` …) – [Administration](user/en/administration.md).

<a id="performance"></a><a id="leistung"></a>

## Performance

Measured on 20 Nov 2026 on a **small arm64 board** (6 cores, 8 GB RAM, NVMe) in Docker,
**one run** per setup, with **255 made-up documents**. PaperRust (release state `e546b8f`, default
settings) was compared with paperless-ngx 3.4.2 in its leanest setup (Valkey, no Tika/Gotenberg,
default settings), both with PostgreSQL 17 and 4 cores each. Both clean and deskew scans – so the
same work. Report:
[results/benchmark/2026-11-20-arm64/bericht.md](../results/benchmark/2026-11-20-arm64/bericht.md)
(German), all tables: [tabellen.md](../results/benchmark/2026-11-20-arm64/tabellen.md).

| | paperless-ngx | PaperRust |
|---|---:|---:|
| First start until login is possible | 53.2 s | 3.3 s |
| Restart until the document list answers | 33.7 s | 0.57 s |
| List/search while 230 documents are being consumed (median / 95 %) | 150 / 187 ms | 10 / 32 ms |
| 10 people at once (median / 95 %) ¹ | 77 / 255 ms | 4 / 16 ms |
| Consume 230 documents (200 text PDFs, 30 scans) | 29 min 56 s | 1 min 46 s |
| One scan (1 page) until searchable | 25.8 s | 8.1 s |
| One phone photo (JPG) until searchable | 32.3 s | 9.4 s |
| Memory freshly started (services + database) | 598 + 41 MiB | 74 + 64 MiB |
| Memory idle after consuming (services + database) | 702 + 49 MiB | 420 + 74 MiB |
| Large archive (2,230 docs), 8 requests at once: requests/s ² | 7.3 | 360 |
| Docker images without PostgreSQL ² | 3.47 GB (2 containers) | 0.65 GB (1 container) |

¹ On 20 Nov measured without uploads and shortened (the folder with the upload files was missing in
the benchmark script; same for both systems). With uploads on 19 Nov (state `5f5de7c`): 87 / 307 ms
vs. 5 / 18 ms. ² Measurement of 18 Nov 2026 (state `c97f15c`); the large archive against the full
paperless-ngx setup (with Tika and Gotenberg), not repeated.

Conditions and limits:

- paperless-ngx with Tika and Gotenberg (then it reads Word files) but without cleaning/deskewing
  needs 25 minutes (1,493 s) for the batch and 19.9 s for one scan; it then keeps 1,317 + 49 MiB
  after consuming.
- **Where PaperRust is worse:** the database needs more memory (fresh 64 instead of 41 MiB, after
  consuming 74 instead of 49 MiB, peak while consuming 115 instead of 52 MiB). On disk the database
  takes 11 % more (97.8 instead of 88.2 MiB, logically 28.4 instead of 18.2 MB), the files 6 % more
  (54.7 instead of 51.4 MiB); for the large archive (18 Nov) the database was twice as large. The
  peak while consuming, 800 MiB (services), is only slightly below paperless-ngx (838 MiB).
- Only one device, clean made-up documents, API measurement only (no rendering time in the browser).
  Do not over-interpret differences below about 10 %. 10,000+ documents not measured.

**Checks before release** (as of 2026-11-20): 24-hour soak test under mixed load without a single
server error, memory stable ([report](../results/release/soak/2026-11-19-24h/BERICHT.md), German);
independent security review of the code, all findings fixed
([report](../results/release/review/security-2026-11-19.md), German); real installation test
of the desktop app (update, uninstall) and Docker smoke test on amd64.

<a id="switching"></a>

## Switching from paperless-ngx

- **One command:** `paperrust migrate --from <paperless folder | database URL | export>` – or
  "Switch from paperless-ngx" in the desktop app. PaperRust checks everything first, shows progress
  and at the end a **reconciliation** (documents, tags, users, files).
- **Versions:** paperless-ngx 2.20 (and 2.19.6), 3.0 to 3.4.2 and the development version after
  3.4.2; PostgreSQL directly, SQLite and MariaDB are copied into PostgreSQL.
- **What comes along:** documents with files, tags, correspondents, types, storage paths, fields,
  notes, views, workflows, mail rules, users **with their passwords** and permissions.
- **Backup:** if the existing database is converted, PaperRust backs it up first by itself.
- **One-way street:** afterwards paperless-ngx can **no longer open** the same database. The way back
  is only the backup or an export from PaperRust (paperless-ngx 3.4.2 format). If unsure, switch into
  a **copy** (`--target`); then paperless-ngx stays untouched.
- Tested with test databases 2.20.15, 3.1.3, 3.4.2 and a real 3.1.3 database (without checking the
  files).

Step by step: [Switching from paperless-ngx](user/en/umstieg.md).

<a id="limits"></a><a id="grenzen"></a>

## Known limits

Honestly – this does not work (yet), or works differently than in paperless-ngx:

**Installation and operation**

- The **Windows installer is not digitally signed**; Windows SmartScreen warns. Compare the checksum.
  Code signing through the SignPath Foundation is planned.
- The **Linux package** (`tar.gz`, x86_64 only) is new: checked on fresh Ubuntu 24.04 and Debian 12
  (start, login, text recognition), but not much used in daily life yet. It needs glibc 2.36 or newer and must not run as root; without WebKitGTK 4.1 and libayatana-appindicator it opens
  in the browser instead of its own window.
- There is no desktop app for **macOS** yet (planned; it needs a Mac and an Apple developer account).
- The **Docker image** (`ghcr.io/paperrust/paperrust`, amd64 and arm64) is measured on arm64 and
  smoke-tested on amd64. There are no ready-made server packages (deb/rpm).
- **Not specifically tested:** Safari (macOS/iOS), Firefox, Android browsers and screen readers
  (contrast and keyboard are tested). The **NAS templates** (Synology, UGREEN, Asustor, Unraid, CasaOS)
  have not been started on a real NAS. Feedback is very welcome.
- **PostgreSQL only** (14 or newer). SQLite/MariaDB are copied when switching; afterwards sorting and
  case handling of umlauts may differ slightly.
- Full functionality needs **Tesseract**, **PDFium** and **`paperrust-pdftool`** (included in the image
  and the desktop app). If they are missing, some functions continue with only a note in the log.
  `gpg` (encrypted mails) and `pdftoppm` (barcodes without PDFium) are not in the image.
- Several processes (web server and worker separate) need `PAPERRUST_CACHE=postgres`.
- Without `--worker` there are no schedules, no mail fetching and no folder watching.
- No `/admin/` (Django admin) and no allauth "headless" API.
- **Set `PAPERLESS_URL`** when PaperRust is reachable from the network: without it, links in password
  emails come from the request (as in paperless-ngx). Behind an HTTPS proxy, set the cookie/proxy
  options from the operations guide.
- An export from **paperless-ngx 1.x** cannot be taken over (not by paperless-ngx 3 either): update to
  2.20 first.
- The desktop app has no "log out" locally (automatic sign-in).

**Consuming, text recognition, files**

- **PDF/A** is produced and checked by PaperRust itself, but not confirmed with veraPDF. If the original
  does not embed its fonts, a normal PDF results (note in the log).
- Some **OCRmyPDF options** from `OCR_USER_ARGS` have no effect (e.g. `unpaper_args`,
  `remove_background`, `fast_web_view`); there is a warning.
- Cleaning and deskewing are own implementations: similar result, but not pixel-identical.
- **Office PDFs** look different from LibreOffice/Gotenberg: presentations only as text pages (without
  images and shapes), spreadsheets without colours and charts. Office metadata is sparser than with
  Tika.
- **HEIC:** no photos with 4:2:2/4:4:4 chroma subsampling and no image sequences.
- **TIFF → PDF** is always recompressed; files are larger than with paperless-ngx.
- **Thumbnails** are about 1.6 times as large as with paperless-ngx.
- **Barcodes** are read with own building blocks; with distorted or blurry scans the result may differ.
- A failed upload from the user interface is deleted and has to be uploaded again.

**Searching, organising, workflows**

- **Automatic matching** uses its own format: it learns again after switching, suggestions may differ
  for single documents.
- **Regular expressions:** special forms of Python's `regex` (e.g. `{e<=1}`, recursion) do not work.
- **Relative dates** in field queries only in the 10 UI languages; some Jinja details in templates are
  missing.
- The **otherwise branch** of workflows is a PaperRust extension: paperless-ngx and the export do not
  know it yet (in the export, otherwise actions currently end up as loose actions).
- The **dry run** only checks the conditions for "Scheduled" and only simulates "Consumption".
- What the **AI** changes automatically is recorded in the AI log (with undo), not in the document's
  normal change history.

**Mail, login, API**

- **Social login** only with OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea; other allauth
  providers are missing. Not tested against the real providers.
- **Gmail/Outlook via OAuth** only checked with test values; proven is an account with (app) password.
- Without a working mail server, "Forgot password" still says "sent" (error in the log).
- **Error messages** of the API and the sign-in pages are in English only.
- The user interface has **10 languages** instead of over 30; other languages see English.
- The REST API accepts only JSON on most endpoints (no forms, no "browsable API").
- **HuggingFace embeddings** only through your own service (text-embeddings-inference) or the
  HuggingFace API, not inside the program.

**Not checked against real counterparts:** Azure text recognition, S3 backup, scanner hardware,
social login providers, OAuth mail, TLS for IMAP/SMTP, GPG with a real keyring, AI text recognition
with a real vision model. Everything is covered by tests and simulated responses.

All differences per component are listed in `docs/contract/*.md` (section "Known differences"), open
gaps compared to paperless-ngx in [docs/LUECKEN-NGX.md](LUECKEN-NGX.md) (German).

<a id="requirements"></a>

## System requirements

**Server (Docker or directly)**

- Linux on **x86-64 (amd64)** or **64-bit ARM (arm64)**, e.g. Raspberry Pi 4/5 with a 64-bit system,
  ARM NAS. 32-bit ARM is not supported. Windows works with a self-built program.
- **PostgreSQL 14 or newer** (checked with 17 and 18).
- **Memory:** recommended from 2 GB for the whole device with a small archive (measured: 494 MiB
  for PaperRust and database idle after consuming, peak while consuming 915 MiB, see
  [Performance](#performance)).
- **Disk:** room for your documents plus about 30–40 % for archive PDFs, thumbnails, search index and
  database; image 0.65 GB plus PostgreSQL 0.69 GB.
- For text recognition: Tesseract 5 with language data, PDFium (both in the Docker image).

**Desktop app**

- **Windows 10 or 11, 64-bit** (x86-64). Installer about 68 MB, includes PostgreSQL 17, Tesseract 5
  (German, English) and PDFium. On Windows 10 Microsoft WebView2 may be needed.
- **Linux x86_64** with glibc 2.36 or newer (Debian 12, Ubuntu 24.04, Fedora 37 or newer), as a
  `tar.gz` with PostgreSQL 17, Tesseract 5 (German, English) and PDFium. For its own window it needs
  WebKitGTK 4.1 and libayatana-appindicator, otherwise it uses the browser. Do not run it as root.
- No macOS package (planned).

**Browser:** a current browser. Automated tests use Chromium (Chrome, Edge); Firefox and Safari
should work but are not tested separately.

## License

GPL-3.0-only. PaperRust is based on paperless-ngx (GPL-3.0, © the paperless-ngx authors). Please do
not report PaperRust bugs to paperless-ngx.
