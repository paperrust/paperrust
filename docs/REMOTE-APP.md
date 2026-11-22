# PaperRust Desktop – Fern-App (Modus 2) und Übergang zu Sync (Modus 3)

Stand 2026-11-09. Entscheidungsvorlage.

> **Umsetzungsstand:** Modus **2a fertig** (P1, docs/DESKTOP.md §22: Erst-Start-Wahl, `app.toml`,
> Erkennung, Login im WebView, WebView-Daten je Profil, Origin-Allowlist, TLS-Pin über WebView2,
> http-Opt-in, Offline-Seite, Einstellungen, Tray je Modus; E2E `results/desktop/remote/`).
> 2b/3 offen (P5). **Nachtrag 2026-11-22:** Oberfläche je Profil in Einstellungen, Verbindungsformular
> und Tray wählbar („PaperRust-Oberfläche (aus der App)“ = 2b, auch gegen paperless-ngx / „Oberfläche des
> Servers“ = 2a), Betriebsart wechseln mit Rückfrage über Tray „Betriebsart wechseln …“ – docs/DESKTOP.md §26.
> Rahmen: docs/DESKTOP.md (Modus 1, vorhanden),
docs/ARCHITECTURE-V2.md, docs/SYNC.md, docs/FRONTEND-LIGHT.md.

## 1. Modi und Varianten

| Modus | Was läuft lokal | Oberfläche | Gegenstelle |
|---|---|---|---|
| 1 Lokal (vorhanden) | Supervisor + eingebettetes PG + `paperrust serve --worker` | Angular von `http://127.0.0.1:<port>/` | – |
| **2a Fern, Server-Oberfläche** | nur die Hülle | Oberfläche **vom Server** (Angular `/` oder Light `/rs/app/`) direkt im WebView | PaperRust **oder** paperless-ngx |
| 2b Fern, App-Oberfläche | Hülle + Client-Kern (`pl-sync-client`, ohne Replikat-Auswahl) | Light-UI aus dem Paket über `prapi://` | PaperRust (voll), paperless-ngx (über v1-Adapter im Kern) |
| 3 Sync | wie 2b + SQLite-Replikat + Dateicache | Light-UI, offline aus dem Replikat | wie 2b |

**Reihenfolge:** 2a zuerst (klein, sofort nützlich, auch gegen paperless-ngx), dann 2b/3 mit Light-UI und
Client-Kern (ROADMAP-NEXT P1 → P5). 2a bleibt dauerhaft als „klassische Oberfläche“ und für Funktionen,
die die Light-UI noch nicht hat.

## 2. Modus-Wahl und Profile

- **Erst-Start** (neuer erster Schritt vor dem heutigen Formular in `apps/desktop-shell/ui/`): „Wie möchtest
  du PaperRust nutzen?“ – **Auf diesem Computer** (→ heutiger Modus-1-Ablauf, Abschnitt 5 DESKTOP.md) /
  **Mit einem Server verbinden** (→ §3). Ohne Formular: `PAPERRUST_DESKTOP_MODE=local|remote`,
  `PAPERRUST_DESKTOP_SERVER_URL=…` (verwaltete Installationen, Tests).
- **Einstellungen** (lokale Seite `ui/settings.html`, Tray-Eintrag „Einstellungen…“): Modus wechseln,
  Server-Profile verwalten (hinzufügen, umbenennen, Abmelden/Entfernen), Oberfläche wählen (Server/App),
  Sync-Auswahl (Modus 3). Wechsel startet die Hülle neu; **lokale Daten bleiben unberührt** (`pgdata/` wird
  bei Modus „remote“ weder gestartet noch gelöscht).
  Tray „Betriebsart wechseln …“ öffnet die Karte direkt; vor dem Wechsel eine Rückfrage, die sagt, was
  passiert (DESKTOP.md §26). Der Tray-Umschalter „Neue Oberfläche“ schaltet bei paperless-ngx auf die
  App-Oberfläche (2b) um, „Oberfläche des Servers“ zurück auf 2a.
- Ablage: `<Datenordner>/app.toml` (Datenordner wie DESKTOP.md §3; Rechte M4):

