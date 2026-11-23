# PaperRust

**A fast, lean document archive that works with your paperless-ngx data.**

PaperRust scans, recognises (OCR) and files your documents and makes them searchable. It is a re-implementation
of [paperless-ngx](https://github.com/paperless-ngx/paperless-ngx) 3.4.2 in Rust: the **same database, the same
files and the same REST API**, but one container next to PostgreSQL - no Python, Redis, Celery, Tika or
Gotenberg. Apps and scripts written for paperless-ngx keep working.

- Website: <https://paperrust.org>
- Source, issues and documentation: <https://github.com/paperrust/paperrust>
- Release notes: <https://github.com/paperrust/paperrust/releases>

> PaperRust is young software (0.1.x). Read the known limits in the release notes and try it with a **copy** of
> your data first.

## Tags

| Tag | Meaning |
|---|---|
| `latest` | Newest stable release |
| `0.1`, `0.1.2`, ... | Release line / exact release (pin one of these in production) |
| `edge` | Manual development build, not for production |

Platforms: `linux/amd64`, `linux/arm64`. The same image is also published as `ghcr.io/paperrust/paperrust`.

> **Note:** Docker Hub receives release tags starting with the next release. Until then only `edge` is here; the
> current stable release is `ghcr.io/paperrust/paperrust:0.1.1` (same image, use it in the compose file below).

## Quick start (docker compose)

```yaml
services:
  db:
    image: postgres:17
    restart: unless-stopped
    volumes:
      - pgdata:/var/lib/postgresql/data
    environment:
      POSTGRES_DB: paperless
      POSTGRES_USER: paperless
      POSTGRES_PASSWORD: paperless
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U paperless -d paperless"]
      interval: 10s
      timeout: 5s
      retries: 10

  paperrust:
    image: paperrust/paperrust:latest
    restart: unless-stopped
    depends_on:
      db:
        condition: service_healthy
    ports:
      - "8000:8000"
    volumes:
      - data:/usr/src/paperless/data
      - media:/usr/src/paperless/media
      - ./consume:/usr/src/paperless/consume
      - ./export:/usr/src/paperless/export
    environment:
      PAPERRUST_SECRET_KEY: change-this-to-a-long-random-string   # openssl rand -base64 48
      PAPERRUST_DBHOST: db
      PAPERRUST_DBNAME: paperless
      PAPERRUST_DBUSER: paperless
      PAPERRUST_DBPASS: paperless
      PAPERRUST_ADMIN_USER: admin
      PAPERRUST_ADMIN_PASSWORD: change-me-too
      PAPERRUST_TIME_ZONE: Europe/Berlin
      PAPERRUST_OCR_LANGUAGE: deu+eng
    stop_grace_period: 45s

volumes:
  data:
  media:
  pgdata:
```

Then open <http://localhost:8000> and sign in with the admin user.

## Configuration

| Variable | Required | Meaning |
|---|---|---|
| `PAPERRUST_SECRET_KEY` | yes | Long random string. When switching from paperless-ngx, use its old `PAPERLESS_SECRET_KEY`. |
| `PAPERRUST_DBHOST`, `PAPERRUST_DBPORT`, `PAPERRUST_DBNAME`, `PAPERRUST_DBUSER`, `PAPERRUST_DBPASS` | host + password | PostgreSQL 14 or newer. SQLite and MariaDB are not supported (a migration tool is included). |
| `PAPERRUST_ADMIN_USER`, `PAPERRUST_ADMIN_PASSWORD` | no | Creates the first administrator on start if missing. |
| `PAPERRUST_URL` | no | Public address behind a reverse proxy, e.g. `https://paperrust.example.com`. |
| `PAPERRUST_TIME_ZONE` | no | e.g. `Europe/Berlin`. |
| `PAPERRUST_OCR_LANGUAGE` | no | Tesseract languages, e.g. `deu+eng`. Included: `eng deu fra ita spa`. |
| `PAPERRUST_OCR_LANGUAGES` | no | Extra languages installed on start (container must start as root). |
| `PAPERRUST_CONSUMER_POLLING_INTERVAL` | no | Poll the consumption folder every N seconds (network shares). |
| `USERMAP_UID`, `USERMAP_GID` (or `PUID`, `PGID`) | no | Owner of created files when the container starts as root (`user: "0:0"`). |

All `PAPERLESS_*` variables of paperless-ngx keep their meaning, and `*_FILE` variants read Docker secrets. Full
list: [configuration](https://github.com/paperrust/paperrust/blob/main/docs/user/en/configuration.md).

## Volumes and port

Same paths as the paperless-ngx image: `/usr/src/paperless/data`, `/usr/src/paperless/media`,
`/usr/src/paperless/consume`, `/usr/src/paperless/export`. Web UI on port `8000`. The container runs as UID/GID
1000 by default; health check built in.

Management commands work as in paperless-ngx, for example:

```sh
docker compose exec paperrust document_exporter ../export
docker compose exec paperrust createsuperuser
```

## Switching from paperless-ngx

Stop paperless-ngx and take a backup. Then start PaperRust with the same PostgreSQL database, the same `data`,
`media`, `consume` and `export` volumes and the same secret key; PaperRust checks the database and adds its own
tables. There is a way back. Guide:
<https://github.com/paperrust/paperrust#switching-from-paperless-ngx>

## License

GPL-3.0. PaperRust is not affiliated with the paperless-ngx project; it reuses paperless-ngx's database schema and
API for compatibility. Third-party licenses are in `/usr/share/licenses/paperrust/` inside the image.
