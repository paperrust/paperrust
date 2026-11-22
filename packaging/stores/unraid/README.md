# PaperRust – Unraid templates

Official [Unraid Community Applications](https://ca.unraid.net) templates for
[PaperRust](https://paperrust.org), a fast, lean document archive that works with your paperless-ngx data
(same database, files and REST API as paperless-ngx 3.4.2).

| Template | Image | Purpose |
|---|---|---|
| [`templates/paperrust.xml`](templates/paperrust.xml) | `ghcr.io/paperrust/paperrust:latest` | PaperRust server: web UI, worker, scheduler, consumption folder |
| [`templates/paperrust-postgres.xml`](templates/paperrust-postgres.xml) | `postgres:17` | Optional PostgreSQL preconfigured for PaperRust |

## Install

1. **Apps** tab → search for *PaperRust*.
2. Install **paperrust-postgres** first (or use an existing PostgreSQL 14+ container) and choose a database password.
3. Install **paperrust**: set *Secret key* (long random string, `openssl rand -base64 48`), *DB host* (the IP of
   your Unraid server, or `paperrust-postgres` if both containers are on the same custom Docker network),
   *DB password* (same as above) and *Admin password*.
4. Open the web UI on port 8000 and sign in with the admin user.

Both containers are on the default bridge network, so PaperRust reaches PostgreSQL through the server IP and the
published port. A custom network (`docker network create paperrust`, then *Network Type: Custom: paperrust* in
both templates) lets you use the container name as *DB host* and drop the published PostgreSQL port.

Data lives under `/mnt/user/appdata/paperrust/` (`data`, `media`, `consume`, `export`, `pgdata`).

## Switching from paperless-ngx

PaperRust uses the same folders and database. Stop paperless-ngx, take a backup, then point the PaperRust template
at the same `data`/`media`/`consume`/`export` folders and the same database, and reuse the old
`PAPERLESS_SECRET_KEY`. Guide: <https://github.com/paperrust/paperrust#switching-from-paperless-ngx>

## Support

- Issues: <https://github.com/paperrust/paperrust/issues>
- Documentation: <https://paperrust.org>

## License

Templates: GPL-3.0 (same as PaperRust). PostgreSQL is distributed under the PostgreSQL License.