```toml
mode = "remote"              # "local" | "remote"
active_profile = "a1b2c3"

[[profile]]
id = "a1b2c3"                # zufällig, Ordnername unter profiles/
name = "Büro"
url = "https://docs.example.ch"          # nur Origin (Schema, Host, Port), ohne Pfad/Benutzerinfo
flavor = "paperrust"         # "paperrust" | "ngx" (erkannt, §3)
ui = "server"                # "server" (2a) | "app" (2b/3)
tls_pin = ""                 # SPKI-SHA-256, nur bei bestätigtem selbstsigniertem Zertifikat
allow_http = false           # nur Loopback/private Netze, nach Bestätigung
extra_origins = []           # SSO/OIDC-Anbieter für den Login in 2a, vom Nutzer bestätigt
sync = false                 # Modus 3 an
```

  Je Profil `profiles/<id>/` mit `webview/` (eigenes WebView-Datenverzeichnis → Cookies je Profil getrennt,
  `WebviewWindowBuilder::data_directory`), später `replica.sqlite` und `files/` (SYNC.md §4). **Keine
  Geheimnisse in `app.toml`.** Token im Schlüsselbund unter `PaperRust/<profile-id>` (SYNC.md §8).

## 3. Server-URL und Anmeldung

1. Eingabe prüfen (umgesetzt `apps/desktop-shell/src/remote.rs`): nur `https://` (oder `http://` nach §4), keine Benutzerinfo (`user@`), kein Pfad außer `/`
   (Unterpfad-Installationen mit `PAPERLESS_FORCE_SCRIPT_NAME`: Pfad erlaubt, wird Teil der Basis), IDN →
   Punycode anzeigen.
2. **Erkennen** (Rust-Seite, `reqwest`/rustls): `GET <url>/rs/api/v2/server` → PaperRust (Version, Features);
   sonst `GET <url>/api/` → 401/403 mit DRF-JSON `{"detail": …}` → paperless-ngx; sonst Fehler „kein
   PaperRust/paperless-ngx unter dieser Adresse“. *Umgesetzt (P1) mit Anpassung:* `/api/` leitet in
   3.4.2 anonym per 302 auf `schema/view/` um und taugt nicht; stattdessen nach `rs/api/v2/server`
   anonym `api/remote_version/` (PaperRust liefert dort ein Objekt `paperrust` → PaperRust v1-Server
   von heute; sonst `version` → paperless-ngx mit Version) und als Rückfall `api/ui_settings/`
   (401/403 + DRF-`detail` → paperless-ngx). Nach der Anmeldung wird `X-Version` geprüft
   (paperless-ngx < 2.x: Warnung „nicht geprüft“).
3. **Anmeldung**
   - **2a:** im WebView auf der Login-Seite des Servers (Session-Cookie im Profil-WebView). Damit
     funktionieren MFA, OIDC/SSO und Passwort-Reset ohne eigenen Code. Die App sieht kein Passwort.
   - **2b/3:** Formular der Light-UI (gleiche Optik wie die Server-Login-Seite) → Client-Kern:
     PaperRust `POST /rs/api/v2/auth/devices` (Geräte-Token, MFA-`code`), paperless-ngx `POST /api/token/`
     (`username`, `password`, `code`; `PaperlessAuthTokenSerializer`, `upstream/src/paperless/serialisers.py`).
     Server nur mit SSO: Token aus der Profilseite des Servers einfügen (v1 `/api/profile/generate_auth_token/`).
   - Token → Schlüsselbund; Passwort nur im Speicher, danach überschrieben.
4. Abmelden: 2a → Server-Logout + WebView-Daten des Profils löschen; 2b/3 → Geräte-Token widerrufen
   (`DELETE /rs/api/v2/auth/devices/<uuid>`, bei ngx nicht möglich – Hinweis), Schlüsselbund, Replikat löschen.

## 4. TLS, selbst signierte Zertifikate, http

| Fall | 2a (WebView direkt) | 2b/3 (Client-Kern) |
|---|---|---|
| gültiges Zertifikat (öffentliche CA oder im OS vertraut) | ja | ja (rustls + `rustls-native-certs`) |
| selbst signiert / eigene CA | WebView vertraut nur dem OS-Speicher. Angebot „Zertifikat diesem Benutzer vertrauen“: Hülle zeigt SHA-256-Fingerabdruck, Nutzer vergleicht, dann Windows `certutil -user -addstore Root <cert>` (Windows fragt selbst nach), macOS `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db` (fragt Passwort). Linux (WebKitGTK): nur Anleitung. **Empfehlung im Dialog: Variante 2b** | **Pinning** des SPKI-Fingerabdrucks nach Bestätigung (eigener rustls-`ServerCertVerifier`, sonst Standardprüfung); Wechsel = harter Fehler |
| `http://` | nur Loopback (`127.0.0.0/8`, `::1`) und private Netze (RFC 1918, `fc00::/7`, `.local`) nach Bestätigung; dauerhaftes Warnsymbol in der Titelleiste; öffentliche Adressen nie | dito |

