# Last- und Gleichzeitigkeits-Suite (tests/stress)

Findet Fehler, die nur unter Last oder bei gleichzeitigen Aufrufen auftreten (Deadlocks, erschöpfter
Verbindungspool, hängende Aufgaben, verlorene/doppelte Dokumente, Speicherwachstum). Die übrigen Tests
prüfen fast nur einzelne, ruhige Aufrufe. Befunde des ersten Laufs: [BEFUNDE.md](BEFUNDE.md).

Nur Python-Standardbibliothek (≥ 3.10), kein `pip install`.

| Datei | Zweck |
|---|---|
| `server.sh` | Wegwerf-Testserver (Port 58360, DB `pl_stress_a` aus `paperless_snapshot`, Arbeitsordner `%TEMP%/paperrust-stress`, Worker an, PDFium/Tesseract aus der lokalen Desktop-Installation) |
| `stress.py` | die Szenarien, Konsistenzprüfung, Bericht (`summary.md`, `summary.json`, `requests.jsonl`) |

## Szenarien

| Kürzel | Inhalt | wichtige Optionen |
|---|---|---|
| `s1` | 200+ Dateien (Text-PDFs, jedes 40. ein PNG) hochladen; **während** des Einlesens und **direkt danach** `bulk_edit` über alle Dokumente: `add_tag`, `remove_tag`, `modify_tags`, `set_correspondent`, `set_permissions`, seriell schnell hintereinander und 2/4/8 gleichzeitig; „Tag an alle“ 30× abwechselnd add/remove (wie der Jetson-Leistungstest); `delete` + Wiederherstellen aus dem Papierkorb parallel zu einem `add_tag`; am Ende „add_tag an alle“ und Prüfung, dass wirklich jedes Dokument den Tag hat | `--docs` |
| `s2` | 10–30 Nutzer gleichzeitig, gemischt: Liste, tiefe Seiten, Filter, Volltextsuche, globale Suche, Autocomplete, Detail, Vorschaubild, Vorschau, Metadaten, Notizen, Statistik, `selection_data`, PATCH Titel/Tags, Notiz anlegen, Upload | `--users`, `--duration`, `--think` |
| `s3` | ein frisch eingelesenes Dokument wird von mehreren Clients gleichzeitig per PATCH geändert (Titel, Tags, Korrespondent/Typ, Notizen), ein Workflow „Dokument geändert“ setzt einen Tag, parallel werden neue Versionen hochgeladen (`update_version`) | `--same-doc-clients`, `--same-doc-seconds`, `--versions` |
| `s4` | 20 Uploads gleichzeitig, dieselbe Datei dreifach, große Datei (Standard 60 MB), kaputtes und abgeschnittenes PDF, leere Datei, `.exe` – keine 5xx, jede Aufgabe endet, Fehlschläge mit Grund | `--parallel-uploads`, `--big-mb` |
| `s5` | 30 Dateien hochladen, Server während der Verarbeitung **hart** beenden (`taskkill /F`), neu starten – jede Aufgabe endet erfolgreich, jede Datei genau einmal als Dokument | `--restart-docs`, `--kill-cmd`, `--start-cmd` |
| `s6` | Dauerlauf (Standard 30 min) mit gemischter Last + jede Minute „Tag an alle“ und ein Upload; alle 30 s Speicher (Working Set, private Bytes), Handles, Threads des Serverprozesses und DB-Verbindungen; Steigung der zweiten Hälfte > 2 MB/min = Befund | `--soak-minutes`, `--soak-users`, `--sample-every` |
| `s7` | Konsistenz (läuft nach **jedem** Szenario automatisch): Anzahl, Detail + Vorschaubild jedes Dokuments, jedes eingelesene Text-PDF über sein eindeutiges Suchwort im Index (genau ein Treffer), lokal zusätzlich: Original/Archiv/Vorschaubild jeder DB-Zeile vorhanden, keine verwaisten Dateien, `paperrust_task_queue` leer, keine hängende oder grundlos fehlgeschlagene `PaperlessTask` | `--full-files` (lädt jedes Original) |
| `s8` | Abbrechende Clients (BEFUNDE B10): Dokumente einlesen, Workflow „Dokument geändert → Tag“ anlegen, dann in Runden PATCH (neuer Titel, Tags leeren) und `bulk_edit` (`modify_tags`: Workflow-Tag weg) auf eigener Verbindung senden und nach 0–400 ms **hart abbrechen** (RST, Antwort nie gelesen). Danach: jedes Dokument, dessen neuer Titel gespeichert wurde, ist über den Titel im Suchindex zu finden, und jedes Dokument trägt wieder den Workflow-Tag (Workflow bzw. Nacharbeit der Massenänderung lief trotz Abbruch) | `--abort-docs`, `--abort-rounds` |

