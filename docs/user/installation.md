# Installation

[English](en/installation.md) · [Übersicht](README.md)

PaperRust ist ein einzelnes Programm (`paperrust`) plus PostgreSQL. Es gibt vier Wege; welcher
passt, hängt davon ab, ob schon ein paperless-ngx läuft.

| Weg | Für wen | Stand |
|---|---|---|
| [Server ohne Django (Standalone)](#standalone) | neue Installation oder vollständiger Umstieg | lauffähig, Lücken siehe [unten](#standalone-limits) |
| [Mischbetrieb vor paperless-ngx](#mixed-operation) | – | **gibt es nicht mehr** (nur PaperRust bis v0.1.0) |
| [Docker](#docker) | Container-Betrieb | Image aus dem Repository bauen (`docker/`); für Linux arm64 gebaut, geprüft und gemessen; amd64 gebaut und im Rauchtest geprüft (2026-11-20); **noch nicht veröffentlicht** |
| [Desktop-App](#desktop) | ein Rechner, ohne Server-Einrichtung | Windows-Installer (noch **nicht signiert**); Linux-Paket (`tar.gz`, x86_64); macOS geplant |

<a id="requirements"></a>

## Voraussetzungen

<a id="postgresql"></a>

- **PostgreSQL ≥ 14** (Mindestversion von paperless-ngx v3.4.2; geprüft mit 18). SQLite und
  MariaDB werden von PaperRust nicht unterstützt – bestehende Datenbanken zieht man vorher um
  ([Umstieg](migration.md#sqlite-mariadb)).
- **Tesseract 5** mit den Sprachdaten (`deu.traineddata`, `eng.traineddata` …) für die
  Texterkennung. Ohne Tesseract laufen digitale PDFs (mit Textebene) trotzdem durch.
- **PDFium** (Bibliothek, z. B. aus `pdfium-binaries`) für Vorschaubilder, das Rendern von Seiten
  für die Texterkennung und Barcodes.
- **`paperrust-pdftool`** (wird mitgebaut, siehe unten) neben `paperrust`: Schwärzen, Verkleinern,
  Reparieren, Office→PDF ([PDF-Werkzeuge](pdf-tools.md)). Fehlt es, laufen diese Funktionen nicht.
- **Weboberfläche:** die gebaute PaperRust-Oberfläche (`frontend-light`) und einige statische
  Dateien für Anmeldeseiten und API-Ansicht (siehe [Statische Dateien](#static-files)).
- Nicht nötig: Python, Redis/Valkey, Celery, Tika, Gotenberg, OCRmyPDF, Ghostscript.

Arbeitsspeicher (gemessen am 2026-11-20, Docker auf einem Jetson Orin Nano, mit Worker): frisch
gestartet 74 MiB für PaperRust plus 64 MiB für PostgreSQL; im Leerlauf nach dem Einlesen von
230 Dokumenten 420 + 74 MiB; Spitze beim Einlesen 800 MiB für PaperRust. Für ein kleines Archiv
reichen 2 GB RAM für das ganze Gerät; Einzelheiten in den
[Release-Notizen](../RELEASE-NOTES-0.1.0.md#leistung).

<a id="build"></a>

## Bauen aus dem Quelltext

Fertige Pakete für Server gibt es noch nicht (für Windows und Linux gibt es die [Desktop-App](#desktop)). Mit einer aktuellen stabilen Rust-Toolchain:

```sh
git clone --recurse-submodules https://github.com/paperrust/paperrust.git
cd paperrust
cargo build --release -p pl-server            # erzeugt target/release/paperrust
cargo build --release -p pl-dbmigrate         # Umzugswerkzeug (nur bei SQLite/MariaDB nötig)
cargo build --release -p pl-pdftools --features engine --bin paperrust-pdftool   # PDF-Werkzeuge
```

<a id="static-files"></a>

## Statische Dateien (Weboberfläche)

PaperRust hat eine eigene Weboberfläche (`frontend-light`, Svelte). Sie läuft unter `/rs/app/`;
die Startadresse `/` und alle Adressen der früheren paperless-ngx-Oberfläche (z. B.
`/documents/12/details`, `/tags`) leiten dorthin weiter, Lesezeichen funktionieren also weiter.

- **Oberfläche bauen:** `cd frontend-light && pnpm install && pnpm build` erzeugt
  `frontend-light/dist/`. PaperRust sucht sie unter `<PAPERRUST_STATICDIR>/../light`
  (Docker: `/usr/src/paperless/light`) oder unter `PAPERRUST_LIGHT_UI_DIR`.
- **Statische Dateien für Server-Seiten** (Login-Seite, Swagger-Ansicht der API): `bash
  scripts/build-static.sh` erzeugt `results/static/`; dann `PAPERRUST_STATICDIR=<repo>/results/static`.
  Für die API-Ansicht vorher `bash scripts/fetch-extra-static.sh <ordner>` und
  `EXTRA_STATIC=<ordner>` setzen.
- Die Angular-Oberfläche von paperless-ngx liefert PaperRust nicht mehr aus (abgelöst am
  2026-11-13). Beim Umstieg von paperless-ngx bleiben Daten, Benutzer und gespeicherte Ansichten
  erhalten.

<a id="standalone"></a>

## Server ohne Django (Standalone)

1. **Datenbank anlegen** (leer, UTF8):

   ```sh
   psql -U postgres -c "CREATE USER paperless PASSWORD 'geheim'"
   psql -U postgres -c "CREATE DATABASE paperless OWNER paperless ENCODING 'UTF8' TEMPLATE template0"
   ```

2. **Konfiguration** – Datei `paperrust.conf` (`paperless.conf` wird weiter gelesen) neben dem
   Installationsordner bzw. Umgebungsvariablen. Mindestens:

   ```sh
   PAPERRUST_DBENGINE=postgresql
   PAPERRUST_DBHOST=localhost
   PAPERRUST_DBNAME=paperless
   PAPERRUST_DBUSER=paperless
   PAPERRUST_DBPASS=geheim
   PAPERRUST_SECRET_KEY=<lange Zufallszeichenkette>
   PAPERRUST_URL=https://paperrust.example.com
   PAPERRUST_TIME_ZONE=Europe/Zurich
   PAPERRUST_OCR_LANGUAGE=deu+eng
   PAPERRUST_DATA_DIR=/srv/paperrust/data
   PAPERRUST_MEDIA_ROOT=/srv/paperrust/media
   PAPERRUST_CONSUMPTION_DIR=/srv/paperrust/consume
   PAPERRUST_STATICDIR=/srv/paperrust/static

   PAPERRUST_BIND=127.0.0.1:8080
   ```

   Alle Variablen: [Konfiguration](configuration.md). Statt `PAPERRUST_*` gehen auch die
   Namen von paperless-ngx (`PAPERLESS_*`).

3. **Routen:** Nichts zu tun – PaperRust bedient alle Routen selbst. `routes.toml`
   (Arbeitsverzeichnis oder `PAPERRUST_ROUTES`) ist optional und kann einzelne Routen nur
   abschalten (`"<id>" = "off"`).

4. **Schema anlegen** – ohne Django; legt auf leerer DB das Schema von paperless-ngx v3.4.2 an
   bzw. prüft eine bestehende DB und spielt die eigenen `paperrust_*`-Tabellen ein:

   ```sh
   paperrust manage migrate
   ```

5. **Administrator anlegen:**

   ```sh
   paperrust manage createsuperuser
   # oder ohne Rückfrage: PAPERRUST_ADMIN_USER / PAPERRUST_ADMIN_PASSWORD setzen und
   paperrust manage manage_superuser
   ```

6. **Starten** (Webserver + Worker + Eingangsordner + Zeitplaner in einem Prozess):

   ```sh
   paperrust serve --worker
   ```

   Die Startmeldung listet, was ohne Django noch fehlt. Prüfung: `GET /__rs/health` → 200.

7. **Von außen erreichbar machen:** PaperRust lauscht standardmäßig nur auf `127.0.0.1`. Für den
   Zugriff im Netz einen Reverse Proxy mit TLS davorsetzen (nginx, Caddy, Traefik) und
   `PAPERRUST_URL`/`PAPERRUST_ALLOWED_HOSTS` passend setzen. Siehe [Sicherheit](security.md).

Dienst einrichten (Beispiel systemd):

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
# Neustart aus der Oberfläche (Exit-Code 75)
Environment=PAPERRUST_RESTART_SUPERVISED=1
RestartForceExitStatus=75

[Install]
WantedBy=multi-user.target
```

<a id="standalone-limits"></a>

### Was ohne Django fehlt

Fast alles, was paperless-ngx 3.4.2 über Adressen anbietet, beantwortet PaperRust selbst. Es fehlen
nur noch die Django-Verwaltung unter `/admin/` und die „headless“-Schnittstelle von allauth
(`/api/auth/headless/…`, nutzt die Weboberfläche von paperless-ngx nicht). Beide antworten mit 404.
Fehlende Funktionen (nicht Adressen) stehen in den [Release-Notizen](../RELEASE-NOTES-0.1.0.md#grenzen).

<a id="mixed-operation"></a>

## Mischbetrieb vor paperless-ngx (Geschichte)

Bis PaperRust v0.1.0 konnte PaperRust vor einem weiterlaufenden paperless-ngx stehen und
unbekannte Pfade per Reverse Proxy an Django weiterreichen. Das gibt es nicht mehr: Seit der
Schemaänderung `pl_objects.0001_paperrust_tag_adjacency` kann paperless-ngx die Datenbank nach
`paperrust manage migrate` nicht mehr nutzen, und die Weiterleitung an Django ist entfernt
(`PAPERRUST_DJANGO_URL` wird ignoriert, gesetzt nur eine Warnung im Log). Jede Route bedient
PaperRust selbst; Pfade ohne Route antworten 404.

Umstieg von paperless-ngx: **vorher sichern und paperless-ngx ganz anhalten**, dann wie
[ohne Django](#standalone) weiter (bzw. [Docker](#docker)). Redis/Valkey und die Celery-Worker von
paperless-ngx werden danach nicht mehr gebraucht und können weg.

<a id="docker"></a>

## Docker

Ein Container-Image wird aus diesem Repository gebaut (`docker/Dockerfile`); veröffentlicht ist
es **noch nicht** (geplant: `paperrust/paperrust`). Stand: für Linux arm64 gebaut und auf einem
Jetson Orin geprüft und gemessen; amd64 wird gleich gebaut und wurde eigens im Rauchtest geprüft (Start, Upload, Texterkennung, Suche, Export, Konsistenz; 2026-11-20).

```sh
git submodule update --init
docker build -f docker/Dockerfile -t paperrust/paperrust:local .
cd docker && mkdir -p consume export
docker compose -f docker-compose.postgres.yml up -d      # PaperRust + PostgreSQL 17, Port 8000
```

Build-Argumente (`docker build --build-arg NAME=wert`):

| Argument | Standard | Wirkung |
|---|---|---|
| `TESSERACT_LANGS` | `eng deu fra ita spa` (wie upstream) | Tesseract-Sprachpakete im Image |
| `CARGO_BUILD_JOBS` | alle Kerne | weniger Jobs = weniger RAM beim Kompilieren; allein das Linken von `paperrust` braucht über 5 GB RAM |
| `CARGO_RELEASE_LTO`, `CARGO_RELEASE_CODEGEN_UNITS` | aus `Cargo.toml` (`fat`, `1`) | kleine Bau-Rechner: `thin` und `16` |
| `PAPERRUST_UID`, `PAPERRUST_GID` | `1000` | Benutzer im Image |

Bauen lässt sich nur für `linux/amd64` und `linux/arm64` (für andere Plattformen gibt es kein PDFium).

- **Ein Dienst statt fünf:** `paperrust serve --worker` (Webserver, Worker, Zeitplaner,
  Eingangsordner); kein Redis, kein Tika, kein Gotenberg. Office-Dateien (auch alte `.doc`/`.xls`/`.ppt`),
  HEIC-Fotos und Mails liest PaperRust selbst und macht daraus ein Archiv-PDF (mit dem eingebauten
  Werkzeug `paperrust-pdftool`, das im Image liegt). Nicht im Image: `gpg` (verschlüsselte Mails) und
  `pdftoppm` (nur nötig ohne PDFium).
- **Gleiche Pfade wie das paperless-ngx-Image:** Volumes `/usr/src/paperless/{data,media,consume,export}`,
  Port 8000, Befehle wie `document_exporter ../export` im Container. `PAPERLESS_*`-Variablen gelten
  unverändert.
- **Beim Start:** auf PostgreSQL warten, `manage migrate`, Administrator aus
  `PAPERRUST_ADMIN_USER`/`PAPERRUST_ADMIN_PASSWORD`, Suchindex anlegen, falls er fehlt.
- **Benutzer:** UID/GID 1000 ohne root. Mit `user: root` gelten `USERMAP_UID`/`USERMAP_GID` und
  `PAPERRUST_OCR_LANGUAGES` wie im upstream-Image; Sprachen besser beim Bauen festlegen
  (`--build-arg TESSERACT_LANGS="eng deu fra ita spa"`, das ist auch der Standard).
- **Umstieg** einer bestehenden paperless-ngx-Docker-Installation (PostgreSQL): Image tauschen,
  `broker`/`gotenberg`/`tika` und `PAPERLESS_REDIS`/`PAPERLESS_TIKA_*` entfernen, alles andere
  bleibt – Vorlage `docker/docker-compose.migrate-from-paperless.yml`, siehe [unten](#migrate) und
  die [Schritt-für-Schritt-Anleitung](umstieg.md).
- **Schlanke Variante:** `docker/docker-compose.slim.yml` mit `docker-compose.slim.env` setzt nur
  Datenbank, `PAPERRUST_SECRET_KEY` und optional `PAPERRUST_URL`; alles andere stellt ein
  Administrator in der Oberfläche ein (*Einstellungen → System*), siehe
  [Einstellungen in der Oberfläche](configuration.md#settings-ui).
- **Getrennte Prozesse** (Webserver/Worker) mit gemeinsamem Cache in PostgreSQL
  (`PAPERRUST_CACHE=postgres`, kein Redis): `docker/docker-compose.multiprocess.yml`.
- **ARM:** Das Image lässt sich für `linux/amd64` und `linux/arm64` bauen (Raspberry Pi 4/5 mit
  64-bit-System, ARM-NAS, Apple-Silicon-Rechner mit Docker). 32-bit-ARM (`armv7`) wird nicht
  unterstützt (kein PDFium dafür).

<a id="migrate"></a>

### Umstieg einer paperless-ngx-Docker-Installation

In der bestehenden compose-Datei wird nur **das Image getauscht**, und die nicht mehr nötigen Dienste
fallen weg; Datenbank, Volumes und `docker-compose.env` bleiben. Die Vorlage
`docker/docker-compose.migrate-from-paperless.yml` enthält alle Schritte als Kommentar.

1. **Sichern** (Datenbank-Dump und die Volumes `data`/`media`, oder `document_exporter`) und
   paperless-ngx stoppen: `docker compose down` (Volumes bleiben).
2. Die compose-Datei ändern:

   | paperless-ngx | PaperRust |
   |---|---|
   | `image: ghcr.io/paperless-ngx/paperless-ngx:…` | `image: paperrust/paperrust:local` (wie oben gebaut) |
   | Dienste `broker`, `gotenberg`, `tika` | entfernen (auch aus `depends_on`) |
   | `PAPERLESS_REDIS`, `PAPERLESS_TIKA_*` | entfernen (würden ignoriert) |
   | `USERMAP_UID`/`USERMAP_GID`, `PAPERLESS_OCR_LANGUAGES` | wirken mit `user: root` beim Dienst (wie in der Vorlage); Sprachen besser per `--build-arg TESSERACT_LANGS` |
   | `PAPERLESS_WEBSERVER_WORKERS`, `PAPERLESS_ENABLE_FLOWER`, `S6_*` | entfallen |
   | Dienstname `webserver`, Port `8000`, Volumes, `env_file` | **unverändert** |
   | `db`-Image | **gleiche Hauptversion** wie bisher (das Datenverzeichnis ist versionsgebunden) |

3. Prüfen und umsteigen mit `paperrust migrate --from …` wie in der
   [Schritt-für-Schritt-Anleitung](umstieg.md) (zuerst Probelauf mit `--dry-run`), dann
   `docker compose up -d`. Den gleichen `PAPERLESS_SECRET_KEY` behalten, damit Benutzer, Tokens und
   Sitzungen weiter gelten.

**Zurück:** Nach dem Umstieg kann paperless-ngx die Datenbank nicht mehr öffnen. Zurück geht es über
die Sicherung aus Schritt 1 (Datenbank und Volumes zurückspielen, paperless-ngx-Image und `broker`
wieder eintragen) oder über einen Export im Format von paperless-ngx 3.4.2
([Zurück](migration.md#back)).

SQLite/MariaDB: zuerst nach PostgreSQL umziehen ([Umstieg](migration.md#sqlite-mariadb)); das
Werkzeug `paperrust-migrate-db` liegt im Image (`docker compose run --rm webserver paperrust-migrate-db …`).

<a id="nas"></a>

### NAS (Synology, UGREEN, Asustor, Unraid, CasaOS)

Fertige Vorlagen liegen unter `docker/nas/` (Anleitung je Gerät: `docker/nas/README.md`):

| NAS | Vorlage | Oberfläche |
|---|---|---|
| Synology (DSM 7.2+) | `docker/nas/docker-compose.yml` | Container Manager → Projekt → Erstellen |
| UGREEN (UGOS Pro) | `docker/nas/docker-compose.yml` | Docker → Projekt → Erstellen |
| Asustor (ADM) | `docker/nas/docker-compose.yml` | Portainer CE → Stacks (Variable `PAPERRUST_DIR` setzen) |
| Unraid | `docker/nas/unraid/paperrust.xml` + `paperrust-postgres.xml` | Docker → Add Container → Template |
| CasaOS / ZimaOS | `docker/nas/casaos/docker-compose.yml` | App Store → Benutzerdefinierte Installation → Import |

- Zwei Container: PaperRust und PostgreSQL 17. Vor dem ersten Start `PAPERRUST_SECRET_KEY`, das
  Datenbank-Passwort (an beiden Stellen gleich) und `PAPERRUST_ADMIN_PASSWORD` ändern.
- **Dateirechte:** Die Vorlagen starten den Container als root; er gibt die Ordner `PUID:PGID`
  (Benutzer des NAS, z. B. Synology `1026:100`, Unraid `99:100`) und läuft danach als dieser
  Benutzer. `USERMAP_UID`/`USERMAP_GID` wie bei paperless-ngx gehen ebenso (haben Vorrang).
- **Eingangsordner** auf NAS-Freigaben: `PAPERRUST_CONSUMER_POLLING_INTERVAL=10` (in den Vorlagen gesetzt),
  weil Netzfreigaben Änderungen oft nicht melden.
- Das Image ist **noch nicht veröffentlicht**; die Vorlagen zeigen auf den Platzhalter
  `ghcr.io/paperrust/paperrust:latest`.

**Aktualisieren auf dem NAS:** Die Oberfläche zeigt wie paperless-ngx einen Hinweis, wenn es eine
neuere PaperRust-Version gibt (Abfrage der PaperRust-Releases über `/api/remote_version/`,
abschaltbar mit `PAPERRUST_ENABLE_UPDATE_CHECK=false`). PaperRust aktualisiert sich **nicht
selbst**: Das neue Image wird über die Container-Verwaltung des NAS geladen (Synology: Projekt →
Aktion → Erstellen bzw. Image aktualisieren; Unraid: Check for Updates; Portainer: Re-pull image;
CasaOS: App aktualisieren). Fehlende Datenbank-Migrationen spielt PaperRust beim Start selbst ein.
Vorher sichern: `docker exec paperrust document_exporter ../export`.

<a id="desktop"></a>

## Desktop-App

`paperrust-desktop` startet ein **eingebettetes PostgreSQL** und `paperrust serve --worker` als
Kindprozess, nur auf `127.0.0.1`, und öffnet die Oberfläche im Browser. Daten liegen unter
`%LOCALAPPDATA%\PaperRust` (Windows), `~/Library/Application Support/PaperRust` (macOS) bzw.
`~/.local/share/paperrust` (Linux).

Stand: Für **Windows (64 Bit)** gibt es einen Installer (`PaperRust-0.1.0-windows-x64-setup.exe`,
rund 68 MB). Er bringt PostgreSQL 17, Tesseract 5 (Deutsch, Englisch) und PDFium mit; sonst muss
nichts installiert werden. Beim ersten Start wählen Sie: neu anfangen oder
[von paperless-ngx umsteigen](umstieg.md#fall-2-desktop-app-windows). Benutzername und Passwort
brauchen Sie dafür nicht ([Anmeldung in der Desktop-App](#desktop-login)).

> [!WARNING]
> Der Installer ist **nicht digital signiert**. Windows SmartScreen warnt deshalb („Der Computer wurde
> durch Windows geschützt“). Laden Sie ihn nur von der offiziellen Projektseite und vergleichen Sie die
> Prüfsumme (SHA-256), bevor Sie „Trotzdem ausführen“ wählen. Eine Code-Signatur über die SignPath
> Foundation ist geplant.

<a id="desktop-linux"></a>

**Linux (x86_64):** ein portables Paket `PaperRust-0.1.0-linux-x86_64.tar.gz`. Es bringt PostgreSQL 17,
Tesseract 5 (Deutsch, Englisch und Ausrichtungserkennung) und PDFium mit.

- Braucht ein glibc-basiertes Linux mit **glibc 2.36 oder neuer**: Debian 12, Ubuntu 24.04, Fedora 37
  oder neuer.
- Entpacken und `./PaperRust.sh` starten. Sind **WebKitGTK 4.1** und **libayatana-appindicator**
  installiert (Debian/Ubuntu: `libwebkit2gtk-4.1-0`, `libayatana-appindicator3-1`), öffnet PaperRust
  ein eigenes Fenster mit Symbol im Infobereich; sonst öffnet es sich im Browser.
  `./PaperRust.sh --portable` legt die Daten in `./data` neben das Programm (nicht in einen
  Cloud-Ordner), `--console` startet ohne Fenster.
- **Nicht als root starten** – das eingebettete PostgreSQL verweigert den Start als root.
- Das Paket wird mit `apps/desktop/pack/linux-build.sh` in einem Debian-12-Container gebaut und läuft
  deshalb auch auf neueren Distributionen. Bisher nur im Rauchtest geprüft.

**macOS:** geplant. Bauen und Signieren braucht einen Mac und ein Apple-Entwicklerkonto.

Ablauf, Datenablage, Ports, Backup und Import einer bestehenden paperless-ngx-Installation:
[docs/DESKTOP.md](../DESKTOP.md) (Entwicklernotizen).

<a id="desktop-login"></a>

### Anmeldung in der Desktop-App

Auf dem eigenen Computer braucht die Desktop-App **keinen Benutzernamen und kein Passwort**:

- **Neu beginnen** legt automatisch ein Konto für Sie an (Name = Ihr Windows-Benutzername). Ein
  Passwort wird zufällig erzeugt und nirgends angezeigt – Sie brauchen es nicht.
- Die App meldet Sie bei jedem Start **automatisch an**. Auch nach „Abmelden“ oder einer abgelaufenen
  Sitzung meldet das App-Fenster Sie gleich wieder an.
- **Im Browser** (Betrieb mit `paperrust-desktop`): sehen Sie die Anmeldeseite, öffnen Sie PaperRust
  einfach noch einmal über das Startmenü – dann sind Sie wieder angemeldet.
- **Bestehende Installationen und Umstieg:** die App übernimmt den ersten aktiven Administrator (zum
  Beispiel den Benutzer, den Sie früher beim ersten Start angelegt haben) und meldet ihn automatisch an.

**Wer kommt hinein?** Nur Ihr eigenes Windows-Konto auf diesem Computer. Der Schlüssel für die
automatische Anmeldung liegt in Ihrem Datenordner, den nur Ihr Konto lesen darf, und gilt nur für
Zugriffe vom eigenen Computer (`127.0.0.1`). Andere Benutzer desselben Computers und andere Geräte
müssen sich normal anmelden. Der Administrator des Computers kann – wie bei allen Ihren Dateien –
trotzdem zugreifen.

**Zugriff von anderen Geräten** (Einstellungen → „Im lokalen Netzwerk freigeben“): Setzen Sie dafür
im **Profil** (Benutzermenü oben rechts → „Mein Profil“) ein Passwort; das alte Passwort wird dabei nicht
abgefragt. Auf anderen Geräten melden Sie sich dann mit Benutzername und diesem Passwort an
(Zwei-Faktor möglich).

<a id="updating"></a>

## Aktualisieren

1. Sicherung ([Administration](administration.md#backup)).
2. Neues `paperrust`-Binary einsetzen.
3. `paperrust manage migrate` (idempotent; spielt fehlende Migrationen ein – die eigenen von
   PaperRust und, bei einer älteren paperless-ngx-3.x-Datenbank, die fehlenden von paperless-ngx).
   Docker und Desktop-App machen das bei jedem Start selbst.
4. Neu starten.

PaperRust baut auf paperless-ngx v3.4.2 auf. Bekannte spätere Änderungen von paperless-ngx (der neue
Tag-Baum) übernimmt es; eine Datenbank mit unbekannten neueren Änderungen lehnt `migrate` ab („This
database is not at paperless-ngx v3.4.2 …“), ebenso eine 2.x-Datenbank – dafür gibt es
`paperrust migrate --from` ([Umstieg](umstieg.md)). Wie neue upstream-Versionen übernommen werden:
[docs/UPSTREAM-SYNC.md](../UPSTREAM-SYNC.md).
