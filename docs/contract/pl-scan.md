# Vertrag: pl-scan (Scanner-Anbindung)

PaperRust-Erweiterung **ohne upstream-Gegenstück** (paperless-ngx v3.4.2 hat weder eSCL noch einen
SMTP-Empfang). Ziel: Scanner anbinden, ohne dass auf dem Gerät ein NAS-Passwort liegt.
Nutzer-Doku: docs/user/scanner.md (DE), docs/user/en/scanner.md (EN).

| Weg | Module | Ergebnis |
|---|---|---|
| Scannen-Knopf (eSCL/AirScan) | `escl`, `job`, `pdf`, `mdns` | ein PDF → `consume_file`, Besitzer = angemeldeter Nutzer, Quelle `WebUI` |
| Geräte-E-Mail (SMTP, nur Empfang) | `smtp`, `token`, `store`, `net` | je Anhang → `consume_file`, Besitzer = Gerätebesitzer, Quelle `ApiUpload` |

Übergabe an die Pipeline (`intake`): genau wie `POST /api/documents/post_document/`
(crates/pl-server/src/upload.rs): Datei in ein eigenes Verzeichnis `SCRATCH_DIR/tmpXXXXXXXX/`
(Unix `0700`), `ConsumableDocument::new(source, path)`, `DocumentMetadataOverrides { filename,
title, owner_id }`, `pl_consumer::task::enqueue_consume` über den `TaskQueue`-Dienst
(`trigger_source` `web_ui` bzw. `api_upload`). Workflows, Rechte, Benachrichtigungen (`/ws/status/`)
laufen dadurch wie beim Upload. Seit 2026-11-14 gibt es nur noch die PaperRust-Warteschlange; die frühere
Einschränkung „nur mit `PAPERRUST_QUEUE=postgres`“ (`pl_server::worker::NEEDS_PG_QUEUE`) ist entfallen.

## Routen (`/api/x-scan/…`, alle `drf_view`, JSON, DRF-Fehlerformat)

Rechte immer serverseitig über `pl_authz::AuthUser` (Session/Token/Basic wie überall, CSRF bei
Session). „add“ = aktiver Nutzer mit `documents.add_document` (Superuser haben alle Rechte).
„SU“ = aktiver Superuser. Fehlende Anmeldung → 401, fehlendes Recht → 403 `{"detail": …}`.

| id | Methode | Pfad | Recht | Anfrage | Antwort |
|---|---|---|---|---|---|
| `scan.scanners.list` | GET | `/api/x-scan/scanners/` | add | – | 200 `{count, results: [{id, name, model}]}`; für SU zusätzlich `base_url, uuid, created` |
| `scan.scanners.create` | POST | `/api/x-scan/scanners/` | SU | `{url, name?}` | 201 Scanner `{id, name, base_url, uuid, model, created}`. Liest vorher `ScannerCapabilities`; sonst 400 `{"url": ["No eSCL scanner found at this address (…)."]}`; doppelt → 400 `{"url": ["This scanner is already known."]}` |
| `scan.scanners.update` | PATCH | `/api/x-scan/scanners/{id}/` | SU | `{name}` | 200 Scanner; unbekannt 404 |
| `scan.scanners.delete` | DELETE | `/api/x-scan/scanners/{id}/` | SU | – | 204; unbekannt 404 |
| `scan.scanners.capabilities` | GET | `/api/x-scan/scanners/{id}/capabilities/` | add | – | 200 `{scanner, make_and_model, sources: [{source, color_modes, resolutions, formats, max_width, max_height}], state, adf_state}`; Scanner nicht erreichbar → 502 `{code, detail}` |
| `scan.discover` | GET | `/api/x-scan/discover/` | SU | – | 200 `{count, results: [{name, base_url, model, uuid, tls, known}]}` (≈ 3 s); Fehler → `error: "discovery_failed"` |
| `scan.jobs.create` | POST | `/api/x-scan/jobs/` | add | `{scanner, source?, duplex?, resolution?, color_mode?, title?}` | 201 Auftrag (s. u.); belegt 409 `{code: "busy"}`; nicht unterstützt 400 `{code: "unsupported"}`; Gerät 502 |
| `scan.jobs.retrieve` | GET | `/api/x-scan/jobs/{id}/` | add + eigener Auftrag (SU: alle) | – | 200 Auftrag; fremd/unbekannt 404 |
| `scan.jobs.cancel` | POST | `/api/x-scan/jobs/{id}/cancel/` | wie retrieve | – | 200 `{cancel_requested, job}` |
| `scan.devices.list` | GET | `/api/x-scan/devices/` | SU | – | 200 `{count, results: [Gerät], smtp: {enabled, listen, port, tls, require_tls, implicit_tls, max_mb, error}}` |
| `scan.devices.create` | POST | `/api/x-scan/devices/` | SU | `{name, owner, allowed_ips?}` | 201 Gerät + `token` (Klartext, **nur hier**). `owner` muss aktiv sein und `add_document` haben, sonst 400 |
| `scan.devices.revoke` | POST | `/api/x-scan/devices/{id}/revoke/` | SU | – | 200 `{revoked: true}`; unbekannt/schon widerrufen 404 |
| `scan.devices.delete` | DELETE | `/api/x-scan/devices/{id}/` | SU | – | 204; nur widerrufene Geräte, sonst 404 |

