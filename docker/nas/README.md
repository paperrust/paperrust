# PaperRust auf dem NAS

Vorlagen für Synology, UGREEN, Asustor, Unraid und CasaOS. Das Image gibt es für **x86-64
(amd64)** und **ARM 64 bit (arm64)** – auf ARM-NAS (z. B. Synology „j“/„play“-Modelle mit
Realtek RTD1619B, Asustor Drivestor) läuft dasselbe Image.

> **Noch nicht veröffentlicht.** Alle Vorlagen zeigen auf den Platzhalter
> `ghcr.io/paperrust/paperrust:latest`. Sobald das Image in einer Registry liegt, ändert
> `docker/nas/set-image.sh <registry/org/name> [github-org/repo]` alle Vorlagen auf einmal; in der
> compose-Datei genügt sonst die Zeile `image:` (oder die Variable `PAPERRUST_IMAGE`).

| Datei | Für |
|---|---|
| `docker-compose.yml` | Synology Container Manager, UGREEN (Docker-App), Asustor (Portainer), jede compose-Oberfläche |
| `unraid/paperrust.xml`, `unraid/paperrust-postgres.xml` | Unraid (Docker-Vorlagen / Community Applications) |
| `casaos/docker-compose.yml` | CasaOS (ZimaOS, ZimaBoard, …) – App-Manifest mit `x-casaos` |
| `set-image.sh` | Image-Namen in allen Vorlagen setzen |

Gemeinsam für alle:

- **Zwei Container:** `paperrust` (Weboberfläche, Worker, Zeitplaner, Eingangsordner) und
  PostgreSQL 17. Kein Redis, Tika, Gotenberg.
- **Vor dem ersten Start ändern:** `PAPERRUST_SECRET_KEY` (lange Zufallszeichenkette),
  Datenbank-Passwort (an beiden Stellen gleich), `PAPERRUST_ADMIN_PASSWORD`.
- **Ordner:** `data` (Suchindex, Protokolle), `media` (Dokumente), `consume` (Eingangsordner, z. B.
  als Scan-Ziel des Scanners freigeben), `export` (Sicherung), `pgdata` (Datenbank). Sichern:
  `media` + Datenbank, am einfachsten über den Export (unten).
- **Dateirechte (PUID/PGID):** Der Container startet als root, legt die Ordner an, gibt sie
  `PUID:PGID` und läuft danach als dieser Benutzer (wie `USERMAP_UID/GID` im paperless-ngx-Image;
  `USERMAP_*` hat Vorrang). Werte = Benutzer, dem die Ordner auf dem NAS gehören.
- **Healthcheck** ist im Image eingebaut; die Oberflächen zeigen „healthy“, sobald PaperRust antwortet
  (erster Start: Datenbank anlegen, ca. 10–60 s).
- **Weboberfläche:** `http://<nas-ip>:8000`, Anmeldung mit `PAPERRUST_ADMIN_USER`/`…_PASSWORD`.
- **Eingangsordner auf NAS-Freigaben** melden Änderungen oft nicht: `PAPERRUST_CONSUMER_POLLING_INTERVAL=10`
  (in den Vorlagen gesetzt) fragt alle 10 s ab.
- **Umstieg von paperless-ngx:** dieselbe Datenbank und dieselben Ordner weiterverwenden, denselben
  `PAPERLESS_SECRET_KEY`; Redis/Tika/Gotenberg-Container entfallen. Ablauf: `docs/user/installation.md#docker`
  Abschnitt 3 und `docs/user/migration.md`.

## Synology (Container Manager, DSM 7.2+)

1. Paket **Container Manager** installieren. In File Station den Ordner
   `/volume1/docker/paperrust` anlegen.
2. Container Manager → **Projekt** → **Erstellen**: Name `paperrust`, Pfad
   `/volume1/docker/paperrust`, Quelle „docker-compose.yml hochladen“ → `docker/nas/docker-compose.yml`.
3. Im Editor die drei `ÄNDERN`-Werte setzen. `PUID`/`PGID`: per SSH `id <ihr-benutzer>`
   (typisch `1026` / `100`).
