# PaperRust on FreeBSD (server)

PaperRust runs natively on FreeBSD 14 (amd64) as a server: one process `paperrust serve --worker`
(web server, worker, scheduler, consumption folder) plus PostgreSQL. The web UI is served by
the server itself; there is no desktop app for FreeBSD.

Packages (from the GitHub Actions workflow `FreeBSD`, artifact `paperrust-freebsd-x86_64`):

| File | Content |
|---|---|
| `paperrust-<version>.pkg` | native package for `pkg add`: programs, web UI, rc.d script, sample config; creates the user `paperrust` and `/var/db/paperrust` |
| `PaperRust-<version>-freebsd-x86_64.tar.gz` | the same files as a tree for `PREFIX=/usr/local` (no user/directory setup) |
| `SHA256SUMS-freebsd-x86_64.txt` | checksums |

Built on FreeBSD 14.4; the packages run on 14.4 and newer 14.x (pkg ABI `FreeBSD:14:amd64`).

## What is installed

```
/usr/local/bin/paperrust               server, worker and `manage` commands
/usr/local/bin/paperrust-pdftool       PDF tools (redact, shrink, repair, Office/HEIC → PDF); must stay next to paperrust
/usr/local/bin/paperrust-migrate-db    switch assistant from paperless-ngx (SQLite/MariaDB → PostgreSQL)
/usr/local/share/paperrust/static/     server statics (PAPERRUST_STATICDIR)
/usr/local/share/paperrust/light/      web UI (found as <STATICDIR>/../light, served under /rs/app/)
/usr/local/share/paperrust/routes.toml
/usr/local/etc/rc.d/paperrust          service script
/usr/local/etc/paperrust.conf.sample   configuration template
/var/db/paperrust/{data,media,consume,export}   data (pkg only; kept on removal)
/var/log/paperrust/console.log         server output (rc.d)
```

## Install

### 1. Package

```sh
pkg add ./paperrust-0.1.2.pkg
```

`pkg add` pulls the dependencies from the FreeBSD repository: `tesseract` (with `tesseract-data`,
all languages), `pdfium` (`graphics/pdfium`, provides `libpdfium.so`), `liberation-fonts-ttf`,
`ca_root_nss`.

Without the package (tarball):

```sh
pkg install tesseract pdfium liberation-fonts-ttf ca_root_nss
tar -xzf PaperRust-0.1.2-freebsd-x86_64.tar.gz --strip-components 1 -C /usr/local
pw groupadd paperrust
pw useradd paperrust -g paperrust -d /var/db/paperrust -s /usr/sbin/nologin -c PaperRust
install -d -o paperrust -g paperrust -m 0750 /var/db/paperrust /var/log/paperrust
```

### 2. PostgreSQL

PostgreSQL 14 or newer (SQLite and MariaDB are not supported; existing paperless-ngx databases on
them are moved with `paperrust-migrate-db`, see `docs/user/migration.md`). On the same machine:

```sh
pkg install postgresql17-server postgresql17-client
sysrc postgresql_enable=YES
service postgresql initdb
service postgresql start
su -m postgres -c psql <<'EOF'
CREATE ROLE paperrust LOGIN PASSWORD 'a-good-password';
CREATE DATABASE paperrust OWNER paperrust ENCODING 'UTF8' TEMPLATE template0;
EOF
```

`postgresqlNN-client` provides `pg_dump`, which `paperrust manage migrate` uses for a backup
before data model conversions. Its major version must be at least the server's. It is not a
package dependency, because the client packages of different PostgreSQL versions conflict;
install the one that matches your server.

Switching from paperless-ngx: point PaperRust at the existing paperless-ngx PostgreSQL database
and keep the same `PAPERLESS_SECRET_KEY` (see `docs/user/umstieg.md`).

### 3. Configuration

```sh
cp /usr/local/etc/paperrust.conf.sample /usr/local/etc/paperrust.conf
chown root:paperrust /usr/local/etc/paperrust.conf
chmod 0640 /usr/local/etc/paperrust.conf
vi /usr/local/etc/paperrust.conf
```

At least `PAPERRUST_DBHOST/DBNAME/DBUSER/DBPASS` and `PAPERRUST_SECRET_KEY`
(`openssl rand -hex 48`). The file is `KEY=value`; the rc script exports it with `sh(1)`, so quote
values with spaces or `$` in single quotes. All settings: `docs/user/configuration.md`; the
paperless-ngx names (`PAPERLESS_*`) work as well.

The server listens on `127.0.0.1:8000` (sample). For access from the network put a reverse proxy
with TLS in front (nginx, Caddy, relayd) and set `PAPERRUST_URL`.

