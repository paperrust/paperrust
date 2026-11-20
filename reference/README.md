# Referenzumgebung: paperless-ngx v3.4.2 (Django) für Paritäts-Diffs

Unverändertes upstream (`upstream/`, Tag v3.4.2) auf PostgreSQL 18 mit festen Testdaten.
**Betrieben nur vom Referenz-Agenten.** Andere Agenten nutzen die laufenden Dienste, starten keine
eigenen Instanzen und rufen höchstens `reference/reset.sh` auf (siehe unten).

> Aktueller Stand: Abschnitt [Status](#status) am Ende.

## Auf einen Blick

| Dienst | Adresse | Zugang |
|---|---|---|
| Django (paperless-ngx v3.4.2, granian/ASGI) | `http://127.0.0.1:8000` | Nutzer/Tokens unten |
| PostgreSQL 18.6 | `postgres://paperless:paperless@127.0.0.1:54329/paperless` | Superuser `postgres` / `postgres-local-only` |
| Redis-kompatibel (Microsoft Garnet 2.2.8) | `redis://127.0.0.1:63799` | ohne Passwort, DB 0 |
| Test-Profil (nur Upstream-Test-Adapter) | `http://127.0.0.1:8001` → DB `test_paperless` | nur bei Bedarf gestartet |

Alles lauscht nur auf `127.0.0.1`. Alle Passwörter/Tokens/Keys sind **lokale Testwerte**.

- `PAPERLESS_SECRET_KEY` = `paperless-rs-reference-secret-key-local-only-0123456789`
- `PAPERLESS_TIME_ZONE` = `Europe/Zurich`; `PAPERLESS_URL` = `http://127.0.0.1:8000`;
  `PAPERLESS_ALLOWED_HOSTS` = `127.0.0.1,localhost`; `PAPERLESS_DEBUG=false`; Redis-Präfix leer
- Medien: `reference/data/media` (`documents/originals|archive|thumbnails`), Daten/Index:
  `reference/data/data` (`index/` = tantivy), Logs: `reference/logs/` (`paperless.log`, `django.*.log`,
  `postgres.log`, `redis.*.log`)
- Gemeinsame Umgebung aller Skripte: `reference/env.sh` (alle `PAPERLESS_*`-Variablen). Für
  `manage.py` (nur lesend!): `source reference/env.sh && "$PY" "$REPO_DIR/upstream/src/manage.py" …`

## Nutzer, Passwörter, API-Tokens

Maschinenlesbar: [`users.json`](users.json) (Format aus `docs/contract/testing.md`).

| Rolle (`name`) | Benutzer | Passwort | Token (`Authorization: Token …`) | id | Rechte |
|---|---|---|---|---|---|
| admin | `admin` | `admin` | `a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001` | 2 | Superuser, Staff |
| user1 | `alice` | `alice` | `a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002` | 3 | Gruppe `editors` (view/add/change auf document, tag, correspondent, documenttype, storagepath, savedview, note, customfield, sharelink, paperlesstask, uisettings; view workflow/mailaccount/mailrule) + direkt `delete_document`, `delete_note` |
| user2 | `bob` | `bob` | `b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003` | 4 | Gruppe `viewers` (view auf document, tag, correspondent, documenttype, storagepath, note, customfield, savedview; add/change savedview, uisettings) + direkt add/change für document, tag, correspondent, documenttype, note |
| noperm | `carol` | `carol` | `c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004` | 5 | keine Modellrechte (API → 403), besitzt aber Dokumente und Tag `Private` |
| inactive | `dave` | `dave` | `d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005` | 6 | `is_active=false` (in `editors`) → 401 „User inactive or deleted.“ |

Sichtbare Dokumente in `/api/documents/`: admin 195, alice 116, bob 91, carol 403.
Weitere Gruppe: `auditors` (view_logentry, view_document; ohne Mitglieder). `AnonymousUser`
(guardian) hat id 1. Passwort-Hashes: PBKDF2-SHA256 mit festen Salts (`paperlessrsref<user>`),
dadurch deterministisch.

## Testdaten (`seed.py`, deterministisch)

Seed 4242, Basiszeit 2025-01-06T08:00Z; alle Zeitstempel (inkl. Audit-Log, Tokens, Notizen, Tasks)
fest. Angelegt direkt über das ORM, ohne Consumer/OCR. Dateien werden erzeugt (fpdf2/Pillow),
Prüfsummen (SHA-256) stimmen, Dateinamen über upstream `generate_filename` (wie der Consumer).

- **Dokumente:** 200 Wurzeldokumente (id 1–200) + 4 Versionen (id 201–204: v1/v2 von Dokument 1
  und 2, `root_document`, `version_index`, `version_label`). 195 sichtbar, 5 im Papierkorb (id 196–200).
  MIME: 189 PDF und 6 PNG (jeweils Original + Archiv-PDF + WebP-Thumbnail), 5 `text/plain` (ohne
  Archiv, `page_count` null). 134 mit ASN (1000+id). Speicherpfade mit Jinja-Vorlagen → Unterordner
  unter `originals/`/`archive/`. Inhalte DE/EN, teils mit „quarterly-tax-statement“, Umlauten, lange
  Titel (128 Zeichen). Ein Dokument ohne Tags je 11.
- **Besitz/Rechte:** admin, alice, bob, carol und „kein Besitzer“ gemischt. guardian: admin-Dokumente
  teils view/change für alice, view für Gruppe `viewers`; alice-Dokumente teils view für bob und
  change für `editors`; **bob-Dokumente** überwiegend *nicht* mit alice geteilt, einzelne per
  Objektrecht an alice (id ≡ 6 mod 50) bzw. per Gruppe `editors` (id ≡ 16 mod 50). bob besitzt Tag
  `Health` (an alice geteilt), Dokumenttyp `Medical`, Korrespondent `Doctor Weber` (beide ungeteilt),
  Speicherpfad `Bob private` (an `editors`). 89 Nutzer- und 44 Gruppen-Objektrechte.
- **Tags:** 20, verschachtelt bis Ebene 4 (Finance → Invoices → Unpaid → Overdue), Inbox-Tag `Inbox`,
  Farben, alle Matching-Algorithmen, Umlaut-Tag; `Car` wurde zu `Vehicle` umbenannt (Audit-Update).
- **Korrespondenten** 12, **Dokumenttypen** 8, **Speicherpfade** 5.
- **Custom Fields** 11 (alle Typen): string, url, date, boolean, integer, float, monetary (mit und ohne
  `default_currency`), documentlink (symmetrisch), select (Optionen mit festen IDs), longtext.
- **Notizen** 40 (eine gelöscht), **Saved Views** 5 (u. a. Custom-Field-Query, ohne Besitzer),
  **UI-Settings** für admin/alice, **Share Links** 5 (eine abgelaufen, eine bis 2099, eine gelöscht;
  Slug = `refshare` + 2 Ziffern + 40×`x`, z. B. `/share/refshare00xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`),
  **Workflows** 3 mit Triggern (Consumption, Added, Updated, Scheduled) und Aktionen (Assignment,
  Removal, E-Mail, Webhook an `127.0.0.1:9`), **Mail-Accounts** 3 (Fake-Server `*.invalid` /
  127.0.0.1:1143; IMAP, STARTTLS, Gmail-OAuth), **Mail-Regeln** 3, **Processed Mail** 4,
  **PaperlessTask** 12 (alle Status, diverse Typen/Quellen), **Audit-Log** 918 Einträge.
- **Volltextindex:** `manage.py document_index reindex` (tantivy, `data/data/index`).

## Start / Stop / Reset

Git-Bash, aus beliebigem Verzeichnis:

```bash
reference/pg-start.sh      reference/pg-stop.sh       # PostgreSQL  (PID: run/postgres.pid)
reference/redis-start.sh   reference/redis-stop.sh    # Garnet      (PID: run/redis.pid)
reference/django-start.sh  reference/django-stop.sh   # Django      (PID: run/django.pid; startet PG+Garnet mit)
reference/reset.sh                                    # Snapshot zurückspielen (~10 s), Django läuft weiter
reference/seed.sh                                     # alles neu: DB, migrate, Schema-Dump, Seed, Index, Snapshot (~2–3 min)
```

- Stoppen nur per gespeicherter PID (`pg_ctl stop` über `postmaster.pid`; Garnet/granian per
  `taskkill /PID <pid> [/T]`), nie per Prozessname.
- **`reset.sh`** (Schreibtests, idempotent): `DROP DATABASE paperless WITH (FORCE)` →
  `CREATE DATABASE paperless TEMPLATE paperless_snapshot`, `data/data` + `data/media` aus
  `snapshot/data.tar`, Redis `FLUSHALL` (Cache, Celery-Queue). Offene DB-Verbindungen (auch eure)
  werden getrennt; Django verbindet sich selbst neu. Fehlt die Template-DB, wird aus
  `snapshot/paperless.dump` (pg_dump -Fc) wiederhergestellt. Bitte nicht parallel aufrufen.
- Dienste starten über `Start-Process` in einer eigenen versteckten Konsole (`detach` in `env.sh`):
  sie überleben das Ende der aufrufenden Shell (sonst sterben PostgreSQL-Backends mit 0xC0000142
  bzw. der Aufrufer hängt an geerbten Pipes).

## Schema (maßgeblich für alle Rust-Agenten)

- [`schema/postgres.sql`](schema/postgres.sql) – `pg_dump --schema-only --no-owner --no-privileges`
  nach `manage.py migrate` (75 Tabellen, inkl. Generated Columns `content_length`,
  `value_monetary_amount`).
- [`schema/migrations.txt`](schema/migrations.txt) – angewandte Migrationen (247).
- [`schema/v3.4.2.json`](schema/v3.4.2.json) – OpenAPI (`manage.py spectacular --format openapi-json`).

## Was funktioniert / was nicht

Funktioniert (geprüft): REST-API inkl. Paginierung, Token/Basic-Auth, Objektrechte, Download/
Preview/Thumb (Original + Archiv, auch txt/png), `/share/<slug>`, Volltextsuche (`?query=`) und
Autocomplete, Metadaten, History (Audit-Log), Suggestions (ohne trainierten Klassifikator),
Statistiken, `/api/status/`, Tasks, Workflows, Mail-Objekte, Papierkorb, PATCH + Reset.

Nicht verfügbar / bewusst aus:
- **Kein Celery-Worker, kein Beat, kein Flower.** Aufgaben (`consume_file`, `bulk_update`,
  Index-Updates, Mail-Abruf, Klassifikator-Training) landen nur in Redis (Garnet) und werden **nicht
  ausgeführt**; `/api/status/` meldet `celery_status: WARNING`. Folgen: Upload (`post_document`)
  erzeugt kein Dokument; per Celery laufende Bulk-Edit-Operationen wirken nicht; der Suchindex folgt
  API-Änderungen nicht (Neuaufbau nur via `seed.sh`, Reset stellt den Seed-Index wieder her).
- **Kein OCR/Konsum:** tesseract, ghostscript, poppler (`pdfinfo`, `pdftoppm`), unpaper fehlen
  (`manage.py check` warnt). Keine Thumbnail-/Archiv-Erzeugung durch paperless selbst.
- **Kein Tika/Gotenberg**, **kein AI/LLM** (`PAPERLESS_AI_ENABLED` aus; `torch`,
  `sentence-transformers`, `transformers`, `tokenizers`, `safetensors`,
  `llama-index-embeddings-huggingface` nicht installiert – nur lazy in `paperless_ai` importiert).
- Mail-Konten zeigen auf nicht existierende Server; Verbindungstests schlagen fehl (gewollt).
- Antwortzeiten: erste Aufrufe von `metadata`/`suggestions` 1–5 s (Imports), danach < 0,5 s.

## Abweichungen zu Produktion (Docker-Image) und Begründung

| Bereich | Docker v3.4.2 | Referenz | Begründung |
|---|---|---|---|
| OS/Python | Debian trixie, CPython 3.12 | Windows 11, CPython 3.12.10 | kein Docker/WSL auf dem Rechner |
| Webserver | `granian --interface asginl --ws --loop uvloop` | gleich, aber `--loop asyncio`, 1 Worker | uvloop gibt es nicht für Windows |
| Python-Pakete | uv.lock (linux) | exakt die Lock-Versionen der linux/x86_64-Auflösung | `uv.lock` sperrt nur linux/darwin; 174 Dateien hash-gleich mit dem Lock, 52 Windows-Wheels nur über PyPI (`wheel-provenance.txt`) |
| psycopg | `psycopg-c` 3.3.4 | `psycopg-binary` 3.3.4 | C-Build unter Windows nicht möglich; gleiche Version |
| zusätzlich | – | `pywin32==311` | Windows-Abhängigkeit von `portalocker`; ohne sie schreibt `concurrent-log-handler` keine Logs |
| libmagic | Debian `libmagic1` 5.46 | MSYS2-UCRT64 `file` 5.48 (`bin/libmagic`, `MAGIC` gesetzt, vorn im PATH) | python-magic braucht libmagic; offizielles MSYS2-Repo, SHA-256 aus Repo-DB geprüft |
| Redis | Redis | Microsoft Garnet 2.2.8 (RESP, Lua an, nur RAM) | kein offizielles Redis für Windows; Garnet: MIT, Microsoft-signiert. Geprüft: GET/SET/EXPIRE, Listen/BRPOP, Pub/Sub, ZSET, EVAL; `INFO` meldet 7.4.3 |
| Settings | `paperless.settings` | `DJANGO_SETTINGS_MODULE=reference_settings` (`overlay/`): `from paperless.settings import *` + (1) private GROSS-Namen (`_CHANNELS_REDIS_URL`) mitkopieren, (2) `STATICFILES_DIRS=[("frontend", reference/frontend)]`, (3) Shim `os.statvfs` über `shutil.disk_usage` | (1) `import *` überspringt `_`-Namen → `/api/status/` sonst 500; (2) Frontend nicht nach `upstream/` bauen; (3) Windows hat kein `statvfs` → `/api/status/` sonst immer 500 |
| PostgreSQL | postgres-Image (glibc `en_US.utf8`) | EDB 18.6, ICU-Kollation `en-US`, `lc_messages=C`, `fsync/synchronous_commit/full_page_writes=off`, `shared_buffers=32MB`, `max_connections=100` | kein glibc-Locale unter Windows; ICU kommt glibc-`en_US` am nächsten (Sortierung von Satzzeichen kann abweichen, für Paritäts-Diffs egal: Django und Rust nutzen dieselbe DB). fsync aus = schnelles Reset, Daten sind Wegwerf |
| Login-Drossel | `PAPERLESS_TOKEN_THROTTLE_RATE` 5/min | unverändert 5/min | wie Produktion; `/api/token/` sparsam nutzen – Tokens stehen oben |
| `/api/status/` | `server_os` Linux, `install_type` docker | `Windows-11-10.0.22631-SP0`, `bare-metal`, Pfade/Speicher von C: | reale Plattform |
| Pfade | `/usr/src/paperless/...` | `<repo>\reference\data\...` | erscheinen in Logs/`/api/status/`/Fehlern |

## Profil „test“ (Upstream-Test-Adapter)

pytest-django legt die Test-DB `test_paperless` an (`--reuse-db`; Rolle `paperless` hat `CREATEDB`).
Den Server dafür startet der Referenz-Agent bzw. der Adapter-Agent nach Absprache:

```bash
REF_PROFILE=test reference/django-start.sh   # :8001, DB test_paperless, PID run/django-test.pid
REF_PROFILE=test reference/django-stop.sh
```

Setzt `PAPERLESS_DBNAME=test_paperless`, `PAPERLESS_SECRET_KEY=test-secret-key-do-not-use-in-production`
(wie `upstream/pyproject.toml` `[tool.pytest_env]`), `PAPERLESS_ALLOWED_HOSTS=testserver,127.0.0.1,localhost`,
eigene Ordner `reference/data-test/`, Redis-Präfix `test`, Port 8001, `PAPERLESS_URL=http://127.0.0.1:8001`.

Testabhängigkeiten (Versionen aus `uv.lock`, Gruppe `testing`) sind in der Referenz-venv installiert
(`requirements-testing-windows.txt`): pytest 9.1.1, pytest-django 4.14.0, pytest-env 1.7.1,
pytest-mock 3.15.1, pytest-httpx 0.36.2, factory-boy 3.3.3, faker 40.39.0, imagehash 4.3.2,
daphne 4.2.3, pytest-xdist, pytest-randomly, time-machine, …  Python: `reference/venv/Scripts/python.exe`.

## Frontend

Das gebaute Angular-Frontend ist nötig, damit Django die UI ausliefert (`documents/views.py::IndexView`
lädt `static/frontend/<sprache>/main.js` usw.; im Git-Tag fehlt `src/documents/static/frontend`).
Statt eines eigenen `ng build` (zu viel RAM auf dem geteilten Laptop) wird das **offizielle,
von upstream-CI gebaute Frontend** aus dem Release-Archiv verwendet:
`https://github.com/paperless-ngx/paperless-ngx/releases/download/v3.4.2/paperless-ngx-v3.4.2.tar.xz`,
SHA-256 `599be82cd8fc184436b757f9b193101fb71a90bf0facf5695dcda43152fffc85` (GitHub-Asset-Digest, geprüft).
Daraus nur `src/documents/static/frontend/` (38 Sprachen) nach `reference/frontend/`; Stichprobe
`documents/views.py`, `documents/models.py`, `paperless/settings/__init__.py` im Archiv identisch mit
`upstream/` (bis auf CRLF des Checkouts). `django-start.sh` führt `collectstatic` aus, sobald das
Frontend neu ist (dauert hier ~10 min wegen whitenoise-Kompression von ~7400 Dateien; einmalig).
Das ist dieselbe Struktur wie im Docker-Image (`STATIC_ROOT` mit `frontend/<lang>/…`).

## Einrichtung von Grund auf (Herkunft der Binärdateien)

1. **PostgreSQL 18.6** – `https://get.enterprisedb.com/postgresql/postgresql-18.6-5-windows-x64-binaries.zip`
   (verlinkt auf der EDB-Seite *download-postgresql-binaries*), SHA-256
   `e2246ba91d22345bc3d017586c09ede52d9df180b1eeb480f050445f1cad84e2` laut EDB-`.sha256`, geprüft
   (die Binaries selbst sind nicht Authenticode-signiert). Entpackt nach `pg/pgsql` ohne pgAdmin/Doku/
   Header. Cluster: `pg-init.sh` (`pg/cluster`, UTF8, ICU en-US, SCRAM, nur 127.0.0.1, Port 54329).
2. **Garnet 2.2.8** – GitHub-Release `microsoft/garnet` v2.2.8 `win-x64-based-readytorun.zip`,
   SHA-256 `41ab3d438c8e1b8d146c33bfb2d010dd514f687d7fd276b97511981839940d00` (GitHub-Asset-Digest),
   `GarnetServer.exe` Authenticode „Microsoft Corporation“ gültig. Braucht die **.NET-10-Runtime**
   10.0.14 portabel in `bin/dotnet` (`builds.dotnet.microsoft.com`, SHA-512 aus `releases.json` geprüft).
3. **Python** – `venv` aus Python 3.12; `uv` (PyPI) nur für `uv export --frozen` aus `upstream/uv.lock`;
   `tools/make_requirements.py` → `requirements-windows.txt` / `requirements-testing-windows.txt`;
   `pip download` → `tools/verify_wheels.py` (Hashvergleich mit dem Lock, Bericht
   `wheel-provenance.txt`, 0 Abweichungen) → `pip install --no-index --no-deps`; `pip check` sauber.
4. **libmagic** – `tools/fetch_msys2.py`: `file` 5.48, `libsystre`, `libtre`, `gettext-runtime`,
   `libiconv` aus `repo.msys2.org/mingw/ucrt64`, SHA-256 gegen `ucrt64.db` → `bin/libmagic`.
5. **Frontend** – siehe Abschnitt Frontend (Release-Archiv → `reference/frontend`).
6. `pg-init.sh`, `seed.sh`, dann `django-start.sh`.

Downloads in `reference/downloads/`. Nicht eingecheckt (`reference/.gitignore`): `pg/ venv/ data/
data-test/ bin/ downloads/ logs/ run/ snapshot/ frontend/ build/`.

## Status

Stand 2026-11-08 (Referenz-Agent):

- PostgreSQL 18.6: **läuft** (127.0.0.1:54329, PID in `run/postgres.pid`)
- Garnet 2.2.8: **läuft** (127.0.0.1:63799, PID in `run/redis.pid`)
- Django v3.4.2 (granian, ASGI): **läuft** (127.0.0.1:8000, PID in `run/django.pid`); Seed, Index und
  Snapshot vorhanden; `reset.sh` geprüft (~10 s, Django läuft weiter)
- Frontend: **wird ausgeliefert** (`/accounts/login/` → `/dashboard`, `static/frontend/<lang>/main.js`)
- Profil test (:8001): einmal geprüft (Host `testserver` → 200), **nicht gestartet**; DB `test_paperless`
  existiert nicht (legt pytest-django an)
- RAM der Dienste zusammen ca. 350–600 MB (PostgreSQL ~250 MB inkl. Backends, Garnet ~65 MB, Django-Worker)
