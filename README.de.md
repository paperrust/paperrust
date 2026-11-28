# PaperRust

[English](README.md) · Deutsch

**Ein schnelles, schlankes Dokumentenarchiv, das mit Ihren paperless-ngx-Daten arbeitet.**

PaperRust scannt, erkennt und ordnet Ihre Dokumente und macht sie durchsuchbar. Es ist eine
Neuentwicklung von [paperless-ngx](https://github.com/paperless-ngx/paperless-ngx) 3.4.2 in Rust:
**gleiche Datenbank, gleiche Dateien, gleiche REST-API**, aber ein einzelnes Programm (`paperrust`)
neben PostgreSQL – ohne Python, Redis, Celery, Tika und Gotenberg. Der Umstieg von paperless-ngx ist
ein Befehl (oder ein Klick in der Desktop-App), und es gibt einen Weg zurück.

Website: <https://paperrust.org> · Dokumentation: [Deutsch](docs/user/README.md) ·
[English](docs/user/en/README.md) · Release-Notizen 0.1.1: [Deutsch](docs/RELEASE-NOTES-0.1.1.md) ·
[English](docs/RELEASE-NOTES-0.1.1.en.md) (0.1.0: [Deutsch](docs/RELEASE-NOTES-0.1.0.md))

> [!IMPORTANT]
> **Stand: 0.1.1, eine frühe Version.** PaperRust ist junge Software. Lesen Sie die
> [bekannten Grenzen](docs/RELEASE-NOTES-0.1.0.md#grenzen) und probieren Sie es zuerst mit einer
> **Kopie** Ihrer Daten aus. Der Windows-Installer ist noch nicht signiert.

## Auf einen Blick

Gemessen gegen paperless-ngx 3.4.2 auf demselben kleinen arm64-Rechner,
gleiche Dokumente, gleiche Datenbank – Einzelheiten und Grenzen weiter unten:

- **17-mal schneller beim Einlesen:** 230 Dokumente in 1 min 46 s statt 29 min 56 s.
- **Raspberry Pi 5:** 230 Dokumente in 3 min 05 s statt 27 min 07 s, mit einem Zwölftel der Rechenzeit
  (100 statt 1 253 CPU-Sekunden) – [Bericht](results/benchmark/2026-11-22-pi5/bericht.md).
- **16-mal schnellerer erster Start** (3,3 s statt 53 s); nach einem Neustart antwortet es nach 0,6 s
  statt 34 s.
- **12- bis 19-mal schnellere Antworten mit mehreren Nutzern gleichzeitig** (Median, 1–10 Personen,
  keine Fehler) – [Bericht](results/benchmark/2026-11-20-arm64/s2s3-rerun/bericht.md).
- **Ein Container statt zwei, 0,65 GB statt 3,5 GB** – ohne Python, Redis, Celery, Tika und
  Gotenberg. Startet mit 74 MiB Arbeitsspeicher (plus Datenbank).
- **Ihre Daten bleiben, wie sie sind:** gleiche Datenbank, gleiche Dateien, gleiche REST-API. Apps für
  paperless-ngx funktionieren weiter, und der [Umstieg](#umstieg-von-paperless-ngx) ist in wenigen
  Befehlen erledigt – mit Weg zurück.

```sh
docker pull ghcr.io/paperrust/paperrust:0.1.1     # linux/amd64 + linux/arm64
```

## Warum PaperRust?

Gemessen am 20. November 2026 auf einem kleinen arm64-Rechner (6 Kerne, 8 GB RAM) in Docker,
je Aufstellung ein Lauf, 255 erfundene Dokumente. PaperRust (Commit `e546b8f`) gegen paperless-ngx
3.4.2 in der sparsamsten Aufstellung (Valkey, ohne Tika/Gotenberg), beide mit PostgreSQL 17 und je
4 Kernen, beide säubern und begradigen Scans:

| | paperless-ngx 3.4.2 | PaperRust |
|---|---:|---:|
| Erststart bis Anmeldung möglich | 53,2 s | 3,3 s |
| Neustart bis die Dokumentliste antwortet | 33,7 s | 0,57 s |
| 230 Dokumente einlesen (200 Text-PDFs, 30 Scans) | 29 min 56 s | 1 min 46 s |
| Ein Scan (1 Seite) bis durchsuchbar | 25,8 s | 8,1 s |
| Liste/Suche während des Einlesens (Median / 95 %) | 150 / 187 ms | 10 / 32 ms |
| Arbeitsspeicher frisch gestartet (Dienste + Datenbank) | 598 + 41 MiB | 74 + 64 MiB |
| Arbeitsspeicher im Leerlauf nach dem Einlesen (Dienste + Datenbank) | 702 + 49 MiB | 420 + 74 MiB |
| Docker-Images ohne PostgreSQL ¹ | 3,47 GB (2 Container) | 0,65 GB (1 Container) |

¹ Gemessen am 18. November 2026 (Commit `c97f15c`).

**Wo PaperRust schlechter ist:** Die Datenbank braucht mehr Arbeitsspeicher (nach dem Einlesen 74
statt 49 MiB) und mehr Platz (logisch 28,4 statt 18,2 MB, das Datenbank-Volume 11 % mehr), und die
Speicherspitze beim Einlesen (800 MiB) liegt nur knapp unter paperless-ngx (838 MiB). Gemessen wurden
in dieser Tabelle nur ein Gerät (Raspberry Pi 5: siehe oben) und saubere Testdokumente; Unterschiede unter etwa 10 % nicht überbewerten.

Vollständiger Bericht mit Methode und Grenzen:
[results/benchmark/2026-11-20-arm64/bericht.md](results/benchmark/2026-11-20-arm64/bericht.md),
alle Tabellen: [tabellen.md](results/benchmark/2026-11-20-arm64/tabellen.md), Zusammenfassung:
[Release-Notizen → Leistung](docs/RELEASE-NOTES-0.1.0.md#leistung).

## Funktionen

- **Ein Programm statt fünf Dienste:** `paperrust serve --worker` ist Webserver, Worker, Zeitplaner
  und Eingangsordner in einem Prozess; Warteschlange und Cache liegen in PostgreSQL.
- **Drop-in-kompatibel:** Apps und Skripte für die paperless-ngx-API laufen weiter; alle
  `PAPERLESS_*`-Variablen gelten unverändert (eigene Einstellungen heißen `PAPERRUST_*`).
- **Dokumente hinzufügen** aus dem Eingangsordner, per Upload, aus Postfächern, von Scannern
  (Scan-Knopf über AirScan/eSCL, Scan per E-Mail) und über die REST-API. PDF, Bilder (auch HEIC und
  mehrseitiges TIFF), Office-Dateien (auch alte `.doc/.xls/.ppt`), E-Mails.
- **Texterkennung** mit Tesseract, eingebautes Säubern und Geraderichten von Scans, Archiv als PDF/A.
- **Ordnen und Suchen:** Korrespondenten, Dokumenttypen, verschachtelte Tags, Speicherpfade,
  Benutzerfelder, ASN, gespeicherte Ansichten, automatische Zuordnung, Volltextsuche.
- **Automatik:** Workflows als lesbarer Satz, Vorlagen, Testlauf vor dem Anwenden, Verlauf
  ([Automatik](docs/user/automatik.md)).
- **KI, wenn gewünscht** (standardmäßig aus; lokal mit Ollama oder OpenAI-kompatiblen Diensten):
  Vorschläge mit Prüfliste, Benutzerfelder, KI-Texterkennung, Chat, MCP-Server ([KI](docs/user/ai.md)).
- **PDF-Werkzeuge:** Schwärzen, Verkleinern, in PDF umwandeln, Reparatur beim Einlesen
  ([PDF-Werkzeuge](docs/user/pdf-tools.md)).
- **Schutz vor Ransomware:** Wächter mit Nur-Lese-Modus, verschlüsselte Sicherungen mit Versionen,
  Integritätsprüfung ([Ransomware](docs/user/ransomware.md)).
- **Eigene Oberfläche** in 10 Sprachen; die klassische Oberfläche im Stil von paperless-ngx bleibt
  wählbar.
- **Benutzer und Rechte:** Rollen, Gruppen, Objektrechte, Zwei-Faktor-Anmeldung, OpenID Connect und
  weitere Social Logins.

## Installation

| Weg | Was Sie bekommen | Anleitung |
|---|---|---|
| **Desktop-App – Windows** (x64) | Installer mit eingebettetem PostgreSQL 17, Tesseract und PDFium; keine Server-Einrichtung, keine Anmeldung am eigenen Rechner. **Noch nicht signiert**, darum warnt Windows SmartScreen; SHA-256-Prüfsumme vergleichen. Eine Code-Signatur über die SignPath Foundation ist geplant. | [Desktop-App](docs/user/installation.md#desktop) |
| **Desktop-App – Linux** (x86_64) | Portables `tar.gz` mit PostgreSQL 17, Tesseract (Deutsch, Englisch) und PDFium. Braucht glibc 2.36 oder neuer (Debian 12, Ubuntu 24.04, Fedora 37 oder neuer). Eigenes Fenster, wenn WebKitGTK 4.1 und libayatana-appindicator installiert sind, sonst Browser. Nicht als root starten. | [Linux-Paket](docs/user/installation.md#desktop-linux) |
| **Desktop-App – macOS** | Geplant (braucht einen Mac und ein Apple-Entwicklerkonto). | – |
| **Docker / Server** | Image aus `docker/Dockerfile` für `linux/amd64` und `linux/arm64`, compose-Dateien in `docker/`. Gleiche Volumes, Port und Befehle wie das paperless-ngx-Image. | [Docker](docs/user/installation.md#docker) |
| **NAS** | Vorlagen für Synology, UGREEN, Asustor (compose), Unraid (XML) und CasaOS in `docker/nas/` | [NAS](docs/user/installation.md#nas), [docker/nas/README.md](docker/nas/README.md) |
| **Aus dem Quelltext** | `paperrust`, `paperrust-pdftool` und `paperrust-migrate-db`, dazu PostgreSQL ≥ 14, Tesseract 5 und PDFium | [Bauen aus dem Quelltext](docs/user/installation.md#build) |

Desktop-Installer und -Pakete hängen an den
[GitHub-Releases](https://github.com/paperrust/paperrust/releases).

### Code holen

Das Git-Submodul `upstream/` (paperless-ngx v3.4.2, unverändert) wird **zum Bauen gebraucht** – mit
Submodulen klonen:

```sh
git clone --recurse-submodules https://github.com/paperrust/paperrust.git
cd paperrust
# schon ohne Submodule geklont?  git submodule update --init
```

### Docker compose

Das veröffentlichte Image `ghcr.io/paperrust/paperrust` (Tags `0.1.1`, `0.1`, `latest`; amd64 und
arm64) startet mit PostgreSQL so:

```sh
cd docker
mkdir -p consume export
# zuerst PAPERRUST_SECRET_KEY (und optional PAPERRUST_ADMIN_USER/PASSWORD) in docker-compose.env setzen
export PAPERRUST_IMAGE=ghcr.io/paperrust/paperrust:0.1.1
docker compose -f docker-compose.postgres.yml pull
docker compose -f docker-compose.postgres.yml up -d      # PaperRust + PostgreSQL 17
```

Ohne `PAPERRUST_IMAGE` baut Compose das Image stattdessen aus diesem Repository
(`docker compose -f docker-compose.postgres.yml build`).

Die Oberfläche ist dann unter <http://localhost:8000> erreichbar. Weitere Varianten in `docker/`:
schlanke Konfiguration (`docker-compose.slim.yml`), getrennte Web-/Worker-Prozesse
(`docker-compose.multiprocess.yml`), Umstieg von paperless-ngx (`docker-compose.migrate-from-paperless.yml`).

### Bauen aus dem Quelltext

Mit einer aktuellen stabilen Rust-Toolchain (festgelegt in `rust-toolchain.toml`):

```sh
cargo build --release -p pl-server                                              # target/release/paperrust
cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool  # PDF-Werkzeuge, neben paperrust legen
cargo build --release -p pl-dbmigrate                                           # paperrust-migrate-db (SQLite/MariaDB → PostgreSQL)
(cd frontend-light && pnpm install && pnpm build)                               # Oberfläche → frontend-light/dist
```

Ohne `paperrust-pdftool` neben `paperrust` fehlen Schwärzen, Verkleinern, Reparatur und Office→PDF.
Datenbank, Konfiguration, statische Dateien und eine systemd-Unit:
[Installation → Server ohne Django](docs/user/installation.md#standalone).

## Umstieg von paperless-ngx

> **Bitte gesund skeptisch sein.** PaperRust 0.1 ist eine frühe Version. Der Kern ist gründlich geprüft –
> aber nicht mit Ihren Dokumenten. Vor dem Umstieg: eine **vollständige Sicherung** von paperless-ngx machen
> (Export oder Datenbank und Medienordner), PaperRust zuerst **mit einer Kopie ausprobieren** und paperless-ngx
> erst abschalten, wenn alles passt. Nichts blind übernehmen. Der Weg zurück zu paperless-ngx bleibt offen.


PaperRust übernimmt paperless-ngx 2.20 und 3.0–3.4.2 (auch das neuere Tag-Baum-Schema nach 3.4.2)
mit einem Befehl – `paperrust migrate --from <paperless-Ordner | Datenbank-URL | Export>` – oder mit
„Von paperless-ngx umsteigen“ in der Desktop-App. Dateien, Benutzer mit ihren Passwörtern, Rechte,
Workflows und Mail-Regeln kommen mit. PaperRust prüft zuerst alles (`--dry-run`), sichert eine
bestehende Datenbank, bevor es sie ändert, und zeigt am Ende einen Abgleich. SQLite- und
MariaDB/MySQL-Datenbanken werden nach PostgreSQL kopiert.

**Der Weg zurück:** Nach dem Umstieg kann paperless-ngx dieselbe Datenbank nicht mehr öffnen. Zurück
geht es über die Sicherung oder einen Export im Format von paperless-ngx 3.4.2. Im Zweifel in eine
**Kopie** umsteigen (`--target`); dann bleibt paperless-ngx unberührt.

### Umstieg in wenigen Befehlen (Docker mit PostgreSQL)

```sh
# 1. sichern (paperless-ngx gestoppt: docker compose down, dann nur die Datenbank starten)
docker compose up -d db
docker compose exec db pg_dump -U paperless -Fc paperless > paperless-vorher.pgdump
# 2. in docker-compose.yml das paperless-ngx-Image durch ghcr.io/paperrust/paperrust:0.1.1 ersetzen
#    (Vorlage: docker/docker-compose.migrate-from-paperless.yml) – Volumes und PAPERLESS_SECRET_KEY bleiben
# 3. Probelauf: prüft alles, ändert nichts
docker compose run --rm webserver paperrust migrate --dry-run \
    --from "postgres://paperless:PASSWORT@db:5432/paperless" --media /usr/src/paperless/media
# 4. umsteigen (derselbe Befehl ohne --dry-run), dann starten und mit den bisherigen Benutzern anmelden
docker compose up -d
```

Soll das Original unberührt bleiben? Mit `--target "postgres://paperless:PASSWORT@db:5432/paperrust"`
(eine leere Datenbank) kopiert PaperRust alles dorthin. Zurück: `paperless-vorher.pgdump`
zurückspielen und das paperless-ngx-Image wieder eintragen.

- Schritt für Schritt: [Deutsch](docs/user/umstieg.md) · [English](docs/user/en/umstieg.md)
- Ausführlich (gleiche Datenbank, SQLite/MariaDB, Export/Import, Rückweg):
  [Deutsch](docs/user/migration.md) · [English](docs/user/en/migration.md)

## Dokumentation

- Handbuch: [Deutsch](docs/user/README.md) · [English](docs/user/en/README.md) – Installation, erste
  Schritte, Konfiguration, Mail, Scanner, Workflows, KI, Rechte, Sicherung, Sicherheit,
  Fehlerbehebung. Dasselbe Handbuch ist in die App eingebaut (`/rs/docs/`).
- Release-Notizen 0.1.0 mit allen bekannten Grenzen: [Deutsch](docs/RELEASE-NOTES-0.1.0.md) ·
  [English](docs/RELEASE-NOTES-0.1.0.en.md)
- Für Entwickler: [Architektur](docs/ARCHITECTURE.md), Verträge je Crate mit „Known differences“ in
  [docs/contract/](docs/contract/), [neue upstream-Versionen übernehmen](docs/UPSTREAM-SYNC.md),
  [Release-Gate](docs/RELEASE-GATE.md), Name und Kennzeichen: [docs/BRANDING.md](docs/BRANDING.md).

## Stand und Grenzen

PaperRust 0.1.0 ist die erste Version. Vor der Freigabe lief ein 24-Stunden-Dauertest unter
gemischter Last ohne einen einzigen Server-Fehler ([Bericht](results/release/soak/2026-11-19-24h/BERICHT.md))
und eine unabhängige Sicherheitsprüfung des Codes, alle Funde behoben
([Bericht](results/release/review/security-2026-11-19.md)).

**So wurde der Kern geprüft:** rund 2 100 automatische Tests (Windows und Linux arm64), Vergleich der
API mit einem laufenden paperless-ngx 3.4.2, Last-, Gleichzeitigkeits- und Ausfalltests, ein
24-Stunden-Dauertest, mehrere Stunden Fuzzing, eine komplette Wiederherstellung auf einem frischen
System und Klicktests der Oberfläche in Chromium. Was wir selbst nicht prüfen konnten (Safari, Firefox,
Android, Screenreader, echte NAS-Geräte, viele echte Dokumentbestände), steht offen unten. **Hier
brauchen wir euch:** Wenn etwas nicht klappt, bitte ein
[Issue anlegen](https://github.com/paperrust/paperrust/issues) – jede Meldung macht PaperRust stabiler.

Wichtigste Grenzen (vollständig in den [Release-Notizen](docs/RELEASE-NOTES-0.1.0.md#grenzen)):

- **Nur PostgreSQL** (ab 14); SQLite/MariaDB werden beim Umstieg kopiert.
- Der Windows-Installer ist noch **nicht signiert**; das Linux-Paket ist neu (auf frischem Ubuntu 24.04
  und Debian 12 geprüft); noch keine macOS-App.
- Nicht eigens geprüft: Safari, Firefox, Android-Browser, Screenreader, echte NAS-Geräte.
- Die **automatische Zuordnung** nutzt ein eigenes Modell und lernt nach dem Umstieg neu.
- Office-Dateien werden zu schlichteren Archiv-PDFs als mit LibreOffice/Gotenberg; PDF/A ist nicht
  mit veraPDF bestätigt.
- Weniger Social-Login-Anbieter (OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud,
  Discord, Gitea); kein Django-Admin (`/admin/`).
- Die Oberfläche hat 10 Sprachen; Fehlermeldungen der API sind nur Englisch.
- Mehrere Anbindungen (Azure-Texterkennung, S3-Sicherung, Scanner-Hardware, OAuth-Mail,
  Social-Login-Anbieter) sind mit simulierten Antworten getestet, nicht gegen die echten Dienste.

Referenzversion: paperless-ngx **v3.4.2** (Submodul `upstream/`, nie verändert). API-Antworten werden
gegen eine laufende paperless-ngx-Instanz verglichen (`tests/parity/`); jede akzeptierte Abweichung
steht als „Known difference“ in `docs/contract/*.md`.

## Mitarbeit und Sicherheit

- Bauen, Testen und Änderungen einreichen: [CONTRIBUTING.md](CONTRIBUTING.md) (Englisch)
- Sicherheitslücken melden (vertraulich, nicht als Issue): [SECURITY.md](SECURITY.md) (Englisch)
- Verhaltenskodex: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)

Fehler in PaperRust bitte hier melden, **nicht** beim paperless-ngx-Projekt.

## Lizenz und Herkunft

PaperRust steht unter der **GNU General Public License v3.0** (`GPL-3.0-only`), siehe
[LICENSE](LICENSE) und [NOTICE](NOTICE).

PaperRust basiert auf [paperless-ngx](https://github.com/paperless-ngx/paperless-ngx) (GPL-3.0,
© die paperless-ngx-Autoren), das selbst auf paperless-ng und Paperless aufbaut. Fachliche Logik,
Datenbankschema und Teile der Dokumentation stammen von dort – vielen Dank an alle, die es gebaut
haben. Fremdkomponenten und ihre Lizenzen stehen in [NOTICE](NOTICE).

PaperRust ist ein unabhängiges Projekt und nicht mit dem paperless-ngx-Projekt verbunden, von ihm
gebilligt oder unterstützt. „Rust“ ist eine Marke der Rust Foundation; PaperRust wird von der Rust
Foundation weder unterstützt noch gebilligt.