### 4. Start

```sh
sysrc paperrust_enable=YES
service paperrust start          # runs `paperrust manage migrate` first, then the server
su -m paperrust -c 'paperrust manage createsuperuser'
```

`service paperrust start` creates the schema on an empty database (or checks an existing
paperless-ngx database and adds the `paperrust_*` tables) and then starts the server under
`daemon(8)` as user `paperrust`. Check: `fetch -qo - http://127.0.0.1:8000/__rs/health` (answers
`200` on loopback only); then open the URL in a browser.

Other commands: `service paperrust stop|restart|status|migrate`. Management commands run as the
service user, e.g. `su -m paperrust -c 'paperrust manage document_exporter /var/db/paperrust/export'`
(paperrust reads `/usr/local/etc/paperrust.conf` itself).

rc.conf knobs (defaults in the script header): `paperrust_args` (`--worker`; `--no-worker` or
`--worker-only` for split processes), `paperrust_migrate` (`YES`), `paperrust_basedir`
(`/var/db/paperrust`), `paperrust_config`, `paperrust_logfile`, `paperrust_user`,
`paperrust_restart_delay`.

`daemon(8)` restarts the server when it ends (exit code 75 = restart requested from the settings
page, or a crash) after `paperrust_restart_delay` seconds; `service paperrust stop` stops it for
good. Log rotation with newsyslog (`daemon` reopens the file on SIGHUP), e.g. in
`/usr/local/etc/newsyslog.conf.d/paperrust.conf`:

```
/var/log/paperrust/console.log  paperrust:paperrust  640  7  10000  *  -  /var/run/paperrust/daemon.pid  1
```

The application log files for the *Logs* page are in `/var/db/paperrust/data/log/`.

## Native libraries and differences from the Linux/Docker build

| Part | FreeBSD | Without it |
|---|---|---|
| Tesseract 5 | `pkg tesseract`; the library `libtesseract.so.5` is found next to `/usr/local/bin/tesseract` (`../lib`), language data in `/usr/local/share/tessdata` | no OCR; digital PDFs with a text layer still work |
| PDFium | `pkg pdfium` (graphics/pdfium, chromium 8066+; the Linux packages use 8111 from pdfium-binaries); `/usr/local/lib/libpdfium.so` is set by the rc script | no PDF thumbnails (default image), no OCR of scanned PDFs, no barcodes (unless `pdftoppm` from `poppler-utils` is installed: then barcodes only) – `paperrust manage check` warns |
| Fonts | `liberation-fonts-ttf` (`/usr/local/share/fonts/Liberation`, found automatically; thumbnail font set by the rc script) | text thumbnails/PDF/A text layer fall back |
| TLS | rustls (no OpenSSL), CA bundle from `ca_root_nss` | – |

Not tested / known limits:

- Only amd64 is built. aarch64 would need a native or emulated arm64 VM (slow) and is not set up.
- Memory-based limits: on Linux PaperRust caps OCR threads by available memory (`/proc/meminfo`,
  cgroups). On FreeBSD this information is not read; set `PAPERRUST_THREADS_PER_WORKER` /
  `PAPERRUST_TASK_WORKERS` yourself on small machines.
- Consumption folder: file events via kqueue (one open file per watched file). Very large
  consumption folders may need a higher `kern.maxfiles`/login class limit or
  `PAPERRUST_CONSUMER_POLLING=<seconds>`.
- The Linux-only `malloc_trim` memory return does not exist on FreeBSD (jemalloc returns memory
  itself; PaperRust's own heap uses mimalloc on all platforms).
- `paperrust-pdftool` (PdfCraft/WordCraft, pure Rust) and the OCR pipeline have only been run in
  the CI smoke test on FreeBSD (text document ingestion), not with real scans.

## Building yourself

On FreeBSD 14 amd64 as root, from a checkout with the `upstream` submodule, with the web UI built
elsewhere (Linux/macOS: `cd frontend-light && pnpm install && pnpm build`,
`bash scripts/fetch-extra-static.sh /tmp/x && EXTRA_STATIC=/tmp/x bash scripts/build-static.sh`,
then copy `frontend-light/dist` and `results/static` over):

```sh
sh packaging/freebsd/build.sh          # pkg deps, rustup (rust-toolchain.toml), release build, out/
sh packaging/freebsd/smoke.sh          # optional: installs PostgreSQL 17 and the package, tests it
```

Linking `paperrust` with fat LTO needs more than 5 GB RAM; on smaller machines use
`LTO=thin CODEGEN_UNITS=16 sh packaging/freebsd/build.sh`. Options are listed in the script header.
