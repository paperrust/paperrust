# Vertrag pl-guard – Schutz vor Ransomware

PaperRust-Erweiterung **ohne upstream-Gegenstück** (paperless-ngx v3.4.2 hat weder Wächter noch
Sicherung). Benutzer-Doku: [docs/user/ransomware.md](../user/ransomware.md) (englisch
`docs/user/en/ransomware.md`). Code: `crates/pl-guard`.

## 1. Bausteine

| Modul | Aufgabe |
|---|---|
| `detect` | ohne DB: Bereich (`originals`/`archive`/`thumbnails`), verdächtige Endungen, Erpresser-Notizen, Magic Bytes, Shannon-Entropie |
| `guard` | Wächter: Warteliste mit Wartezeit, Bewertung gegen den Katalog, Zeitfenster/Schwelle, Durchsicht (Größe+mtime), Stichprobe, `notify`-Ereignisse; Traits `Catalog`/`Reporter` (+ Speicher-Varianten für Tests) |
| `db` | `DbCatalog` (`documents_document`), `DbReporter` (Funde/Ereignis/Nur-Lese-Zustand, Alarm), `run_guard` |
| `readonly` | prozessweiter Schalter, DB-Nachführung (5 s), axum-Middleware (423), Aufgaben-Sperre `gate` |
| `backup` | Sicherung: `crypto` (age), `target` (Ordner/S3), `s3` (SigV4-Client), `dbdump` (COPY), `retention`, Lauf/Prüfen/Wiederherstellen |
| `api` | `/api/x-guard/…` |
| `cli` | `manage guard …`, `manage backup …` (eingebunden von pl-cli) |
| `store` | Tabellen `paperrust_guard_*`, `paperrust_backup_*` |

## 2. Routen

Alle mit `drf_view` (DRF-Fehlerformat, Content-Negotiation), Ids in `routes.toml` (`= "rust"`).

| Route-id | Methode | Pfad | Recht | Antwort |
|---|---|---|---|---|
| `guard.status` | GET | `/api/x-guard/status/` | angemeldet | `{read_only, since}`; Superuser zusätzlich `reason, event_id, guard_enabled, backup_configured, last_backup` |
| `guard.events.list` | GET | `/api/x-guard/events/` | aktiver Superuser | `{state, guard{enabled,threshold,window_minutes,scan_interval,sample_files,watch}, count, results[Event], recent_findings[Finding]}` (50 jüngste) |
| `guard.events.retrieve` | GET | `/api/x-guard/events/{id}/` | aktiver Superuser | `{event, findings[≤5000]}`; unbekannt/keine Zahl → 404 |
| `guard.lift` | POST | `/api/x-guard/lift/` `{"note"?}` | aktiver Superuser | `{state}`; Nebenwirkung: Zustand `read_only=false, lifted_at, lifted_by`, offene Ereignisse → `resolved` mit Notiz; Log-Warnung mit Nutzername |
| `guard.backup.status` | GET | `/api/x-guard/backup/` | aktiver Superuser | `{target, config_error, schedule, last_success, runs[≤20]}`; Läufe > 24 h „running“ → `error` („interrupted“) |
| `guard.backup.run` | POST | `/api/x-guard/backup/run/` | aktiver Superuser | 202 `{"started": true}`; ohne Ziel/ungültige Einstellung 400 `{"non_field_errors": [...]}`; Lauf im Web-Prozess (tokio-Task), parallel gesperrt per Advisory-Lock |

Nicht angemeldet → 401/403 wie alle DRF-Views (pl-authz `AuthUser`). Fehlende Rechte → 403
`{"detail": "You do not have permission to perform this action."}`. DB-Fehler → 500 ohne Interna.

`Event`: `id, created, status (open|resolved), files (Zahl verschiedener Pfade), summary, resolved_at, resolved_by, note`.
`Finding`: `id, created, event_id, path (relativ zu MEDIA_ROOT), reason, detail, weight`.
`reason`: `modified | encrypted | ransom_note | suspicious_extension | deleted | unknown_extension | bad_magic`.

## 3. Nur-Lese-Modus

- Zustand in `paperrust_guard_state` (eine Zeile, id 1); jeder Prozess liest ihn alle 5 s
  (`readonly::refresher`, gestartet in `pl-server::worker::start_background` für alle Modi) und setzt
  den Schalter sofort beim Auslösen/Aufheben im eigenen Prozess.
- **HTTP** (`readonly::middleware`, in `pl-server::build_app` außen um alle Routen inkl.
  Django-Weiterleitung): Methoden außer GET/HEAD/OPTIONS auf `/api/…` und `/rs/api/…` →
  **423 Locked** `{"detail": "<MESSAGE>", "code": "read_only_mode"}`. Ausgenommen: `/api/x-guard/`,
  `/api/token/`, `/api/ui_settings/`, `/rs/api/v2/auth/`, alles außerhalb von `/api/` (Anmeldung).