Felder:

- `source`: `flatbed` | `adf` | `adf_duplex` (Standard `flatbed`); `duplex: true` macht aus `adf`
  `adf_duplex` (mit `flatbed` → 400). `color_mode`: `color` | `grayscale` | `bw`.
  `resolution`: dpi (50–2400, muss in den Fähigkeiten stehen). Fehlt etwas: Farbe (sonst erster
  Modus), 300 dpi (sonst nächstkleinere), Format JPEG (sonst PDF), Bereich = min(Gerät, A4).
- Auftrag: `{id (uuid), scanner, source, state, pages, error: {code, message} | null, task_id, created}`,
  `state` ∈ `scanning`, `processing`, `done`, `failed`, `cancelled`. Fehlercodes: `unreachable`,
  `busy`, `adf_empty`, `jam`, `cancelled`, `unsupported`, `protocol`, `too_large`, `pdf`, `intake`.
- Gerät: `{id, name, token_hint, owner_id, owner_username, allowed_ips, created, revoked_at,
  last_used, last_ip, documents_received}` – nie `token_hash`.

Nutzer geben **keine Adressen** an: gescannt wird nur mit gespeicherten Scannern (SSRF-Schutz);
Adressen sieht nur SU.

## eSCL-Ablauf (`escl.rs`, `job.rs`)

Quelle: Mopria eSCL Scan Technical Specification 2.x, Verhalten wie sane-airscan.

1. `GET {base}ScannerStatus` (optional): `pwg:State` ≠ `Idle` → `busy`; beim Einzug
   `scan:AdfState` Jam/Misfeed → `jam`, `ScannerAdfEmpty` → `adf_empty`.
2. `POST {base}ScanJobs` (`text/xml`, `scan:ScanSettings` mit `pwg:InputSource` Platen/Feeder,
   `scan:Duplex`, `scan:ColorMode`, `scan:X/YResolution`, `pwg:DocumentFormat` +
   `scan:DocumentFormatExt`, `pwg:ScanRegions` in `escl:ThreeHundredthsOfInches`).
   `201` + `Location` (relativ oder absolut; muss gleicher Host/Port/Schema sein, sonst `protocol`),
   `503` → `busy`, `409` → `adf_empty` (Einzug) bzw. `busy`.
3. `GET {job}/NextDocument` wiederholt: `200` Seite (≤ 200 MB), `503` → 500 ms warten
   (höchstens 600-mal hintereinander), `404` → fertig. Vorlagenglas: nach der ersten Seite fertig.
   Nach `404` mit Seiten: Status prüfen (AdfJam / Auftrag `Aborted` mit `*Jam*`) → `jam`, die
   Seiten werden verworfen. Ohne Seite: Status → `jam` / `adf_empty` / `protocol`. Andere Codes →
   Status → `jam` / `cancelled` / `protocol`, danach `DELETE {job}`.
4. Abbruch: vor jeder Anfrage geprüft → `DELETE {job}` → `cancelled`.
5. Höchstens 500 Seiten je Auftrag; je Scanner ein laufender Auftrag (sonst 409 ohne Gerätezugriff).
   Aufträge leben im Speicher, eine Stunde nach dem Ende entfernt.

PDF (`pdf.rs`): JPEG-Seiten unverändert eingebettet (`DCTDecode`, Farbraum aus dem SOF-Segment,
Seitengröße = Pixel / angeforderte dpi · 72 pt). PDF-Seiten über `pl_pdfops::PdfMerger`, andere
Bildformate über `pl_pdfops::convert::images_to_pdf`. Dateiname `scan-YYYY-MM-DD_HH-MM-SS.pdf`.

