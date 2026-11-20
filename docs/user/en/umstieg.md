# Switching from paperless-ngx – step by step

[Deutsch](../umstieg.md) · [Documentation overview](README.md) · Technical details: [Switching (in depth)](migration.md)

This page explains in simple steps how to move your documents from paperless-ngx to PaperRust.
You need **one command**. PaperRust checks everything first, shows its progress and tells you at
the end exactly whether everything arrived.

> **Please stay healthily sceptical.** PaperRust 0.1.0 is a first version. The core is thoroughly tested –
> but not with your documents. Before switching: make a **full backup** of paperless-ngx (export, or database
> and media folder), **try PaperRust with a copy first**, and only turn paperless-ngx off once everything is
> right. Don't migrate blindly. The way back to paperless-ngx stays open.

## The essentials

- **Your old data stays intact.** PaperRust only reads paperless-ngx. If PaperRust does change
  something in your existing database, it makes a backup itself first.
- **Which version?** PaperRust takes over paperless-ngx **2.20**, **3.0 to 3.4** and also the newer
  paperless-ngx with the new tag tree (after 3.4.2) directly. You do **not** need to update
  paperless-ngx first. Only older 2.x versions (before 2.19.6) need an export.
  **paperless-ngx 1.x:** update to 2.20 first and start it once; an export from 1.x cannot be
  taken over (not by paperless-ngx 3 either).
- **What comes along?** All documents with their files, tags, correspondents, document types,
  storage paths, custom fields, notes, saved views, workflows, mail rules, users **with their
  passwords** and permissions.
- **At the end there is a comparison**: how many documents, tags and users there were before and
  how many there are now, and whether every file was found.

## Before you start: two minutes of preparation

1. **Stop paperless-ngx**, so nothing new arrives during the switch.
   Docker: `docker compose down`.
2. **Back up** (recommended, even though PaperRust does not change your data): the database and
   the `data` and `media` folders. Docker with PostgreSQL:

   ```bash
   docker compose up -d db
   docker compose exec db pg_dump -U paperless -Fc paperless > paperless-before.pgdump
   ```

3. **Try without risk:** every command below can run with `--dry-run`. PaperRust then only checks
   and writes nothing.

## Case 1: paperless-ngx in Docker with PostgreSQL (most common)

This is how it works, for example, with paperless-ngx 3.1.3 and PostgreSQL 17.

1. Prepare as above (stop, back up).
2. In your `docker-compose.yml`, replace the paperless-ngx image with PaperRust. A ready-made
   template is in `docker/docker-compose.migrate-from-paperless.yml`. The `data` and `media`
   volumes and the database stay as they are. Important: **the same `PAPERLESS_SECRET_KEY`** as
   before.
3. Trial run:

   ```bash
   docker compose run --rm webserver paperrust migrate --dry-run \
       --from "postgres://paperless:PASSWORD@db:5432/paperless" --media /usr/src/paperless/media
   ```

4. Switch (the same command without `--dry-run`). If your paperless-ngx version is older than
   3.4.2 (2.20 included), PaperRust first makes an **automatic backup** and then brings the database up to date.
   The backup is a full copy of your database on the database server (name e.g.
   `paperless_vor_umstieg_20261110_143000`) – or, if the `pg_dump` program is available, a file
   `paperless-vor-umstieg-….pgdump` in the `data/umstieg` folder. The comparison report names it.
5. Start: `docker compose up -d`. Sign in with your existing users and passwords.

