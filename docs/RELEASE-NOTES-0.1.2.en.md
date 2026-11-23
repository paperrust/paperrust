# PaperRust 0.1.2 – Release notes

[Deutsch](RELEASE-NOTES-0.1.2.md) · As of 2026-11-23

A release after [0.1.1](RELEASE-NOTES-0.1.1.en.md) with a folder view, switching from paperless-ngx 3.5, Markdown
notes, parallel ingest and news in the desktop app. On first start PaperRust adds two things to the database (note
format, folder tables); both are additions only, existing data is unchanged and paperless-ngx can still read the
database.

## New

- **Folder view (inspired by Papermerge, optional):** Settings → Appearance → "Folder view". Folder tree with
  "Unsorted", subfolders, file documents by drag and drop or "Move to folder" (with undo), tile view with thumbnails,
  upload straight into the open folder. Folders complement tags, they don't replace them. Exports put them into
  `paperrust-extras.json`, so the way back to paperless-ngx loses nothing. ([Folders](user/en/folders.md))
- **Switching from paperless-ngx 3.5.x:** databases of paperless-ngx 3.5.0 and 3.5.1 are taken over directly
  (before: refused because of the new migration `0030_note_format`), exports from 3.5 too.
- **Markdown notes** as in paperless-ngx 3.5.1, with preview; links only http/https/mailto.
- **Parallel ingest:** opt-in with `PAPERRUST_TASK_WORKERS=auto` (or a number): several documents at once.
  On a Raspberry Pi 5 this reads 255 documents in 162 instead of 214 seconds, but a single long scan then takes
  longer (24 instead of 15 seconds), so the default stays one task after the other. The locks against duplicate
  ASNs, duplicate documents and duplicate tags are always active and formally checked.
- `localhost`, `127.0.0.1` and `[::1]` are always allowed hosts (as in paperless-ngx 3.5); chat answers appear more
  evenly.

## New in the desktop app

- **PaperRust interface also with a paperless-ngx server:** when the app is connected to a server that still runs
  paperless-ngx, it can show its built-in PaperRust interface instead of the server's own one (tray → "New
  interface" or Settings → Interface). Sign in once with user name and password. Lists, search and editing work;
  PaperRust-only features (AI assistant, timeline, workflows, roles) show "not available". Back via "Server's
  interface".
- **Change the operating mode after setup:** tray → "Change mode …" or Settings → Mode: local ↔ connect to a
  server, with confirmation and restart. Local data and server profiles are kept.
- The PaperRust interface starts in the design chosen last.

## Fixed

- **Duplicate detection with "shrink PDFs":** uploading the same original again is reported as a duplicate
  again (before, it was stored a second time).
- **More robust task queue** (found by a formal check): a short database outage can no longer make a task run
  twice for a moment; a task no longer stays "started" after a long pause of a process; a late result can no
  longer overwrite a newer one.
- **Uploading into a folder that is being deleted** ends in "Unsorted" instead of failing.
- An unreadable settings file of the desktop app is no longer overwritten when changing the mode (before, all
  server profiles would have been lost).
- Linux packages are built on Debian 12 again (glibc 2.36) and run on Debian 12 and Ubuntu 22.04.

## Known limits

As in [0.1.1](RELEASE-NOTES-0.1.1.en.md#known-limitations). New: with a paperless-ngx server, the PaperRust interface
only signs in with user name and password (no single sign-on); AI assistant answers through the desktop app
appear when complete, not step by step. Folders are available in the web interface; the desktop app's offline copy
and the classic interface don't know them yet, folder shares are API-only.

## Updating

- Server without Docker (Linux package, `tar.gz`): run `paperrust manage migrate` once after updating. Docker, the
  desktop app and the FreeBSD service do this on start.
- Docker: set the image to `ghcr.io/paperrust/paperrust:0.1.2` and restart.
- Desktop/Linux package: install the new package. As always, back up first.
