# Ausfall- und Wiederherstellungs-Suite (tests/chaos)

Prüft, was passiert, wenn etwas ausfällt: Datenbank weg, Server hart beendet, Platte voll bzw.
Schreibfehler, Netz weg (Mail, Webhook, KI, Remote-OCR) – und ob Sicherung und Wiederherstellung
wirklich alles zurückbringen. Ziel je Szenario (docs/RELEASE-GATE.md Abschnitte 2 und 3):
**nichts verloren, nichts doppelt, verständliche Meldungen, Erholung ohne Handarbeit.**
Ergebnisse und Befunde: `results/release/chaos/BERICHT.md`.

Nur Python-Standardbibliothek (≥ 3.10) und Git-Bash. Alles lauscht nur auf `127.0.0.1`, alle
Zugangsdaten sind lokale Testwerte.

| Datei | Zweck |
|---|---|
| `pg.sh` | **eigene** Wegwerf-PostgreSQL (Binärdateien aus `reference/pg/pgsql/bin`, Daten in `%TEMP%/paperrust-chaos/pg`, Port 54400). Die geteilte Referenz-Instanz (54329) wird nie angefasst. `kill` = `pg_ctl stop -m immediate` (wie Absturz/Stromausfall). |
| `server.sh` | PaperRust-Testserver gegen diese Instanz (Port 58460, DB `pl_chaos_a` aus `reference/snapshot/paperless.dump`, Medien aus `data.tar`, `serve --worker`, Binary nach `%TEMP%/paperrust-chaos/bin` kopiert). `reset-empty` = frisches System für Wiederherstellungen, `manage …` = `paperrust manage` mit derselben Umgebung. |
| `chaos.py` | die Szenarien, Konsistenzprüfung, Bericht (`summary.json`, `summary.md`) |
| `imap_stub.py` | kleiner IMAP-Server mit Fehlerinjektion (offline, Verbindung reißt bei FETCH/STORE ab) |
| `dbdiff.py` | Hilfe zur Befundanalyse: Zeilen einer Tabelle zwischen zwei DBs vergleichen |

## Szenarien

| Kürzel | Inhalt |
|---|---|
| `c1` | Datenbank **hart** weg (immediate) während 12 Uploads eingelesen werden, eine Massenänderung über alle Dokumente läuft und bedient wird (Liste, Detail, PATCH, bulk_edit, Status, Aufgaben, Upload, Oberfläche); einmal 10 s, einmal 2 min (`--long-outage`). Geprüft: Antwortzeit und Meldung während des Ausfalls (503 „Datenbank nicht erreichbar“ statt 30 s Warten + 500), Erholungszeit, jede Datei genau einmal als Dokument, keine fehlgeschlagene Aufgabe, Massenänderung ganz oder gar nicht. |
| `c2` | DB-Neustart mitten in langen Aufgaben: `document_exporter` (Ordner, ZIP/lzma), `document_archiver --overwrite`, `document_index reindex`, „Neu verarbeiten“ (bulk_edit `reprocess`, Server-Aufgaben). Geprüft: klare Meldung statt Absturz, abgebrochener Export sieht nicht gültig aus, Wiederholung klappt, Server-Aufgaben scheitern nicht endgültig. |
| `c3` | „Platte voll“ als Schreibfehler: Schreibrecht auf Originale / Archiv / Vorschaubilder / Suchindex / Export-Ziel per `icacls /deny` entzogen (Windows ohne Adminrechte kann kein kleines Laufwerk einhängen). Geprüft: Meldung, keine halben Dokumente/verwaisten Dateien, Erholung nach Freigabe, Index wird nachgezogen. Echtes ENOSPC: Unit-Tests in pl-consumer. |
| `c4` | Netz weg: Mail-Abruf gegen `imap_stub` (Server aus, Abbruch bei FETCH, Abbruch bei der Mail-Aktion), Webhook-Empfänger 3 s nicht erreichbar, KI-Dienst (OpenAI-Schnittstelle) nicht erreichbar, Remote-OCR (Azure-Schnittstelle als Mock) 20 s weg. |
| `c5` | Server **hart** beendet (`taskkill /F`) in jeder Phase des Einlesens, gesteuert über Log-Marken: direkt nach Upload, Beginn, Texterkennung, Vorschaubild, Beginn Speichern, in der Transaktion, nach Commit vor Löschen des Originals, nach „consumption finished“ (Index), kurz danach. Nach jedem Abbruch Neustart, Datei genau einmal, Konsistenz. |
| `c6` | Sicherung und Wiederherstellung: Notiz, Benutzerfeld, Rechte, Workflow mit Sonst-Zweig + KI-Bedingung, Verlauf anlegen; Export in allen Arten (Ordner, mit Passphrase, geteiltes Manifest, ZIP stored/deflated/bzip2/lzma/zstd) → frische leere DB + leerer Medienordner → `migrate` → Import → Vergleich **jeder Tabelle** (Zeilen + Prüfsumme, normalisiert) und **jeder Datei** (SHA-256); danach `manage backup init/run/check` (pl-guard, Ransomware-Schutz) → frisches System → `backup restore --database --media` → gleicher Vergleich. |
| `c7` | Konsistenz (läuft nach jedem Szenario): Original/Archiv/Vorschaubild je DB-Zeile vorhanden, Prüfsummen (MD5/SHA-256) stimmen, keine verwaisten Dateien, keine Reste im Arbeitsordner, jedes Testdokument im Suchindex, Warteschlange leer, keine hängenden Aufgaben, `manage document_sanity_checker` ohne WARNING/ERROR. |

## Ausführen

```sh
cargo build -p pl-server --bin paperrust -j 2      # target/debug/paperrust.exe
bash tests/chaos/pg.sh init                         # eigene PostgreSQL auf 54400
bash tests/chaos/server.sh reset && bash tests/chaos/server.sh start
python tests/chaos/chaos.py c6 c5 c2 c3 c4 c1       # alle (ca. 45 min)
python tests/chaos/chaos.py --long-outage 30 c1     # einzeln, kürzer
# aufräumen
bash tests/chaos/server.sh drop && bash tests/chaos/pg.sh destroy
```

Optionen: `--docs` (Uploads in c1), `--long-outage` (Sekunden, Standard 120), `--kill-reps` (Abbrüche je
Phase in c5), `--export-kinds` (Liste für c6), `--out` (Ergebnisordner, Standard
`%TEMP%/paperrust-chaos-results/<zeit>`). Ein anderes Binary: `CH_BIN=<pfad> python tests/chaos/chaos.py …`.
Exit-Code 1, sobald es einen Befund gab.

Mit `c1`/`c2`/`c3`/`c5` schaltet die Suite die Testdaten-Workflows für die Dauer ab (sie schicken je
Änderung einen Webhook an `127.0.0.1:9` bzw. eine Mail, wie in tests/stress); `c4` nutzt den
Webhook-Workflow gezielt mit eigenem Empfänger.

## Grenzen

- „Platte voll“ ist unter Windows ohne Adminrechte nicht echt herstellbar (kein VHD/RAM-Laufwerk);
  der Schreibfehler per ACL trifft dieselben Codepfade, aber mit „Zugriff verweigert“ statt ENOSPC.
- Der KI-Dienst wird nur als „nicht erreichbar“ geprüft (kein echtes Modell); die KI-Nachbearbeitung
  großer Bestände (`paperrust.ai.backfill`) ist nicht abgedeckt.
- Netzunterbrechung **mitten** in einer laufenden PostgreSQL-Verbindung über ein echtes Netz (statt
  Prozessende auf demselben Rechner) ist nicht nachgestellt.