Ob WebView2/wry eine Zertifikatsausnahme ohne OS-Speicher erlaubt (WebView2 `ServerCertificateErrorDetected`),
ist im Spike P1 zu prüfen; der Plan hängt nicht davon ab.

**Ergebnis Spike P1 (Windows):** ja. `ICoreWebView2_14::add_ServerCertificateErrorDetected` über
Tauri `with_webview` + `webview2-com`; `ALWAYS_ALLOW` nur für die Profil-Origin und nur wenn der
SPKI-SHA-256 des Zertifikats = bestätigter `tls_pin` (abgelaufen/widerrufen nie). Damit entfällt für
2a der Weg über `certutil`/Zertifikatsspeicher; der Dialog zeigt Fingerabdruck (SHA-256 des
Zertifikats) und speichert den SPKI-Pin, den auch der Rust-Teil prüft. E2E: selbst signierter Server
lädt, Zertifikatswechsel → Ablehnung (DESKTOP.md §22). macOS/Linux: offen (kein Gegenstück in wry).

## 5. Tauri-Sicherheit

- **IPC nur für eigene, gepackte Seiten** (`tauri://localhost` / `http(s)://tauri.localhost`: Splash,
  Erst-Start, Einstellungen, Light-UI). Fremde Origins (Server in 2a, 127.0.0.1 in Modus 1) bekommen **keine**
  Capability – wie heute (`apps/desktop-shell/capabilities/default.json`, keine `remote`-Einträge). Die
  Light-UI erhält nur die Befehle `app_state`, `open_settings`, `open_external` (eigene Capability `light`).
- **Navigation** (`allowed_navigation`, heute `apps/desktop-shell/src/main.rs` – fest `127.0.0.1:<port>`):
  verallgemeinern zu einer Liste exakter Origins (Schema + Host + Port): Modus 1 → `http://127.0.0.1:<port>`;
  2a → Profil-`url` + bestätigte `extra_origins` (SSO); alles andere → System-Browser (wie heute).
  Downloads wie heute (`on_download`).
- **CSP:** eigene Seiten wie heute (`tauri.conf.json`, ohne `unsafe-inline`); `connect-src` ergänzt um
  `prapi:` / `http://prapi.localhost`. Fremde Seiten bringen ihre eigene CSP mit (Server-Sache).
- **`prapi://`-Schema** (2b/3, `register_asynchronous_uri_scheme_protocol`): beantwortet nur Anfragen aus der
  App-Origin (Prüfung `Origin`/`Referer`), CORS nur für diese, Pfade nur `/rs/api/v2/…`, `/api/…`
  (Allowlist für die übergangsweise v1-Nutzung, FRONTEND-LIGHT §6) und `/local/…` (Kern-Funktionen). Kein
  offener Port → andere lokale Benutzer kommen nicht heran (anders als Modus 1, DESKTOP.md §4). Das Token
  verlässt den Rust-Kern nie; die WebView sieht es nicht.
- Keine Fernsteuerung der Hülle durch Serverinhalte: Server-Seiten können weder Profile ändern noch Dateien
  lesen; „Datei öffnen“ läuft über Downloads.

## 6. Änderungen an Supervisor und Hülle

