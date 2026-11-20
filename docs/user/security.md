# Sicherheit

[English](en/security.md) · [Übersicht](README.md)

Maßstab von PaperRust: **nicht unsicherer als paperless-ngx v3.4.2**, wo möglich strenger. Rechte,
Anmeldung und Sitzungen verhalten sich wie upstream (Django/DRF/allauth/guardian nachgebaut und
gegen Django geprüft). Sicherheitslücken bitte vertraulich melden: [SECURITY.md](../../SECURITY.md).

<a id="deployment"></a>

## Betrieb

- **Nur Loopback als Standard:** PaperRust lauscht auf `127.0.0.1:8080`. Eine andere Adresse muss
  ausdrücklich in `PAPERRUST_BIND` stehen (Warnung im Log). Für Zugriff aus dem Netz einen Reverse
  Proxy mit **TLS** davorsetzen.
- **`PAPERRUST_SECRET_KEY`** lang und zufällig setzen und geheim halten; er signiert Sitzungen,
  CSRF und Freigabelinks. Gleicher Schlüssel wie bisher, wenn eine paperless-ngx-Datenbank
  weiterverwendet wird.
- **`PAPERRUST_URL`, `PAPERRUST_ALLOWED_HOSTS`, `PAPERRUST_CSRF_TRUSTED_ORIGINS`** wie bei
  paperless-ngx setzen ([Konfiguration](configuration.md#hosting-and-security)).
- **Datenbank:** eigener Benutzer mit eigenem Passwort, PostgreSQL nicht öffentlich erreichbar.
  Die Desktop-App erzeugt ein zufälliges 32-Zeichen-Passwort (SCRAM) und bindet nur an `127.0.0.1`.
- **Mehrbenutzer-Rechner:** andere lokale Benutzer erreichen `127.0.0.1` ebenfalls – die Anmeldung
  (mit MFA möglich) schützt die Oberfläche. Die Desktop-App meldet nur das eigene Windows-Konto
  automatisch an: ein Zufallsschlüssel je Start im Datenordner (nur für dieses Konto lesbar), gültig
  nur für Verbindungen von `127.0.0.1`/`::1`, Fehlversuche gedrosselt
  ([Anmeldung in der Desktop-App](installation.md#desktop-login)).
- **Proxy-Header:** Drosselungen werten `X-Forwarded-For` wie DRF aus. Den äußeren Reverse Proxy
  so einstellen, dass er fremde `X-Forwarded-For`-Werte ersetzt.

<a id="accounts"></a>

## Anmeldung und Rechte

- Passwörter mit PBKDF2-SHA256 wie Django; Anmeldedrossel, TOTP-MFA mit Replay-Sperre,
  Passwort-Zurücksetzen, OpenID Connect (State je Sitzung, PKCE, Prüfung von iss/aud/exp wie allauth).
- Rechte (global und je Objekt) werden **immer serverseitig** geprüft, auch für Suche, Vorschläge,
  KI und MCP. Filter aus Anfrageparametern können Rechte nicht umgehen.
- Fehlermeldungen und Protokolle enthalten keine Passwörter, Tokens oder Interna.

<a id="files"></a>

## Dateien, Import, ausgehende Verbindungen

- **Pfade:** Speicherpfad-Vorlagen und Dateioperationen bleiben unter den Medienordnern (Quelle und
  Ziel geprüft, `..`, `\`, NUL abgelehnt).
- **Import strenger als upstream:** Dateinamen im Manifest müssen relativ und innerhalb der
  Medienordner bleiben, verwiesene Dateien innerhalb des Importordners; Symlinks werden nicht
  verfolgt; ZIP-Grenzen gegen Zip-Bomben (`PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES`) und
  ZIP64-Prüfungen; PBKDF2-Parameter begrenzt. Exporte aus fremder Hand trotzdem nur bewusst einlesen.
- **Uploads** werden gestreamt; Größengrenzen für Nicht-Datei-Daten wie Django (2,5 MiB).
- **Ausgehende Ziele** (Webhooks, Remote-OCR, LLM-Endpunkte) werden wie upstream geprüft; Webhooks
  nur `http`/`https`.
- **Pickle** wird nirgends aus nicht vertrauenswürdigen Quellen gelesen; der Klassifikator hat ein
  eigenes Format.

<a id="ai"></a>

## KI

Dokumentinhalt gilt als nicht vertrauenswürdig; automatische KI-Verarbeitung darf nur vorschlagen
und zuordnen, hat Mengengrenzen und protokolliert jede Änderung. MCP mit Ratenbegrenzung vor der
Token-Prüfung. Details: [KI → Sicherheit](ai.md#security).

<a id="reviews"></a>

## Prüfungen

- Unabhängige Sicherheitsprüfung (Stand 2026-11-08): 0 kritisch, 2 hoch, 6 mittel, 20 niedrig; alle
  hohen und mittleren Befunde sind behoben.
- Zweite unabhängige Code-Prüfung vor der Freigabe (2026-11-19,
  [Bericht](../../results/release/review/security-2026-11-19.md)): keine kritischen oder hohen
  Befunde; die mittleren und niedrigen Befunde sind behoben.
- Sicherheits-Testsuite gegen Django und PaperRust: `tests/security/` (Authentifizierung, CSRF,
  Header, Datenlecks, Rechte, Pfad-Traversal, Ratenbegrenzung).
- Paritätstests: gleiche Statuscodes und Antworten wie Django, auch für verweigerte Zugriffe.