HTTP-Client: reqwest, Verbindung 5 s, Anfrage 180 s, keine Weiterleitungen, eigener User-Agent
(`pl_core::brand`), `https`-Zertifikate der Drucker werden nicht geprüft (s. Known differences).
XML: eigener kleiner Baum (`xml.rs`), lokale Namen ohne Namensraum, DOCTYPE abgelehnt,
Tiefe ≤ 32, ≤ 20 000 Knoten, Antworten ≤ 2 MB.

## mDNS (`mdns.rs`)

Eine Anfrage (PTR `_uscan._tcp.local`, `_uscans._tcp.local`, QU-Bit) an `224.0.0.251:5353` von
einem beliebigen Port („legacy unicast“, RFC 6762 §6.7) – PaperRust belegt 5353 nicht. Nach der
halben Zeit einmal wiederholt. Antworten: PTR → SRV (Ziel, Port) → A (IPv4, sonst Absender-IP) +
TXT (`rs` Pfad, Standard `eSCL`; `ty` Modell; `UUID`). Adresse `http(s)://ip:port/<rs>/`.
Parser mit Grenzen (Kompressionssprünge ≤ 32, ≤ 512 Einträge), robust gegen abgeschnittene Pakete.

## SMTP-Empfang (`smtp.rs`)

Start: `pl_server::worker::start_background` (nur HTTP-Modi, nur mit PgQueue) ruft
`pl_scan::smtp::run_if_enabled`; lauscht nur mit `PAPERRUST_SMTP_INTAKE=on`. Fehlerhafte
Einstellungen → Fehler im Log, kein Start (Server läuft weiter); `devices.list` zeigt `smtp.error`.

Einstellungen (docs/SETTINGS.md §1.3, pl-settings Schema): `PAPERRUST_SMTP_INTAKE` (B, off),
`_LISTEN` (B, `0.0.0.0:2525`), `_MAX_MB` (B, 50), `_ALLOWED_IPS` (B), `_RATE_PER_MINUTE` (B, 30),
`_REQUIRE_TLS` (B, off), `_IMPLICIT_TLS` (B, off), `_TLS_CERT` / `_TLS_KEY` (D, PEM-Pfade).

Protokoll (RFC 5321/4954/3207):

| Befehl | Verhalten |
|---|---|
| Verbindung | IP nicht in `_ALLOWED_IPS` → `554 5.7.1`; IP gesperrt → `421`; > 16 Verbindungen → `421`. Mit `_IMPLICIT_TLS` zuerst TLS-Handshake |
| `EHLO`/`HELO` | `250-SIZE <max>`, `8BITMIME`, `STARTTLS` (wenn Zertifikat und noch kein TLS), `AUTH PLAIN LOGIN` (nicht vor TLS, wenn `_REQUIRE_TLS`), `ENHANCEDSTATUSCODES` |
| `STARTTLS` | `220` → Handshake; noch gepufferte Klartext-Daten → Verbindung zu (Befehlseinschleusung); danach Zustand zurückgesetzt. Ohne Zertifikat `502` |
| `AUTH PLAIN` / `LOGIN` | Token im Passwort (LOGIN: auch im Benutzernamen) → `235`; sonst `535`. Ohne TLS bei `_REQUIRE_TLS` → `530` |
| `MAIL FROM` | vor EHLO `503`; `SIZE=` über Grenze → `552` |
| `RCPT TO` | angemeldet: jede Adresse `250` (Name für diesen Empfang, **keine Weiterleitung**); sonst nur Adresse mit gültigem Token (`scan-…@…`, `name+scan-…@…`) → `250`, unbekanntes Token → `550` (zählt als Fehlversuch), sonst `550 5.7.1 Relaying denied`. ≤ 20 Empfänger, ein Gerät je Nachricht |
| `DATA` | Punkt-Entschachtelung; über `_MAX_MB` → bis zum Ende lesen, `552`; Mengengrenze → `451`; kein PDF/JPEG/TIFF-Anhang → `554 5.6.1`; Übergabefehler → `451`; sonst `250 2.0.0 OK: n document(s) queued` |
| `RSET`/`NOOP`/`VRFY`/`QUIT` | `250`/`250`/`252`/`221`; Unbekanntes `500`, nach 10 Fehlern `421` |