4. Weiter → Webportal nicht nötig → **Fertig**. Die Unterordner entstehen beim ersten Start.
5. Scanner: Freigabe `docker/paperrust/consume` als SMB-Ziel einrichten.

## UGREEN (UGOS Pro, App „Docker“)

1. App **Docker** installieren, Ordner `docker/paperrust` auf einem Volume anlegen.
2. Docker → **Projekt** → **Erstellen**: Pfad auf diesen Ordner, compose-Inhalt aus
   `docker/nas/docker-compose.yml` einfügen, `ÄNDERN`-Werte setzen (UGOS-Benutzer meist
   `PUID=1000`, `PGID=10` oder `100` – in der Systemsteuerung/SSH mit `id` prüfen).
3. **Bereitstellen**. Port 8000 belegt? In `ports:` links ändern, z. B. `"8010:8000"`.

## Asustor (ADM, Portainer CE)

1. App **Portainer CE** aus App Central installieren und öffnen.
2. Ordner `/volume1/Docker/paperrust` anlegen (File Explorer).
3. Portainer → **Stacks** → **Add stack** → Web editor: Inhalt von `docker/nas/docker-compose.yml`.
   Unter **Environment variables** `PAPERRUST_DIR=/volume1/Docker/paperrust` hinzufügen (Portainer
   kennt keine Pfade relativ zur Datei). `ÄNDERN`-Werte setzen, **Deploy the stack**.

## Unraid

1. Beide Vorlagen nach `/boot/config/plugins/dockerMan/templates-user/` kopieren
   (`my-paperrust-postgres.xml`, `my-paperrust.xml`) – oder, sobald veröffentlicht, in
   Community Applications nach „PaperRust“ suchen.
2. Empfohlen: eigenes Docker-Netz, damit die Container sich per Namen finden:
   `docker network create paperrust`; bei beiden Containern **Network Type: Custom: paperrust**.
3. Docker → **Add Container** → Template `paperrust-postgres`: Passwort setzen, Apply.
4. **Add Container** → Template `paperrust`: `DB host` = `paperrust-postgres` (eigenes Netz) oder
   die Unraid-IP (Bridge, dann `DB port` = Port der DB), `DB password`, `Secret key`,
   `Admin password`. PUID/PGID stehen auf `99`/`100` (nobody/users, Unraid-Standard).
5. Apply; WebUI über das Container-Menü.

## CasaOS / ZimaOS

1. App Store → **+** (Benutzerdefinierte Installation) → **Import** → Inhalt von
   `docker/nas/casaos/docker-compose.yml`.
2. `ÄNDERN`-Werte setzen (Datenbank-Passwort an beiden Stellen gleich), **Installieren**.
   Daten liegen unter `/DATA/AppData/paperrust/`; PUID/PGID/TZ setzt CasaOS selbst.

## Aktualisieren

PaperRust prüft in der Oberfläche (wie paperless-ngx), ob es eine neuere Version gibt
(`/api/remote_version/`, fragt die GitHub-Releases von PaperRust ab; abschaltbar mit
`PAPERRUST_ENABLE_UPDATE_CHECK=false`). Der Hinweis führt zur Release-Seite – **installiert wird
über die Container-Verwaltung des NAS**, nicht in PaperRust:

| NAS | Aktualisieren |
|---|---|
| Synology | Container Manager → Projekt `paperrust` → **Aktion → Erstellen** (lädt `:latest` neu) bzw. Image → **Aktualisierung verfügbar** → Aktualisieren |
| UGREEN | Docker → Projekt → **Neu bereitstellen** mit „Image neu laden“ |
| Asustor/Portainer | Stack → **Update the stack** mit „Re-pull image“ |
| Unraid | Docker → **Check for Updates** → **apply update** |
| CasaOS | App → Einstellungen → **Aktualisieren** bzw. „Check then update“ |

Beim Start nach dem Update spielt PaperRust fehlende Datenbank-Migrationen selbst ein. Vorher
sichern: `docker exec paperrust document_exporter ../export` (landet im Ordner `export`).
Wer eine feste Version will, ersetzt `:latest` durch die Versionsnummer (z. B. `:0.2.0`).