Ein Hintergrund-Thread („LockWatch“, nur mit DB-Zugang) meldet alle 2 s Sitzungen, die länger als 5 s auf
eine Zeilensperre warten, mit der blockierenden Sitzung. So werden Verklemmungen sichtbar, die
PostgreSQL nicht als Deadlock erkennt (eine Transaktion wartet in Rust auf eine Dateisperre oder eine
zweite Pool-Verbindung).

**Bewertung:** jede 5xx-Antwort, jede Zeitüberschreitung (Standard 60 s je Anfrage) bzw. jeder
Verbindungsabbruch und jede Inkonsistenz ist ein Fehler. 4xx nur, wenn der Aufruf ihn fachlich nicht
erwarten lässt (die erwarteten Codes stehen je Aufruf im Skript, z. B. 400/415 für leere Datei/`.exe`).
Exit-Code 1, sobald es einen Fehler oder Befund gab.

## Ausführen (lokal)

Git-Bash im Repo. Das Binary wird nach `%TEMP%/paperrust-stress/bin` kopiert (ein laufendes `.exe`
sperrt sonst `target/debug/paperrust.exe` für andere Builds).

```sh
cargo build -p pl-server --bin paperrust -j 2          # falls target/debug/paperrust.exe fehlt/alt
bash tests/stress/server.sh start                       # DB pl_stress_a + Daten aus dem Snapshot, Worker an
python tests/stress/stress.py --local s1                # einzelnes Szenario
python tests/stress/stress.py --local s1 s4 s3 s5 s2    # Standardfolge (ca. 30 min)
python tests/stress/stress.py --local --soak-minutes 45 s6
python tests/stress/stress.py --local --repeat 3 s3     # jedes Szenario mehrfach
bash tests/stress/server.sh drop                        # Server stoppen, DB und Arbeitsordner löschen
```

Ergebnis: `%TEMP%/paperrust-stress-results/<lauf>/summary.md` (oder `--out <ordner>`): je Szenario
Anfragen je Operation mit HTTP-Codes, p50/p95/max, Fehlerbeispiele, Befunde, gruppierte WARN/ERROR-Zeilen
aus dem Server-Log seit Szenariobeginn, Sperr-Wartezeiten, Konsistenzzahlen.

Umgebung von `server.sh`: `ST_PORT` (58360), `ST_DB` (muss mit `pl_stress_` beginnen), `ST_WORK`,
`ST_BIN` (Quelle des Binaries), `ST_WORKERS` (Task-Worker, Standard 2), `ST_TOOLS` (Ordner mit
`pdfium/pdfium.dll` und `tesseract/`, Standard `%LOCALAPPDATA%/Programs/PaperRust`) bzw. direkt
`PAPERRUST_PDFIUM_PATH`, `PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`. Ohne PDFium scheitert
jedes PDF mit „PDFium not loaded“.

Mit `--local` schaltet die Suite die vorhandenen Workflows für die Dauer des Laufs ab und danach wieder
an (`--keep-workflows` verhindert das): die Testdaten-Workflows senden je geändertem Dokument einen Webhook
an `127.0.0.1:9` bzw. eine Mail und fluten sonst die Warteschlange (BEFUNDE B6). `s3` legt einen eigenen
Workflow „Dokument geändert“ an und löscht ihn am Ende.

Mit `--local` meldet sich die Suite mit dem Test-Token des Referenz-`admin` an
(`reference/users.json`): HTTP-Basic kostet je Anfrage eine PBKDF2-Prüfung (Debug-Build ≈ 140 ms) und
würde die Last verfälschen.

## Gegen einen entfernten Server (per URL)

Nur gegen Testinstanzen: die Suite legt Dokumente, Tags, Korrespondenten und einen Workflow an, ändert
Titel/Tags/Rechte vieler Dokumente und löscht/restauriert Dokumente.

```sh
python tests/stress/stress.py --url http://jetson.local:8000 --token <API-Token> s1 s2 s3 s4
python tests/stress/stress.py --url http://jetson.local:8000 --user admin --password <testpasswort> s1
```

Ohne `--local` gibt es keine Datei-/DB-Prüfung, keinen Server-Log-Auszug und kein `s5`. Nachrüsten:
`--log-dir` (Ordner mit `paperrust.log`), `--media` (Medienordner), `--psql <psql> --dsn "host=… dbname=…"`
(nur lesende Abfragen), `--pid` (Speicherproben, nur auf demselben Rechner), `--kill-cmd`/`--start-cmd`
(z. B. `"ssh jetson docker kill pr"` / `"ssh jetson docker start pr"`) für `s5`.

Belastung dosieren: `--docs 100 --users 10 --duration 120` für einen schnellen Lauf; die Standardwerte
sind für einen geteilten Laptop gewählt (20 Nutzer, 4 parallele Uploads im Import, 2 Task-Worker).
