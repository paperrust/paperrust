# PaperRust Desktop – Entscheidungsvorlage und Prototyp

> **Stand für Nutzer (2026-11-20):**
> - **Windows (x64):** NSIS-Installer, noch **nicht signiert** – Windows SmartScreen warnt. Eine
>   Code-Signatur über die SignPath Foundation ist geplant.
> - **Linux (x86_64):** portables `tar.gz`, gebaut in einem Debian-12-Container mit
>   `apps/desktop/pack/linux-build.sh`; bringt Tesseract (deu/eng/osd), PDFium und PostgreSQL 17 mit.
>   Braucht glibc ≥ 2.36 (Debian 12, Ubuntu 24.04, Fedora 37 oder neuer). Eigenes Fenster nur mit
>   WebKitGTK 4.1 und libayatana-appindicator, sonst im Browser. Nicht als root starten.
> - **macOS:** geplant (braucht einen Mac und ein Apple-Entwicklerkonto).
>
> Anleitung für Nutzer: [Installation → Desktop-App](user/installation.md#desktop)
> ([English](user/en/installation.md#desktop)). Der Rest dieses Dokuments sind Entwicklernotizen
> (Entscheidungen, Prüfprotokolle) in zeitlicher Reihenfolge; ältere Abschnitte sind teils überholt
> (z. B. Tesseract unter Linux: inzwischen im Paket enthalten). Genannte Pfade unter `results/` sind
> lokale Ausgabeordner der Bauskripte und nicht Teil des Repositorys.

Neuer Build 2026-11-18d: unsignierter NSIS-Installer `results/desktop/2026-11-18d/PaperRust-0.1.0-windows-x64-setup.exe` (67,6 MiB, Stand `4995876`: neues Design als Standard, Posteingang-Korrektur, Erledigt-Knopf, ASN-Nummernkreise mit Ablageort, Speicherpfad-Baukasten, Scannen/Scanner/Schutz/Freigabelinks im neuen Design, Doku-Sprung). Programme aus dem Bau 2026-11-18c, dort getestet (Rauchtest, Standard-Design, Posteingang, Baukasten, Nummernkreise, Tauri-Fenster, Doku-Anker); Übernahme mit Papierkorb in 2026-11-18b. Ältere Installer-Dateien gelöscht, Berichte bleiben.

Build 2026-11-18: unsignierter NSIS-Installer `results/desktop/2026-11-18/PaperRust-0.1.0-windows-x64-setup.exe` (67,4 MiB, Stand `241409a`: entwirrtes PaperRust-Design, Rollen, keine „paperless“-Texte). Gesamttest aus dem entpackten Installer mit Release-Fenster bestanden: Rauchtest Modus 1, Tauri-Fenster Modus 1 (Erst-Start, OCR, Suche, Sicherung, Neustart), Übernahme einer ngx-Datenbank, Fern-Modus 2a (35 Prüfungen), App-Oberfläche 2b/3 (Sync, offline Liste/Suche/PDF). Befunde danach behoben (nicht im Installer): Papierkorb wurde nach Übernahme sofort geleert, `paperrust-desktop --help/--version` Exit 2. Nicht getestet: echte Installation/Deinstallation (gleiche Produktkennung wie die installierte App), Updater, Tray, ngx-Teil von 2a. `summary.md` dort.

Build 2026-11-17: unsignierter NSIS-Installer `results/desktop/2026-11-17/PaperRust-0.1.0-windows-x64-setup.exe` (67,3 MiB, Stand `9843c74`, mit den Korrekturen aus dem Testprotokoll), Rauchtest bestanden: `--version`/`--help`, Erst-Start nach 12 s, `manage migrate` vor dem Start, Statusseite findet PDFium und Tesseract, OCR-Upload, Oberfläche ohne Konsolen- und CSP-Meldungen, sauberes Beenden (`summary.md` dort). Nicht geprüft: echte Installation/Deinstallation, Tauri-Fenster/Tray/Updater, Signatur, Import und Neustart mit bestehenden Daten.

Build 2026-11-16: unsignierter NSIS-Installer `results/desktop/2026-11-16/PaperRust-0.1.0-windows-x64-setup.exe` (67,3 MiB, Stand `6185fc9`; paperrust mit Standard-Features, paperrust-pdftool, Tauri-Hülle, frontend-light, PostgreSQL 17.11.1, Tesseract 5.4.0 + deu/eng/osd, PDFium 8111), Rauchtest des entpackten Installers mit eigenem Datenordner bestanden (Erst-Start, OCR-Upload, Oberfläche, sauberes Beenden; `summary.md` dort).

Stand 2026-11-09 (D2: Windows-Paket + unsignierter NSIS-Installer gebaut, Tauri-Hülle fertig – Erst-Start mit Import, Sicherung im Betrieb, Fehlerseite, Job-Objekt, Loggrenze –, E2E auf frischem Profil, Abschnitte 19–20; D1: Rauchtest, Tauri-Grundgerüst, Sicherheitsbefunde M4/D-N1–5). Erste Fassung 2026-11-08 (W20). Ziel (Entscheidung des Nutzers, docs/STANDALONE.md): lokale App
**PaperRust** ohne Server-Setup und ohne Docker, mit **eingebettetem PostgreSQL**. Ein Codepfad
mit dem Serverbetrieb: dasselbe `paperrust`-Binary, dasselbe Schema, Umzug Server ↔ Desktop
per Kopie/Export.

## Kurzfassung (Empfehlung)

| Frage | Empfehlung | Minimal-Variante (erster lauffähiger Stand) |
|---|---|---|
| Hülle | **Tauri 2** (Fenster, Tray, Einzelinstanz, Installer, signiertes Auto-Update) – Grundgerüst `apps/desktop-shell/` gebaut und gestartet (Abschnitt 16) | **System-Browser + Konsole** (`paperrust-desktop`, `apps/desktop/`, Rauchtest bestanden) |
| PostgreSQL | **17.x mitgeliefert** (Binaries im Installer), gesteuert über `initdb`/`pg_ctl` – **kein** Download zur Laufzeit | dieselbe Steuerung, Binaries per Hand neben das Programm gelegt |
| Server | `paperrust serve --worker` als **Kindprozess** | gleich |
| Daten | `%LOCALAPPDATA%\PaperRust`, `~/Library/Application Support/PaperRust`, `$XDG_DATA_HOME/paperrust` | gleich |
| Ports | nur `127.0.0.1`, Wunschport aus `desktop.toml`, sonst frei vom OS | gleich |
| OCR/PDF | Tesseract + PDFium **mitgeliefert** (abschaltbar per Paket-Variante) | optional: genutzt, wenn neben dem Programm vorhanden |

Die Supervisor-Logik ist hüllenunabhängig (`paperrust_desktop::supervisor::Supervisor`). Die
Tauri-Hülle ruft dieselbe Bibliothek auf; nichts am Prototyp wird für Tauri weggeworfen.

## 1. Hülle

| | Tauri 2 | wry/tao direkt | System-Browser + Tray |
|---|---|---|---|
| Fenster | WebView2 (Win), WKWebView (macOS), WebKitGTK (Linux) | dieselben WebViews | Browser des Nutzers |
| Tray, Einzelinstanz, Autostart | Plugins (`tray-icon`, `single-instance`, `autostart`) | selbst bauen (`tray-icon`, eigene Sperre) | `tray-icon` + eigene Sperre |
| Installer | eingebaut: MSI/NSIS, `.app`/`.dmg`, AppImage/deb/rpm; „Sidecars“ (`externalBin`) für `paperrust` + PostgreSQL | selbst (WiX, `create-dmg`, appimagetool …) | selbst |
| Auto-Update | Plugin `updater` (Ed25519-signierte Manifeste) | selbst | selbst |
| Download-/Datei-Dialoge, Drag & Drop in die UI | WebView kann Downloads (PDF, Export-ZIP) – über `on_download` lenkbar | selbst | Browser kann alles |
| Abhängigkeitsgröße/Build | groß (tauri, tauri-build, WebView-Bindings; Tauri-CLI fürs Bundle) – **nicht auf diesem 8-GB-Laptop parallel zu Paritätsläufen** | mittel | klein (heute: nur Crates aus dem Lock) |
| Lizenz | MIT/Apache-2.0 | MIT/Apache-2.0 | – |
| Risiko | Linux-WebKitGTK rendert Angular 19 etwas anders als Chromium (testen); WebView2 auf Win10 ggf. nachinstallieren (Bootstrapper im Installer) | wie Tauri, mehr Eigenbau | keine „App“-Anmutung, Browser-Tab kann versehentlich geschlossen werden (Server läuft weiter) |

**Empfehlung: Tauri 2.** Installer + signiertes Update + Tray sind genau das, was sonst
Eigenbau wäre. Zwischenschritt: System-Browser (Prototyp), damit Supervisor, Erst-Start und
Updates unabhängig von der Hülle fertig und getestet sind.

Hinweis WebView: Die Angular-UI nutzt Cookies (Sitzung, CSRF) auf `http://127.0.0.1:<port>`. In
Tauri lädt das Fenster genau diese URL (kein `tauri://`-Schema, keine IPC nötig) – die UI bleibt
unverändert (Leitplanke „kein Fork“). Downloads (`/api/documents/<id>/download/`, Bulk-Download)
müssen im WebView in den Download-Ordner gelenkt werden (Tauri `on_download`).

## 2. Eingebettetes PostgreSQL

| Variante | Lizenz | Bezug | Bewertung |
|---|---|---|---|
| Crate `postgresql_embedded` (theseus-rs) | MIT/Apache-2.0; PostgreSQL selbst PostgreSQL License | Standard: **Download zur Laufzeit** von GitHub (theseus-rs/postgresql-binaries); Feature `bundled`: Archiv beim **Bauen** laden und ins Binary einbetten | Laufzeit-Download verletzt „offline, kein Netzwerkzwang“; `bundled` macht das Desktop-Binary um das ganze Archiv größer und das Entpacken beim ersten Start langsamer. Bringt HTTP/TLS/Archiv-Abhängigkeiten mit. |
| **Mitgelieferte Binaries + eigene Steuerung** (Prototyp) | PostgreSQL License | Archiv beim **Paketbau** (CI) laden, SHA-256 prüfen, unter `pgsql/` ins Installationsverzeichnis | Volle Kontrolle (Port, Locale, Stopp-Modus, Upgrade), keine neue Abhängigkeit, Binaries wie jede andere Programmdatei signierbar (macOS-Notarisierung verlangt das für **jede** ausführbare Datei). |
| System-PostgreSQL voraussetzen | – | Nutzer installiert | widerspricht „ohne Server-Setup“ |

Quellen der Binaries (für den Paketbau):
- **theseus-rs/postgresql-binaries**: aus dem Quellcode gebaut, Linux (gnu/musl), macOS, Windows MSVC, je x86-64 und ARM64, PG 14–18 → eine Quelle für alle drei Plattformen. Vor Übernahme prüfen: ICU enthalten? (`pg_config --configure`), Archivgröße.
- Windows alternativ EDB-ZIP (so läuft die Referenz-DB, `reference/pg/pgsql`): entpackt 145 MB (bin 81 MB, lib 37 MB, share 28 MB) **inkl.** ICU-DLLs; für den Desktop nur die nötigen Teile (Server, `initdb`, `pg_ctl`, `pg_isready`, `createdb`, `psql`, `pg_dump`, `pg_restore`, `pg_upgrade`, DLLs, `share/`; ohne `include/`, `.lib`, pgAdmin, StackBuilder, `symbols`) – geschätzt 50–70 MB entpackt, **nicht gemessen**.

**Version: 17.x** (wie beim Nutzer, PG 17.4 in Docker) – dann geht der Umzug seiner bestehenden
Datenbank per `pg_dump`/`pg_restore` ohne Versionswechsel; der Supervisor prüft `PG_VERSION`
gegen die mitgelieferte Hauptversion und verweigert den Start bei Abweichung mit klarer Meldung.
(Die Referenzumgebung läuft auf 18.6; der Code ist versionsneutral, `SUPPORTED_MAJOR = 17` ist nur
die Paketvorgabe.)

**Empfehlung:** mitgelieferte PG-17-Binaries von **theseus-rs für alle Plattformen** + eigene
Steuerung (fertig im Prototyp). Kein `postgresql_embedded`.

Geprüft 2026-11-09 (nur die `manifest.json` der Releases geladen, ~0,6 MB je Ziel, Prüfsumme gegen
GitHub-`digest`): Release **17.11.1** (REL_17_11, 2026-10-18) enthält auf Windows x64, Linux x64 und
macOS arm64 **ICU** (Windows `icu*78.dll`, Linux `libicu*.so.72`, macOS `libicu*.78.dylib`) und
bündelt OpenSSL/lz4/zstd/libxml selbst (`dependency-notices/` liegt bei). Archivgrößen: Windows 48 MB,
Linux 40 MB, macOS 90 MB (davon `libLLVM.dylib` 124 MB entpackt – nur für JIT, `jit = off` →
weglassen). Entpackt mit Python/Entwicklerdateien: Windows 124 MB, Linux 114 MB, macOS 264 MB;
ausgedünnt geschätzt Windows ~85 MB (ICU-Daten allein 33 MB). Gepinnt in
`apps/desktop/pack/sources.json`.

### Betriebsdetails (umgesetzt in `apps/desktop/src/pg.rs`)

- `initdb -U paperrust --pwfile=<tmp> --auth=scram-sha-256 --encoding=UTF8` + Locale (s. u.). Die
  Passwortdatei liegt nur für die Dauer von `initdb` in `run/` (Unix 0600) und wird sofort gelöscht;
  das Passwort steht nie in einer Kommandozeile (nur `PGPASSWORD` in der Umgebung der Werkzeuge).
- `postgresql.conf` bekommt einmal `include_if_exists = 'paperrust.conf'`; `paperrust.conf` wird bei
  jedem Start neu geschrieben: `listen_addresses='127.0.0.1'`, `port`, `unix_socket_directories=''`,
  `max_connections=40`, `shared_buffers=64MB`, Log nur Warnungen.
- Start `pg_ctl start -w -t 120 -l logs/postgresql.log` (ohne Pipes – der abgekoppelte Server würde
  sie erben), dann `pg_isready`. Läuft noch ein Cluster (Supervisor abgestürzt), wird er erst mit
  `-m fast` gestoppt.
- Stopp `pg_ctl stop -m fast -w`, nur bei Fehlschlag `-m immediate` (Crash-Recovery beim nächsten
  Start, bestätigte Transaktionen bleiben).
- Windows: alle Kinder mit `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` (kein Konsolenfenster,
  Strg+C trifft nur den Supervisor, der die Reihenfolge bestimmt). PostgreSQL verweigert den Start
  als Administrator nicht, weil `pg_ctl` selbst ein eingeschränktes Token benutzt; unter Unix
  verweigert `postgres` den Start als root (Meldung im Log – Desktop-Apps laufen ohnehin nicht als root).
- Geerbte `PG*`, `PAPERLESS_*`, `PAPERRUST_*`, `DJANGO_SUPERUSER_*` werden für alle Kinder entfernt, damit
  eine Umgebung des Nutzers nie auf eine fremde Datenbank zeigt.

**Locale (entschieden 2026-11-09):** Standard für **neue** Installationen ist jetzt
`locale_provider = "icu"` (`--locale-provider=icu --icu-locale=und --locale=C`): „natürliche“
Sortierung `a,A,b,B,c` wie Docker-Postgres mit `en_US.utf8` (gemessen mit den Referenz-Binaries;
`builtin`/`C.UTF-8` sortiert `A,B,a,b,c`). ICU ist in allen theseus-rs-Zielen enthalten (s. o.).
Wirkt nur bei `initdb`; bestehende Cluster (z. B. der Rauchtest-Cluster mit `builtin`) behalten
ihren Provider. `builtin` bleibt per `desktop.toml` wählbar.

## 3. Datenablage

| OS | Verzeichnis |
|---|---|
| Windows | `%LOCALAPPDATA%\PaperRust` |
| macOS | `~/Library/Application Support/PaperRust` |
| Linux/BSD | `$XDG_DATA_HOME/paperrust` (nur absolut), sonst `~/.local/share/paperrust` |
| alle | Override `PAPERRUST_DESKTOP_HOME` (portabel, Tests) |

Bewusst **nicht `%APPDATA%`** (Roaming): bei Domänen-Profilen würde Windows Datenbank und
Dokumente (GB) bei jeder An-/Abmeldung synchronisieren, und ein laufender PostgreSQL-Cluster in
einem synchronisierten Ordner (auch OneDrive/iCloud/Dropbox!) zerstört ihn. Darum gehört
`pgdata` nie in einen Cloud-Ordner; ein späterer Einstellungsdialog darf nur `media/`
verschieben, nie `pgdata/`.

Aufbau: `desktop.toml` (Ports, DB-Passwörter, `SECRET_KEY`, Erst-Start-Fortschritt), `pgdata/`,
`data/` (`PAPERLESS_DATA_DIR`; `data/index/` legt der Supervisor beim ersten Start an und baut ihn mit
`document_index reindex` auf), `media/` (inkl. `documents/{originals,thumbnails}` wie upstream
`init-folders`), `consume/` (Eingangsordner, Tray „Eingangsordner öffnen“), `logs/` (`desktop.log`, `server.log`,
`postgresql.log`, `manage.log` und – als `PAPERLESS_LOGGING_DIR` – die Protokolle
`paperrust.log`/`mail.log` für das Menü „Protokolle“; ältere `paperless.log` bleibt lesbar), `run/` (Sperre,
`instance.toml`, `stop.request`, Arbeitsverzeichnis), `backups/`, `tmp/` (`PAPERLESS_SCRATCH_DIR` –
upstream-Standard wäre `%TEMP%\paperless`, geteilt mit jeder anderen Instanz).

Rechte (M4, Abschnitt 18): Unix alle Verzeichnisse `0700`, private Dateien und Logs `0600`; Windows
einmalig eine geschützte ACL „nur aktueller Benutzer“ auf das Wurzelverzeichnis (vererbt). Liegt
`PAPERRUST_DESKTOP_HOME` außerhalb des eigenen Profils, steht eine Warnung im Log.

## 4. Ports

Standard: nur `127.0.0.1`, niemals `0.0.0.0` – Ausnahme nur mit der ausdrücklich eingeschalteten
Freigabe im lokalen Netzwerk (`lan_sharing`, Abschnitt 21); PostgreSQL bleibt auch dann auf
`127.0.0.1`. Nach einem Absturz des Supervisors belegt der eigene, noch
laufende Cluster den Wunschport – er wird **vor** der Port-Wahl gestoppt (sonst wechselte der DB-Port
nach jedem Absturz; Rauchtest-Befund). Wunschport aus `desktop.toml` (erster Start: DB 54417, App
58080), sonst frei vom OS. „Frei“ = niemand antwortet auf einen Verbindungsversuch **und** Binden
gelingt (fängt den Windows-Fall „anderer Dienst auf 0.0.0.0 ohne exklusive Bindung“ ab). Nie
gewählt: 5432, 8000, 8001, 8080, 8100, 8101, 54329, 63799 (übliche Dienste, Referenz- und
Paritätsumgebung). Der App-Port bleibt stabil, damit Sitzungs-Cookie und Lesezeichen gelten;
wechselt er, wird `desktop.toml` aktualisiert (und `PAPERLESS_URL` passt sich an).

**SMTP-Empfang für Scanner (pl-scan, 2026-11-13):** zweite Ausnahme, ebenfalls nur ausdrücklich.
Ist `PAPERRUST_SMTP_INTAKE` aus (Standard), öffnet die App keinen weiteren Port. Schaltet man den
Empfang in **Einstellungen → System** ein, lauscht der Server nach dem Neustart auf
`PAPERRUST_SMTP_INTAKE_LISTEN` (Standard `0.0.0.0:2525`, damit der Drucker ihn erreicht); Windows
fragt dann nach der Firewall-Freigabe (nur „Private Netzwerke“ erlauben). Die Hülle setzt dafür
nichts – der Wert kommt aus der Oberfläche oder der Umgebung. Absicherung: Geräte-Token, nur
Hochladen, kein Relay, IP-Liste, Grenzen (docs/user/scanner.md, docs/contract/pl-scan.md).

**Sicherheit auf Mehrbenutzer-Rechnern:** andere lokale Benutzer erreichen `127.0.0.1` ebenfalls.
Schutz: DB nur mit zufälligem 32-Zeichen-Passwort (SCRAM), App nur mit Anmeldung (upstream-Login,
MFA möglich). Die automatische Anmeldung des Eigentümers (Abschnitt 25) hängt an einem
Zufallsschlüssel im ACL-geschützten `run/`-Ordner – andere lokale Konten kommen damit nicht hinein.

## 5. Start-Ablauf und Erst-Start

1. Verzeichnisse anlegen, **Einzelinstanz-Sperre** (`run/instance.lock`, Betriebssystem-Dateisperre,
   wird auch nach Absturz frei). Zweite Instanz: liest `run/instance.toml` und öffnet nur die URL.
2. `desktop.toml` laden oder mit frischen Secrets erzeugen; Installation prüfen (fehlende
   Programme werden namentlich gemeldet).
3. PostgreSQL: Port wählen → beim ersten Mal `initdb` → Start → beim ersten Mal `createdb paperless`.
   Fortschritt in `[setup]` (`cluster_initialized`, `database_created`, `superuser_created`), damit
   ein abgebrochener Erst-Start sauber weitermacht. Ein nicht leeres `pgdata` ohne Cluster wird
   nicht überschrieben (Meldung).
3b. **Anwendungsrolle** (D-N3, bei jedem Start, idempotent): Rolle `paperless` (`NOSUPERUSER
   NOCREATEDB NOCREATEROLE`) anlegen bzw. Passwort angleichen, Datenbank + Schema `public` und alle
   Objekte darin an sie übergeben. Server und `manage` verbinden sich nur noch als diese Rolle;
   der Cluster-Superuser `paperrust` bleibt dem Supervisor (initdb, createdb, pg_dump, psql).
4. **`paperrust manage migrate` bei jedem Start** (leere DB → v3.4.2-Schema; bestehende → Prüfung +
   `paperrust_*`-Migrationen; idempotent). Nötig: `paperrust serve` startet seit 2026-11-16 nicht
   auf einer Datenbank mit offenen Migrationen (klare Meldung statt späterer 500er). Die Sicherung
   vor den Datenmodell-Umbaustufen nimmt das mitgelieferte `pgsql/bin/pg_dump` (neben `paperrust`).
5. Kein aktiver Superuser in `auth_user` → **ohne Dialog** lokaler Eigentümer (OS-Benutzername,
   zufälliges Passwort, das nirgends gespeichert wird; bzw. `PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}`)
   → `manage createsuperuser --no-input` mit `DJANGO_SUPERUSER_PASSWORD` in der Umgebung. Danach bei
   jedem Start Eigentümer prüfen bzw. ersten aktiven Superuser übernehmen (`[setup] owner_user`,
   Abschnitt 25).
5b. Fehlt `data/index/`: anlegen + `manage document_index reindex` (upstream Docker
   `init-search-index`; ohne Verzeichnis verwarf pl-indexer jede Index-Aktualisierung).
6. `paperrust serve --worker` mit vollständiger Umgebung (s. u.), Health `GET /__rs/health` → 200
   **von diesem Kind** (Dienstname, künftig Instanz-ID, Windows: Socket-Besitzer per `netstat`; bei
   fremdem Dienst auf dem Port einmal neuer Port), URL nach `run/instance.toml`, Browser/Fenster öffnen.
7. Unerwartetes Ende des Servers → Neustart (bis 3× in 10 min, wachsende Pause), sonst Fehler.
8. Beenden (Strg+C, Konsole schließen, Abmelden, `paperrust-desktop stop` → `run/stop.request`;
   Tauri: Tray „Beenden“):
   Server stoppen (Gnadenfrist 20 s), dann PostgreSQL `-m fast`. PostgreSQL wird auf jedem
   Fehlerpfad nach Schritt 3 gestoppt.

Umgebung des Servers (`server::server_env`, für `serve` zusätzlich `PAPERRUST_DESKTOP_{LOGIN_TOKEN,OWNER}`,
Abschnitt 25): `PAPERLESS_DB{ENGINE,HOST,PORT,NAME,USER,PASS,SSLMODE}`,
`PAPERLESS_{DATA_DIR,MEDIA_ROOT,CONSUMPTION_DIR,SCRATCH_DIR,STATICDIR}`, `PAPERLESS_{SECRET_KEY,URL,ALLOWED_HOSTS,
TIME_ZONE,OCR_LANGUAGE}`, `PAPERRUST_{BIND,ROUTES,LOG,DB_REQUIRE_AUTH}`,
`PAPERRUST_WORKER_NAME=desktop`, `PAPERRUST_STOP_ON_STDIN_EOF=1`, `PAPERRUST_INSTANCE_ID=<zufällig>`,
`NO_COLOR=1` (sonst ANSI-Farbcodes in `server.log`, Rauchtest-Befund), optional `PAPERRUST_{TESSERACT_PATH,TESSDATA_DIR,PDFIUM_PATH}`.
Alles über die Prozessumgebung (nicht `paperless.conf`), weil pl-server/pl-ocr manche Werte nur aus
der Prozessumgebung lesen; Arbeitsverzeichnis `run/`, damit pl-core kein fremdes `../paperless.conf` findet.

### Server: Kindprozess statt in-process

`pl_server::cli::run` ist privat, liest alles aus der Umgebung und hört nur auf Strg+C. In-process
bräuchte eine neue öffentliche Funktion mit Shutdown-Future und würde das ganze pl-server-Geflecht
(alle Fach-Crates, tantivy, Worker) in den Supervisor ziehen – lange Builds, ein Absturz/Panik im
Worker nähme die DB-Steuerung mit. Kindprozess: Absturz-Isolation, Neustart möglich, **dasselbe
Binary wie im Serverbetrieb**, Supervisor baut in < 1 min. Kosten: ein Prozess mehr. Sauberer
Stopp unter Windows über stdin-EOF (pl-server, erledigt) – gemessen: Server endet in < 0,1 s mit
Exit 0; stirbt der Supervisor hart, beendet sich der Server ebenfalls sofort (EOF).

## 6. Updates und Migrationen

- **App-Update:** Tauri-Updater (signiertes Manifest, Ed25519-Schlüssel beim Nutzer/CI). Installer
  ersetzt `paperrust`, Supervisor, Frontend; Daten bleiben. **Umgesetzt (Z4), Abschnitt 23.**
- **Schema:** `manage migrate` bei jedem Start. Vor dem ersten Start einer **neuen App-Version**
  automatisch `pg_dump -Fc` nach `backups/pre-update-<version>-<zeit>/` (nächster Schritt, s. u.),
  dann migrieren.
- **PostgreSQL-Nebenversion** (17.4 → 17.x): einfach neue Binaries, keine Aktion.
- **Hauptversion** (17 → 18, frühestens in einer PaperRust-Hauptversion): `pg_upgrade` braucht alte
  **und** neue Binaries → dieses eine Update liefert beide mit; Supervisor: alten Cluster stoppen,
  `pg_upgrade --link` in neues `pgdata18/`, prüfen, altes erst nach erfolgreichem Start löschen.
  Rückfallweg: Export/Import. Bis dahin verweigert der Start bei abweichender `PG_VERSION` mit Meldung.

## 7. Backup

Inhalt einer Sicherung (`backups/<UTC-Zeit>/`): `paperrust.pgdump` (`pg_dump -Fc`) + `export/`
(`manage document_exporter`: Dateien + `manifest.json`, upstream-kompatibel, also auch in
paperless-ngx importierbar). Drei Wege, derselbe Code (`supervisor::BackupJob`, D2):

| Weg | Ablauf |
|---|---|
| Tray „Sicherung jetzt“ (Tauri) | bei **laufender** App: Kanal `Control` → eigene Aufgabe neben dem Server (pg_dump ist dank MVCC konsistent); Menüeintrag zeigt „Sicherung läuft …“, danach öffnet sich der Ordner, Tooltip meldet Erfolg/Fehler |
| `paperrust-desktop backup` bei laufender App (Skript, Aufgabenplanung) | legt `run/backup.request` an; die Instanz sichert wie über das Tray und schreibt `run/backup.result` (`ok <Ordner>` / `error <Meldung>`); der Befehl wartet darauf |
| `paperrust-desktop backup [ZIEL]` bei geschlossener App | PostgreSQL kurz starten, `migrate`, sichern, stoppen |

Beenden während einer Sicherung wartet bis zu 5 min auf sie, dann Abbruch. Offen: Zeitplan,
Aufbewahrung (z. B. 7 täglich/4 wöchentlich), Ziel auf externem Laufwerk, Wiederherstellen-Assistent.

## 8. Import einer bestehenden paperless-ngx-Installation

| Ausgangslage | Weg | Stand |
|---|---|---|
| beliebige Version, Export möglich | in paperless-ngx `document_exporter` → Erst-Start „paperless-ngx-Export importieren“ (Ordner oder ZIP) | **umgesetzt (D2)**: `migrate` → `manage document_importer <Ordner/ZIP>`; danach kein Admin-Dialog, wenn der Export einen aktiven Superuser enthält |
| SQLite / MariaDB / MySQL / PostgreSQL von **v3.0.0 bis v3.4.2** | Erst-Start „Bestehende Datenbank übernehmen“: Quell-URL (+ optional alter `MEDIA_ROOT`) | **umgesetzt (D2)**: `paperrust-migrate-db` (liegt im Paket) **vor** `migrate` in die leere DB, als Anwendungsrolle, URLs nur über `PAPERRUST_MIGRATE_SOURCE/TARGET` (nicht in der Prozessliste), Ausgabe `logs/migrate-db.log` + Bericht `logs/migrate-db-report.json`; Medienordner wird ohne Überschreiben kopiert; Index wird neu aufgebaut |
| PostgreSQL älterer Version (Fall des Nutzers: 3.1.3, PG 17.4, Docker) | wie oben (postgres://…) – kein Zwischenschritt über 3.4.2 | **umgesetzt (2026-11-10)**: fehlende Migrationen ab 3.0.0 werden beim Kopieren nachgeholt (`pl-dbmigrate/src/catchup.rs`, docs/MIGRATION.md#bestand); 2.x weiter nur über Export |

Erst-Start-Wahl (Tauri-Formular, Konsole: Menü 1/2/3, ohne Dialog: `PAPERRUST_DESKTOP_IMPORT=<Ordner|ZIP>`
bzw. `PAPERRUST_DESKTOP_IMPORT_DB=<URL>` + `PAPERRUST_DESKTOP_IMPORT_MEDIA`) erscheint nur, solange
die Datenbank weder Benutzer (außer `AnonymousUser`) noch Dokumente hat – ein fehlgeschlagener
Import kann also wiederholt werden; für den DB-Umzug wird ein schon migriertes, aber leeres Schema
neu angelegt. **Nicht E2E-getestet**: Import/Umzug mit echten Daten (nur Prüflogik + Unit-Tests).

**Umstieg mit einem Klick (Z1):** `supervisor::detect_setup(<Pfad|URL>, media)` erkennt aus einer
Eingabe den Weg (Export-Ordner/ZIP → Import; paperless-Ordner, `data`-Ordner oder `db.sqlite3` →
DB-Umzug mit dem Medienordner daneben; DB-URL → DB-Umzug). Genutzt von `PAPERRUST_DESKTOP_IMPORT`
(nimmt jetzt jede Quellart) und Konsolen-Menü 2. Nach dem ersten erfolgreichen Start läuft im
Hintergrund `paperrust-migrate-db verify` (Quelle nur lesend, Stichprobe über die API des eben
gestarteten Servers mit vorübergehendem Token) → `logs/umstieg-bericht.{html,json,txt}`, Urteil im
`desktop.log`. **Offen (Hülle):** das Tauri-Formular (`apps/desktop-shell`) hat noch zwei getrennte
Felder statt eines Eingabefelds mit `detect_setup`, und es zeigt den Bericht noch nicht an (Link
„Prüfbericht öffnen“ auf `logs/umstieg-bericht.html` wäre der einfache Weg). E2E der
Erkennung/Prüfung mit Testdaten: `tests/dbmigrate/run_switch.sh`.

## 9. Tesseract und PDFium

pl-ocr ruft heute das **`tesseract`-Programm** (TSV/TXT, OSD) und lädt **PDFium** per `libloading`
(`PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`, `PAPERRUST_PDFIUM_PATH`). Ohne beide kann
PaperRust Dokumente mit Textebene und Office/Mail-Text verarbeiten, aber keine Scans.

Empfehlung: **mitliefern** (ein Dokumentenarchiv ohne OCR ist für Scans wertlos), Sprachpakete
`deu`+`eng` (tessdata_fast, ~1–4 MB je Sprache; `best` größer) im Paket, weitere nachladbar oder
als Paket-Variante. Bezug: Windows UB-Mannheim-Build bzw. selbst gebaut, macOS/Linux eigener
Build oder Systempaket (Linux: Abhängigkeit im deb/rpm statt mitliefern), PDFium:
bblanchon/pdfium-binaries (alle Plattformen). Layout siehe `apps/desktop/src/layout.rs`
(`tesseract/`, `pdfium/` neben dem Programm, beides optional erkannt).

**OCR-Sprachen (Stand Z3).** Im Paket: `deu`, `eng` (Standard `ocr_language = "deu+eng"`,
`apps/desktop/src/config.rs` `default_ocr`) und `osd` (Seitendrehung, `-l osd --psm 0` in
`pl-ocr`); alle aus tessdata_fast 4.1.0, gepinnt in `sources.json`. Eine weitere Sprache, z. B.
Französisch:

1. `fra.traineddata` aus <https://github.com/tesseract-ocr/tessdata_fast> (gleiche Variante wie
   die mitgelieferten) nach `<Programmordner>\tesseract\tessdata\` kopieren
   (Installer: `%LOCALAPPDATA%\PaperRust Desktop\tesseract\tessdata\`).
2. Sprache einstellen: `desktop.toml` → `[server] ocr_language = "deu+eng+fra"` (wirkt beim
   nächsten Start) oder in der Oberfläche (Einstellungen → Konfiguration → OCR-Sprache).
   Fehlt eine eingestellte Sprache, schlägt das Einlesen mit „MissingDependencyError: OCR engine
   does not have language data …“ fehl (`pl-ocr` `check_languages`, wie OCRmyPDF).
3. Neuinstallation/Deinstallation ersetzt den Programmordner. Dauerhaft: eigenen Ordner mit
   **allen** gewünschten `*.traineddata` (inkl. `osd`, `deu`, `eng`) anlegen und
   `PAPERRUST_TESSDATA_DIR=<Ordner>` als Benutzer-Umgebungsvariable setzen (`layout.rs`).

**Schrift für Text-Vorschaubilder (Z3).** Upstream nimmt `PAPERLESS_THUMBNAIL_FONT_NAME`
(Standard `/usr/share/fonts/liberation/LiberationSerif-Regular.ttf` – gibt es unter Windows nicht →
Log „Thumbnail font not available“, Textdateien bekamen nur das Standardbild). Das Paket enthält
`fonts\LiberationSerif-Regular.ttf` (Liberation 2.1.5, SIL OFL 1.1, Lizenz in
`THIRD-PARTY-LICENSES\liberation-fonts\`, Prüfsumme gepinnt und mit dem Homebrew-Cask abgeglichen);
`layout.rs` erkennt sie, der Supervisor setzt `PAPERLESS_THUMBNAIL_FONT_NAME` auf den Pfad
(`server.rs`). Eigene Schrift: Umgebungsvariable `PAPERLESS_THUMBNAIL_FONT_NAME` für die App
setzen (hat Vorrang vor der mitgelieferten).

## 10. Lizenzen

| Teil | Lizenz | Pflicht beim Weitergeben |
|---|---|---|
| PaperRust (inkl. Desktop) | GPL-3.0-only | Quellcode anbieten, Lizenztext, Hinweis „based on paperless-ngx“ |
| Angular-Frontend (upstream) | GPL-3.0 | wie oben |
| PostgreSQL | PostgreSQL License (BSD-artig) | Copyright-Hinweis mitliefern; GPL-verträglich |
| Tesseract, tessdata | Apache-2.0 | NOTICE/Lizenztext; mit GPL-3.0 verträglich (nicht mit GPL-2.0-only) |
| Leptonica (Tesseract-Abhängigkeit) | BSD-2-artig | Hinweis |
| PDFium | BSD-3-Clause, enthält Drittkomponenten (FreeType FTL/GPL-2 dual, libjpeg-turbo, zlib, libpng, ICU, OpenJPEG, lcms) | alle Hinweise sammeln (pdfium-binaries liefert `LICENSE`-Sammlung) |
| Tauri, wry, tao | MIT/Apache-2.0 | Hinweise |
| EDB-Windows-Binaries (falls genutzt) | PostgreSQL License + Drittlizenzen (OpenSSL Apache-2.0, ICU, libxml2, zstd, lz4 …) | `server_license.txt`/Drittlizenzen beilegen |

Umsetzung (D2): `pack.ps1` legt `LICENSE` + `NOTICE` (GPL-3.0), `THIRD-PARTY-LICENSES.txt` (Übersicht:
Teil, Version, Lizenz, Quelle, Ort der Texte) und `THIRD-PARTY-LICENSES\` ins Paket: PostgreSQL
(`LICENSE` + `dependency-notices\vcpkg\{icu,openssl,libxml2,libiconv,lz4,zlib,zstd}`), PDFium
(`LICENSE` + `licenses\` mit FreeType, ICU, libjpeg-turbo, libpng, zlib, OpenJPEG, lcms2, abseil …),
Tesseract (Apache-2.0 + Liste der mitgelieferten MSYS2-DLLs mit Lizenzen), paperless-ngx (GPL-3.0),
Rust-Crates beider Workspaces (`rust-crates.txt`: Name, Version, Lizenzausdruck aus `cargo metadata`,
525 Crates) + Standardtexte `common\{Apache-2.0,MIT}.txt`. **Offen vor einer Veröffentlichung:**
Copyright-Zeilen je MIT/BSD-Crate vollständig (`cargo about generate`, Werkzeug nicht installiert),
Lizenztexte der MSYS2-DLLs einzeln beilegen (UB Mannheim liefert keine mit), Markenrichtlinie „Rust“
(docs/BRANDING.md). Code-Signatur/Notarisierung: Abschnitt 19.

## 11. Code

### `apps/desktop/` (Crate `paperrust-desktop`, Mitglied des Root-Workspace)

| Datei | Inhalt |
|---|---|
| `src/paths.rs` | Datenablage je OS, Override, Verzeichnisse (inkl. `tmp/`, `media/documents/…`), `stop.request`, `backup.request`/`backup.result`, `index_dir` |
| `src/perms.rs` | M4: Unix 0700/0600, Windows-ACL „nur aktueller Benutzer“ (`whoami` + `icacls`), Warnung bei fremdem Ort |
| `src/config.rs` | `desktop.toml` (Secrets, Ports, Anwendungsrolle, Erst-Start-Fortschritt), atomar, privat, `Debug` ohne Secrets |
| `src/ports.rs` | Port-Wahl auf 127.0.0.1 mit Sperrliste |
| `src/lock.rs` | Einzelinstanz-Sperre, `instance.toml` (privat, URL streng geprüft), Stopp-Wunsch von außen |
| `src/logs.rs` | Rotation beim Start **und Größengrenze im Betrieb** (`RotatingFile`, 10 MiB × 6, `PAPERRUST_DESKTOP_LOG_MAX_MB`), `desktop.log` (+stderr), Server-Ausgabe über Pipe (`pump`) |
| `src/job.rs` | **Windows-Job-Objekt** (`KILL_ON_JOB_CLOSE`, `BREAKAWAY_OK`, FFI auf kernel32 ohne neue Abhängigkeit), `open_external` (Browser/Explorer außerhalb des Jobs) |
| `src/layout.rs` | Installationslayout + Overrides (`PAPERRUST_DESKTOP_{SERVER_BIN,PG_BIN,ROUTES,STATIC,MIGRATE_DB_BIN}`, `PAPERRUST_{TESSERACT_PATH,TESSDATA_DIR,PDFIUM_PATH}`) |
| `src/pg.rs` | initdb/start/stop/status/pg_isready/createdb/psql/pg_dump, Anwendungsrolle, `PGREQUIREAUTH`, Versionsprüfung, Log-Rotation vor dem Start |
| `src/server.rs` | Umgebung (`PAPERRUST_*`, `PAPERRUST_DB_REQUIRE_AUTH=scram-sha-256`), `manage …`, Kindprozess, Health mit **Instanz-ID** (scharf), sauberer Stopp, `Debug` geschwärzt |
| `src/supervisor.rs` | Ablauf (Abschnitt 5), **Erst-Start-Wahl** (neu / Export importieren / DB übernehmen, `SetupChoice`, `setup_from_env`), Index-Aufbau, Port-Neuversuch, **Sicherung im Betrieb** (`Control`, `BackupJob`, `request_live_backup`), `manage`, Ereignisse für die Hülle |
| `src/lib.rs` | `DesktopError` + `log_file()` (welches Log) + `advice()` (Hinweis für die Fehlerseite) |
| `src/main.rs` | Konsolen-Hülle: `run [--no-browser]`, `backup [ZIEL]` (bei laufender App über die Instanz), `manage …`, `stop`, `paths`; Erst-Start-Menü 1/2/3 |
| `pack/pack.ps1`, `pack/sources.json`, `pack/installer.ps1`, `pack/licenses/` | Paketbau Windows + NSIS-Installer (Abschnitte 17, 19) |
| `e2e/desktop-e2e.mjs` | Ende-zu-Ende-Test des fertigen Pakets über WebView2-CDP (Abschnitt 20) |
| `e2e/settings-smoke.mjs` | Rauchtest `desktop.*` über API v2 → Supervisor-Neustart auf neuem Port (Abschnitt 24) |

Abhängigkeiten unverändert (nur Crates aus dem Root-Lock; Job-Objekt per eigener FFI, weil
`windows-sys` im Workspace das Feature `Win32_System_JobObjects` nicht hat). Tests:
`cargo test -p paperrust-desktop -j 1` → **44 Tests grün** (43 Bibliothek + 1 CLI; neu u. a.
`job::job_kills_member_when_closed`, `logs::rotating_file_respects_limit`, `supervisor::setup_validation`,
`setup_from_environment`, `copies_tree_without_overwriting`, `control_without_channel_never_fires`),
`cargo clippy -p paperrust-desktop -j 1 --all-targets -- -D warnings` sauber.

### `apps/desktop-shell/` (Crate `paperrust-desktop-shell`, Binary `paperrust-app`) – Abschnitt 16

Eigener Workspace (leere `[workspace]`-Tabelle, eigenes `Cargo.lock`), damit Tauris ~460 Pakete
weder das Root-Lock noch die Builds der anderen Agenten berühren. Tests: 5 grün, clippy sauber.

Bekannte Grenzen:
- Konsolen-Hülle: Passwort im Konsolen-Dialog sichtbar (die Tauri-Hülle hat ein Formular). Strg+C
  oder `stop` **während** des Konsolen-Dialogs wird erst danach bemerkt.
- Job-Objekt: stirbt die App hart, beendet Windows PostgreSQL sofort (wie `-m immediate`); der
  nächste Start macht Crash-Recovery aus dem WAL (bestätigte Transaktionen bleiben). Unix: kein
  Gegenstück nötig (Server endet über stdin-EOF, Waisen-Cluster wird beim nächsten Start gestoppt).
- `postgresql.log`/`pg_ctl.log`/`manage.log` rotieren nur vor dem jeweiligen Start bzw. Lauf
  (PostgreSQL hält sein Log offen; es loggt nur Warnungen).
- Datei-/Ordnerauswahl im Erst-Start-Formular: Pfad einfügen (kein nativer Dialog – bräuchte
  `tauri-plugin-dialog`, neue Abhängigkeit).

## 12. CORE-REQUESTs

Erledigt (von pl-server/Koordinator umgesetzt, im Rauchtest bzw. E2E geprüft):
1. ~~pl-server: Stopp über stdin-EOF~~ (`PAPERRUST_STOP_ON_STDIN_EOF=1`) – Server endet in < 0,1 s.
2. ~~pl-server: kein Django~~ (früher `PAPERRUST_DJANGO_URL=off`, vom Desktop gesetzt). Seit 2026-11-14
   gibt es die Weiterleitung nicht mehr; der Desktop setzt weder `PAPERRUST_DJANGO_URL` noch
   `PAPERRUST_QUEUE` noch `PAPERLESS_CACHE_BACKEND` (Test `env_is_standalone_and_loopback`).
3. ~~Frontend-Auslieferung in Rust~~ (`ui.index`, `static.files`) – UI, Login, `main.js` (3 MB) aus `STATICDIR`.
4. ~~Workspace-Mitglied `apps/desktop`~~ – steht im Root-`Cargo.toml`.
6. ~~Instanz-ID im Health-Check~~ (R1, d847e3b): `/__rs/health` gibt `instance` aus; der Desktop
   akzeptiert seit D2 **nur noch** eine Antwort mit seiner Instanz-ID (D-N5 scharf, Test
   `server::health_must_be_ours`). Ältere Server-Binaries ohne das Feld gelten als fremd.
7. ~~`require_auth` für die Server-DB-Verbindung~~ (R1): Desktop setzt `PAPERRUST_DB_REQUIRE_AUTH=scram-sha-256`.

Umbenennung (R1): der Desktop setzt nur noch `PAPERRUST_*` (`PAPERRUST_BIND`,
`PAPERRUST_ROUTES`, `PAPERRUST_LOG`), keine `PAPERLESS_RS_*` mehr (Test `env_is_standalone_and_loopback`).

Offen:

5. **(optional) Tauri-Hülle in den Root-Workspace** – erst, wenn die Hülle aus dem Root-Workspace
   gebaut werden soll (der Paketbau kommt ohne aus):

```diff
 [workspace]
 resolver = "2"
-members = ["crates/*", "apps/desktop"]
+members = ["crates/*", "apps/desktop", "apps/desktop-shell"]
```
   plus in `apps/desktop-shell/Cargo.toml` die leere `[workspace]`-Tabelle entfernen und als
   `[workspace.dependencies]` aufnehmen (exakt gepinnt, Stand 2026-11-09, MSRV 1.90):
   `tauri = { version = "=2.12.5", features = ["tray-icon", "image-png"] }`,
   `tauri-build = "=2.7.4"`, `tauri-plugin-single-instance = "=2.5.5"`. Folge: ~150 neue Pakete im
   Root-`Cargo.lock`, `cargo deny` prüfen (Lizenzen MIT/Apache-2.0; `webview2-com-sys` enthält
   `WebView2Loader`-Header von Microsoft, BSD-3).
8. **(niedrig) `.gitignore`**: `results/desktop/` und `results/pack/` (Paket, ZIP, Installer,
   Download-Cache, zusammen ~1 GB) stehen nicht im Repo-`.gitignore`. Bis dahin legt `pack.ps1` in
   beiden Ordnern eine eigene `.gitignore` mit `*` an. Wunsch: `/results/desktop/` und `/results/pack/`
   in die Root-`.gitignore`.

## 13. Befehle

```powershell
# nur bei > 2,5 GB freiem RAM (Release-Builds: > 3,5 GB); gemeinsames Target-Verzeichnis
$env:CARGO_TARGET_DIR="<repo>/target"
cargo test   -p paperrust-desktop -j 1
cargo clippy -p paperrust-desktop -j 1 --all-targets -- -D warnings
cd apps/desktop-shell; cargo clippy -j 1 --all-targets -- -D warnings; cargo test -j 1; cd ../..

# Release-Programme (D2 gemessen, -j 1: paperrust 19 min, paperrust-desktop 2 min,
# paperrust-migrate-db ~5 min, paperrust-app 11 min beim ersten Mal)
cargo build --release -p pl-server --bin paperrust -j 1
cargo build --release -p paperrust-desktop -j 1
cargo build --release -p pl-dbmigrate --bin paperrust-migrate-db -j 1
cd apps/desktop-shell; cargo build --release -j 1; cd ../..

# Paket + ZIP + unsignierter NSIS-Installer -> results\desktop\
.\apps\desktop\pack\pack.ps1 -Zip -Installer

# E2E auf frischem Profil (Abschnitt 20)
node apps/desktop/e2e/desktop-e2e.mjs <Installationsordner> <neuer Datenordner> results/e2e/desktop

# Betrieb (aus dem Paket)
paperrust-desktop.exe paths | run [--no-browser] | stop | backup [ZIEL] | manage <befehl>
$env:PAPERRUST_DESKTOP_HOME = "<Ordner>"   # anderer Datenordner (portabel, Tests)
```

## 14. Nächste Schritte (Aufwand grob)

| # | Schritt | Aufwand |
|---|---|---|
| 1 | **Signatur + Updater** (Konten/Schlüssel des Nutzers, Abschnitt 19): Authenticode-Zertifikat, `bundle.windows.signCommand`, Updater-Schlüssel + `latest.json`-Ablage; Tauri-Updater-Plugin einbauen | 1 Tag + Wartezeit Zertifikat |
| 2 | Import/Umzug mit echten Daten durchspielen (Export des Nutzers, SQLite/MariaDB-Testdatenbank) – heute nur Prüflogik, Unit-Tests und ein Import-Lauf mit dem eigenen Backup (Abschnitt 20) | 0,5 Tag |
| 3 | `pack.sh` für macOS/Linux (theseus-rs + pdfium gepinnt; Tesseract dort aus dem System bzw. eigener Build; `.app`/`.dmg` + AppImage/deb über denselben Tauri-Bundler) + CI-Läufe | 2 Tage |
| 4 | Auto-Backup vor Versionswechsel, Backup-Zeitplan + Aufbewahrung, Wiederherstellen (`pg_restore` + Dateien) | 1–2 Tage |
| 5 | Paketgröße: Oberfläche 23 MB (seit F1: zusammengefasste Ablage, vorher 317 MB) (37 Sprachen × ~8,5 MB, `assets/` je Sprache doppelt) – Angular-Build mit gemeinsamem `assets/` (frontend, R2) würde ~250 MB sparen; ZIP 199 MB, Installer 59 MB (LZMA solid fasst die Doppelungen zusammen) | 0,5 Tag (R2) |
| 6 | Token-Bindung der App an das Fenster (andere lokale Benutzer), Autostart-Option, Download-Ziel-Dialog, nativer Ordner-Dialog im Erst-Start (`tauri-plugin-dialog`) | 1 Tag |
| 7 | Copyright-Vermerke je Crate (`cargo about`), Lizenztexte der Tesseract-DLLs einzeln | 0,5 Tag |

## 15. Rauchtest-Protokoll (2026-11-09, Windows 11, PG 18.6 Referenz-Binaries)

Eigener Cluster `%TEMP%\paperrust-desktop-smoke` (bleibt stehen), DB-Port 54417 → nach dem ersten
Absturztest 63823 (Befund 3, behoben), App-Port 58080. Server-Binary `results/bin/paperrust-d1.exe`
(Stand vor der Umbenennung durch R1), Statics aus `reference/data/static` (`frontend/dist` von R2
gab es noch nicht).

| # | Schritt | Ergebnis |
|---|---|---|
| 1 | `paths` | ok; zeigt jetzt auch routes/static/Tesseract/PDFium |
| 2 | `run --no-browser`, Erst-Start | initdb 7,3 s, migrate 6,6 s, Superuser per Umgebung, Server-Start → Health 200 in 0,5 s; gesamt 15 s |
| 3 | `/` ohne Sitzung | 302 → `/accounts/login/?next=/` |
| 4 | Login per curl (GET Formular, CSRF, POST) | 302 → `/dashboard`; `/` 200 (Angular-Index), `main.js` 3,1 MB 200, `/api/ui_settings/` mit `app_title=PaperRust` |
| 5 | Upload `simple-digital.pdf` | Aufgabe `success` in 0,19 s, Inhalt „This is a test document.“, download/thumb/preview 200 |
| 6 | Suche | **Befund 1**: Index-Verzeichnis fehlte → nichts indexiert. Behoben; nach Neustart `reindex`, Suche findet Dokument, neue Uploads sofort |
| 7 | `multi-page-images.pdf` (Scan) | `failure: no PDF renderer available: PDFium not loaded` – erwartet ohne Tesseract/PDFium im Layout |
| 8 | Absturz: Supervisor `taskkill /F` | Server endet sofort per stdin-EOF (Exit 0), PostgreSQL läuft weiter (gewollt) |
| 9 | Start nach Absturz | Cluster wird gestoppt und neu gestartet; **Befund 3**: DB-Port wechselte. Behoben → zweiter Absturztest: Port bleibt 63823 |
| 10 | Zweiter Start (kein initdb) | migrate 0,1 s, Start bis „bereit“ 1,8 s; Sitzungs-Cookie bleibt gültig |
| 11 | Zweite Instanz | `PaperRust läuft bereits (http://127.0.0.1:58080/)`, Exit 1 (mit Browser: öffnet nur die URL) |
| 12 | Server-Absturz (`taskkill /F` auf `paperrust`) | Supervisor startet ihn nach 2 s neu, Health 200 |
| 13 | Sauberes Beenden `stop` | Server Exit 0, dann `pg_ctl stop -m fast`; gesamt 0,8 s; keine Prozesse übrig |
| 14 | `backup` | 3,6 s: `paperrust.pgdump` (82 Tabellen-Daten-Einträge, `pg_restore --list` ok) + `export/` mit `manifest.json` (2 Dokumente, 1 Benutzer …) und Dateien |
| 15 | Sicherheits-Fixes auf bestehendem Cluster | ACL gesetzt (nur `DESKTOP-…\jane:(OI)(CI)(F)`, `desktop.toml`/`pgdata` erben), PostgreSQL läuft weiter; Rolle `paperless` angelegt, 509 Objekte umgehängt, DB-Eigentümer `paperless`, Server-Verbindungen nur als `paperless`; Login, Liste, Upload `with-form.pdf` ok |
| 16 | Frischer Erst-Start mit Anwendungsrolle (`…-smoke2`, danach gelöscht) | migrate als Nicht-Superuser 1,3 s, Login ok; `run/initdb.pw` danach weg; ACL gesetzt |
| 17 | Tauri-Hülle gegen den Rauchtest-Cluster | Fenster zeigt nach 1,9 s die PaperRust-Anmeldung (Screenshot geprüft), zweiter Start endet sofort (Plugin), `stop` beendet Server + PostgreSQL + Hülle |
| 18 | Tauri-Erst-Start ohne Admin-Variablen (`…-smoke3`) | Formular „Willkommen“ erscheint (Screenshot); **Befund 6**: `stop` während des Formulars wurde nicht bemerkt (Supervisor-Thread wartet im Dialog). Behoben (eigene Aufgabe für den Stopp-Wunsch); Nachtest: `stop` beantwortet das Formular, PostgreSQL gestoppt, Hülle beendet, keine Prozesse übrig |
| 19 | Frischer Erst-Start mit neuem Standard `icu` (`…-smoke2`, danach gelöscht) | `datlocprovider = i`, `und`; 247 Migrationen als Anwendungsrolle; bereit nach 10 s |

Befunde und Fixes:
1. **Index fehlte** (`index directory does not exist`) → `data/index` anlegen + `document_index reindex`, wenn er fehlt.
2. **ANSI-Farbcodes in `paperrust.log`** → `NO_COLOR=1` für Server/`manage`.
3. **DB-Port wechselte nach jedem Absturz** (eigener Waisen-Cluster belegte ihn) → Waise vor der Port-Wahl stoppen.
4. **Kein Stopp ohne Konsole** (Skripte, Installer/Updater, Hintergrundlauf) → `paperrust-desktop stop` + `run/stop.request`.
5. **`paths`/`stop`/zweiter Start rotierten das `desktop.log` der laufenden Instanz** → Log erst nach der Sperre öffnen.
6. **Tauri: Stopp während des Erst-Start-Formulars hing** → Stopp-Wunsch als eigene Aufgabe, beantwortet das Formular.
7. Kleinere: doppeltes `Stopping`-Ereignis; eigener `PAPERLESS_SCRATCH_DIR` statt `%TEMP%\paperless`; `media/documents/{originals,thumbnails}` wie upstream angelegt; `jit = off`.

## 16. Tauri-2-Hülle (`apps/desktop-shell/`)

Stand D2: **fertig für ein erstes Paket** (Windows, Release, WebView2). Release-Binary 11 MB
(`lto = "thin"`, `strip`), erster Release-Build mit `-j 1` 11 min, danach ~3–4 min. Tests: 5
Unit-Tests (Navigation, Stopp beantwortet Formular, Zustand/Zurücksetzen, Formularfelder → Wahl,
Fehler → Log + Hinweis), clippy sauber.

- **Fenster** `main` (1280×860, min. 480×600): zuerst `ui/index.html` (Splash mit Fortschritt aus den
  Supervisor-Ereignissen), bei Bedarf das **Erst-Start-Formular** mit drei Wegen (neu beginnen /
  paperless-ngx-Export importieren / bestehende Datenbank übernehmen, Abschnitt 8; Prüfung
  `validate_setup` vor dem Absenden, Fehlertext im Formular; „neu beginnen“ seit 2026-11-19 ohne
  Konto-Felder, Abschnitt 25), dann Wechsel auf `http://127.0.0.1:<port>/rs/desktop/login?token=…`
  (automatische Anmeldung). Zustand per IPC `shell_state` (Polling 300 ms).
- **Fehlerseite** bei Startproblemen: Meldung, Hinweis „was tun“ (`DesktopError::advice`: Port belegt,
  Datenbank startet nicht, Datenbank-Version passt nicht, Migration/Import fehlgeschlagen …), Link
  „Log öffnen (postgresql.log | server.log | manage.log | migrate-db.log | desktop.log)“ – nur
  diese Namen, kein freier Pfad aus der Seite –, „Log-Ordner“, **„Erneut versuchen“** (startet den
  Supervisor neu) und „Beenden“. Das Fenster wird dafür auf die lokale Seite zurücknavigiert (nicht
  neu erzeugt – Befund E2E, Abschnitt 20).
- **Tray:** „PaperRust öffnen“, **„Sicherung jetzt“** (aktiv, sobald der Server läuft; während der
  Sicherung „Sicherung läuft …“, danach öffnet sich der Ordner), „Eingangsordner öffnen“,
  „Sicherungen öffnen“, „Logs öffnen“, „Beenden“; Tooltip zeigt Status und Sicherungsergebnis.
- **Job-Objekt** beim Start (`paperrust_desktop::job`): PostgreSQL, Server, `manage` und WebView2
  sterben mit der App; Browser/Explorer werden mit `CREATE_BREAKAWAY_FROM_JOB` gestartet.
- **Drag & Drop:** `disable_drag_drop_handler()` – sonst fängt Tauri unter Windows alle Drops ab und
  das Hochladen per Ziehen in die PaperRust-Oberfläche funktioniert nicht.
- **Sicherheit:** IPC-Capability nur für die lokalen Seiten (keine `remote`-Einträge) → die
  PaperRust-Oberfläche hat keinen Zugriff auf Tauri-Befehle. Navigation nur zu den eigenen Seiten und
  genau `http://127.0.0.1:<App-Port>`; alles andere öffnet der System-Browser. CSP ohne `unsafe-inline`.
- **Einzelinstanz, Beenden, Downloads, Stopp von außen:** wie D1 (Plugin + Dateisperre; Tray /
  `paperrust-desktop stop` / Abmelden; `on_download` in den Download-Ordner).
- **Ohne Formular:** `PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}` bzw. `PAPERRUST_DESKTOP_IMPORT[_DB|_MEDIA]`
  beantworten den Erst-Start auch in der Tauri-Hülle (verwaltete Installationen, Tests).
- Nicht automatisiert getestet: Tray-Klicks (die Sicherung selbst ist über denselben `BackupJob`
  per `paperrust-desktop backup` bei laufender App E2E-geprüft), Downloads im WebView, „Erneut
  versuchen“ und „Log öffnen“ (Knöpfe sichtbar, Klick nicht ausgeführt), macOS/Linux.

## 17. Paketbau (`apps/desktop/pack/`)

`pack.ps1 [-Zip] [-Installer]` baut `results/desktop/PaperRust-<version>-windows-x64/` (+ `.zip`,
+ `-setup.exe`); Layout wie `layout.rs`:

| Teil | Quelle | Größe im Paket |
|---|---|---|
| `paperrust.exe` | `cargo build --release -p pl-server --bin paperrust -j 1` (fat LTO, `strip = "symbols"`, `debug = 0`, keine PDB im Paket) | 69 MB |
| `paperrust-app.exe` / `paperrust-desktop.exe` / `paperrust-migrate-db.exe` | Release-Builds | 11 / 2,2 / 6,3 MB |
| `static\` | `frontend/dist` (R2), **ohne** `.gz`/`.br` (über 127.0.0.1 nutzlos; pl-server liefert dann unkomprimiert) | 22 MB (seit F1: zusammengefasste Ablage, vorher 303 MB) (351 Dateien, 37 Sprachen) |
| `pgsql\` | PostgreSQL 17.11.1 theseus-rs, ausgedünnt per **Keep-Liste** (Z3, Begründung unten); **Selbsttest** initdb ICU + Start + `SELECT 1`, `pg_isready`, `createdb`, ICU-Sortierung, plpgsql-Trigger, Snowball `german`, `client_encoding=WIN1252`, `pg_dump`/`pg_restore --list`, Stopp | 59,6 MB (vorher 65,7 MB; 696 statt 1 016 Dateien) |
| `tesseract\` | UB Mannheim 5.4.0.20240606 (NSIS-Installer **nur entpackt**, 7-Zip; fehlt 7-Zip, wird das gepinnte 7-Zip-MSI per `msiexec /a` ausgepackt – ohne Admin); nur die 26 DLLs, die `tesseract.exe` wirklich lädt (Importe per `llvm-readobj`, 25 Trainings-DLLs pango/cairo/glib/ICU raus), DWARF-Debug-Abschnitte abgeschnitten (`Remove-PeDebugSections`: libtesseract 97 → 3 MB; `llvm-strip` scheitert an den MinGW-Symboltabellen); tessdata_fast `deu`, `eng`, `osd`; Selbsttest `--version`, `--list-langs` | 37 MB (vorher 176 MB) |
| `pdfium\pdfium.dll` | bblanchon chromium/8111 | 7,3 MB |
| `fonts\LiberationSerif-Regular.ttf` | Liberation 2.1.5 (SIL OFL 1.1), `sources.json` `fonts`; Schrift für Text-Vorschaubilder (Abschnitt 9) | 0,4 MB |
| `routes.toml` | Repo-`routes.toml` unverändert (seit 2026-11-14 schaltet die Datei Routen nur noch ab, nicht genannte ids sind an; vorher `results/parity/routes-all.toml`); Prüfung: kein alter Wert `django` | – |
| `PaperRust.cmd`, `README.txt`, `LICENSE`, `NOTICE`, `THIRD-PARTY-LICENSES(.txt)`, `PACKAGE.json` | Startskript (`--portable` = Daten in `.\data`), Kurzanleitung, Lizenzen (Abschnitt 10), Versionen + SHA-256 aller Fremdteile und Programme | 0,5 MB |
| **gesamt** | | **213,8 MB** entpackt (1 123 Dateien), ZIP **84,3 MB**, Installer **54,6 MB** (vor Z3: 498,6 / 198,7 / 59,0 MB) |

**PostgreSQL ausgedünnt (Z3), Nachweis im Code:**

| behalten | warum |
|---|---|
| `bin\` `initdb`, `pg_ctl`, `postgres`, `pg_isready`, `createdb`, `psql`, `pg_dump` | `pg::REQUIRED_TOOLS` (`apps/desktop/src/pg.rs`); `pack.ps1` liest die Liste aus dem Quelltext (`Get-PgKeepTools`), damit sie nicht auseinanderläuft |
| `bin\pg_restore.exe` (0,2 MB) | Wiederherstellen einer Sicherung von Hand (Abschnitt 7/8), Selbsttest `pg_restore --list` |
| 11 DLLs in `bin\` (ICU `icudt/icuin/icuuc`, OpenSSL, libxml2, iconv, zstd, lz4, zlib, libpq) | statische Importe der behaltenen Programme und Module, rekursiv per `llvm-readobj --coff-imports` ermittelt (`Get-ImportClosure`) |
| ICU-Daten `icudt78.dll` (31,6 MB) | `locale_provider = "icu"` ist Standard (`config.rs` `default_locale`), initdb `--locale-provider=icu`; bestehende Cluster brauchen ICU zum Start. Eine ausgedünnte ICU-Datendatei ginge nur mit einem eigenen ICU-Build (nicht gemacht) |
| `lib\plpgsql.dll` | initdb legt die Erweiterung an; Trigger in `pl-sync` sind `LANGUAGE plpgsql` |
| `lib\dict_snowball.dll` | initdb legt die Snowball-Textsuche-Konfigurationen an |
| `lib\*_and_*.dll` (Zeichensatz-Konvertierungen, 2,4 MB) | initdb registriert sie in `pg_conversion`; geladen bei `client_encoding` ≠ UTF8 (psql in der Windows-Konsole) |
| `share\` ohne `doc\` | `postgres.bki` & Co. (initdb), `timezone\`/`timezonesets\` (Zeitzonen, `timezone_abbreviations`), `tsearch_data\`, `extension\plpgsql*` |

| entfernt | warum unbedenklich |
|---|---|
| 31 der 39 Programme in `bin\` (u. a. `pg_upgrade`, `pg_basebackup`, `vacuumdb`, `pg_resetwal`, `pg_waldump`, `python.exe`, `pg_config`) | ruft PaperRust nie auf (`grep` über `apps/`, `crates/`: nur `REQUIRED_TOOLS`); `pg_upgrade` käme bei 17 → 18 aus dem **neuen** Paket (Abschnitt 6) |
| 12 DLLs (`icuio`, `icutu`, `legacy` (OpenSSL-Legacy-Provider), `libecpg*`, `libpgtypes`, `libxslt`, `libexslt`, `pkgconf`, `charset`, `python3*`) | von keinem behaltenen Programm/Modul importiert |
| contrib-Erweiterungen in `lib\` und `share\extension\` (pg_trgm, hstore, pgcrypto, postgres_fdw …), Replikation (`libpqwalreceiver`, `pgoutput`), `pgevent`, `pgxs\`, `pkgconfig\`, `share\doc\` | kein `CREATE EXTENSION` in PaperRust oder upstream-Migrationen (`grep` über `crates/`, `apps/`, `upstream/src`), keine Replikation |

Programme (`paperrust*.exe`): Release-Profil hat schon `strip = "symbols"` und kein `debug`
(`Cargo.toml`, `apps/desktop-shell/Cargo.toml`); `llvm-readobj --sections` zeigt keine
`.debug_*`-Abschnitte, PDBs werden nicht mitgeliefert. `panic = "unwind"` bleibt. Kleiner ginge es
nur über `opt-level = "s"`/`"z"` (langsamer, nicht gemacht).

Tesseract: unverändert ausgedünnt (D2); `tesseract.exe` bleibt trotz libtesseract im Prozess nötig
(Versionsabgleich und Rückfallweg in `pl-ocr` `tesslib.rs`). Sprachdaten nur `deu`, `eng`, `osd`
(weitere: Abschnitt 9).

Regeln: Fremddateien nur aus `sources.json` (URL + SHA-256 gepinnt). Tesseract-Prüfsumme
gepinnt 2026-11-09 (`c885fff6…b60c9`): GitHub liefert für das Asset keinen `digest`; Größe stimmt,
Authenticode-Signatur „Universität Mannheim“ intakt (Zertifikat 2023 abgelaufen); der Spiegel
digi.bib.uni-mannheim.de verweigert automatische Downloads (403) → kein Zweitabgleich (TOFU, wie
tessdata). Bauwerkzeuge ebenfalls gepinnt (`tools`: 7-Zip 26.05 MSI, tauri-cli 2.12.4). Cache-Dateiname
= Prüfsumme; jede Datei wird vor Verwendung erneut geprüft. Hinweis: die ausgedünnten Tesseract-DLLs
sind dadurch nicht mehr bytegleich mit dem Installer (PACKAGE.json nennt die Quell-Prüfsumme).

Befunde beim ersten Voll-Lauf (behoben): PG-Selbsttest hing (`pg_ctl start | Out-Null` – der
abgekoppelte `postgres` erbt die Pipe; jetzt `Start-Process` + `WaitForExit`, `Handle` für den
Exit-Code in PS 5.1); `results/desktop` und `results/pack` stehen nicht in `.gitignore`
(→ eigene `.gitignore` mit `*`, CORE-REQUEST 8).

**Z3 (2026-11-09):** Größen vorher/nachher je Teil in `results/desktop/size.md` (entpackt 498,6 →
213,8 MB, ZIP 198,7 → 84,3 MB, Installer 59,0 → 54,6 MB). E2E (`desktop-e2e.mjs`, jetzt mit
Schritt 3b „Textdatei-Vorschaubild ≠ Standardbild“) auf dem neuen Paket bestanden, Protokoll
`results/e2e/desktop-z3/`.

## 18. Sicherheit (Befunde der internen Sicherheitsprüfung, Abschnitt 8)

| Befund | Umsetzung | Nachweis |
|---|---|---|
| **M4** Rechte | `perms.rs`: Unix alle Verzeichnisse 0700 (auch bestehende), private Dateien/Logs 0600; Windows einmalig ACL „nur aktueller Benutzer“ (`icacls /inheritance:r /grant:r *<SID>:(OI)(CI)F`, Flag `setup.home_protected`); Warnung, wenn `home` außerhalb `%LOCALAPPDATA%`/`%USERPROFILE%` bzw. `$HOME` liegt | Test `perms::protect_real_directory` (Windows: genau ein ACE, auch für Unterordner/Datei; Unix: Modi), `warns_outside_profile`; Rauchtest Nr. 15/16 (`icacls`, PostgreSQL läuft mit der ACL) |
| **D-N1** Debug | `ServerSpec` manuelles `Debug` mit `***` für `is_secret`-Variablen; `DesktopConfig`-Debug auch ohne `app_password` | `server::debug_hides_secrets`, `config::debug_hides_secrets` |
| **D-N2** `initdb.pw` | `Supervisor::open` löscht eine liegengebliebene Datei (vor allem anderen) | `supervisor::open_reports_missing_install_and_second_instance`; Rauchtest Nr. 16 |
| **D-N3** Superuser | Anwendungsrolle `paperless` ohne Superuser-/CREATEDB-/CREATEROLE-Recht, Eigentümer der DB, Objekte werden umgehängt (Bestands-DBs); Server/manage nutzen nur sie; `PGREQUIREAUTH=scram-sha-256` für die Werkzeuge; Server-Seite: `PAPERRUST_DB_REQUIRE_AUTH=scram-sha-256` (CORE-REQUEST 7, erledigt) | `pg::app_role_is_not_superuser`, `server::debug_hides_secrets` (DBUSER = Anwendungsrolle), `config::rejects_bad_values`; Rauchtest Nr. 15/16 (`pg_roles`, Eigentümer, `pg_stat_activity`) |
| **D-N4** URL aus `instance.toml` | nur exakt `http://127.0.0.1:<1–65535>[/]`, sonst ignoriert (Warnung); Datei privat geschrieben | `lock::only_local_urls_from_instance_file` (u. a. `file:///…calc.exe`, `@`, `..`, fremde Hosts) |
| **D-N5** Port/Health | Health muss `"service":"paperrust"` **und** die eigene Instanz-ID (`PAPERRUST_INSTANCE_ID` → `instance`, seit D2 Pflicht) tragen, Kind muss danach leben, Windows: Socket-Besitzer = Kind (`netstat -ano`); bei fremdem Dienst einmal neuer Port | `server::foreign_listener_is_not_healthy`, `health_must_be_ours`, `netstat_listener`; Rauchtest: D1: älteres Binary ohne `service`-Feld als „fremd“ abgelehnt; D2: Altformat wieder abgeschafft, E2E mit Instanz-ID |

## 19. Installer, Signatur, Updater

**Gebaut (D2):** unsignierter **NSIS-Installer** über den Tauri-Bundler, ohne Admin-Rechte und ohne
`cargo install`: `pack.ps1 -Installer` → `installer.ps1` lädt das vorgebaute `cargo-tauri.exe`
2.12.4 (7,8 MB, SHA-256 gepinnt), erzeugt eine Bundle-Konfiguration (Ressourcen = Paketverzeichnis)
und ruft `cargo tauri bundle --bundles nsis` (baut nicht neu). Der Bundler lädt NSIS 3.13 +
`nsis_tauri_utils.dll` selbst und prüft sie gegen eingebaute Prüfsummen. Ergebnis
`results/desktop/PaperRust-0.1.0-windows-x64-setup.exe`, **54,6 MB** (Z3; D2: 59,0 MB). LZMA solid;
seit Z3 übersetzt `installer.ps1` das vom Bundler erzeugte `installer.nsi` noch einmal mit
`SetCompressorDictSize 64` (`-LzmaDictMB`, Tauri bietet dafür keine Einstellung): 58,2 → 54,6 MB,
~3 min mehr Bauzeit, ~64 MB RAM beim Installieren. Prüfung Z3: Installer mit 7-Zip entpackt, alle
1 122 Dateien bytegleich mit dem Paketverzeichnis (außer `paperrust-app.exe`, die der Bundler um
die Bundle-Typ-Kennung ergänzt).

| Eigenschaft | Wert |
|---|---|
| Installationsart | „nur für mich“ (`currentUser`, kein UAC): `%LOCALAPPDATA%\PaperRust Desktop\` – Produktname „PaperRust Desktop“ **bewusst** ≠ Datenordner `%LOCALAPPDATA%\PaperRust\` (sonst lägen Programm und Daten/ACL in einem Ordner) |
| Startmenü | „PaperRust Desktop“ → `paperrust-app.exe` |
| WebView2 | Bootstrapper wird bei Bedarf still geladen (Windows 11: vorhanden) |
| Sprache | Deutsch + Englisch, Lizenzseite = GPL-3.0 |
| Stilles Installieren/Deinstallieren | `setup.exe /S /D=<Ordner>`, `uninstall.exe /S` (geprüft: 34 s, 8 390 Dateien, 498,8 MB; Deinstallation entfernt Programmordner + Registry-Eintrag, **Daten bleiben**) |
| Signatur | **keine** – SmartScreen warnt beim ersten Start („Weitere Informationen → Trotzdem ausführen“) |
| MSI | nicht gebaut (WiX-Download ~30 MB, kein Mehrwert gegenüber NSIS für Einzelplatz) |

**Was der Nutzer für Signatur und Updater bereitstellen muss (Konten/Kosten):**

1. **Windows-Code-Signatur:** ein Authenticode-Zertifikat (OV, oder EV für sofortiges
   SmartScreen-Vertrauen) – seit 2023 nur noch auf Hardware-Token/HSM oder als Cloud-Signatur
   (z. B. Azure Trusted Signing, ~10 USD/Monat, Identitätsprüfung; oder SSL.com/Certum mit
   Cloud-HSM). Einbindung: `bundle.windows.signCommand` (z. B. `trusted-signing-cli … %1`) in der
   Bundle-Konfiguration von `installer.ps1`; signiert werden müssen **alle** EXE/DLL im Paket
   (`paperrust*.exe`, `pgsql\bin\*.exe|dll`, `tesseract\*`, `pdfium.dll`) und der Installer.
   Zeitstempel-Server angeben (sonst ungültig nach Ablauf des Zertifikats – genau das Problem des
   UB-Mannheim-Installers).
2. **Updater** (umgesetzt, Abschnitt 23 – dort die Schritte für den Inhaber): ein Ed25519-Schlüsselpaar (`cargo tauri signer generate`; privater Schlüssel +
   Passwort nur beim Nutzer/CI, öffentlicher in `tauri.conf.json` → `plugins.updater.pubkey`),
   eine Ablage für `latest.json` + signierte Installer (GitHub Releases o. Ä., Konto des Nutzers),
   Einbau von `tauri-plugin-updater` (neue Abhängigkeit, CORE-REQUEST zusammen mit 5). Vor dem
   Update: Sicherung (Abschnitt 6), `paperrust-desktop stop` (Installer/Updater).
3. **macOS** (später): Apple-Developer-Konto (99 USD/Jahr) für Signatur + Notarisierung.

## 20. E2E-Protokoll D2 (2026-11-09, Windows 11, Paket aus dem Installer, frisches Profil)

Ablauf: Installer still in einen leeren Ordner (`setup.exe /S /D=<scratch>\e2e\install`), Datenordner
`PAPERRUST_DESKTOP_HOME=<scratch>\e2e\home` (neu), eigenes WebView2-Profil
(`WEBVIEW2_USER_DATA_FOLDER`), Steuerung des **echten Tauri-Fensters** mit Playwright über WebView2-CDP
(`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=…`), Skript
`apps/desktop/e2e/desktop-e2e.mjs`, Protokoll `results/e2e/desktop/e2e-log.json`, Screenshots
`results/e2e/desktop/01…09-*.png`. Ports: DB 54417, App 58080 (Standard; 54329/8100/8101 nie).
Referenz-DB und andere Cluster unberührt.

| # | Schritt | Ergebnis |
|---|---|---|
| 1 | App-Start, Erst-Start-Formular | sichtbar nach 18 s (initdb mit ICU ~10 s) – `01-erststart-formular.png`, `02-erststart-import-option.png` (Wahl „Export importieren“) |
| 2 | Formular „Neu beginnen“ ausgefüllt, abgeschickt | `migrate` (247 Migrationen als Anwendungsrolle) + `createsuperuser` + Index, Anmeldeseite nach 4 s – `03`, `04-anmeldung.png` |
| 3 | Login im Fenster | Dashboard (PaperRust-Branding) – `05-dashboard.png` |
| 4 | Upload `scan-zahnarzt.pdf` (nur Bild, 300 dpi, keine Textebene) | Aufgabe `success` nach **3,1 s**; OCR mit Tesseract + PDFium **aus dem Paket**: Inhalt „Zahnarztpraxis Sonnenberg Rechnung Nr. 4711 Datum: 03.11.2026 … Kennwort fuer die Suche: Quokkaflausch …“ (fehlerfrei), Datum 03.11.2026 erkannt |
| 5 | Volltextsuche „Quokkaflausch“ | API `count = 1`, UI-Liste mit Vorschaubild – `06-suche.png`, Detailseite `07-dokument.png` |
| 6 | Sicherung bei laufender App (`paperrust-desktop backup` → Instanz) | **1,8 s**: `paperrust.pgdump` 368 KB + `export/` (manifest.json, metadata.json, Original, Archiv-PDF, Vorschau) |
| 7 | Beenden (`paperrust-desktop stop`) | 5,0 s, App Exit 0, **keine Prozesse übrig** (postgres, paperrust, WebView2) |
| 8 | Neustart | bereit nach **4,1 s**; Anmeldung nötig (Sitzungs-Cookie ist ein Browser-Sitzungs-Cookie, upstream-Verhalten); Suche findet das Dokument – `08-nach-neustart.png`; zweites Beenden sauber |
| 9 | Fehlerseite: Datenordner mit `PG_VERSION` 16 | nach 2 s „PaperRust konnte nicht starten“ + Hinweis + „Log öffnen (postgresql.log)“ – `09-fehlerseite.png`; „Beenden“ → Exit 0 |
| 10 | **Job-Objekt**: `taskkill /F` nur auf `paperrust-app.exe` (laufend: 11× postgres, paperrust, 6× WebView2) | nach 3 s **keine Prozesse übrig**; nächster Start: PostgreSQL „not properly shut down; automatic recovery“, Daten da |
| 11 | **Import** des Exports aus Schritt 6 in einen neuen Datenordner (`PAPERRUST_DESKTOP_IMPORT`, Konsolen-Hülle aus dem Paket) | bereit nach 12 s; 1 Dokument mit OCR-Inhalt, Benutzer `admin` (Superuser) aus dem Export, **kein** Admin-Dialog |
| 12 | **Datenbank-Umzug** SQLite (paperless-ngx 3.4.2, 211 Dokumente, `tests/dbmigrate/.work/sqlite/data/db.sqlite3`, Kopie) in einen neuen Datenordner (`PAPERRUST_DESKTOP_IMPORT_DB`) | bereit nach 13 s; Bericht `migrated`, 211 Dokumente, 1 Superuser, alle Tabellen gehören der Anwendungsrolle `paperless`, Index neu aufgebaut. Gegenprobe mit einer DB, die eine fremde Migration enthält (`extra/new.sqlite3`): sauberer Abbruch „Migrationsstand … entspricht nicht 3.4.2“, Ziel zurückgerollt, App beendet mit Meldung |
| 13 | Portables ZIP (Vorlauf mit älterem `paperrust-app`) | entpackt in leeren Ordner, Erst-Start, Login, OCR-Upload ok (gleicher Inhalt wie der Installer) |

Befunde (behoben):
1. **Fehlerseite beendete die App**: Fenster zerstören + neu anlegen vom Supervisor-Thread → Prozess
   weg. Jetzt Rücknavigation auf die lokale Seite auf dem Hauptthread.
2. **CSS `display:grid` überschrieb `hidden`** (Fehlerbereich + Formular gleichzeitig sichtbar) → `[hidden] { display:none !important }`.
3. PG-Selbsttest in `pack.ps1` hing (Pipe-Vererbung), ExitCode leer in PS 5.1 (siehe Abschnitt 17).
4. E2E-Skript: API-Version 7 → 406 (upstream 3.4.2 erlaubt nur 9/10 – korrektes Verhalten), Aufgaben-Status
   in v10 klein geschrieben.

Nicht geprüft: Tray-Klicks per Hand, Drag & Drop in die Oberfläche (Handler abgeschaltet, nicht
ausprobiert), Import mit großen/echten Exporten, MariaDB-Quelle, Medienordner-Kopie beim Umzug
(nur Unit-Test), 2. Benutzerkonto auf demselben Rechner, Windows 10, Rechner ohne WebView2.

## 21. Protokolle, Doku und Freigabelinks in der Desktop-App (2026-11-09)

**Protokolle (Menü „Protokolle“).** Ursache des leeren Menüs: Ohne Django schrieb niemand
`paperless.log`/`mail.log`; `/api/logs/` liest nur diese Dateien aus `PAPERLESS_LOGGING_DIR`.
Jetzt schreibt `paperrust` selbst (pl-server `logfile.rs`, jeder Betrieb, jede Plattform) wie
upstream `LOGGING`: Logger `paperless.*`, `paperless_ai`, `ocrmypdf` (ab INFO) → `paperless.log`,
`paperless_mail` → `mail.log`, Format `[2026-11-09 14:03:07,123] [INFO] [paperless.consumer] …`
(Zeitzone `PAPERLESS_TIME_ZONE`), Rotation `PAPERLESS_LOGROTATE_MAX_SIZE` (1 MiB) /
`PAPERLESS_LOGROTATE_MAX_BACKUPS` (20). Abschalten: `PAPERRUST_LOG_FILES=no`. Der Supervisor setzt
`PAPERLESS_LOGGING_DIR=<home>/logs`; `desktop.log` bleibt unverändert.

**Umbenennung 2026-11-17.** Die Protokolldatei der Oberfläche heißt jetzt `paperrust.log` (Logger
werden als `paperrust.*` angezeigt, auch auf der Konsole); eine ältere `paperless.log` bleibt im Menü
sichtbar. Die Konsolenausgabe des Servers, bisher `paperrust.log`, heißt deshalb jetzt `server.log`.

**Doku.** Die Benutzer-Doku (`docs/user/`, de + en) ist in `paperrust` eingebaut und liegt unter
`/rs/docs/` (pl-server `docs.rs`, Routen `rs.docs.index`/`rs.docs.page`): offline, passend zur
Version. Die Doku-Links der Oberfläche zeigen auf `rs/docs/<seite>` (relativ, `frontend/doc-links.json`
`defaultBase`); der Server wählt die Sprache nach dem Sprach-Cookie der Oberfläche
(`django_language`), sonst `Accept-Language` (Deutsch → `de/`, sonst `en/`). Online-Doku statt der
eingebauten: `PAPERRUST_DOCS_URL=https://…/docs/user/` (Umleitung). Kein Paketschritt nötig.

**Freigabelinks.** Die App lauscht nur auf `127.0.0.1`; ein Freigabelink
(`http://127.0.0.1:58080/share/…`) funktioniert also nur auf diesem Computer. Umsetzung:

| Modus | `desktop.toml` | Wirkung |
|---|---|---|
| lokal (Standard) | `lan_sharing = false` | `PAPERRUST_BIND=127.0.0.1:<port>`, `PAPERRUST_SHARE_MODE=local` → Hinweis im Freigabe-Dialog (und im Bündel-Dialog): „Links funktionieren nur auf diesem Computer … herunterladen, per E-Mail senden oder Freigabe im lokalen Netzwerk einschalten“ (de/fr/it übersetzt, sonst Englisch; Frontend-Patch 09) |
| lokales Netzwerk (opt-in) | `[server]` `lan_sharing = true`, optional `lan_address = "192.168.1.20"` | `PAPERRUST_BIND=0.0.0.0:<port>`, `PAPERLESS_URL=http://<LAN-IP>:<port>`, `PAPERLESS_ALLOWED_HOSTS` um die LAN-Adresse ergänzt, `PAPERRUST_SHARE_MODE=lan` → Freigabelinks in allen vier Ansichten (Dialog, Bündel-Dialog, Verwaltung Links/Bündel) mit der LAN-Adresse, Hinweis „funktioniert im selben Netz, nicht aus dem Internet“ |

LAN-Adresse ohne `lan_address`: IPv4 der Netzwerkkarte mit der Standardroute, bei jedem Start
ermittelt (UDP-„connect“ ohne Paket). Ohne Netz bleibt die App lokal (Warnung in `desktop.log`).
Die Oberfläche erfährt den Modus über `<meta name="paperrust_share_mode">` (nur im PaperRust-Template
von `index.html`, nur wenn `PAPERRUST_SHARE_MODE` gesetzt ist – Server-Betrieb und Parität
unverändert, keine neue API).

Sicherheit (`lan_sharing`): erreichbar im LAN sind Anmeldeseite, API und Oberfläche (alles nur mit
Anmeldung, Login-Drosselung wie upstream), anonym nur `/share/<slug>` (wie upstream) und die Doku
`/rs/docs/`. Verkehr ist **unverschlüsselt (http)** – Passwort und Dokumente gehen im Klartext durchs
LAN; nur in vertrauenswürdigen privaten Netzen einschalten, nie im öffentlichen WLAN. Windows-Firewall
fragt beim ersten Start mit `0.0.0.0` nach – nur „Private Netzwerke“ zulassen (bei Ablehnung bleibt
die App lokal erreichbar, nur das LAN nicht). Standard bleibt aus. Ein Schalter in Tray/Einstellungen
der Hülle fehlt noch (Konfigurationsschlüssel; App danach neu starten).

Aus dem Internet erreichbare Freigabelinks: mit einem PaperRust-Server (eigene Adresse, TLS) und der
Fern-/Sync-Betriebsart der App, siehe [REMOTE-APP.md](REMOTE-APP.md).

## 22. Fern-App Modus 2a: mit einem Server verbinden (P1, 2026-11-09)

Umsetzung von [REMOTE-APP.md](REMOTE-APP.md) §2–§7 für 2a in `apps/desktop-shell/`. (Die Roadmap
nannte „§21“; der Abschnitt war schon vergeben.)

**Betriebsart.** `<Datenordner>/app.toml` (`mode = "local" | "remote"`, `active_profile`,
`[[profile]]` mit `id`, `name`, `url`, `flavor`, `ui`, `tls_pin`, `allow_http`, `extra_origins`,
`sync`; keine Geheimnisse; fehlende Felder → Standardwerte, ungültige Profile werden beim Laden
verworfen). Start-Entscheidung (`mode::decide`): `PAPERRUST_DESKTOP_MODE` > `app.toml` > lokal, wenn
`desktop.toml`/`pgdata/` schon da sind oder der Erst-Start per Umgebung beantwortet ist
(`PAPERRUST_DESKTOP_ADMIN_*`/`_IMPORT*`) > sonst **Erst-Start-Frage**. Bestehende Installationen
bleiben also ohne Frage lokal. `PAPERRUST_DESKTOP_SERVER_URL=…` legt ein Profil an und startet fern
(http nur mit `PAPERRUST_DESKTOP_ALLOW_HTTP=1` und nur im eigenen Netz).

**Erst-Start.** Ein Formular „Wie möchtest du PaperRust nutzen?“: *Auf diesem Computer* (neu
beginnen / Export importieren / Datenbank übernehmen – die drei Wege aus Abschnitt 5) oder *Mit einem
Server verbinden*. Abweichend von REMOTE-APP §2 kein vorgeschalteter eigener Schritt, sondern **vor**
dem Supervisor beantwortet: Wer lokal wählt, startet den Supervisor mit dieser Vorauswahl (kein
zweites Formular; initdb läuft erst nach der Wahl). So wird bei „Server“ nie ein `pgdata/` angelegt,
und die Formularfelder/IDs von Modus 1 bleiben gleich (`desktop-e2e.mjs` unverändert).

**Verbinden** (`server_connect`, Rust/`reqwest` 0.12 + rustls mit Webpki- und Windows-Zertifikaten):
1. Adresse prüfen (`remote::parse_server_url`): ohne Schema → `https://`; nur `https`/`http`; keine
   Benutzerinfo, kein `?`/`#`; Unterpfad bleibt Teil der Basis (`PAPERLESS_FORCE_SCRIPT_NAME`); IDN →
   Punycode wird angezeigt.
2. `http://` nur für Loopback, RFC 1918, `fc00::/7`, `*.local`, `localhost` – mit Bestätigung
   („Ich verstehe das Risiko“); öffentliche Adressen nie. Fenstertitel dauerhaft
   „PaperRust – <Profil> – ⚠ unverschlüsselt (http)“.
3. Erkennung (`remote::detect`, anonym): `GET rs/api/v2/server` (200 JSON → PaperRust, ab P3) →
   `GET api/remote_version/` (JSON mit Objekt `paperrust` → **PaperRust**, nur `version` →
   **paperless-ngx** + Version) → `GET api/ui_settings/` (401/403 mit DRF-`{"detail"}` →
   paperless-ngx älter; Hinweis „nicht geprüft“ unter 2.x). Sonst „kein PaperRust- oder
   paperless-ngx-Server“. Aufgezeichnete Antworten beider Server sind Unit-Tests. (`/api/` selbst
   taugt nicht: 3.4.2 leitet es anonym per 302 auf `schema/view/` um.)
4. Zertifikat nicht vertrauenswürdig (rustls `UnknownIssuer`/`NotValidForName`): Zertifikat ohne
   Prüfung holen und anzeigen (Inhaber, Aussteller, gültig bis, **SHA-256-Fingerabdruck** wie
   `openssl x509 -fingerprint -sha256`); nach Bestätigung wird der **SPKI-SHA-256** als `tls_pin`
   gespeichert. Abgelaufen/noch nicht gültig → harter Fehler, kein Angebot.
5. Speichern, App startet neu im Fern-Modus.

**Fern-Modus.** Kein Supervisor, kein PostgreSQL, keine Ports (E2E: nur `paperrust-app.exe` +
WebView2). Einzelinstanz-Sperre `run/instance.lock` hält die Hülle selbst (auch gegen eine lokal
laufende Konsolen-Hülle); `paperrust-desktop stop` beendet auch den Fern-Modus. Fenster mit
**eigenem WebView-Datenordner je Profil** (`profiles/<id>/webview`) → Cookies je Profil getrennt.
Ablauf: Erkennung → bei Erfolg lädt das Fenster die Server-Adresse; die **Anmeldung** ist die Seite
des Servers (Passwort, MFA, SSO, Passwort-Reset ohne eigenen Code; die App sieht kein Passwort).
Danach alle 15 s `GET accounts/login/` (jede Antwort < 500 = online).

**Navigation** (`nav::NavPolicy`): exakte Origin (Schema + Host + Port). Modus 1 wie bisher genau
`http://127.0.0.1:<App-Port>`; 2a die Origin des Profils + bestätigte `extra_origins`. Alles andere
→ System-Browser. Leitet die **Anmeldeseite** des Servers (`<basis>accounts/…`) zu einer fremden
https-Origin weiter (OIDC/SSO), fragt die Hülle („Erlauben“ / „Im Browser öffnen“ / „Abbrechen“);
„Erlauben“ trägt die Origin in `extra_origins` ein. Server-Seiten haben **keine** IPC-Rechte (E2E:
`settings_state`/`shell_state`/`profile_remove` aus der Server-Seite → „not allowed“; `__TAURI__`
ist wegen `withGlobalTauri` sichtbar, aber wirkungslos).

**Offline.** Verbindungsfehler beim Laden (WebView2 `NavigationCompleted`, sofort) oder in der
15-s-Prüfung → eigene Seite „Server nicht erreichbar“ (Profil, Adresse, Fehler, „Zuletzt
verbunden“, Countdown); automatischer Neuversuch 15 s → 30 → 60 → 120 s (dann alle 2 min),
„Erneut versuchen“ sofort, „Einstellungen“. Zurück online → Server-Seite (Sitzung bleibt).

**Einstellungen** (`ui/settings.html`, Tray „Einstellungen …“ in beiden Modi, Knopf auf Fehler-/
Offline-Seite): Betriebsart wechseln (startet neu; `pgdata/` wird weder gestartet noch gelöscht),
Profile verwenden/umbenennen/abmelden/entfernen, „Server hinzufügen …“, Oberfläche (nur „vom Server“;
„App-Oberfläche“ folgt mit Sync). **Abmelden**: Server-Logout (`accounts/logout/`) und Löschen von
`profiles/<id>/webview` (beim aktiven Profil beim Neustart über `logout.request`). Entfernte Profile:
Ordner wird beim nächsten Start weggeräumt.

**Tray je Modus** (`tray::menu_spec`): lokal wie bisher + „Einstellungen …“; fern „PaperRust
öffnen“, „Profil wechseln ▸“ (angehakt = aktiv), „Einstellungen …“, „Logs öffnen“, „Beenden“
(ohne Sicherung/Eingangsordner); Tooltip „verbunden mit <Profil>“ / „offline (<Profil>)“.

**Neustart.** Eigener Neustart statt `AppHandle::restart`: Tauri startet den Nachfolger als normales
Kind – unter Windows **im Job-Objekt** der App, das ihn beim Ende des Vorgängers sofort mitbeendete
(E2E-Befund: App war nach „Verbinden“ weg). Jetzt: Einzelinstanz freigeben, Nachfolger mit
`CREATE_BREAKAWAY_FROM_JOB` starten (`PAPERRUST_DESKTOP_RESTART_AFTER=<pid>`), der Nachfolger wartet
auf das Ende des Vorgängers. Ein lokaler Server wird vorher geordnet beendet. Dauer ~2 s (fern),
~9 s (lokal → fern inkl. PostgreSQL-Stopp).

**Eigene Kennung bei `PAPERRUST_DESKTOP_HOME`.** Mit Override (portabel, Tests) bekommt die App die
Kennung `ch.paperrust.desktop.h<10 Hex>` (SHA-256 des Pfads): eigene Einzelinstanz und eigener
WebView-Standardordner. Vorher holte eine portable Installation bzw. ein Testlauf nur das Fenster
einer installierten App nach vorn (gleiche Mutex-Kennung). Ohne Override unverändert
`ch.paperrust.desktop`.

### Spike TLS: selbst signierte Zertifikate in WebView2 (Ergebnis)

**Geht ohne Zertifikatsspeicher.** WebView2 (`ICoreWebView2_14`) meldet
`ServerCertificateErrorDetected`; über `PlatformWebview::controller()` (Tauri `with_webview`) +
`webview2-com` (gleiche Version wie wry, keine neuen Crates) setzt die Hülle `ALWAYS_ALLOW` genau
dann, wenn (a) die Anfrage an die Origin des aktiven Profils geht, (b) der SPKI-SHA-256 des
vorgelegten Zertifikats dem bestätigten `tls_pin` entspricht und (c) der Fehler nicht „abgelaufen“/
„widerrufen“ ist; sonst Standardverhalten (Fehlerseite). Die Ausnahme gilt nur in diesem Fenster
(WebView2 merkt sie sich für die Sitzung), kein `certutil -addstore`, keine Rückfrage von Windows.
Der Rust-Teil (Erkennung, 15-s-Prüfung) prüft denselben Pin auf jeder Antwort
(`danger_accept_invalid_certs` + `tls_info`, Pin vor Verwendung der Antwort). Wechselt das
Zertifikat, lehnen beide ab: E2E nach 8 s Fehlerseite „Das Zertifikat des Servers hat sich
geändert …“ mit Hinweis „Profil entfernen und neu verbinden“. macOS/Linux (WebKit): kein
Gegenstück in wry – dort bliebe nur Schlüsselbund/Anleitung (nicht umgesetzt).

### Tests

- Unit Hülle (`cd apps/desktop-shell && cargo test -j 4`): **32** (vorher 5; der Navigationstest steckt jetzt in `nav`) – `mode` 6 (Rundlauf,
  fehlende Felder, ungültige Profile, anlegen/entfernen, Start-Entscheidung, Namen/IDs), `remote` 11
  (URL-Regeln inkl. IDN/Unterpfad, http nur lokal/privat, Origins, Erkennung PaperRust v1/v2,
  paperless-ngx 3.4.2/alt, Fremdserver, Abbruch bei Netz-/Pin-Fehler, rustls-Fehlertexte,
  Zertifikatsfelder + SPKI-Pin mit **rcgen**-Zertifikat zur Testlaufzeit, Backoff), `nav` 4,
  `tray` 2, `main` 9 (bisherige 4 + Vorauswahl, Umgebungs-Profil, http-Titel, Profil-Aufräumen,
  Kennung je Home). `apps/desktop` (`cargo test -p paperrust-desktop`): 45 + 1 grün. clippy
  `--all-targets -D warnings` sauber (beide), `cargo fmt --check` sauber.
- E2E Fern: `apps/desktop-shell/e2e/remote-e2e.mjs`, Protokoll `results/desktop/remote/e2e-log.json`
  + Bilder `01…20-*.png` (Tabelle unten).
- E2E Modus 1: `apps/desktop/e2e/desktop-e2e.mjs` **unverändert** gegen das D2-Paket mit neuer
  Hülle, frischer Datenordner: **bestanden** (112 s; Erst-Start-Formular → Konto, Login, OCR-Upload
  10,6 s, Suche, Sicherung im Betrieb 2,6 s, `stop` 6,8 s ohne Restprozesse, Neustart 9,1 s mit
  erhaltener Sitzung, Fehlerseite PG 16), `results/desktop/remote/mode1-regression/`. Hinweis zum
  Skript: Pfade relativ übergeben – es zählt jeden Prozess, dessen Kommandozeile den Datenordner
  enthält, als „übrig“ (auch `node` selbst bzw. ein `timeout`-Wrapper).

### E2E-Protokoll P1 (2026-11-09, Windows 11, Debug-Hülle im D2-Paket, frischer Datenordner)

Gegenstellen nur lokal: PaperRust-Testserver (Standalone, `paperrust serve --worker`, DB-Kopie
`pl_p1remote` aus `paperless_snapshot`, `127.0.0.1:8191`), TLS-Vorbau (Node, selbst signiert,
`127.0.0.1:8443` → 8191), Referenz-Django `127.0.0.1:8000` **nur lesend** (Erkennung + Anmeldeseite;
keine Anmeldung, die schriebe `last_login`/Session in die Referenz-DB).

| # | Schritt | Ergebnis |
|---|---|---|
| 1 | Erst-Start | Wahl lokal (3 Wege) / „Mit einem Server verbinden“ – `01`, `02` |
| 2 | ungültige Eingaben | `https://user@…`, `http://example.com` (öffentlich), `ftp://…` abgelehnt – `03` |
| 3 | `http://127.0.0.1:8191` | http-Bestätigung verlangt, ohne Haken kein Verbinden – `04`; „PaperRust 0.1.0“ erkannt in 3 s, Neustart 1,8 s |
| 4 | Fern-Start | Anmeldeseite des Servers – `05`; `app.toml` (remote, flavor paperrust, allow_http), `profiles/<id>/webview/EBWebView`; **kein** `pgdata/`, kein PostgreSQL-/Server-Prozess |
| 5 | Login admin im Fenster | Dashboard, Dokumentliste, 195 Dokumente über die Sitzung – `06`, `07` |
| 6 | Sicherheit | jeder Tauri-Befehl aus der Server-Seite „not allowed“; `ftp://`-Navigation bleibt aus |
| 7 | Server gestoppt + Neuladen | Offline-Seite nach 5,5 s (WebView2-Ladefehler → sofortige Prüfung), „Zuletzt verbunden“, Countdown – `08` |
| 8 | Server gestartet, „Erneut versuchen“ | Dashboard nach 5,2 s, Sitzung erhalten – `09` |
| 9 | Einstellungen → „Server hinzufügen“ `http://127.0.0.1:8000` | „paperless-ngx 3.4.2“, Neustart, Anmeldeseite von paperless-ngx – `10`, `11`; zweites Profil mit eigenem WebView-Ordner |
| 10 | „Verwenden“ (PaperRust-Profil) | Neustart 2,0 s, Profil-Cookies noch da (Dashboard ohne neue Anmeldung) |
| 11 | „Abmelden“ | Server-Logout, Neustart, WebView-Ordner neu angelegt (Markerdatei weg), Anmeldeseite – `12` |
| 12 | `https://127.0.0.1:8443` (selbst signiert) | Dialog-Fingerabdruck = `openssl`-Fingerabdruck, ohne Haken kein Verbinden – `13`; `tls_pin` = SPKI-SHA-256 laut `openssl`; **WebView2 lädt die Seite** (Handler) – `14`; Login, Dashboard über TLS – `15` |
| 13 | Zertifikat getauscht (neuer Schlüssel) | nach 8 s Fehlerseite „Zertifikat … geändert“ – `16`; altes Zertifikat zurück + „Erneut versuchen“ → verbunden |
| 14 | Einstellungen → lokal | Neustart, initdb, Erst-Start-Formular des Supervisors (ohne Server-Option) – `17`; Konto angelegt, lokales Dashboard – `18` |
| 15 | Einstellungen → fern | lokaler Server + PostgreSQL geordnet gestoppt, Neustart 8,6 s, TLS-Profil – `19`; `pgdata/` unverändert |
| 16 | `paperrust-desktop stop` im Fern-Modus | 3,0 s, Exit |
| 17 | Start, Einstellungen → lokal | lokales Konto meldet sich an (Daten nach Hin- und Rückwechsel erhalten) – `20`; `stop` 3,8 s; **keine Prozesse übrig** |

Gesamt 225 s, 41/41 Prüfungen grün.

Nicht geprüft: SSO-Weiterleitung mit echtem IdP (nur Unit-Test der Entscheidung), MFA-Anmeldung
(Seite des Servers, keine App-Logik), Tray-Klicks per Hand (Menü als Daten getestet), Release-Build
der Hülle (E2E mit Debug-Build), macOS/Linux, WebView2 ohne `ICoreWebView2_14` (dann Warnung im Log,
keine Ausnahme), Anmeldung an der Referenz (absichtlich, nur lesend).

### Code (`apps/desktop-shell/`)

| Datei | Inhalt |
|---|---|
| `src/main.rs` | Start je Betriebsart (`begin`), Befehle (`server_connect`, `first_start_local`, `settings_*`, `profile_*`, `sso_decide`, …), Fenster mit Profil-Datenordner, Verbindungsschleife (`remote_loop`), eigener Neustart, Kennung je Home |
| `src/mode.rs` | `app.toml`, Profile, Start-Entscheidung |
| `src/remote.rs` | URL-Prüfung, Netzbereiche, Erkennung (`Get`-Trait, aufgezeichnete Antworten), `Fetcher` (reqwest, Pin), X.509-Felder/SPKI-Pin (kleiner DER-Leser), Backoff |
| `src/nav.rs` | Origin-Allowlist + SSO-Entscheidung |
| `src/tray.rs` | Menü je Modus |
| `src/wv2.rs` | WebView2: `ServerCertificateErrorDetected` (Pin), `NavigationCompleted` (Offline) |
| `ui/index.html`, `ui/app.js` | Erst-Start-Wahl, Verbindungsformular mit http-/TLS-Bestätigung, Offline-, Fehler-, SSO-Seite |
| `ui/settings.html`, `ui/settings.js` | Einstellungen |
| `e2e/remote-e2e.mjs`, `e2e/tls-proxy.mjs` | E2E Fern-Modus, TLS-Vorbau nur für Tests |

`apps/desktop/src/paths.rs`: neu `app_config_file()`, `profiles_dir()`, `profile_dir(id)`. Neue
Abhängigkeiten der Hülle (eigener Workspace, Root-Lock unberührt): `reqwest =0.12.28` (`rustls-tls`,
`rustls-tls-native-roots`), `url`, `sha2`, `base64`, `serde_json`, `toml`; Windows
`webview2-com =0.39.1`, `windows`/`windows-core 0.62` (schon über wry im Lock); Test: `rcgen 0.13`.

## 23. Automatische Updates (Z4, 2026-11-09)

Tauri-Updater (`tauri-plugin-updater =2.14.1`) in `apps/desktop-shell/` (`src/update.rs`, Fenster
`ui/update.html`, Einstellungen-Karte „Updates“, Tray „Nach Updates suchen …“ /
„Update auf X installieren …“). **Scharf erst mit dem Schlüssel des Inhabers** (unten): der Stand im
Repo hat `plugins.updater.pubkey = ""` → Updates in jedem Build abgeschaltet („ohne
Update-Schlüssel gebaut“), alles andere funktioniert wie bisher.

**Ablauf.**
1. **Suchen** im Hintergrund: 20 s nach jedem Start, danach alle `interval_hours` (Standard 24,
   1–720; Wanduhr, stündliche Prüfung – nach Ruhezustand wird nachgeholt), sofort mit „Jetzt suchen“
   bzw. nach dem Speichern der Einstellungen. Manifest = statische Datei im Tauri-Format
   (`latest.json`: `version`, `notes`, `pub_date`, `platforms.windows-x86_64[-nsis].{url,signature}`)
   unter `<Update-Adresse>` mit `{{channel}}` (= `stable` | `beta`; außerdem `{{target}}`,
   `{{arch}}`, `{{current_version}}`). Kanal **stable** liest `stable.json` und nimmt nie eine
   Vorab-Version (`1.2.0-rc.1`); **beta** liest `beta.json` **und** `stable.json` und nimmt die höchste
   Version. Nie gleich/älter (kein Downgrade, auch nicht beim Wechsel beta → stable; ein geladenes
   Beta-Update wird dann verworfen). Manifest höchstens 256 KB, Hinweise auf 20 000 Zeichen gekürzt
   und nur als Text angezeigt.
2. **Laden + Prüfen** (Plugin): aus dem Manifest wird mit `restore_update` ein `Update`
   (Zielplattform, Versionsvergleich; erzwingt `requireSignedVersion` – die Signatur muss die
   angekündigte Version im signierten Kommentar tragen, ein manipuliertes Manifest kann also keine
   ältere, gültig signierte Version als neue ausgeben). `download` prüft minisign/Ed25519 gegen den
   eingebauten öffentlichen Schlüssel; erst danach landet der Installer in
   `<Datenordner>/updates/PaperRust-<v>-setup.exe` + `pending.json` (Version, Kanal, Hinweise,
   Manifest, SHA-256). Fehlschlag → nichts auf der Platte, Meldung in `updates/state.json` und im
   Fenster.
3. **Anbieten**: Update-Fenster (eigenes kleines Fenster; die PaperRust-Oberfläche bleibt, wie sie
   ist) mit Version, Kanal, Größe, Versionshinweisen und „Jetzt neu starten und installieren“ /
   „Beim nächsten Start“. Bei jedem Start wird ein bereitliegendes Update **erneut geprüft** und
   wieder angeboten.
4. **Einspielen** nur nach Klick: Datei erneut gegen Manifest + Schlüssel prüfen (ungültig →
   verworfen, App läuft weiter), `state.json.installing` setzen, dann wie „Beenden“: **lokal Server
   und PostgreSQL geordnet stoppen** (Supervisor, `pg_ctl -m fast`), danach den Installer starten
   und die App beenden. Gestartet wird ein **Helfer** (Kopie von `paperrust-app.exe` unter
   `updates/`, Modus `--paperrust-apply-update`, außerhalb des Job-Objekts): er ruft
   `setup.exe /P /UPDATE /D=<Programmordner>` auf (passiv: Fortschritt, keine Fragen; Update-Modus:
   keine Deinstallation der alten Version, kein WebView2-Download, Daten werden nie angefasst – der
   Installer kennt nur den Programmordner), wartet und startet danach die App mit **derselben
   Umgebung** neu (auch wenn der Installer abbricht: dann die alte Version). Nicht `/R` der
   Tauri-Vorlage: die startet über `nsis_tauri_utils::RunAsUser` mit der Umgebung des Explorers – ein
   im Prozess gesetztes `PAPERRUST_DESKTOP_HOME` ginge verloren und die neue Version öffnete einen
   anderen Datenordner (beim E2E wäre das der echte gewesen).
5. **Nächster Start**: `installing` gegen die eigene Version → „Update auf X eingespielt“ (Downloads
   weg) bzw. „nicht eingespielt“ (`failed_version`: diese Version wird nicht mehr ungefragt
   angeboten). Datenbank-Migrationen wie bisher beim Start (`manage migrate`).

Fern-Modus: identisch (App aktualisiert sich selbst, kein Supervisor zu stoppen); Server-Updates
sind Sache des Servers. Portable Installation (ZIP, kein `uninstall.exe` neben der App) und
Entwickler-Builds: nur Hinweis „Version X verfügbar“, kein Download, kein Einspielen.

**Schalter.**

| Wo | Wirkung |
|---|---|
| `app.toml` `[updates]` `enabled`, `channel`, `interval_hours`, `endpoint` | Einstellungen (Karte „Updates“); `endpoint` leer = Standard |
| `PAPERRUST_DESKTOP_UPDATES=off` (`0`/`false`/`no`/`aus`) | abgeschaltet (verwaltete Installationen) – keine Suche, kein Angebot |
| `PAPERRUST_DESKTOP_UPDATE_URL` | Update-Adresse (vor `app.toml`); nur https, http nur für 127.0.0.1/localhost (Tests) |
| `PAPERRUST_DESKTOP_UPDATE_CHANNEL` | Kanal (vor `app.toml`) |
| `tauri.conf.json` `plugins.updater.pubkey` leer | abgeschaltet („ohne Update-Schlüssel gebaut“); zur Laufzeit **nicht** überschreibbar |

Standard-Adresse (eine Konstante, `update::DEFAULT_ENDPOINT`, Platzhalter bis zur Entscheidung des
Inhabers): `https://github.com/paperrust/paperrust/releases/download/updater/{{channel}}.json` –
eine GitHub-Release mit dem Tag `updater`, deren Assets `stable.json`/`beta.json` bei jeder
Veröffentlichung ersetzt werden; die Installer hängen an der jeweiligen Versions-Release.
Weiterleitungen (GitHub → objects.githubusercontent.com) nur auf erlaubte Adressen.

**Was der Inhaber tun muss** (Konto/Schlüssel/Ablage – nicht automatisiert):
1. Schlüssel erzeugen (einmalig, außerhalb des Repos, mit Passwort):
   `$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD='…'; .\apps\desktop\pack\updater.ps1 -NewKey -KeyPath $HOME\.paperrust\updater.key`.
   Privaten Schlüssel + Passwort sicher aufbewahren (Passwort-Manager, ggf. CI-Secret
   `TAURI_SIGNING_PRIVATE_KEY`/`…_PASSWORD`). **Verlust = keine Updates mehr** für alle installierten
   Versionen (nur Neuinstallation mit neuem Schlüssel).
2. Inhalt von `updater.key.pub` in `apps/desktop-shell/tauri.conf.json` → `plugins.updater.pubkey`
   eintragen und committen (öffentlich, unkritisch). Ab diesem Build suchen die Apps Updates.
3. Ablage festlegen: Organisation/Repo auf GitHub (dann ggf. nur `DEFAULT_ENDPOINT` anpassen) oder
   eigener https-Server (statische Dateien genügen).
4. Je Veröffentlichung: `pack.ps1 -Zip -Installer`, dann
   `updater.ps1 -KeyPath … -Installer results\desktop\PaperRust-<v>-windows-x64-setup.exe -Version <v> -Channel stable|beta -Url <https-Adresse des Installers> -NotesFile <hinweise.md> -OutDir results\desktop\updater`
   → `<kanal>.json` + `<installer>.sig`; Installer an die Versions-Release hängen, `<kanal>.json` in
   der Release `updater` ersetzen. Das Skript prüft SemVer, „stable ohne Vorab-Version“, https und
   warnt, wenn der Schlüssel nicht zu `tauri.conf.json` passt.
5. Zusätzlich empfohlen (unabhängig vom Updater): Authenticode-Signatur des Installers (§19), sonst
   warnt SmartScreen auch beim Update.

**Test-Schlüssel:** `updater.ps1 -NewTestKey` → `results/pack/updater-test-key/` (git-ignoriert,
ohne Passwort, nur Tests). Im Repo liegen nur öffentliche Testdaten
(`apps/desktop-shell/tests/fixtures/updater/`: `test.pub`, `other.pub`, `payload.bin` +
Signaturen, `.gitattributes` `-text`).

### Tests

- Unit Hülle (`cd apps/desktop-shell && cargo test -j 2`): **41** (vorher 32; neu `update` 9):
  Versionsvergleich + Kanäle (stable nie Vorab-Version, beta nimmt die höchste aus beta/stable, kein
  Downgrade), Schalter (Umgebung `off`-Varianten, ohne Schlüssel, Einstellungen, Umgebung vor
  `app.toml`, Intervall-Grenzen, http nur lokal), Adress-Vorlagen, Manifest-Prüfung (Pflichtfelder,
  Plattform, URL-Regeln, Größe, Kürzen der Hinweise), Fälligkeit, Ablage + Start-Abgleich (eingespielt
  / misslungen, Pfad im Dateinamen abgelehnt), Installer-Kommandozeile; **mit dem echten Plugin**
  (Tauri-`MockRuntime`): gültige Signatur, manipulierte/verlängerte Datei, Manifest-Version ≠
  signierte Version, Signatur ohne Version, kaputte Signatur, fremder Schlüssel → abgelehnt; `check`
  gegen einen lokalen HTTP-Server: stable ignoriert 0.1.1-test, manipulierte Datei → nichts abgelegt,
  portable → nur Hinweis, gültig → abgelegt + erneute Prüfung ok, Kanalwechsel verwirft Beta,
  Manipulation auf der Platte → Einspielen verweigert, Server weg → Fehler. `tray`/`mode`-Tests
  angepasst. clippy `--all-targets -D warnings`, `cargo fmt --check` sauber.
  Testbefund: der Download des Plugins (reqwest 0.13, `rustls-no-provider`) panikt ohne
  prozessweiten Krypto-Anbieter – die Hülle setzt `ring` vor dem Download.
- E2E: `apps/desktop-shell/e2e/updater-e2e.ps1 -Work <scratch> [-Out results\desktop\updater-e2e]`
  baut aus dem Paket-ZIP zwei Installer „PaperRust Updatetest“ (eigene Registry-Schlüssel; Debug-Hülle
  mit `TAURI_CONFIG` = Version + Test-pubkey; danach normaler Debug-Build wiederhergestellt), signiert
  0.1.1-test mit dem Testschlüssel (`updater.ps1`) und startet `updater-e2e.mjs` (lokaler
  HTTP-Server, WebView2-CDP).

### E2E-Protokoll Z4 (2026-11-09, Windows 11, Debug-Hülle, frischer Datenordner, Paket Z3)

| # | Schritt | Ergebnis |
|---|---|---|
| 1 | Basis 0.1.0 still in Scratch-Ordner (`/S /D=`) | 20 s, Registry `DisplayVersion 0.1.0` |
| 2 | Start lokal (Konto per Umgebung), Kanal stable | bereit nach 23 s; erste Suche nach 20 s: nur `stable.json` gelesen, „Kein Update“ |
| 3 | Einstellungen → Kanal beta, Server liefert manipulierten Installer | „Signatur ungültig – Update 0.1.1-test verworfen“, nichts abgelegt – `01-einstellungen-signatur-abgelehnt.png` |
| 4 | richtige Datei, „Jetzt suchen“ | 60,6 MB geladen + geprüft in ~10 s, SHA-256 = Original; Update-Fenster mit Hinweisen als Text (`<b>` sichtbar, nicht ausgeführt) – `02-update-bereit.png` |
| 5 | Datei unter `updates/` manipuliert, „installieren“ | verweigert, verworfen, App läuft weiter – `03-einspielen-verweigert.png` |
| 6 | neu geladen, „Jetzt neu starten und installieren“ | Server + PostgreSQL geordnet gestoppt (9,1 s, `database system is shut down`, kein `postmaster.pid`, keine Prozesse), Installer passiv 30 s, Helfer startet 0.1.1-test |
| 7 | nach dem Update | Registry 0.1.1-test, `paperrust-app.exe` ersetzt, Log „Update auf 0.1.1-test eingespielt“, `state.json` aufgeräumt, Marker + `PG_VERSION` unverändert, gleicher Datenordner (Anmeldung admin über API 200), Suche danach: kein Update, Fenster öffnet nicht erneut – `04-nach-update.png` |
| 8 | `paperrust-desktop stop`, `uninstall.exe /S` | keine Prozesse übrig; Uninstall-Eintrag weg, Datenordner bleibt. Hinweis: die stille Deinstallation der Tauri-Vorlage lässt `HKCU\Software\PaperRust\<Produkt>` (Ordner, Sprache) stehen – das E2E räumt ihn weg |

23/23 Prüfungen, 144 s (ohne Installer-Bau, ~12 min; Endstand, dritter Lauf – die ersten beiden fanden die Befunde 2 und ein Prüfproblem im Skript: `desktop.log` wird beim Start rotiert). Protokoll + Bilder: `results/desktop/updater-e2e/` (git-ignoriert). Befunde (behoben): (1) `/R` der Tauri-Vorlage
startet über `RunAsUser` ohne die Prozess-Umgebung → Helfer; (2) Fenster bauen in
`run_on_main_thread` blockierte unter Windows (Update-Fenster erschien nie) → Bau auf einem
Hintergrund-Task; (3) veraltete Suchfehler neben einem bereitliegenden Update → getrennte Anzeige.

Nicht geprüft: Release-Build der Hülle (E2E mit Debug-Build; Release nur ohne Schlüssel = Updates
aus), echtes GitHub-Hosting/Weiterleitungen (nur lokal; Regeln als Unit-Test), Proxy, Tray-Klicks
per Hand, Fern-Modus-Update im E2E (gleicher Codepfad ohne Supervisor), Abbruch des Installers durch
den Nutzer, macOS/Linux (Einspielen dort noch nicht umgesetzt: nur Hinweis).

### Code

| Datei | Inhalt |
|---|---|
| `apps/desktop-shell/src/update.rs` | Einstellungen/Schalter, Kanäle, Manifest, Ablage `updates/`, Plugin-Anbindung (`to_update`, `check`, `verify_pending`), Helfer + Installer-Start |
| `apps/desktop-shell/src/main.rs` | Hintergrund-Schleife `update_loop`, Befehle `update_*`/`open_update`, Update-Fenster, `finish` (Einspielen nach geordnetem Stopp), Helfer-Modus am Anfang von `main` |
| `ui/update.html`, `ui/update.js`, Karte „Updates“ in `ui/settings.*` | Oberfläche |
| `tauri.conf.json` (`plugins.updater`), `capabilities/default.json` (Fenster `update`; Plugin-Befehle **nicht** freigegeben) | Konfiguration |
| `apps/desktop/pack/updater.ps1` | Schlüssel erzeugen, signieren (`--app-version`), Manifest je Kanal |
| `apps/desktop-shell/e2e/updater-e2e.{ps1,mjs}`, `tests/fixtures/updater/` | Tests |

Neue Abhängigkeiten der Hülle (eigener Workspace): `tauri-plugin-updater =2.14.1` (ohne `zip`,
`system-proxy`), `semver`, `rustls 0.23` (`ring`, schon im Lock); +15 Pakete im Hüllen-Lock
(u. a. `minisign-verify`, `tauri-plugin`, `osakit`/`tar` nur macOS/Linux). Test: Tauri-Feature `test`.

## 24. Restposten vor dem Release (2026-11-10)

### Modus 2b: Links in die klassische Oberfläche, `/ws/status/`

- **Klassische Links** (`/documents/<id>/…`, `/settings`, Volltextsuche `/documents?query=…`,
  Platzhalter „In klassischer Oberfläche öffnen“): online fängt die Hülle die Navigation auf der
  App-Origin ab (`main.rs: classic_link` → `Gateway::classic_url`) und öffnet `<server>/<pfad>` im
  **System-Browser**; das Fenster bleibt in der App. Kein Durchreichen über prapi: die
  Angular-Oberfläche braucht Sitzungs-Cookie und Skripte, Server-HTML liefe dort nur mit
  `CSP: sandbox` und ohne Cookies (leere Seite). Offline (oder abgemeldet) zeigt das Gateway
  „Nur online verfügbar“ mit „Zurück zur App“ (503, ohne Skript). Neuladen einer solchen Seite ist
  in der prapi-Vorprüfung erlaubt (nur GET).
- **`/ws/status/`** (Upload-/Verarbeitungsfortschritt, `document_updated`): eigene Schemata können
  keine WebSockets, deshalb vermittelt der Client-Kern über `127.0.0.1:<zufällig>`
  (`pl_sync_client::wsrelay`, Adresse mit 244-Bit-Schlüssel über `prapi …/local/ws`, Prüfung von
  Pfad/Schlüssel/Origin vor jeder Server-Verbindung). Online: Handshake zum Server mit dem
  Geräte-Token (`Bearer prt_…`, pl-server nimmt es am Socket an), danach nur Byte-Kopie. Offline /
  Server weg: 503; die Light-UI versucht es still erneut (5 s … 60 s). paperless-ngx-Profile: keine
  Adresse (Socket dort nur mit Sitzung) → keine Live-Meldungen, kein Fehler.
- Test-Haken: `PAPERRUST_DESKTOP_E2E_OPEN_LOG=<datei>` – Hülle schreibt Links, die sie im
  System-Browser öffnen würde, in die Datei (E2E startet keinen Browser des Rechners).

E2E `apps/desktop-shell/e2e/app-2b-e2e.mjs` (erweitert), Debug-Hülle + Light-UI-Build, lokaler
PaperRust-Testserver (`serve --worker`, damals mit `PAPERRUST_QUEUE=postgres`, DB-Kopie aus
`paperless_snapshot`, `127.0.0.1:8193`), Ergebnis `results/desktop/app-2b/leftovers/`: alle Schritte
grün in 29 s – Anmeldung, Abgleich 195 Dokumente, **klassischer Link online** → Fenster bleibt auf
`/rs/app/documents`, Ziel `http://127.0.0.1:8193/documents/1/details` im Browser-Protokoll;
**WebSocket online**: Vermittler-Port, CSP `connect-src` gesetzt, Upload über die Light-UI-API
(200) → `status_update STARTED:new_file` über den Vermittler; Server gestoppt → Liste/Suche/PDF
offline wie bisher, **klassischer Link offline** → „Nur online verfügbar“ (`07-klassisch-offline.png`),
**WebSocket offline** → abgelehnt (1006, kein Öffnen); Server wieder da → online, Abmelden widerruft.
Nebenbefund (erledigt 2026-11-13): im Repo-`routes.toml` stand `documents.post_document = "django"`; gegen einen
Standalone-Server mit genau dieser Datei scheitert Hochladen mit 405 (Paket: alle Routen `rust`,
dort kein Problem). Der Testserver lief deshalb mit einer Kopie, in der die Route `rust` ist.

### Rauchtest Einstellungen auf frischem Profil (Supervisor, `desktop.*` über API v2)

`apps/desktop/e2e/settings-smoke.mjs <paket> <home> <out>`: Paket-Layout aus den aktuellen
Debug-Programmen (`paperrust-desktop.exe`, `paperrust.exe`) + Ressourcen des Pakets (pgsql, static,
tesseract, pdfium, fonts, `routes.toml` wie `pack.ps1`: alle Routen `rust`), frischer
`PAPERRUST_DESKTOP_HOME`, Konto über `PAPERRUST_DESKTOP_ADMIN_*`, `paperrust-desktop run --no-browser`.
Ergebnis (`results/desktop/settings-smoke/smoke-log.json`), 2026-11-10:

| # | Schritt | Ergebnis |
|---|---|---|
| 1 | Erst-Start (initdb, Migration, Administrator, Index) | Server erreichbar nach 14,9 s; Standard-App-Port 58080 war belegt → freier Port 50393 (Warnung im Log) |
| 2 | `GET /rs/api/v2/settings/desktop.server.port` (Geräte-Token, Superuser, Loopback) | `50393`, Herkunft `desktop` |
| 3 | `PATCH /rs/api/v2/settings {"values":{"desktop.server.port":"50409"}}` | `202 {"applied":true,"restarting":true}` |
| 4 | Supervisor | `Desktop-Einstellungen geändert keys=["desktop.server.port"] restart=true`, Server beendet (Exit 0) und auf 50409 neu gestartet |
| 5 | neuer Port | erreichbar nach 1,0 s, alter Port zu, `desktop.toml` und `run/desktop-settings.json` = 50409, Token gilt weiter (`/rs/api/v2/me` 200) |
| 6 | `paperrust-desktop stop` | Server + PostgreSQL geordnet beendet, keine Prozesse übrig |

Hinweis für Tests auf geteilten Rechnern: der Port in `desktop.toml` steht vor dem ersten Start auf
58080 – wer fremde Instanzen auf dem Rechner hat, liest den tatsächlichen Port aus dem Log
(„PaperRust-Server gestartet … port=“), sonst spricht er mit der fremden Instanz.

## 25. Anmeldung im Desktop-Betrieb (2026-11-19)

Wunsch des Besitzers: in der lokalen Desktop-Installation **keine Anmeldung mit Benutzername und
Passwort**. Wer sich abmeldete, kannte den beim Erst-Start angelegten Benutzer oft nicht mehr.

### Ablauf

1. **Erst-Start ohne Konto-Formular.** „Neu beginnen“ hat keine Felder mehr. Der Supervisor legt einen
   **lokalen Eigentümer** an (`manage createsuperuser --no-input`): Benutzername = Windows- bzw.
   OS-Benutzername (`USERNAME`, sonst `USER`/`LOGNAME`), bereinigt auf `[A-Za-z0-9@.+_-]`, höchstens
   150 Zeichen, sonst `local`; ist der Name schon vergeben (auch in anderer Groß-/Kleinschreibung),
   mit Zähler (`jane2`). Passwort: 48 Zufallszeichen aus `OsRng`, **nirgends gespeichert oder
   angezeigt** (nur einmal per Umgebung an `createsuperuser`). Der Name steht in `desktop.toml`
   (`[setup] owner_user`). `PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}` (verwaltete Installationen,
   Tests) gilt weiter und legt genau dieses Konto als Eigentümer an.
2. **Bestandsinstallationen und Import.** Bei jedem Start prüft der Supervisor den Eigentümer
   (`autologin::owner_query`): eingetragen und aktiv → bleibt; fehlt der Eintrag (Installation von vor
   dieser Version, z. B. Benutzer `Jane`), wurde er gelöscht oder deaktiviert → **erster aktiver
   Superuser** (kleinste id) wird übernommen und eingetragen. Nach einem ngx-Export oder einer
   Datenbank-Übernahme mit eigenen Benutzern ebenso. Ohne aktiven Superuser bleibt nur die
   automatische Anmeldung aus (Warnung im Log); bringt ein Import keinen Superuser mit, legt der
   Supervisor wie bei „neu beginnen“ einen Eigentümer an.
3. **Schlüssel je Start.** `Supervisor::open` erzeugt 32 Byte aus `OsRng` (64 Hex-Zeichen). Vor dem
   Serverstart schreibt der Supervisor ihn nach `run/desktop-login.token` (Datei im Datenordner mit
   ACL „nur aktueller Benutzer“, Unix 0600) und gibt dem Server `PAPERRUST_DESKTOP_LOGIN_TOKEN` und
   `PAPERRUST_DESKTOP_OWNER` über die **Prozessumgebung** (nie Kommandozeile; `server::is_secret`
   maskiert den Schlüssel in `Debug`/Logs; `manage`-Aufrufe bekommen beides nicht). Beim Beenden wird
   die Datei gelöscht; ein alter Schlüssel ist nach dem nächsten Start ohnehin wertlos.
4. **Server-Route** `GET /rs/desktop/login?token=…&next=…` (`crates/pl-accounts/src/desktop.rs`,
   Route-Id `rs.desktop.login`, in `pl_accounts::routes()`, nicht gelistete Ids sind ohnehin an):
   - ohne beide Variablen (bzw. Schlüssel < 32 Zeichen): **404** wie ein unbekannter Pfad;
   - TCP-Gegenstelle nicht Loopback (`127.0.0.0/8`, `::1`, auch `::ffff:127.0.0.1`) → **403**;
     `X-Forwarded-For`/Trusted-Header werden bewusst nicht beachtet;
   - Drossel (gemeinsamer Cache wie die allauth-Drosseln): nach 10 Fehlversuchen/Minute bzw.
     30/Stunde → 403, auch mit richtigem Schlüssel; gezählt werden nur Fehlversuche;
   - Vergleich in konstanter Zeit (SHA-256 beider Seiten, dann `constant_time_eq`); falsch/fehlend →
     **403 ohne Hinweis**, Log-Zeile wie fehlgeschlagene Anmeldung (`paperless.auth`);
   - Eigentümer muss existieren und aktiv sein, sonst 403 (Grund nur im Log);
   - Anmeldung mit `auth::login` wie der normale Login (Sitzung rotieren, CSRF rotieren,
     `last_login`, Sitzungsdauer wie `LoginView`), Log `Desktop auto-login for user …`;
   - Weiterleitung auf `next` nur relativ (`/…`, kein `//`, kein `\`, keine Steuer-/Leerzeichen),
     sonst `/rs/app/`;
   - jede Antwort `Cache-Control: no-store`, `Referrer-Policy: no-referrer`. Das Zugriffsprotokoll
     schreibt nur den Pfad, nicht die Abfrage – der Schlüssel landet nicht in `server.log`;
   - Zwei-Faktor wird hier nicht abgefragt: der Schlüssel ist selbst der Nachweis „dieses
     Windows-Konto“. MFA bleibt für die Anmeldung von anderen Geräten wirksam.
5. **Tauri-Fenster.** Bei „bereit“ öffnet die Hülle statt `/` bzw. `/rs/app/m/` die Anmelde-URL
   (`local_open_url`, Ziel = gewählte Oberfläche). Navigiert das Fenster später auf die Anmeldeseite
   des lokalen Servers (`http://127.0.0.1:<port>/accounts/login/` oder `/api/auth/login/`, z. B. nach
   „Abmelden“ oder abgelaufener Sitzung), fängt `on_navigation` das ab (`desktop_relogin`,
   `nav::local_login_next`) und meldet über den Schlüssel neu an (Ziel = `next` der Anmeldeseite).
   Schleifenschutz: höchstens 3 automatische Anmeldungen je 60 s, danach erscheint die normale
   Anmeldeseite. Fern-Modus (2a/2b) ist nicht betroffen.
6. **Browser-Variante** (`paperrust-desktop`, `open_browser = true`): der Supervisor öffnet die
   Anmelde-URL im Standardbrowser; eine zweite Instanz (Startmenü, Verknüpfung) liest den Schlüssel
   aus `run/` und öffnet ebenfalls die Anmelde-URL. Nach dem Abmelden zeigt die Anmeldeseite im
   Desktop-Betrieb und nur für Loopback den Hinweis „PaperRust erneut über das Startmenü öffnen –
   die Anmeldung erfolgt automatisch“. **Bewusst kein Knopf und keine automatische Weiterleitung
   auf der Seite:** die Seite müsste dafür den Schlüssel enthalten oder ohne Schlüssel anmelden –
   beides wäre für **jeden lokalen Benutzer** erreichbar (`127.0.0.1` ist für alle Konten des
   Rechners offen). Der Weg über das Startmenü liest den Schlüssel dagegen aus dem ACL-geschützten
   Ordner – das kann nur dieses Windows-Konto. Einfachste sichere Lösung.
7. **LAN-Freigabe** (`lan_sharing = true`): der Schlüssel gilt weiter nur für Loopback; andere Geräte
   melden sich normal an. Dafür im Profil ein Passwort setzen: `PATCH /api/profile/` verlangt wie
   upstream (`ProfileView.patch`) **kein altes Passwort** – geprüft in
   `crates/pl-auth-api/src/profile.rs`. Endet die Sitzung nach der Passwortänderung, meldet die
   Hülle im Fenster automatisch neu an.

### Sicherheitsbetrachtung

| Wer | kommt rein? | warum |
|---|---|---|
| dieses Windows-Konto (Fenster, Startmenü) | ja, ohne Passwort | liest `run/desktop-login.token` (ACL nur dieses Konto) |
| anderes lokales Konto auf demselben Rechner | nein (nur mit Passwort des Eigentümers) | erreicht `127.0.0.1`, aber nicht den Schlüssel; 256 Bit raten ist aussichtslos, Fehlversuche werden gedrosselt und protokolliert |
| Administrator des Rechners | ja | kann jede Datei lesen – wie bisher auch Datenbank und `desktop.toml` |
| anderes Gerät im LAN (Freigabe an) | nur mit Benutzer + Passwort (+ MFA) | Route verlangt TCP-Loopback, `X-Forwarded-For` zählt nicht |
| Webseite im Browser des Benutzers | nein | kennt den Schlüssel nicht; `Referrer-Policy: no-referrer` verhindert, dass er über den Referer abfließt |

Restrisiken: (a) In der Browser-Variante steht die Anmelde-URL samt Schlüssel im Verlauf des
Browsers dieses Kontos – nach dem nächsten Start der App ist sie wertlos. (b) Der Server-Prozess
und seine Kindprozesse (OCR-Werkzeuge) erben die Umgebung mit dem Schlüssel; sie laufen unter
demselben Konto. (c) Ein anderes lokales Konto kann durch absichtliche Fehlversuche die Drossel
auslösen; der Eigentümer sieht dann bis zu einer Stunde die normale Anmeldeseite (kein
Datenzugriff, nur Unannehmlichkeit). (d) Wer im Fenster bewusst als anderer Benutzer arbeiten will,
wird nach dem Abmelden wieder als Eigentümer angemeldet – dafür einen Browser verwenden.
(e) `desktop.toml` mit `owner_user` kann eine ältere Version nicht mehr lesen (`deny_unknown_fields`)
– ein Zurückgehen auf eine ältere Version braucht das Entfernen der Zeile.

### Code und Tests

- `apps/desktop/src/autologin.rs`: Schlüssel, Datei, URL, Eigentümer-Name, Passwort,
  Eigentümer-Abfrage/-Entscheidung, Server-Umgebung – Tests `tokens_are_random_hex`,
  `token_file_roundtrip`, `login_urls`, `owner_names`, `owner_takeover`, `numbered_owner_names`,
  `server_env_only_with_owner`, `owner_query_escapes`, `passwords_are_strong_and_random`.
- `apps/desktop/src/supervisor.rs`: `ensure_superuser` (ohne Dialog), `free_owner_name`,
  `ensure_owner`, `serve_spec`, `publish_login_token`; `SetupChoice::Fresh(Option<…>)`,
  `AdminPrompt::ask` entfällt – Test `fresh_start_needs_no_credentials`.
- `crates/pl-accounts/src/desktop.rs` – Tests `route_is_off_without_both_variables`,
  `token_comparison`, `loopback_only`, `next_must_be_relative`, `hardened_headers`,
  `refuses_non_loopback_even_with_right_token`, `wrong_or_missing_token_is_403_and_throttled`
  (ohne Datenbank; der Erfolgsweg mit Sitzung braucht eine Datenbank und ist nicht als Unit-Test
  abgedeckt). Anmeldeseite: Hinweis `auth.login.desktop_hint` (alle zehn Sprachen).
- `apps/desktop-shell/src/nav.rs` (`local_login_next`, `ReloginGuard`), `main.rs`
  (`local_open_url`, `desktop_relogin`), `ui/` (Felder „Neu beginnen“ entfernt, `submit_admin`
  entfällt) – Tests `login_page_detection`, `relogin_guard_limits_loops`,
  `local_start_and_relogin_use_the_token`, `form_fields_to_choice`.
- E2E-Skripte (`apps/desktop/e2e/desktop-e2e.mjs`, `apps/desktop-shell/e2e/remote-e2e.mjs`) auf den
  neuen Ablauf umgestellt (kein Konto-Formular, Oberfläche statt Anmeldeseite) – **noch nicht
  ausgeführt**.
