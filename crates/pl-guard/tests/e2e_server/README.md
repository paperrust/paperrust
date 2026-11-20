# E2E: Schutz vor Ransomware im laufenden Server

Aufbau: `run_server.sh` (Wegwerf-DB `pl_guard_e2e` aus `paperless_snapshot`, Kopie von
`reference/data/media` mit Markierung `.paperrust-test-media`, Wächter mit Schwelle 10 und Durchsicht
alle 20 s, Sicherung in einen Ordner, Light-UI aus `frontend-light/dist`, ohne Django).

```sh
E2E_DIR=… bash run_server.sh migrate
PAPERRUST_BACKUP_PASSPHRASE='…' E2E_DIR=… bash run_server.sh manage backup init
E2E_DIR=… bash run_server.sh manage backup run
E2E_DIR=… bash run_server.sh &                       # Server auf 127.0.0.1:8141
node ../simulate-ransomware.mjs "$E2E_DIR/run/media" --count 15
```

## Ergebnis 2026-11-14 (Windows-Laptop, Debug-Build)

| Schritt | Ergebnis |
|---|---|
| `backup run` vor dem Angriff | 608 Dateien + DB-Abzug (181 KB), 12,7 s |
| `guard scan` vorher | keine Funde |
| Status/Schreiben vorher | `read_only:false`, `PATCH /api/documents/3/` → 200 |
| Angriff (15 Originale → `*.locked`, Notiz) | nach **61 s** (Wartezeit 60 s) Nur-Lese-Modus, Ereignis 1 |
| während Nur-Lese-Modus | `PATCH` → **423** `{"detail": "PaperRust is in read-only mode …", "code": "read_only_mode"}`, Upload `post_document` → 423, `GET` → 200, Anmelden (`/accounts/login/`) möglich |
| Funde | 31 Dateien: 15 `deleted`, 15 `suspicious_extension`, 1 `ransom_note`; Log `paperless.log` (ERROR `paperless.guard`) mit betroffenen Dateien; keine Mail (kein `PAPERLESS_EMAIL_HOST`) |
| Rechte | Nicht-Superuser: `events` → 403, `status` nur `read_only/since` |
| Oberfläche | rotes Banner auf jeder Seite, Seite *Schutz* mit Schritten, Ereignis und Dateiliste |
| `backup check` / `backup restore --media` | 609 Objekte vorhanden; 15 Dateien zurückgeholt, 593 schon vorhanden, alle Prüfsummen geprüft |
| nach Aufräumen | `guard scan`: keine Funde |
| Aufheben in der Oberfläche (mit Notiz) | „Read-only mode lifted“, Ereignis „Reviewed“ + Notiz, Banner weg |
| „Jetzt sichern“ in der Oberfläche | Lauf OK, 0 neue Dateien, 244 KiB (geänderter DB-Abzug) |

Beobachtet, nicht geklärt: Beim ersten Öffnen der Schutz-Seite nach der Anmeldung antworteten
`/api/x-guard/events/` und `/backup/` einmal mit 401 (Umleitung zur Anmeldung), obwohl `status` kurz
davor mit derselben Sitzung 200 gab. Nach erneuter Anmeldung nicht wieder aufgetreten (je 4 Abrufe
200). Im Testaufbau blockiert die CSP Inline-Styles, daher sitzt die Bestätigungs-Blase („Yes“) an
falscher Stelle; sie wurde per Element-Klick ausgelöst (betrifft alle Bestätigungsknöpfe dieses Aufbaus).