Grenzen: 5 Fehlanmeldungen (auch unbekannte Token als Empfänger) je IP in 10 min → Sperre bis zum
Ablauf; Zeilen ≤ 4 KiB (Befehle) bzw. 1 MiB (DATA); 120 s je Befehl; 30 min je Sitzung.
Anhänge: `pl_parsers::mail::MailMessage::attachments()` (derselbe MIME-Parser wie für Mail-Regeln),
Typ am Inhalt (`%PDF-`, `FF D8 FF`, `II*\0`/`MM\0*`), Dateiname über Django-`sanitize_file_name` +
pathvalidate, fehlende Endung ergänzt. Log: Geräte-ID/-Name, IP, Aufgaben-ID – nie Token.
Nach Erfolg: `paperrust_scan_device.last_used/last_ip/documents_received`.

## Token (`token.rs`)

`scan-` + 26 Zeichen `[a-z0-9]` aus dem OS-RNG (≈ 134 bit, ohne Modulo-Verzerrung), Vergleich
ohne Groß/Klein (Adressen werden manchmal kleingeschrieben). Gespeichert nur `sha256` (hex) und
ein Hinweis (`scan-ab…`). Gilt nur im SMTP-Empfang (eigene Tabelle, eigene Form; pl-authz kennt
es nicht): kein Lesen, kein Löschen. Widerruf sofort (kein Cache). Gerät wirkt nur, solange der
Besitzer aktiv ist.

## Tabellen (`migrations/0001_paperrust_scan.sql`, expand-only)

`paperrust_scanner (id, name, base_url UNIQUE, uuid, model, created, created_by)`,
`paperrust_scan_device (id, name, token_hash UNIQUE, token_hint, owner_id, allowed_ips, created,
created_by, revoked_at, last_used, last_ip, documents_received)`. Keine Fremdschlüssel auf
Django-Tabellen. Angelegt beim ersten Zugriff (`store::ensure`, Advisory-Sperre) oder mit
`paperrust manage migrate` (`pl_scan.0001_paperrust_scan`).
Rückbau: `DROP TABLE IF EXISTS paperrust_scan_device, paperrust_scanner;`

## Oberfläche (frontend-light)

`tools/scan` (`Scan.svelte`, Recht add): Scanner, Quelle, Duplex, Auflösung, Farbe, Titel,
Fortschritt (Abfrage jede Sekunde), Ergebnis/Fehler. `tools/scanners` (`Scanners.svelte`, SU):
Scanner suchen/eintragen/umbenennen/entfernen, Geräte anlegen (Token einmal angezeigt),
widerrufen, löschen, Zustand des SMTP-Empfangs. Seitenleiste: „Scannen“ (add), „Scanner“ (SU).
Texte: `scan.*`, `scanners.*` in allen 10 Katalogen.

## Docker / Desktop

Docker: `docker/docker-compose.smtp-intake.yml` (Zusatzdatei) setzt `PAPERRUST_SMTP_INTAKE=on` und
gibt 2525 frei; ohne sie kein Port. Desktop: kein eigener Code; der Port ist nur offen, wenn der
Empfang eingeschaltet ist (docs/DESKTOP.md §4).

## Tests

| Test | Inhalt | Läuft |
|---|---|---|
| Unit (`src/**`) | Token, XML, eSCL-Fähigkeiten/Status/ScanSettings, mDNS-Pakete (auch kaputte), JPEG→PDF, gemischte Seiten, IP-Listen, SMTP-Konfiguration/Grenzen/Typerkennung, Übergabe-kwargs | immer |
| `tests/escl_flow.rs` | Test-Scanner (`tests/support`): Einzug mehrseitig mit 503-Pausen, beidseitig, Vorlagenglas, Stau, belegt (POST/Status), Einzug leer, Abbruch (DELETE), zweiter Auftrag belegt, fremde Location, unerreichbar | immer |
| `tests/smtp_flow.rs` | lokaler SMTP-Client: AUTH PLAIN/LOGIN, Token als Empfänger, falsches/widerrufenes Token, kein Relay, zu groß (SIZE und DATA), Nicht-PDF, mehrere Anhänge + Pfad-Tricks, Sperre, Mengengrenze, IP-Listen, STARTTLS + Pflicht, Einschleusung nach STARTTLS, direktes TLS | immer |
| `tests/api_db.rs` | HTTP-API mit echter DB (Wegwerf-Kopie von `paperless_snapshot`): Rechte, Scanner eintragen, Scannen → `consume_file` (Besitzer, Quelle, 4-seitiges PDF), Gerät/Token (nur Hash), SMTP mit `PgDevices`, Widerruf sofort, inaktiver Besitzer | `#[ignore]`, braucht `PAPERRUST_SCAN_TEST_ADMIN_URL` |

