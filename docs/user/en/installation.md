# Installation

[Deutsch](../installation.md) · [Documentation overview](README.md)

PaperRust is a single program (`paperrust`) plus PostgreSQL. There are four ways to run it; which
one fits depends on whether a paperless-ngx installation already exists.

| Way | For whom | State |
|---|---|---|
| [Server without Django (standalone)](#standalone) | new installation or complete switch | works, gaps see [below](#standalone-limits) |
| [Mixed operation in front of paperless-ngx](#mixed-operation) | – | **no longer exists** (only PaperRust up to v0.1.0) |
| [Docker](#docker) | container operation | build the image from the repository (`docker/`); built, checked and measured for Linux arm64; amd64 built and smoke-tested (2026-11-20); **not published yet** |
| [Desktop app](#desktop) | a single computer without server setup | Windows installer (**not signed** yet); Linux package (`tar.gz`, x86_64); macOS preview (`.dmg`, unsigned, not yet tested) |

<a id="requirements"></a>

## Requirements

<a id="postgresql"></a>

- **PostgreSQL ≥ 14** (minimum of paperless-ngx v3.4.2; tested with 18). PaperRust does not
  support SQLite or MariaDB – move existing databases first ([switching](migration.md#sqlite-mariadb)).
- **Tesseract 5** with language data (`eng.traineddata`, `deu.traineddata`, …) for OCR. Born-digital
  PDFs (with a text layer) are processed without Tesseract.
- **PDFium** (library, e.g. from `pdfium-binaries`) for thumbnails, page rendering for OCR, and barcodes.
- **Web UI:** the built PaperRust UI (`frontend-light`) plus a few static files for the login pages
  and the API view (see [static files](#static-files)).
- **`paperrust-pdftool`** (built alongside, see below) next to `paperrust`: redaction, compression,
  repair, Office→PDF ([PDF tools](pdf-tools.md)). Without it these functions are unavailable.
- Not needed: Python, Redis/Valkey, Celery, Tika, Gotenberg, OCRmyPDF, Ghostscript.

Memory (measured on 2026-11-20, Docker on a small arm64 board, with worker): right after starting
74 MiB for PaperRust plus 64 MiB for PostgreSQL; idle after consuming 230 documents 420 + 74 MiB;
peak while consuming 800 MiB for PaperRust. For a small archive, 2 GB RAM for the whole device is
enough; details in the [release notes](../../RELEASE-NOTES-0.1.0.en.md#performance).

<a id="build"></a>

## Building from source

There are no ready-made packages for servers yet (for Windows and Linux there is the
[desktop app](#desktop)). With a current stable Rust toolchain:

```sh
git clone --recurse-submodules https://github.com/paperrust/paperrust.git
cd paperrust
cargo build --release -p pl-server            # produces target/release/paperrust
cargo build --release -p pl-dbmigrate         # migration tool (only needed for SQLite/MariaDB)
cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool   # PDF tools
```

<a id="static-files"></a>

## Static files (web UI)

PaperRust has its own web UI (`frontend-light`, Svelte). It runs under `/rs/app/`; the start
address `/` and all addresses of the former paperless-ngx UI (e.g. `/documents/12/details`, `/tags`)
redirect there, so bookmarks keep working.

- **Build the UI:** `cd frontend-light && pnpm install && pnpm build` produces
  `frontend-light/dist/`. PaperRust looks for it in `<PAPERRUST_STATICDIR>/../light`
  (Docker: `/usr/src/paperless/light`) or in `PAPERRUST_LIGHT_UI_DIR`.
- **Static files for server pages** (login page, Swagger view of the API): `bash
  scripts/build-static.sh` produces `results/static/`; then `PAPERRUST_STATICDIR=<repo>/results/static`.
  For the API view run `bash scripts/fetch-extra-static.sh <dir>` first and set `EXTRA_STATIC=<dir>`.
- PaperRust no longer ships the paperless-ngx Angular UI (retired on 2026-11-13). When switching
  from paperless-ngx, data, users and saved views are kept.

<a id="standalone"></a>

## Server without Django (standalone)

1. **Create the database** (empty, UTF8):

   ```sh
   psql -U postgres -c "CREATE USER paperless PASSWORD 'secret'"
   psql -U postgres -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
   ```

2. **Configuration** – a `paperrust.conf` (`paperless.conf` is still read) next to the installation directory,
   or environment variables. At least:

   ```sh
   PAPERRUST_DBENGINE=postgresql
   PAPERRUST_DBHOST=localhost
   PAPERRUST_DBNAME=paperless
   PAPERRUST_DBUSER=paperless
   PAPERRUST_DBPASS=secret
   PAPERRUST_SECRET_KEY=<long random string>
   PAPERRUST_URL=https://paperrust.example.com
   PAPERRUST_TIME_ZONE=Europe/Zurich
   PAPERRUST_OCR_LANGUAGE=eng+deu
   PAPERRUST_DATA_DIR=/srv/paperrust/data
   PAPERRUST_MEDIA_ROOT=/srv/paperrust/media
   PAPERRUST_CONSUMPTION_DIR=/srv/paperrust/consume
   PAPERRUST_STATICDIR=/srv/paperrust/static

   PAPERRUST_BIND=127.0.0.1:8080
   ```

   All variables: [Configuration](configuration.md). Instead of `PAPERRUST_*`, the paperless-ngx
   names (`PAPERLESS_*`) also work.

3. **Routes:** nothing to do – PaperRust serves every route itself. `routes.toml` (working
   directory or `PAPERRUST_ROUTES`) is optional and can only switch single routes off
   (`"<id>" = "off"`).

4. **Create the schema** – without Django; on an empty database it creates the schema of
   paperless-ngx v3.4.2, on an existing one it checks it and adds PaperRust's own `paperrust_*` tables:

   ```sh
   paperrust manage migrate
   ```

5. **Create an administrator:**

   ```sh
   paperrust manage createsuperuser
   # or non-interactively: set PAPERRUST_ADMIN_USER / PAPERRUST_ADMIN_PASSWORD and
   paperrust manage manage_superuser
   ```

6. **Start** (web server + worker + consumption directory + scheduler in one process):

   ```sh
   paperrust serve --worker
   ```

   The start-up log lists what is still missing without Django. Health check: `GET /__rs/health` → 200.

7. **Reachable from outside:** by default PaperRust listens on `127.0.0.1` only. Put a reverse proxy
   with TLS in front (nginx, Caddy, Traefik) and set `PAPERRUST_URL`/`PAPERRUST_ALLOWED_HOSTS`.
   See [Security](security.md).

Service example (systemd):

```ini
[Unit]
Description=PaperRust
After=network.target postgresql.service

[Service]
User=paperrust
WorkingDirectory=/srv/paperrust/run
EnvironmentFile=/srv/paperrust/paperrust.conf
ExecStart=/usr/local/bin/paperrust serve --worker
Restart=on-failure
# restart from the web UI (exit code 75)
Environment=PAPERRUST_RESTART_SUPERVISED=1
RestartForceExitStatus=75

[Install]
WantedBy=multi-user.target
```

<a id="standalone-limits"></a>

### What is missing without Django

PaperRust itself answers almost every address that paperless-ngx 3.4.2 offers. Only two are still
missing: the Django admin under `/admin/` and the "headless" interface of allauth
(`/api/auth/headless/…`, not used by the paperless-ngx web UI). Both answer 404. Missing features (as
opposed to addresses) are listed in the [release notes](../../RELEASE-NOTES-0.1.0.en.md#limits).

<a id="mixed-operation"></a>

## Mixed operation in front of paperless-ngx (history)

Up to PaperRust v0.1.0, PaperRust could sit in front of a running paperless-ngx and forward unknown
paths to Django via reverse proxy. This no longer exists: since the schema change
`pl_objects.0001_paperrust_tag_adjacency`, paperless-ngx can no longer use the database after
`paperrust manage migrate`, and forwarding to Django has been removed (`PAPERRUST_DJANGO_URL` is
ignored; if set, only a warning is logged). PaperRust serves every route itself; paths without a
route answer 404.

Switching from paperless-ngx: **make a backup and stop paperless-ngx completely first**, then
continue [without Django](#standalone) (or with [Docker](#docker)). Redis/Valkey and the
paperless-ngx Celery workers are no longer needed afterwards and can be removed.

<a id="docker"></a>

## Docker

A container image is built from this repository (`docker/Dockerfile`); it is **not published
yet** (planned: `paperrust/paperrust`). Status: built for Linux arm64 and checked and measured on an
arm64 test board; amd64 was built the same way and smoke-tested on its own (start, upload, OCR, search, export, sanity check; 2026-11-20).

```sh
git submodule update --init
docker build -f docker/Dockerfile -t paperrust/paperrust:local .
cd docker && mkdir -p consume export
docker compose -f docker-compose.postgres.yml up -d      # PaperRust + PostgreSQL 17, port 8000
```

Build arguments (`docker build --build-arg NAME=value`):

| Argument | Default | Effect |
|---|---|---|
| `TESSERACT_LANGS` | `eng deu fra ita spa` (as upstream) | Tesseract language packages in the image |
| `CARGO_BUILD_JOBS` | all cores | fewer jobs = less RAM while compiling; the final link of `paperrust` alone needs more than 5 GB RAM |
| `CARGO_RELEASE_LTO`, `CARGO_RELEASE_CODEGEN_UNITS` | from `Cargo.toml` (`fat`, `1`) | small build machines: `thin` and `16` |
| `PAPERRUST_UID`, `PAPERRUST_GID` | `1000` | user inside the image |

Only `linux/amd64` and `linux/arm64` can be built (there is no PDFium build for other platforms).

- **One service instead of five:** `paperrust serve --worker` (web server, worker, scheduler,
  consumption folder); no Redis, no Tika, no Gotenberg. PaperRust reads Office files (also old
  `.doc`/`.xls`/`.ppt`), HEIC photos and mails itself and turns them into an archive PDF (with the
  built-in tool `paperrust-pdftool`, which is in the image). Not in the image: `gpg` (encrypted mails)
  and `pdftoppm` (only needed without PDFium).
- **Same paths as the paperless-ngx image:** volumes `/usr/src/paperless/{data,media,consume,export}`,
  port 8000, commands such as `document_exporter ../export` inside the container. `PAPERLESS_*`
  variables work unchanged.
- **On start:** wait for PostgreSQL, `manage migrate`, administrator from
  `PAPERRUST_ADMIN_USER`/`PAPERRUST_ADMIN_PASSWORD`, create the search index if missing.
- **User:** UID/GID 1000, not root. With `user: root`, `USERMAP_UID`/`USERMAP_GID` and
  `PAPERRUST_OCR_LANGUAGES` behave as in the upstream image; better choose languages at build time
  (`--build-arg TESSERACT_LANGS="eng deu fra ita spa"`, which is also the default).
- **Switching** an existing paperless-ngx Docker installation (PostgreSQL): swap the image, remove
  `broker`/`gotenberg`/`tika` and `PAPERLESS_REDIS`/`PAPERLESS_TIKA_*`, keep everything else –
  template `docker/docker-compose.migrate-from-paperless.yml`, see [below](#migrate) and the
  [step-by-step guide](umstieg.md).
- **Slim variant:** `docker/docker-compose.slim.yml` with `docker-compose.slim.env` sets only the
  database, `PAPERRUST_SECRET_KEY` and optionally `PAPERRUST_URL`; an administrator configures
  everything else in the web UI (*Settings → System*), see
  [Settings in the UI](configuration.md#settings-ui).
- **Separate processes** (web server/worker) with a shared cache in PostgreSQL
  (`PAPERRUST_CACHE=postgres`, no Redis): `docker/docker-compose.multiprocess.yml`.
- **ARM:** the image builds for `linux/amd64` and `linux/arm64` (Raspberry Pi 4/5 with a 64-bit
  OS, ARM NAS, Apple Silicon machines with Docker). 32-bit ARM (`armv7`) is not supported (no
  PDFium build for it).

<a id="migrate"></a>

### Switching a paperless-ngx Docker installation

In your existing compose file you only **swap the image** and remove the services that are no longer
needed; database, volumes and `docker-compose.env` stay. The template
`docker/docker-compose.migrate-from-paperless.yml` contains all steps as comments.

1. **Back up** (database dump and the `data`/`media` volumes, or `document_exporter`) and stop
   paperless-ngx: `docker compose down` (volumes stay).
2. Change the compose file:

   | paperless-ngx | PaperRust |
   |---|---|
   | `image: ghcr.io/paperless-ngx/paperless-ngx:…` | `image: paperrust/paperrust:local` (built as above) |
   | services `broker`, `gotenberg`, `tika` | remove (also from `depends_on`) |
   | `PAPERLESS_REDIS`, `PAPERLESS_TIKA_*` | remove (would be ignored) |
   | `USERMAP_UID`/`USERMAP_GID`, `PAPERLESS_OCR_LANGUAGES` | work with `user: root` on the service (as in the template); for languages prefer `--build-arg TESSERACT_LANGS` |
   | `PAPERLESS_WEBSERVER_WORKERS`, `PAPERLESS_ENABLE_FLOWER`, `S6_*` | no longer needed |
   | service name `webserver`, port `8000`, volumes, `env_file` | **unchanged** |
   | `db` image | **same major version** as before (the data directory is tied to it) |

3. Check and switch with `paperrust migrate --from …` as described in the
   [step-by-step guide](umstieg.md) (trial run with `--dry-run` first), then `docker compose up -d`.
   Keep the same `PAPERLESS_SECRET_KEY`, so users, tokens and sessions keep working.

**Going back:** after the switch paperless-ngx can no longer open the database. The way back is the
backup from step 1 (restore database and volumes, put the paperless-ngx image and `broker` back) or
an export in the paperless-ngx 3.4.2 format ([going back](migration.md#back)).

SQLite/MariaDB: move to PostgreSQL first ([switching](migration.md#sqlite-mariadb)); the tool
`paperrust-migrate-db` is in the image (`docker compose run --rm webserver paperrust-migrate-db …`).

<a id="nas"></a>

### NAS (Synology, UGREEN, Asustor, Unraid, CasaOS)

Ready-made templates are in `docker/nas/` (step-by-step guide per device: `docker/nas/README.md`,
German):

| NAS | Template | Where |
|---|---|---|
| Synology (DSM 7.2+) | `docker/nas/docker-compose.yml` | Container Manager → Project → Create |
| UGREEN (UGOS Pro) | `docker/nas/docker-compose.yml` | Docker → Project → Create |
| Asustor (ADM) | `docker/nas/docker-compose.yml` | Portainer CE → Stacks (set variable `PAPERRUST_DIR`) |
| Unraid | `docker/nas/unraid/paperrust.xml` + `paperrust-postgres.xml` | Docker → Add Container → Template |
| CasaOS / ZimaOS | `docker/nas/casaos/docker-compose.yml` | App Store → Custom install → Import |

- Two containers: PaperRust and PostgreSQL 17. Before the first start change
  `PAPERRUST_SECRET_KEY`, the database password (same value in both places) and
  `PAPERRUST_ADMIN_PASSWORD`.
- **File ownership:** the templates start the container as root; it hands the folders to
  `PUID:PGID` (the NAS user, e.g. Synology `1026:100`, Unraid `99:100`) and then runs as that
  user. `USERMAP_UID`/`USERMAP_GID` as in paperless-ngx work too (and take precedence).
- **Consumption folder** on NAS shares: `PAPERRUST_CONSUMER_POLLING_INTERVAL=10` (set in the templates),
  because network shares often do not report changes.
- The image is **not published yet**; the templates point at the placeholder
  `ghcr.io/paperrust/paperrust:latest`.

**Updating on a NAS:** like paperless-ngx, the UI shows a notice when a newer PaperRust version
exists (`/api/remote_version/` asks the PaperRust releases; disable with
`PAPERRUST_ENABLE_UPDATE_CHECK=false`). PaperRust does **not** update itself: pull the new image
with the NAS container manager (Synology: Project → Action → Build, or update the image; Unraid:
Check for Updates; Portainer: re-pull image; CasaOS: update the app). PaperRust applies missing
database migrations on start. Back up first: `docker exec paperrust document_exporter ../export`.

<a id="desktop"></a>

## Desktop app

`paperrust-desktop` starts an **embedded PostgreSQL** and `paperrust serve --worker` as a child
process, on `127.0.0.1` only, and opens the UI in the browser. Data lives in
`%LOCALAPPDATA%\PaperRust` (Windows), `~/Library/Application Support/PaperRust` (macOS) or
`~/.local/share/paperrust` (Linux).

State: For **Windows (64-bit)** there is an installer (`PaperRust-0.1.0-windows-x64-setup.exe`,
about 68 MB). It includes PostgreSQL 17, Tesseract 5 (German, English) and PDFium; nothing else needs
to be installed. On first start you choose: start fresh or
[switch from paperless-ngx](umstieg.md#fall-2-desktop-app-windows). You need no user name or password
for it ([signing in to the desktop app](#desktop-login)).

> [!WARNING]
> The installer is **not digitally signed** yet. That is why Windows SmartScreen shows a warning
> ("Windows protected your PC"). Only download it from the official project page and compare the
> checksum (SHA-256) before you choose "Run anyway". Code signing through the SignPath Foundation is
> planned.

<a id="desktop-linux"></a>

**Linux (x86_64):** a portable package `PaperRust-0.1.0-linux-x86_64.tar.gz`. It includes PostgreSQL 17,
Tesseract 5 (German, English, plus orientation detection) and PDFium.

- Needs a glibc-based Linux with **glibc 2.36 or newer**: Debian 12, Ubuntu 24.04, Fedora 37 or newer.
- Unpack and start `./PaperRust.sh`. With **WebKitGTK 4.1** and **libayatana-appindicator** installed
  (Debian/Ubuntu: `libwebkit2gtk-4.1-0`, `libayatana-appindicator3-1`) PaperRust opens its own window
  with a tray icon; without them it opens in your browser. `./PaperRust.sh --portable` keeps the data
  in `./data` next to the program (not in a cloud-synced folder), `--console` runs it without a window.
- **Do not run it as root** – the embedded PostgreSQL refuses to start as root.
- The package is built in a Debian 12 container with `apps/desktop/pack/linux-build.sh`, so it also
  runs on newer distributions. So far it has only been smoke-tested.

<a id="desktop-macos"></a>

**macOS (preview):** `PaperRust-<version>-macos-aarch64.dmg` for Apple Silicon (M1 and newer), as an
extra `…-macos-x86_64.dmg` for Intel Macs, plus a portable `tar.gz`. It ships PostgreSQL 17,
Tesseract 5 (German, English) and PDFium. It is built automatically on GitHub Actions and has **not
been tested on a real Mac yet**.

- Needs **macOS 15 (Sequoia) or newer** (the bundled libraries are built for it; the exact minimum is
  in `PACKAGE.json`).
- Open the `.dmg` and drag **PaperRust** to **Applications**.
- The app is **not notarized by Apple** (no developer account). macOS therefore blocks the first start.
  Allow it once in Terminal:

  ```sh
  xattr -dr com.apple.quarantine /Applications/PaperRust.app
  ```

  Or: open the app, close the message, then **System Settings → Privacy & Security → "Open Anyway"**.
  Only download the file from the official project page and compare the checksum (SHA-256).
- Portable: unpack the `tar.gz`, run `xattr -dr com.apple.quarantine <folder>`, then `./PaperRust.sh`
  (`--portable`, `--console` as on Linux).

Start-up, data layout, ports, backup and import of an existing paperless-ngx installation:
[docs/DESKTOP.md](../../DESKTOP.md) (German, developer notes).

<a id="desktop-login"></a>

### Signing in to the desktop app

On your own computer the desktop app needs **no user name and no password**:

- **Start fresh** creates an account for you automatically (name = your Windows user name). A random
  password is generated and never shown – you do not need it.
- The app **signs you in automatically** on every start. Even after "Sign out" or an expired session
  the app window signs you in again right away.
- **In the browser** (running `paperrust-desktop`): if you see the sign-in page, simply open PaperRust
  again from the Start menu – you are signed in again.
- **Existing installations and switching from paperless-ngx:** the app takes over the first active
  administrator (for example the user you created on the first start earlier) and signs it in
  automatically.

**Who gets in?** Only your own Windows account on this computer. The key for the automatic sign-in
lives in your data folder, which only your account can read, and only works for access from this
computer (`127.0.0.1`). Other users of the same computer and other devices have to sign in normally.
The computer's administrator can still get in – as with all your files.

**Access from other devices** (Settings → "Share in the local network"): set a password in your
**profile** (user menu top right → "My profile"); the old password is not asked for. On other devices you
then sign in with your user name and this password (two-factor possible).

<a id="updating"></a>

## Updating

1. Back up ([Administration](administration.md#backup)).
2. Replace the `paperrust` binary.
3. `paperrust manage migrate` (idempotent; applies missing migrations – PaperRust's own and, for an
   older paperless-ngx 3.x database, the missing paperless-ngx ones). Docker and the desktop app do
   this themselves at every start.
4. Restart.

PaperRust builds on paperless-ngx v3.4.2. Known later paperless-ngx changes (the new tag tree) are
taken over; a database with unknown newer changes is rejected by `migrate` ("This database is not at
paperless-ngx v3.4.2 …"), and so is a 2.x database – for that there is `paperrust migrate --from`
([Switching](umstieg.md)).
How new upstream versions are adopted: [docs/UPSTREAM-SYNC.md](../../UPSTREAM-SYNC.md).