- **Worker** (`readonly::gate`, um jeden Handler in `TaskRegistry::register`): Aufgaben schlagen mit
  `ReadOnlyMode: <MESSAGE>` fehl, außer `documents.tasks.sanity_check`, `paperrust.guard.backup`,
  `documents.tasks.index_optimize`, `documents.tasks.clear_expired_sessions`. Damit sind Einlesen
  (Upload, Ordner, Mail, Scanner, SMTP-Empfang → `consume_file`), Massenbearbeitung, Neu verarbeiten
  und Papierkorb leeren gesperrt.
- Aufheben nur durch Superuser (`guard.lift`) oder `manage guard lift`.

## 4. Wächter

Läuft im Worker-Prozess (`start_background`, `runs_worker`), neu gestartet bei Änderung eines
`PAPERRUST_GUARD_*`-Schlüssels (`settings_live::restartable`). Ablauf siehe Modulkopf `guard.rs`.

Bewertung einer Datei nach `grace_secs` (60 s) – höchstens ein Fund je Datei:

| Katalog | Datei | Ergebnis |
|---|---|---|
| Dokument mit Prüfsumme | vorhanden, SHA-256 gleich | – |
| Dokument mit Prüfsumme | vorhanden, SHA-256 anders | `encrypted` (Kopf passt nicht, Entropie ≥ 7,5) sonst `modified` |
| Vorschaubild eines Dokuments | vorhanden | Kopf-/Entropieprüfung → `encrypted`/`bad_magic` |
| Dokument | fehlt | `deleted` |
| kein Dokument | vorhanden | Name: `ransom_note`/`suspicious_extension`/`unknown_extension`, sonst Inhalt `encrypted`/`bad_magic`, sonst nichts (verwaiste Datei: meldet die Integritätsprüfung) |
| kein Dokument | fehlt | – (PaperRust hat umbenannt/gelöscht) |

Systemdateien (`Thumbs.db`, `.DS_Store`, `@eaDir`, `#recycle`, `media.lock`, `._*`, `*.paperrust-tmp`)
und alles außerhalb von `documents/originals|archive|thumbnails` werden ignoriert. Dieselbe Datei mit
demselben Grund wird höchstens alle 24 h gemeldet.

Schwelle: `threshold` Funde innerhalb von `window_minutes` (gleitend, im Speicher) → `Reporter::trigger`
= Ereignis anlegen (freie Funde der letzten `window_minutes` daran hängen), Nur-Lese-Modus, Alarm
(Log-Fehler `paperless.guard`, Mail über pl-accounts SMTP an `PAPERRUST_GUARD_MAIL` oder alle aktiven
Superuser mit E-Mail, nur wenn `PAPERLESS_EMAIL_HOST` gesetzt ist). Weitere Funde während des
Nur-Lese-Modus hängen am offenen Ereignis.

## 5. Sicherung

Ablage am Ziel (Ordner oder S3, gleich aufgebaut):

```text
config.json                    {"format":1,"tool","created","recipient":"age1…"} (Klartext)
keys/repo.key.age              X25519-Schlüssel, age-scrypt mit der Passphrase
objects/<2 hex>/<sha256>       Dateiinhalt bzw. DB-Abzug (gzip), age an recipient
snapshots/<YYYYmmddTHHMMSSZ>[-n].age   gzip(JSON-Kopf + "sha\tgröße\tmtime_ns\tpfad" je Datei), age
```

Lauf (`backup::run`, Aufgabe `paperrust.guard.backup`, Zeitplan `PAPERRUST_BACKUP_CRON`, nur mit Ziel):
1. Advisory-Lock `paperrust_backup_run`; Zeile in `paperrust_backup_run`.
2. `config.json` lesen; `recipient` muss zu `DATA_DIR/backup/repo.json` passen (sonst Abbruch:
   mögliche Manipulation am Ziel).
3. DB-Abzug (`dbdump`: alle Tabellen in `public`, `COPY … TO STDOUT` in einer
   `REPEATABLE READ READ ONLY`-Transaktion, gzip) → Objekt.
4. Alle Dateien unter `MEDIA_ROOT` außer Systemdateien; Prüfsumme aus `paperrust_backup_filecache`
   (Pfad, Größe, mtime gleich) oder neu gehasht; unbekannte Objekte (`paperrust_backup_object`, zusätzlich
   `HEAD`/Datei-Existenz am Ziel) verschlüsselt hochladen; beim Verschlüsseln wird erneut gehasht –
   weicht der Inhalt ab, bricht der Lauf ab statt ein falsch benanntes Objekt abzulegen.