With paperless-ngx 3.x you can also simply start PaperRust instead of steps 3 and 4: on start-up it
applies the missing changes of an older version itself (like paperless-ngx does on an update). You
only get the comparison report through the command above, though. **paperless-ngx 2.20** is
converted only by the command above, because more happens there (see [taking over 2.20](#v2)); a
plain start shows a note about it.

**Prefer a copy?** If your existing database must not be touched at all, create a new, empty
database and give it as the target:
`--target "postgres://paperless:PASSWORD@db:5432/paperrust"`. PaperRust then copies everything into
it and checks every table. paperless-ngx can keep running with the old database at any time.

<a id="fall-2-desktop-app-windows"></a><a id="desktop"></a>

## Case 2: desktop app (Windows)

1. Stop paperless-ngx.
2. Install and start PaperRust. On first start PaperRust asks whether you want to start fresh or
   **switch from paperless-ngx**.
3. Choose "Switch from paperless-ngx" and give **one** location: the paperless folder (with `data`
   and `media`), an export file (`.zip`) or the address of your database.
4. Wait. PaperRust then shows the **comparison**: the result (OK, WARN or FAIL), a summary
   (documents, tags, users, files) and the first notes. **"Ganzen Bericht öffnen"** (open full
   report) shows every detail, **"PaperRust öffnen"** (open PaperRust) gets you started. If the
   comparison is still running you can already open PaperRust; the report is then saved as
   `umstieg-bericht.html` in the app's log folder.
5. For a paperless-ngx 2.x database also give the old media folder: PaperRust needs the files to
   recompute the checksums.

## Case 3: moving to a new computer (export)

If PaperRust should run on a different computer, the export is the easiest way. The files move
along with it.

1. On the old computer, export from paperless-ngx:

   ```bash
   docker compose exec webserver document_exporter ../export
   ```

2. Copy the `export` folder (or a ZIP file of it) to the new computer.
3. On the new computer:

   ```bash
   paperrust migrate --from /path/to/export
   ```

Note: an export contains no API keys (apps have to reconnect) and not the list of mails already
fetched. Everything else is included.

## Case 4: paperless-ngx with SQLite or MariaDB/MySQL

PaperRust works with PostgreSQL. Your data is copied into a new PostgreSQL database; the old one
stays unchanged.

- **SQLite** (the paperless-ngx default without a separate database): just give the paperless
  folder, PaperRust finds `db.sqlite3` and `media` itself:
  `paperrust migrate --from /path/to/paperless --target "postgres://…/paperrust"`.
- **MariaDB/MySQL**: `--from "mariadb://user:password@host:3306/paperless" --media
  /path/to/media --target "postgres://…/paperrust"`.

You need an empty PostgreSQL database for this beforehand (the desktop app already includes one).

<a id="v2"></a>

## Taking over paperless-ngx 2.20

paperless-ngx 3 rebuilt its database considerably compared with 2.x. PaperRust does this
conversion itself – with the same steps as paperless-ngx 3 –, so you do **not** have to update to
paperless-ngx 3 first.

- **Which 2.x?** 2.20 (all point releases) and 2.19.6. paperless-ngx itself even requires exactly
  2.20.15 for version 3; PaperRust also applies the one step that older 2.20 releases lack. Older
  2.x: update paperless-ngx to 2.20.15 and start it once first – or use an export (case 3).
- **How?** Exactly as above: the `paperrust migrate` command (case 1), the desktop app (case 2) or
  SQLite/MariaDB (case 4). Give the **media folder** too (`--media …`): PaperRust recomputes the
  checksum of every file (SHA-256 instead of MD5, like paperless-ngx 3). With many documents this
  takes a while.
- **What changes?** Saved views: whether a view shows on the dashboard or in the sidebar is then
  stored in the user settings (as in paperless-ngx 3). Old entries of the task list are dropped –
  paperless-ngx 3 deletes them too. The OCR mode "skip" becomes "auto".
- **Encrypted documents** (old GPG encryption) cannot be read by paperless-ngx 3 or PaperRust.
  PaperRust stops before changing anything. Decrypt them in paperless-ngx 2.20 first
  (`decrypt_documents`).
- **Same database:** here too PaperRust makes an automatic backup first (see case 1).

<a id="newer"></a>

## Newer paperless-ngx versions (after 3.4.2)

The development version of paperless-ngx after 3.4.2 already stores the tag tree the way PaperRust
does. PaperRust takes over such a database directly, on all ways above. If paperless-ngx brings a
change that PaperRust does not know yet, the switch stops **before** anything is changed and names
the way out: make an export (case 3).

## The comparison at the end

After every run PaperRust shows a summary. It is currently printed in German; for example:

```text
Ergebnis: OK – Alles übernommen und geprüft.

Kurzfassung:
  Dokumente: alle 204 übernommen
  Tags: alle 20 übernommen
  Benutzer: alle 5 übernommen
  Dateien im Ziel: 590 gefunden und in Ordnung, 0 fehlen, 0 beschädigt
```

In English: result OK – everything transferred and verified; all 204 documents, 20 tags and 5
users transferred; 590 files found and intact, none missing or damaged.

| Result | Meaning | What to do? |
|---|---|---|
| **OK** | Everything arrived and was checked. | Nothing. Start PaperRust. |
| **WARN** | Transferred, but with notes. Often a file was **already broken in paperless-ngx** or a thumbnail is missing. | Read the notes. Usually harmless. |
| **FAIL** | Something is missing or wrong – or the switch was not possible. | Your old data is unchanged. Read the report or pass it on. |

You find the full report as `umstieg-bericht.html` (for viewing), `umstieg-bericht.txt` and
`umstieg-bericht.json` in the report folder (Docker: `data/umstieg`, otherwise the current folder or
`--report-dir`).

## If something goes wrong

PaperRust prints these messages in German; the table gives their meaning in English.

| Message (meaning) | Meaning | Solution |
|---|---|---|
| "not enough space for the files" | There is not enough free storage on the target. | Free up space or choose another folder with `--target-media`. |
| "target database not reachable" | Address, password or database is wrong, or the database service is not running. | Check the details; Docker: `docker compose up -d db`. |
| "target database is not empty" | When copying, the target must be empty. PaperRust never overwrites anything. | Create a new, empty database. |
| "paperless-ngx version too old (older than 2.20)" | paperless-ngx before 2.19.6 is not taken over directly. | Update paperless-ngx to 2.20.15 and start it once – or make an export (case 3). |
| "newer paperless-ngx version" | Your paperless-ngx has a change that PaperRust does not know yet. | Make an export (case 3). Your database is unchanged. |
| "encrypted (GPG)" | paperless-ngx 2.x stored documents encrypted. | Run `decrypt_documents` in paperless-ngx 2.20, then try again. |
| "This is a paperless-ngx 2.x database" (on start-up) | PaperRust was started directly on a 2.x database. | Run `paperrust migrate --from … --media …` once (case 1). |
| "could not back up itself" | Neither `pg_dump` nor a copy on the server was possible (e.g. missing permission, or paperless-ngx is still running). | Stop paperless-ngx completely and try again – or back up yourself (see above) and repeat the command with `--no-backup`. |

The [German page](../umstieg.md) lists the messages exactly as PaperRust prints them.

<a id="after"></a>

## After the switch: what is different

- **Interface:** PaperRust opens in the PaperRust design. If you want the familiar look, choose the
  **classic interface** under *Settings → Display* – it looks like paperless-ngx. The **tour** (user
  menu) shows the new places. Workflows and mail rules are in the new
  [Automation](automatik.md) area.
- **Automatic matching learns anew:** PaperRust stores the learned model in its own format. It learns
  again by itself after the switch (every hour); to do it right away, run
  `paperrust manage document_create_classifier`. Your rules ("text contains …") apply unchanged.
- **Add-on programs are no longer needed:** Redis/Valkey, Tika, Gotenberg and the Celery workers are
  not needed any more. You can replace paperless-ai or paperless-gpt with the built-in
  [AI](ai.md#replaces) (set up its settings once more).
- **Settings:** Your `PAPERLESS_*` variables and `paperless.conf` keep working. The log reports once
  what has no effect (e.g. `PAPERLESS_REDIS`). To take them over into the interface:
  [Configuration → moving settings](configuration.md#settings-ui).
- **Languages:** The interface is available in 10 languages (see below).
- **API and apps:** the same REST API as paperless-ngx 3.4.2. Apps and scripts for paperless-ngx
  connect as before (after moving by export, with a new API key).
- Known differences: [release notes → known limits](../../RELEASE-NOTES-0.1.0.en.md#limits).

## Going back to paperless-ngx

- **Copy (new database) or export:** simply start paperless-ngx again with the old database.
  Nothing has changed there.
- **Same database:** PaperRust changed the database (tags without the django-treenode helper
  columns). paperless-ngx can **no longer open it** – not even paperless-ngx 3.4.2. Either restore the
  backup: for a copy on the server, set `PAPERLESS_DBNAME` in `docker-compose.yml` to the name of the
  copy (e.g. `paperless_vor_umstieg_20261110_143000`); for a file,
  `pg_restore -U paperless -d paperless --clean paperless-vor-umstieg-….pgdump`. Everything added in
  PaperRust since the switch is lost that way.
- **Keep everything you did in PaperRust:** `paperrust manage document_exporter /export`, then
  `document_importer /export` in an **empty** paperless-ngx 3.4.2 installation. The export has the
  paperless-ngx 3.4.2 format (see [switching → going back](migration.md#back)).

## What does not work (yet)

- Taking over paperless-ngx **before 2.19.6** directly – please update to 2.20.15 first or use an
  export (case 3). This applies to 2.x; for **1.x** only the way via 2.20 works (an export from 1.x
  is refused with a clear message, nothing is changed).
- A **future** paperless-ngx version with changes PaperRust does not know yet – please use an
  export.
- The switch has been tested with test databases from paperless-ngx 2.20.15, 3.1.3 and 3.4.2 and
  with a real paperless-ngx 3.1.3 database (dry run, copy and in place, all counts equal). In this
  real-world test the files themselves were not checked, and an export from a real paperless-ngx 3.1
  was not tried (see [switching (details)](migration.md#real-db)).
- The new interface comes in 10 languages (see [Interface language](usage.md#language)). If you had
  picked another language in paperless-ngx, you see the texts in English.