| Ort | Änderung |
|---|---|
| `apps/desktop-shell/src/main.rs` | aufteilen: `mode.rs` (`app.toml`, Profile), `local.rs` (heutiger Supervisor-Weg: `spawn_supervisor`/`supervise` unverändert), `remote.rs` (URL prüfen, erkennen, TLS-Dialog, Fenster auf Profil-URL), `nav.rs` (Origin-Liste statt Port), `tray.rs` (Menü je Modus) |
| Start | `main()` liest `app.toml`: `local` → wie heute; `remote` → **kein** Supervisor, kein PostgreSQL, keine Ports; Fenster lädt nach Erkennung die Server-URL (2a) bzw. die Light-UI (2b/3) |
| Tray | Modus remote: „PaperRust öffnen“, „Profil wechseln ▸“, „Einstellungen…“, „Logs öffnen“, „Beenden“ (ohne Sicherung/Eingangsordner); Tooltip „verbunden mit <name>“/„offline“ |
| `apps/desktop-shell/ui/` | Startseite Modus-Wahl, Verbindungsformular, TLS-Bestätigung, Einstellungen, Offline-Seite |
| `apps/desktop` (Bibliothek) | `paths.rs`: `app_config_file()`, `profile_dir(id)`; `lock.rs` gilt auch für remote (Einzelinstanz); Supervisor unverändert. Konsolen-Hülle `paperrust-desktop`: nur Modus 1 (Fern = Tauri) |
| Abhängigkeiten Hülle | 2a: `reqwest` (gleiche Version/Features wie Root-Workspace: `0.12`, `rustls-tls`) für Erkennung und Zertifikatsabruf – `apps/desktop` hat heute keinen HTTP-Client (Health über rohes `TcpStream`, `apps/desktop/src/server.rs`); Hülle ist eigener Workspace, das Root-Lock bleibt unberührt. 2b: `keyring` (Schlüsselbund) über `pl-sync-client` |
| Paket | Modus 2 braucht kein PG/Tesseract/PDFium; **eine** Installer-Variante bleibt (Modus wählbar), eine schlanke „Client“-Variante ohne PG ist optional später (Installer ~59 MB → geschätzt < 15 MB) |

## 7. Offline-Verhalten

- **2a:** Verbindungsfehler beim Laden/Navigieren → lokale Seite „Server nicht erreichbar“ (Profilname, letzte
  erfolgreiche Verbindung, Fehlertext, „Erneut versuchen“, automatischer Neuversuch alle 15 s mit Backoff bis
  2 min). Ist Sync (Modus 3) im Profil an: zusätzlich „Offline-Bestand öffnen“ (Light-UI auf dem Replikat).
  Kein Offline-Lesen ohne Sync.
- **2b/3:** Light-UI bleibt bedienbar; Kern schaltet auf `local`, Banner „offline – lokaler Bestand“,
  Schreiben gesperrt (bis 3b). Rückkehr: Kern nimmt Long-Poll wieder auf, Banner verschwindet.
- Erkennung: Fehlschlag von Long-Poll/Anfrage + `GET /rs/api/v2/server` bzw. `/api/` als Probe; OS-Netzstatus nur
  als Hinweis.

## 8. Tests (nur lokale Testserver)

| Ebene | Inhalt |
|---|---|
| Unit (Hülle) | URL-Prüfung (Schema, `user@host`, Pfad, IDN, private vs. öffentliche Adressen für `http`), Navigations-Allowlist je Modus (gleicher Host anderer Port/Schema → abgelehnt; SSO-Origin nur nach Bestätigung), `app.toml` Rundlauf + Migration fehlender Felder, Tray-Menü je Modus |
| Unit (Kern) | Erkennung PaperRust/ngx/Fremd über aufgezeichnete Antworten, TLS-Pinning (Test-CA mit `rcgen` zur Testlaufzeit erzeugt: richtiger Pin ok, anderer Pin Fehler, abgelaufen Fehler) |
| Integration | lokaler PaperRust-Testserver (Kopie `reference/`, eigener Port) und Referenz-Django `http://127.0.0.1:8000` (paperless-ngx v3.4.2, `reference/README.md`): Erkennung, Token-Abruf mit/ohne MFA, Widerruf, falsches Passwort (Throttle-Meldung) |
| E2E | wie `apps/desktop/e2e/desktop-e2e.mjs` (WebView2-CDP, frisches `PAPERRUST_DESKTOP_HOME`): Erst-Start „Mit Server verbinden“ → Login im WebView → Dokumentliste sichtbar → Server stoppen → Offline-Seite → Server starten → Erneut versuchen → Abmelden löscht Profil-WebView-Daten; Modus-Wechsel lokal↔remote ohne Datenverlust in `pgdata/` |
| Sicherheit | Server-Seite ruft Tauri-Befehle (`window.__TAURI__.core.invoke`, wegen `withGlobalTauri` evtl. vorhanden) → jeder Aufruf abgelehnt; `withGlobalTauri` für remote prüfen/abschalten; Navigation zu Fremd-Origin öffnet System-Browser; Token taucht in keinem Log/keiner Datei im Profilordner auf |
