# Switching from paperless-ngx

[Deutsch](../migration.md) · [Documentation overview](README.md)

PaperRust takes over the database schema of paperless-ngx v3.4.2 and the same file layout.
Switching is therefore not a data import but a change of the program that points at database and
files. **Afterwards** PaperRust adapts the schema step by step to its own data model (first step:
tags without the django-treenode helper columns). From then on paperless-ngx can no longer open the
same database – going back works via the backup or an export ([going back](#back)). Which way fits:

| Starting point | Way |
|---|---|
| **any** – recommended | [One-step switch with verification report](#one-step) (picks the right way itself) |
| paperless-ngx with **PostgreSQL** | [Keep using the same database](#same-database) |
| paperless-ngx with **SQLite** or **MariaDB/MySQL** | [Move the database with `paperrust-migrate-db`](#sqlite-mariadb) |
| new machine and the files move along, or paperless-ngx before 2.19.6 or with a change after 3.4.2 that PaperRust does not know | [Export and import](#export-import) |
| just trying it | on a **copy** of the database ([one-step switch](#one-step) with a new target database); paperless-ngx stays untouched. [Mixed operation](installation.md#mixed-operation) no longer exists |

> [!IMPORTANT]
> Back up before every step: database (`pg_dump -Fc`), `data/` and `media/`
> ([Administration](administration.md#backup)). PaperRust uses the schema of paperless-ngx
> **v3.4.2**. Databases of paperless-ngx **2.20** (also 2.19.6) and **3.0.0 to 3.4.1** are brought to
> that state by PaperRust itself (the same Django migrations as paperless-ngx, without Python, see
> [switching → 2.20](umstieg.md#v2)); updating paperless-ngx first is not needed. Databases of a newer
> paperless-ngx with the known changes after 3.4.2 (new tag tree) are taken over as they are. Older
> releases need an export.

<a id="one-step"></a>

> **Please stay healthily sceptical.** PaperRust 0.1.0 is a first version. The core is thoroughly tested –
> but not with your documents. Before switching: make a **full backup** of paperless-ngx (export, or database
> and media folder), **try PaperRust with a copy first**, and only turn paperless-ngx off once everything is
> right. Don't migrate blindly. The way back to paperless-ngx stays open.

## One-step switch (recommended)

You only say **where** your paperless-ngx data is. PaperRust detects the kind of source, picks the
way, takes everything over and verifies the result. The source is **only read**; files are copied
without overwriting.

| You give | PaperRust detects | Way |
|---|---|---|
| export folder (with `manifest.json`) or export ZIP from `document_exporter` | export, any recent paperless-ngx version | import (`document_importer`) |
| paperless folder with `data/` and `media/`, the `data` folder or `db.sqlite3` | paperless-ngx with SQLite, media folder next to it | database move + copy files |
| `postgres://…`, `mariadb://…`, `mysql://…`, `sqlite:///…` (+ media folder) | database | database move + copy files |

A database of paperless-ngx **2.20 or 3.0.0 to 3.4.2** is taken over directly; if it is older than
3.4.2, PaperRust applies the missing changes – when copying into a new database without touching the
source (a 2.x source is copied with its own schema, verified and converted in the target within the
same transaction), on the same database after an automatic backup (pg_dump file or a copy of the
database on the server). For 2.x give the media folder: the checksums are recomputed from the files
(SHA-256) like paperless-ngx 3 does. A database with the known changes after 3.4.2 – i.e. also
paperless-ngx **3.5.0/3.5.1** (upstream `documents.0028`/`0029`/`0030`) – is taken over. If it is older than 2.19.6 or has an unknown newer change,
the switch stops before writing anything and explains the way out: create an export in paperless-ngx
and give that instead (exports of any recent version are read).

### Desktop app (Windows)

1. Stop paperless-ngx. paperless-ngx 2.20 and 3.0 to 3.4 are taken over directly; for older
   releases run `document_exporter` in paperless-ngx and use the export.
2. Install and start PaperRust. On first start the choice appears.
3. Choose "Switch from paperless-ngx" (console: `2`) and enter **one** path or URL, e.g.
   `D:\paperless` (folder with `data` and `media`), `D:\export\export.zip` or
   `postgres://paperless:PASSWORD@nas:5432/paperless`. For a database URL you are asked for the old
   media folder.
4. Wait. PaperRust then opens as usual; sign in with your existing users and passwords.
5. **Verification report:** after the first start the app shows the comparison itself (result,
   summary, first notes, button "Ganzen Bericht öffnen" = open full report) before it opens the
   interface. The app's log folder (`…\PaperRust\logs\`) contains `umstieg-bericht.html` (plus `.json` and `.txt`): all object
   types side by side, every file checked, and a spot check through the running app. At the top:
   **OK**, **WARN** or **FAIL** (meaning below).

Without a dialog (managed installs): `PAPERRUST_DESKTOP_IMPORT=<path or URL>`, optionally
`PAPERRUST_DESKTOP_IMPORT_MEDIA=<media folder>`.

### Docker / server

The `paperrust-migrate-db switch` command ships in the image. Inside the container the target
database (`PAPERRUST_DBHOST` …) and media folder (`PAPERRUST_MEDIA_ROOT`) are already in the
environment; the report goes to `data/umstieg/`.

1. **Back up** (see above) and stop paperless-ngx: `docker compose down`.
2. Swap the image to PaperRust in the compose file
   ([what to change](installation.md#migrate)). For
   SQLite/MariaDB also add a `db` service with PostgreSQL and an empty database and set
   `PAPERRUST_DBHOST=db`. The `data` and `media` volumes stay as they are.
3. Dry run (only checks, writes nothing):

   ```sh
   docker compose up -d db
   # paperless-ngx with SQLite (data folder in the data volume):
   docker compose run --rm webserver paperrust-migrate-db switch --dry-run --from /usr/src/paperless/data
   # paperless-ngx export (folder or ZIP on the host):
   docker compose run --rm -v /path/to/export:/export webserver paperrust-migrate-db switch --dry-run --from /export
   # MariaDB: --from "mariadb://paperless:PASS@mariadb:3306/paperless" --media /usr/src/paperless/media
   ```

4. The same command without `--dry-run` performs the switch.
5. `docker compose up -d`, then add the spot check through the running app:

   ```sh
   docker compose exec webserver paperrust-migrate-db verify --from <as above> \
       --api-url http://localhost:8000 --api-temp-token
   ```

**PostgreSQL without a move** (only the image swapped, same database): after starting,
`docker compose exec webserver paperrust-migrate-db switch --from "postgres://paperless:PASS@db:5432/paperless" --api-url http://localhost:8000 --api-temp-token`
– source and target are the same database, so it only verifies.

Without Docker the same works with `--target postgres://…` and `--target-media <folder>`;
`paperrust-migrate-db switch --help` lists all options.

### The verification report

| Part | What is compared |
|---|---|
| Objects | count per type in source and target: documents, tag assignments, tags, correspondents, document types, storage paths, custom fields and field values, notes, saved views, workflows (with triggers/actions), users, groups, mail accounts and rules, share links |
| Documents | every row: title, content (SHA-256 of the text), tags, checksums, trash state |
| Files | every original and archive file against the checksum in the database (SHA-256, older data MD5), thumbnails present, missing and orphaned files – in source **and** target |
| Spot check | 10 documents (`--sample`) through API v1 of the running app: title, content, tags and checksum of the downloaded original (documents with versions: newest version, as the app shows it) |

| Result | Meaning |
|---|---|
| **OK** | everything taken over and verified |
| **WARN** | taken over, but with notes: e.g. a file was **already broken in paperless-ngx** ("bereits in der Quelle"), thumbnail missing, orphaned files, spot check not possible, export from another version |
| **FAIL** | something is missing or differs (count, document row, file checksum) or the switch was not possible – the report lists every case. The source is unchanged; paperless-ngx can keep running as before |

Exit code: `0` OK, `3` WARN, `1` FAIL. `--api-temp-token` uses the API token of an active
administrator from the target database, or creates one for the spot check and deletes it afterwards.

<a id="same-database"></a>

## Keep using the same database (PostgreSQL)

1. Back up (`pg_dump -Fc`). paperless-ngx must be **at least 3.0.0**; an update to 3.4.2 is not
   needed.
2. Stop paperless-ngx (web server, Celery worker, scheduler, consumer). Let open tasks finish first.
3. Start PaperRust with **the same** configuration: same `PAPERLESS_DB*` values, same
   `PAPERLESS_DATA_DIR`, `PAPERLESS_MEDIA_ROOT`, `PAPERLESS_FILENAME_FORMAT` and above all **the same
   `PAPERLESS_SECRET_KEY`** (otherwise sessions, share links and encrypted fields become invalid).
4. `paperrust manage migrate` – checks the migration state, applies the missing Django migrations for
   paperless-ngx 3.0.0–3.4.1 (one transaction, like Django on an update; aborts for 2.x or newer than
   3.4.2), adds missing permissions/content types and creates PaperRust's own `paperrust_*` tables.
   With comparison and automatic backup: `paperrust migrate --from postgres://…` (source = target,
   see above).
5. `paperrust serve --worker` ([Installation](installation.md#standalone)).

What stays the same: users, passwords, API tokens, sessions, documents, tags, workflows, mail
rules, saved views, share links, audit log, task list.

Example from the project (docs/STANDALONE.md): paperless-ngx 3.1.3 in Docker with PostgreSQL 17.4 →
backup → swap the image to PaperRust → `paperrust migrate` (or simply start it) applies the five
missing migrations of 3.1.3. No database move, no intermediate step via 3.4.2 needed.

<a id="sqlite-mariadb"></a>

## SQLite or MariaDB/MySQL → PostgreSQL

PaperRust runs on PostgreSQL only. The easiest way is `paperrust migrate --from …`
([Switching from paperless-ngx](umstieg.md), case 4). Underneath works the tool
`paperrust-migrate-db` (crate `pl-dbmigrate`), which you can also call directly. It copies a
paperless-ngx database (2.20/2.19.6 and 3.0 to 3.4.2; missing steps are caught up in the target)
completely to PostgreSQL. The source is **only read**, the target must be
a new, empty database. The move runs in **one** transaction: on any error (also Ctrl+C) the target
stays empty.

```sh
# 1. stop paperless-ngx completely
# 2. empty target database
psql -U postgres -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
# 3. dry run: reads every row, checks every conversion, writes nothing
paperrust-migrate-db --dry-run --source sqlite:///path/to/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless
# 4. move, with report
paperrust-migrate-db --source sqlite:///path/to/data/db.sqlite3 \
    --target postgres://paperless:PASS@localhost:5432/paperless --report move.json
# 5. optionally compare again later
paperrust-migrate-db --verify-only --source … --target …
```

- Sources: `sqlite:///absolute/path/db.sqlite3` (Windows `sqlite:///C:/path/db.sqlite3`),
  `mariadb://user:pass@host:3306/paperless`, `mysql://…`, `postgres://…`. Pass passwords preferably via
  `PAPERRUST_MIGRATE_SOURCE` / `PAPERRUST_MIGRATE_TARGET` (the old names `PAPERLESS_MIGRATE_*` still work).
- Checked: row count and checksum per table, sequences, all constraints and indexes.
- Afterwards set `PAPERRUST_DBENGINE=postgresql` and the `PAPERRUST_DB*` values, **leave everything else
  unchanged** (paths, `PAPERLESS_SECRET_KEY`). If the read cache was active
  (`PAPERLESS_DB_READ_CACHE_ENABLED`) and paperless-ngx still runs: `manage.py invalidate_cachalot`
  before the first start.
- Special cases (NUL characters, invalid UTF-8, over-long texts, foreign tables) are reported one by one
  by the dry run; options `--nul replace|strip`, `--invalid-utf8 replace`, `--skip-unknown-tables`.
  Nothing is changed silently.

Full guide with all options and special cases (German): [docs/MIGRATION.md](../../MIGRATION.md).


After the move PostgreSQL behaves slightly differently from SQLite/MariaDB (same for paperless-ngx on
PostgreSQL): sorting follows the collation, `NULL` sorts last in ascending order, `icontains` with
umlauts (ü = Ü, ü ≠ u), JSON is normalised.

<a id="export-import"></a>

## Export and import

The paperless-ngx way works with PaperRust too, in both directions:

```sh
# on the old installation (paperless-ngx)
docker compose exec webserver document_exporter ../export        # or manage.py document_exporter
# on the new installation (PaperRust, empty database after "paperrust manage migrate")
paperrust manage document_importer /path/to/export
```

Advantages: files move along, the source need not be exactly 3.4.2 (same version recommended).
Disadvantages compared with the database move: API tokens, sessions, task list, mail processing state
(`processedmail`), workflow runs and allauth e-mail addresses are not exported; twice the disk space.
Comparison (German): [docs/MIGRATION.md](../../MIGRATION.md#welcher-weg).

<a id="files"></a>

## What lives outside the database

| Location | After switching |
|---|---|
| `MEDIA_ROOT` (originals, archive, thumbnails) | used unchanged |
| `DATA_DIR/index` (full-text index, tantivy) | same format, read and written further; rebuild with `paperrust manage document_index reindex` if needed |
| `DATA_DIR/classification_model.pickle` | **not** read by PaperRust. PaperRust trains its own model (`DATA_DIR/paperrust_classifier.model`), on schedule or right away with `paperrust manage document_create_classifier` |
| `DATA_DIR/llm_index` (AI vector index) | same format (SQLite + sqlite-vec), can be shared |
| Redis/Valkey, Celery worker | no longer needed and can be removed after switching (`PAPERLESS_REDIS` too; if left set it has no effect, a hint is logged) |

<a id="real-db"></a>

## Test with a real database (prepared)

All automated tests use test data (real paperless-ngx databases 2.20.15, 3.1.3 and 3.4.2, created
with the unmodified upstream code). The test with a large, real data set is prepared and waits for
the owner, because only they have the database. On your own computer:

1. Back up the real database (`pg_dump -Fc … > nas.pgdump`) and copy it to the computer. Provide the
   media folder read-only (e.g. a network share); it is not copied.
2. `cargo build -p pl-dbmigrate`, then
   `bash tests/dbmigrate/real_db_check.sh nas.pgdump //nas/paperless/media`.

The script restores the backup into a **throwaway database** and first wipes every secret there
(mail passwords, OAuth and API tokens, sessions, password hashes, MFA, webhook headers, AI/OCR keys,
share links). Then detection, dry run, switch into a new database and in-place switch (on a second
copy) run. The screen and `zahlen.txt` show **only numbers**: version, counts per object type, file
findings as counts, duration – no titles, contents, names or file names. The full reports stay in
the private folder `tests/dbmigrate/.work/real` and must not be passed on. At the end the throwaway
databases are dropped (`KEEP=1` keeps them).

<a id="back"></a>

## Going back to paperless-ngx

**No longer with the same database.** On its first `migrate` PaperRust changes the schema (tags
without the django-treenode columns, migration `pl_objects.0001_paperrust_tag_adjacency`);
paperless-ngx 3.4.2 can no longer open that database afterwards. Two ways back:

1. **Restore the backup** (everything since the switch is lost): the copy or file from the switch,
   see [switching → going back](umstieg.md#going-back-to-paperless-ngx).
2. **Export from PaperRust, import into paperless-ngx** (keeps everything you did in PaperRust):
   `paperrust manage document_exporter /export` writes the export format of paperless-ngx **3.4.2**
   (the tag tree fields are computed). Then run `document_importer /export` in an **empty**
   paperless-ngx 3.4.2 installation.

Note: PaperRust's own data (sync state, AI history, life areas, settings from `paperrust_setting`)
is not part of the export. Documents consumed by PaperRust are equivalent but not byte-identical
(e.g. lossless thumbnails, barcode-split PDFs with different checksums); the paperless-ngx
classifier retrains its pickle.

<a id="differences"></a>

## Differences you may notice after switching

- **Interface:** PaperRust's own design is the default; the classic interface in the style of
  paperless-ngx can be chosen under *Settings → Display*. 10 instead of more than 30 languages.
- **Office documents and mails:** text without Tika; PaperRust makes the PDF rendition (Gotenberg
  does this in paperless-ngx) itself with `paperrust-pdftool`. Office metadata is sparser than with
  Tika.
- **OCR:** Tesseract/PDFium instead of OCRmyPDF; image cleaning (`PAPERLESS_OCR_CLEAN`) and deskewing
  (`PAPERLESS_OCR_DESKEW`) are built in (no `unpaper` needed) and keep their meaning. Difference: with
  deskewing on, pages that are already straight keep their original image in the archive (OCRmyPDF
  re-renders every scanned page). PDF/A is created and checked by PaperRust itself, but not with
  veraPDF; if the original does not embed its fonts, a normal PDF is created. Some `OCR_USER_ARGS`
  options of OCRmyPDF have no effect (note in the log).
- **Classifier:** own model that learns anew after the switch; suggestions can differ for single
  documents (2–4.5 % of correspondents in the test).
- **Encrypted mails (GPG):** work if `gpg` is installed (not in the Docker image).
- **Sign-in:** OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea; no other allauth
  providers. No `/admin/`.
- **Without Django** no other addresses are missing ([list](installation.md#standalone-limits)).
- Full list with what it means for users: [release notes → known limits](../../RELEASE-NOTES-0.1.0.en.md#limits).

All differences per component: `docs/contract/*.md`, section "Known differences".