## Ende-zu-Ende im laufenden Server (2026-11-13)

Fertiges Binary (`paperrust serve --worker`; damals noch mit `PAPERRUST_DJANGO_URL=off`, `PAPERRUST_QUEUE=postgres`, seit 2026-11-14 überflüssig)
auf einer Wegwerf-DB (`pl_scan_e2e` aus `paperless_snapshot`, danach gelöscht), echter Consumer
mit OCR, `PAPERRUST_SMTP_INTAKE=on` auf 127.0.0.1:2531, Light-UI aus `frontend-light/dist`.
Skripte: `crates/pl-scan/tests/e2e_server/`.

| Schritt | Ergebnis |
|---|---|
| `manage migrate` | `pl_scan.0001_paperrust_scan` eingespielt |
| Start | Log „SMTP intake listening WITHOUT TLS“ (Warnung wie vorgesehen), Light-UI eingehängt |
| Gerät per API (Admin), Besitzer alice | 201, Token nur in der Antwort |
| SMTP-Client (Node, AUTH PLAIN, PDF-Anhang 3 Seiten) | `250 … 1 document(s) queued` → Dokument 205, Besitzer alice, 3 Seiten, Aufgabe `api_upload` erfolgreich; Gerät zeigt „zuletzt benutzt“ und 1 Dokument |
| Light-UI als admin: Scanner per Adresse eintragen | Name/Modell übernommen, Liste aktualisiert |
| Light-UI „Scannen“: Einzug, beidseitig, 200 dpi, Graustufen, Titel | Test-Scanner bekam `Feeder`/`Duplex true`/`Grayscale8`/200; 4 Seiten → Dokument 206 „UI-Scan Duplex“, Besitzer admin |
| API als alice (nur Rechte der Gruppe editors): Vorlagenglas | Dokument 207, Besitzer alice, 1 Seite; bob sieht den Auftrag nicht (404), carol 403, alice darf keine Geräte (403) |
| Widerruf im laufenden Server | nächstes AUTH → `535` |

Gefundene Fehler: keine. Kleinigkeit: Die Besitzer-Auswahl beim Gerät zeigt alle aktiven Nutzer;
wer keine Dokumente hinzufügen darf, wird erst beim Speichern abgelehnt (400 mit Meldung).

## Known differences / Grenzen

- **Kein upstream-Gegenstück** – alles PaperRust-Erweiterung (`x-scan`-Präfix wie `x-ai`).
- **Nicht am echten Gerät geprüft** (der laufende Server ist geprüft, s. o.): HP OfficeJet Pro 9010 (eSCL-Antworten, Duplex-Reihenfolge,
  Stau-Meldung, SMTP-Verhalten, ob „SSL“ STARTTLS oder direktes TLS ist). Die Test-Fähigkeiten
  (`tests/data/caps_officejet.xml`) sind nachgebaut, nicht mitgeschnitten.
- Duplex-Seiten werden in Lieferreihenfolge übernommen (kein Umsortieren/Drehen der Rückseiten).
- Bei Stau werden bereits gescannte Seiten verworfen (Nutzer scannt neu).
- mDNS nur IPv4 und nur über die Standard-Schnittstelle; in Docker-Bridge-Netzen keine Treffer.
- `https`-Scanner: Zertifikat wird nicht geprüft (Drucker haben selbst ausgestellte Zertifikate);
  übertragen werden nur Scan-Ergebnisse vom Drucker zu PaperRust.
- Scan-Aufträge nur im Speicher: Neustart während eines Scans → Auftrag weg (Gerät bricht ab).
- SMTP: keine Weiterleitung, keine Zustellberichte (DSN), kein `PIPELINING`, kein `CHUNKING`.
  Ohne Zertifikat Token im Klartext → nur LAN. Im Docker-Bridge-Netz sieht PaperRust oft die
  Gateway-IP statt der Drucker-IP (IP-Listen wirken dann nicht wie gedacht).
- Quelle der SMTP-Dokumente ist `ApiUpload` (nicht `ConsumeFolder`), auch wenn der Drucker vorher in
  den Eingangsordner gescannt hat; Workflows mit Quelle „Eingangsordner“ greifen nicht.
- Grenzen (Fehlversuche, Mengen) leben im Speicher je Prozess.