5. Object Lock: neue Objekte mit Sperrfrist `lock_days`; benutzte Objekte, deren Sperre in weniger als
   `lock_days/2` abläuft, per `PutObjectRetention` verlängern.
6. Verzeichnis schreiben (Ziel + Klartext-Kopie `DATA_DIR/backup/snapshots/<id>.gz`).
7. Aufräumen (nicht im Nur-Lese-Modus): Aufbewahrung 7/4/12 (jüngster Stand je Tag/ISO-Woche/Monat,
   neuester immer); gesperrte Verzeichnisse bleiben bis zum nächsten Versuch; danach nicht mehr
   referenzierte Objekte löschen – nur wenn für alle verbleibenden Stände lokale Kopien vorliegen
   und die gespeicherte Sperrfrist abgelaufen ist.

Wiederherstellen (`backup::restore`, `manage backup restore`): Passphrase → Schlüssel; Verzeichnis
laden; jede Datei entschlüsseln, SHA-256 prüfen, atomar umbenennen; unsichere Pfade (`..`, `\`, `:`)
→ Abbruch; vorhandene abweichende Dateien nur mit `--overwrite`. DB: `manage migrate`, dann
`dbdump::restore` (eine Transaktion: `SET CONSTRAINTS ALL DEFERRED`, `TRUNCATE` aller Tabellen in
`public` mit `RESTART IDENTITY`, je Tabelle `DISABLE TRIGGER USER` + `COPY … FROM STDIN` +
`ENABLE TRIGGER USER`, Sequenzen auf max+1).

S3: eigener Client (`backup::s3`) mit AWS Signature V4, Pfad-Adressierung, `x-amz-content-sha256` =
SHA-256 der Nutzlast, bei Object Lock `Content-MD5` + `x-amz-object-lock-mode` +
`x-amz-object-lock-retain-until-date`. Gelöscht wird mit `DELETE`; 403 wegen Sperrfrist gilt als
„noch gesperrt“, nicht als Fehler.

## 6. Kommandozeile

| Befehl | Wirkung |
|---|---|
| `manage guard status` | Zustand, letzte 10 Ereignisse, Funde offener Ereignisse |
| `manage guard lift [--note T]` | Nur-Lese-Modus aufheben |
| `manage guard scan` | alle Dateien und alle Dokumente sofort bewerten (ohne Wartezeit/Auslösen); Exit 1 bei Funden |
| `manage backup init [--show-key] [--passphrase-file F]` | Ziel einrichten (Passphrase ≥ 12 Zeichen aus `PAPERRUST_BACKUP_PASSPHRASE` oder Datei) |
| `manage backup run` | Lauf wie geplant (`trig = manage`) |
| `manage backup list` | Verzeichnisse, neueste zuerst |
| `manage backup check [--snapshot ID] [--key-file F]` | alle Objekte eines Stands vorhanden? |
| `manage backup restore [--snapshot ID] [--media DIR] [--database] [--overwrite] [--key-file F]` | wiederherstellen, alle Prüfsummen geprüft |

## 7. Einstellungen

21 neue Schlüssel, alle in `pl-settings` (Kategorie Integrität), `help_en` und docs/SETTINGS.md §1.12:
`PAPERRUST_GUARD`, `_THRESHOLD`, `_WINDOW_MINUTES`, `_SCAN_INTERVAL`, `_SAMPLE_FILES`, `_WATCH`,
`_EXTRA_EXTENSIONS`, `_MAIL` (A/W); `PAPERRUST_BACKUP_TARGET`, `_DATABASE`, `_KEEP_DAILY`, `_KEEP_WEEKLY`,
`_KEEP_MONTHLY`, `_S3_ENDPOINT`, `_S3_REGION`, `_S3_ACCESS_KEY`, `_S3_SECRET_KEY` (geheim), `_S3_OBJECT_LOCK`,
`_S3_LOCK_DAYS` (A/L), `_CRON` (A/W, Zeitplaner), `PAPERRUST_BACKUP_PASSPHRASE` (D, geheim, nie gespeichert).

## 8. Tests

| Test | Was | Läuft |
|---|---|---|
| Unit (22) | Erkennung, Entropie, Konfiguration, Aufbewahrung, Verzeichnis-Format, age, SigV4 gegen das AWS-Beispiel, lokales Ziel, Zeitplan-Eintrag | immer |
| `tests/simulated_attack.rs` | simulierter Angriff auf einen Temp-Medienordner: laufender Wächter mit echten Dateisystem-Ereignissen löst genau einmal aus; Durchsicht allein erkennt `.locked`/Notiz; normale PaperRust-Vorgänge lösen nichts aus; stille Verschlüsselung (gleiche Größe, mtime zurückgesetzt) findet die Stichprobe; `guard scan` | immer |
| `tests/s3_store.rs` | S3-Nachbau (`tests/support/s3mock.rs`): Signatur geprüft, Object Lock verhindert Löschen/Überschreiben, Auflisten über Seiten, Sperrfrist verlängern, `init`/Passphrase | immer |
| `tests/backup_roundtrip.rs` | Sicherung → Wiederherstellung in zweite DB + neuen Medienordner, Vergleich aller Tabellen (md5) und Dateien (SHA-256), Deduplizierung (2. Lauf 0 neu, 3. Lauf 1 neu), älterer Stand, kein Klartext am Ziel; dasselbe gegen den S3-Nachbau mit Object Lock | `#[ignore]`, braucht `PAPERRUST_GUARD_TEST_DATABASE_URL` + `_TARGET_DATABASE_URL` (Wegwerf-DBs aus `paperless_snapshot`) |
| `tests/guard_db.rs` | Wächter gegen die DB: Katalog, Auslösen, Ereignis mit 12 Funden, Aufgaben-Sperre, weiterer Fund am Ereignis, Aufheben | `#[ignore]`, wie oben |
| `tests/e2e_server/` | laufender Server: Sicherung, simulierter Angriff (Skript), Nur-Lese-Modus (423), Banner/Seite, Wiederherstellen, Aufheben in der Oberfläche – Ergebnis in `README.md` dort | von Hand (2026-11-14) |

## 9. Known differences / Grenzen

- **Kein upstream-Gegenstück**, daher keine Parität; neue Tabellen nur `paperrust_*`, expand-only.
- **S3 nur gegen den Nachbau geprüft**, nicht gegen AWS/MinIO/Synology C2: kein Download eines
  Testservers in dieser Umgebung. Die Signatur stimmt mit dem AWS-Dokumentationsbeispiel überein.
  Nicht umgesetzt: Multipart-Upload (Objekte werden ganz in den Speicher gelesen – bei sehr großen
  Einzeldateien entsprechend RAM), virtuelle-Host-Adressierung, Versionierung (bei Object Lock
  erzeugt ein Überschreiben auf echtem S3 eine neue Version; wir überschreiben nie, Namen sind
  inhaltsadressiert).
- **Objektnamen = SHA-256 des Klartexts** (wie in der Aufgabe erlaubt): wer das Ziel lesen kann, kann
  prüfen, ob eine ihm bekannte Datei enthalten ist.
- **Datenbank-Abzug ohne Schema:** Wiederherstellen setzt `manage migrate` derselben Version voraus;
  Tabellen außerhalb von `public`, Large Objects und Rollen werden nicht gesichert. `TRUNCATE`
  betrifft alle Tabellen in `public`. `DISABLE TRIGGER USER` braucht Tabellenbesitz (der App-Nutzer).
- **Verschwundene Dateien** erkennt die Durchsicht nur als Anzahl (Namen werden aus Speichergründen
  nicht gehalten); benannt werden sie über Dateisystem-Ereignisse, die Stichprobe oder `guard scan`.
- **Lokales Ziel** ist nicht vor Ransomware geschützt, wenn der befallene Rechner es beschreiben kann
  (Objekte werden nur schreibgeschützt markiert).
- Der Wächter läuft nur im Worker-Prozess; ohne Rust-Worker (Celery-Brücke) gibt es keinen Wächter,
  der Nur-Lese-Modus wirkt aber in jedem Prozess.
- scrypt der Passphrase im Debug-Build langsam (Tests nutzen Arbeitsfaktor 10); CORE-REQUEST für
  `[profile.dev.package.scrypt] opt-level = 3`.

## Ausfall-Befund C6-2 (tests/chaos, 2026-11-18)

`backup restore --database` scheiterte auf jedem System nach dem Datenmodell-Umbau mit
„TRUNCATE: cannot truncate a table referenced in a foreign key constraint“: Tabellen im Schema
`ngx_legacy` (alte guardian-Rechte) verweisen auf `auth_user`/`auth_group`/`auth_permission`. Das
Leeren nutzt jetzt `TRUNCATE … RESTART IDENTITY CASCADE` (`backup/dbdump.rs`); die Altlast-Zeilen
gehören zum ersetzten Stand. Grenze: der Datenbank-Abzug enthält nur das Schema `public`
(`ngx_legacy` wird nicht gesichert). Nachweis: `python tests/chaos/chaos.py c6` (Teil „pl-guard“:
init/run/check → frisches System → restore → Vergleich aller Tabellen und Dateien).
