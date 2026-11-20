# Befunde der Last- und Gleichzeitigkeits-Suite (2026-11-18)

Stand: Binary `target/debug/paperrust.exe` vom 2026-11-18 00:50 (Debug-Build, entspricht für die
betroffenen Dateien `pl-documents-write/src/{bulk_edit,views}.rs`, `pl-consumer/src/{bulk,consumer,db}.rs`
dem Commit 4e6a58d; ein frischer Build scheiterte, weil `pl-ai` gerade von einem anderen Agenten
umgebaut wird). Server: `tests/stress/server.sh` (DB `pl_stress_a` aus `paperless_snapshot`, 2
Task-Worker, Pool-Standard 10 Verbindungen), Windows-Laptop, PostgreSQL 18 lokal.
Ablauf und Rohzahlen: Abschnitt [Läufe](#läufe) am Ende.

Kein Produktcode geändert. Jede Ursache ist im Code nachgelesen und – wo angegeben – durch das
PostgreSQL-Log bzw. `pg_stat_activity` während des Fehlers belegt.

## Übersicht

| Nr. | Schwere | Kurz | Szenario | Häufigkeit |
|---|---|---|---|---|
| B1 | **kritisch** | Verklemmung Dateisperre (MEDIA_LOCK) ↔ DB-Zeilensperre: Anfragen/Aufgaben hängen, bis ein Client aufgibt (sonst unbegrenzt) | s3, s1 | s3 mit Versionen: jeder Lauf; s1: mehrfach je Lauf |
| B2 | **kritisch** | Verbindungspool erschöpft sich selbst: 4 von 10 Verbindungen sind dauerhaft LISTEN-Verbindungen, PATCH (u. a.) hält eine Verbindung und holt eine zweite → ab ~6 gleichzeitigen Änderungen 30 s Stillstand und HTTP 500 für **alle** Anfragen; Worker verliert Heartbeat/Lease | s3, s2 | s3 mit ≥ 6 Clients: jeder Lauf; s2 mit 30 Nutzern: 35× 500 in 2 min |
| B3 | **hoch** | Falsche Parametertypen aus dem Statement-Cache: gleiches SQL mit `int4` (Consumer) und `int8` (bulk_edit) → `insufficient data left in message` (Import schlägt fehl) bzw. `incorrect binary data format in bind parameter 1` (bulk_edit 400) | s1 | Lauf 0: 11 Importe (Hauptlauf: 1) fehlgeschlagen, 19× bulk_edit 400 |
| B4 | **hoch** | **Ursache der 6 Fehler „Tag an alle“ (Jetson):** Deadlock zwischen `bulk_edit` (Trigger sperrt Dokumentzeilen nach id) und der Nacharbeit `bulk_update_documents` des vorigen Aufrufs (`UPDATE … SET modified … WHERE id = ANY($1)` sperrt in Tabellenreihenfolge) → PostgreSQL bricht nach `deadlock_timeout` (1 s) ab → HTTP 400 | s1 | Jetson 6/30; Laptop: Lauf 0 1×, Hauptlauf 4× (davon 1 in den 30 seriellen Aufrufen) |
| B5 | mittel | Parallele `add_tag` auf dieselben Dokumente: `duplicate key … paperrust_document_tag_pkey` → HTTP 400; im Hintergrundweg (≥ 500 Dokumente) **stilles** Scheitern nach HTTP 200 | s1 | Lauf 0: 52 von 991 bulk_edit; Hauptlauf: 47, davon 10 als fehlgeschlagene Hintergrundaufgabe |
| B10 | **hoch** | Bricht der Client ab (Timeout, Seite verlassen, Proxy-Timeout), verwirft der Server die laufende Anfrage mitten im Handler: Änderung gespeichert, aber Workflow „Geändert“, Suchindex, WebSocket nicht ausgeführt | s3 | Zusatzlauf: 1 von 1 (Workflow-Tag fehlt) |
| B6 | mittel | Jede Massenänderung erzeugt eine eigene `bulk_update_documents`-Aufgabe über alle Dokumente; zusammen mit Workflows „Dokument geändert“ (Webhook je Dokument) wächst die Warteschlange schneller, als sie abgearbeitet wird | s1 | Lauf 0: 697 + 3463 wartend; Hauptlauf (ohne Workflows): 129 bzw. 391 wartend |
| B7 | mittel | Nach hartem Abbruch wird eine schon committete Einlese-Aufgabe erneut ausgeführt und endet „fehlgeschlagen: File not found“, obwohl das Dokument existiert | s5 | 1 von 30 |
| B8 | mittel (Windows) | Suchindex-Schreibfehler unter Last (`os error 5`, `FileAlreadyExists(.del)`), `index_document` scheitert ohne Wiederholung | s2, s6 | 4 in 5 min s2, 5 in 30 min s6 |
| B9 | niedrig | Privater Speicher wächst im 30-min-Dauerlauf 356 → 449 MB (Bestand wuchs mit) | s6 | beobachten |

Details, Belege und Vorschläge: unten, danach „Ohne Befund geprüft“, „Nicht geprüft“ und die Läufe.

---

## B1 – Verklemmung MEDIA_LOCK ↔ Zeilensperre (kritisch)

**Beobachtung.** s3 (3 Clients ändern ein Dokument per PATCH, parallel 2 neue Versionen hochgeladen):
alle drei PATCH hängen 60 s bis zum Client-Timeout (`status=499 … outcome="client closed"`),
danach geht es weiter. LockWatch während des Hängers:

```
pid 20796 wartet 58s auf pid 23396 (idle in transaction): SELECT "id","title","content",… (letzte Abfrage)
    wartend: UPDATE documents_document SET "modified" = $2 WHERE id = $1
pid 22004 wartet 58s auf pid 20796: wartend: UPDATE documents_document SET modified = $2, filename = CASE …
pid 18764 wartet 44s auf pid 20796/22004: wartend: SELECT "id" FROM "paperrust_document" WHERE "id" = $1 … FOR UPDATE
```

In s1 (nach dem Import, bulk `add_tag` + `delete`) dasselbe Muster mit einer Worker-Transaktion als
„idle in transaction“-Halter (zwei bulk_edit-Anfragen 499 nach 60 s).

**Ursache.** `MediaLock` (prozessweite Dateisperre wie upstream `FileLock(MEDIA_LOCK)`, wartet **ohne
Zeitlimit**, `pl-documents-write/src/files.rs:121-145`, `pl-consumer/src/consumer.rs:1753-1771`) wird an
manchen Stellen **innerhalb einer offenen DB-Transaktion mit Zeilensperren** geholt, an anderen Stellen
**vor** den Zeilensperren. Umgekehrte Sperrreihenfolge zweier verschiedener Sperrarten – PostgreSQL
sieht nur eine „idle in transaction“-Sitzung und erkennt keinen Deadlock:

1. `Consumer::store_all` (`pl-consumer/src/consumer.rs:888` `pool.begin()`): bei einer neuen Version
   sperrt `create_version` das Stammdokument (`SELECT … FROM paperrust_document … FOR UPDATE`,
   `consumer.rs:1160-1170`), danach `MediaLock::acquire` (`consumer.rs:906`) – **Zeile → Dateisperre**.
   Gleiches Muster beim normalen Einlesen nach `consumption_finished` und in `reprocess.rs:161/222`.
2. `docs::save(…, Some(media), …)` ruft `files::update_filename_and_move_files` (`docs.rs:135-136`)
   → `MediaLock` – auch wenn der Aufrufer in einer Transaktion steckt: bulk `delete`
   (`views.rs:104` bzw. `tasks.rs:64` → `softdelete::soft_delete_inner` → `docs::save`, je Dokument,
   während die vorherigen Dokumente schon gesperrt sind).
3. `update_filename_and_move_files` → `update_one` (`pl-documents-write/src/files.rs:229-245`) holt
   **zuerst** `MediaLock` und schreibt **dann** `UPDATE documents_document SET modified …, filename = CASE …`
   (`files.rs:340-352`) – **Dateisperre → Zeile**. Ebenso `pl-consumer/src/files.rs:108/169/260`.

Ablauf im Beleg: Worker-Transaktion A (neue Version) hält die Zeile des Stammdokuments und wartet
auf MEDIA_LOCK; Anfrage B (PATCH → post_save) hält MEDIA_LOCK und wartet auf die Zeile; weitere
PATCH warten auf B. Aufgelöst wird das nur, weil der HTTP-Client nach 60 s abbricht und axum die
Future von B verwirft (Sperre frei, Transaktion zurückgerollt). **Sind nur Worker-Aufgaben beteiligt
(z. B. bulk_update_documents + Einlesen), hängt es unbegrenzt**; die betroffenen Aufgaben verlieren
dann ihre Lease und werden erneut ausgeführt (siehe B2).

Upstream hat dieselbe Struktur (`transaction.atomic()` + `FileLock` im Consumer), aber dort laufen die
Änderungen per PATCH ohne umgebende Transaktion und der Zeitraum ist kürzer; in PaperRust ist es in
Sekunden reproduzierbar (s3 mit `--versions 2`, jeder Lauf).

**Vorschlag.**
- Feste Sperrreihenfolge: MEDIA_LOCK **immer vor** der DB-Transaktion holen (in `store_all`
  vor `pool.begin()`, in bulk delete/soft delete vor dem Transaktionsbeginn einmal für alle Dokumente),
  nie innerhalb. `docs::save(…, Some(media))` in Transaktionen verbieten (Dateinamen nach dem Commit
  nachziehen, wie upstream `post_save` außerhalb von `atomic`).
- `MediaLock::acquire` mit Zeitlimit (z. B. 30 s) und Fehler statt Endlosschleife; zusätzlich
  `SET LOCAL lock_timeout = '30s'` in langen Transaktionen, damit ein Hänger zumindest endet.
- Langfristig: In-Process-Sperre (tokio `Mutex`) für MEDIA_LOCK zusätzlich zur Dateisperre, damit
  sich die eigenen Tasks nicht im 50-ms-Takt abfragen.

## B2 – Verbindungspool erschöpft sich selbst (kritisch)

**Beobachtung.** s3 ohne Versionen, 8 Clients PATCH auf dasselbe Dokument: nach wenigen Sekunden
antwortet der Server 30 s lang gar nicht, dann `HTTP 500` (HTML „Server Error (500)“) – auch für
einfache `GET /api/documents/<id>/`. Server-Log: `pool timed out while waiting for an open connection`
(24× `acquired connection, but time to acquire exceeded slow threshold`). Keine Sperr-Wartezeiten in
PostgreSQL. `pg_stat_activity` während des Stillstands: genau 10 Verbindungen, 4 davon
`LISTEN "paperrust_settings" | "paperrust_sync_pending" | "paperrust_task_queue" | "paperrust_status"`,
6 im Zustand `idle` (gehalten von Rust, keine Transaktion). Mit Versions-Uploads zusätzlich:
`pl_queue::worker: heartbeat timed out`, `lease lost (another worker owns the task now); execution
aborted`, `pl_queue::pg: worker lost, task requeued`, `claiming tasks failed … pool timed out`.

**Ursache.**
1. `sqlx::postgres::PgListener::connect_with(pool)` nimmt eine Verbindung **aus dem Pool** und behält
   sie dauerhaft (sqlx 0.8.6 `listener.rs:59-62`). Vier (mit Sync fünf) Listener:
   `pl-queue/src/events.rs:395`, `pl-queue/src/worker.rs:323`, `pl-settings/src/reload.rs:262`,
   `pl-sync/src/sequencer.rs:117` (+ `pl-sync/src/lib.rs:607`). Pool-Standard 10
   (`pl-core/src/state.rs:217-243`, `PAPERLESS_DB_POOLSIZE`/`PAPERRUST_DB_MAX_CONNECTIONS`) → nur
   **6** Verbindungen für alle Web-Anfragen **und** den Worker (Aufgaben, Heartbeats, Claim).
2. Verschachteltes Holen: `update()` (`pl-documents-write/src/update.rs:179`) hält `conn` bis zum Ende
   und ruft dabei `pl_documents::serialize_documents(state, …)` (`update.rs:230`, holt über
   `state.db()` je Abfrage eine weitere Verbindung), `index::add_or_update(state, …)`,
   `hooks::document_updated(state, …)`. Ebenso `bulk_edit_inner` mit `all: true`
   (`selection.rs:118-123` → `resolve_all(state, …)` bei gehaltenem `conn`). Sind alle freien
   Verbindungen von Anfragen belegt, die auf ihre zweite warten, steht alles bis `acquire_timeout`
   (sqlx-Standard 30 s) → 500.
3. Der Worker teilt den Pool: Heartbeat/`complete` bekommen keine Verbindung → Lease verloren →
   laufende Aufgabe wird abgebrochen (`worker.rs:398-411, 436-438`) und neu eingereiht. Bei
   `consume_file` kann das zu doppelter Arbeit führen; bei einer Aufgabe, die schon committet hat,
   wird das Ergebnis verworfen (`lease lost before completion; result discarded`).

Django hat eine Verbindung je Anfrage-Thread, das Problem gibt es dort nicht.

**Vorschlag.** Listener auf eigene, nicht gepoolte Verbindungen (`PgListener::connect(url)` bzw. ein
eigener Mini-Pool) oder Poolgröße = gewünschte Nutzlast + Anzahl Listener; Worker (Heartbeat/Claim) mit
eigenem kleinen Pool; in Handlern keine zweite Verbindung holen, solange eine gehalten wird
(`serialize_documents_db(&mut *conn, …)` bzw. `conn` vor dem Serialisieren freigeben); Test: 20
gleichzeitige PATCH gegen Pool 10 müssen ohne 500 durchlaufen (Szenario s3 `--same-doc-clients 8
--versions 0`).

## B3 – Statement-Cache mit unterschiedlichen Parametertypen (hoch)

**Beobachtung.** s1 (Import + bulk_edit): Import-Aufgaben scheitern mit
`The following error occurred while storing document stress_…_00065.pdf after parsing: error returned
from database: insufficient data left in message`; bulk_edit antwortet 400, Log:
`Uncaught("database: error returned from database: incorrect binary data format in bind parameter 1")`.
PostgreSQL-Log (`reference/logs/postgres.log`) für alle 30 Fälle:

```
ERROR:  insufficient data left in message | incorrect binary data format in bind parameter 1
CONTEXT:  unnamed portal parameter $1
STATEMENT:  INSERT INTO "documents_document_tags" ("document_id", "tag_id") VALUES ($1, $2), ($3, $4), ($5, $6)[, …]
```

**Ursache.** sqlx cacht vorbereitete Anweisungen je Verbindung **nach SQL-Text**. Derselbe Text wird mit
verschiedenen Rust-Typen gebunden:
- `pl-consumer/src/db.rs:444-450` (Tags beim Einlesen): `values_panic([(doc_id as i32).into(), (*id as i32).into()])` → `int4`
- `pl-documents-write/src/bulk_edit.rs:589-603` `insert_tag_rows` (add_tag): `(*d).into(), (*t).into()` mit `i64` → `int8`

Bei gleicher Zeilenzahl (1–6 Paare – genau das, was `add_tag` liefert, wenn nur die frisch
eingelesenen Dokumente den Tag noch nicht haben) ist das SQL identisch. Hat eine Pool-Verbindung die
Anweisung zuerst mit `int4` vorbereitet, schickt bulk_edit 8 Byte → „incorrect binary data format“;
umgekehrt schickt der Consumer 4 Byte an eine `int8`-Anweisung → „insufficient data left“. Welche
Verbindung was zuerst vorbereitet hat, ist Zufall → tritt nur unter gemischter Last auf, typisch
**kurz nach einem großen Import**. Der Import-Fehler verliert das Dokument (Aufgabe FAILURE).

**Vorschlag.** Einheitliche Typen je Tabelle/Spalte (die Spalten sind `integer` → überall `i32`
binden) oder dynamisch gebaute sea-query-Anweisungen mit `.persistent(false)` ausführen. Weitere
Kandidaten suchen: gleiche sea-query-Konstrukte in mehreren Crates mit `i64` vs `i32`
(z. B. `id IN (…)`-Listen, `documents_document_tags`, `paperrust_document`-Updates). Ein Test, der
alle über `build_sqlx` erzeugten Anweisungen zweier Pfade auf derselben Verbindung ausführt, findet das.

## B4 – Deadlock „Tag an alle“ nach dem Import (hoch, Ursache der 6 Jetson-Fehler)

**Beobachtung Jetson** (`results/benchmark/2026-11-18-jetson/raw/auffaellig_bulk_edit_rs_runde1.txt`):
30× `bulk_edit` add/remove abwechselnd über 255 Dokumente; 6× `status=400 ms≈1013`, jeweils
`An error occurred performing bulk edit: Uncaught("database: … deadlock detected")`, zeitgleich
`slow statement … UPDATE paperrust_document SET modified = $2 WHERE id = ANY($1) AND deleted_at IS NULL
rows_affected=255 elapsed=1.06s` und `bulk_update_documents: signals done in 1080 ms documents=255`.
**Lokal reproduziert** (s1), PostgreSQL-Log:

```
ERROR:  deadlock detected
DETAIL:  Process 22004 waits for ShareLock on transaction 2589212; blocked by process 19384.
         Process 19384 waits for ShareLock on transaction 2589213; blocked by process 22004.
         Process 22004: INSERT INTO "documents_document_tags" ("document_id", "tag_id") VALUES …
         Process 19384: UPDATE paperrust_document SET modified = $2 WHERE id = ANY($1) AND deleted_at IS NULL
CONTEXT:  while locking tuple (63,104) in relation "paperrust_document"
```

**Ursache.** Zwei Transaktionen sperren dieselben ~250 Dokumentzeilen in **verschiedener Reihenfolge**:
1. `bulk_edit` (`views.rs:93-150` `run_method`, eine Transaktion) → `INSERT/DELETE documents_document_tags`
   → Anweisungs-Trigger `paperrust_sync_cap_paperrust_document_tag_{ins,del}` → `paperrust_sync_bump(1, ids)`
   sperrt die Dokumente **sortiert nach id** (`SELECT … ORDER BY y.id FOR NO KEY UPDATE`,
   `pl-sync/migrations/0003_paperrust_sync_native.sql:61-64`).
2. Die Aufgabe `bulk_update_documents` **des vorigen** bulk_edit (läuft im Worker, sobald der vorige
   Aufruf committet hat) → `Consumer::bulk_update_signals` → `bulk_block` →
   `UPDATE paperrust_document SET modified = $2 WHERE id = ANY($1)` (`pl-consumer/src/bulk.rs:408-414`)
   sperrt in **physischer Tabellenreihenfolge** (nach vielen Updates ≠ id-Reihenfolge).

Bei schnell aufeinanderfolgenden Massenänderungen überlappen sich beide → Deadlock; PostgreSQL bricht
nach `deadlock_timeout` (1 s, daher die ≈1013 ms) die bulk_edit-Transaktion ab, `run_method` macht
daraus 400 „Error performing bulk edit, check logs for more detail.“ Die Änderung ist dann **nicht**
gespeichert. Mit dem Import hat es nur indirekt zu tun: direkt danach sind die Dokumente frisch
geschrieben (Tabellenreihenfolge durcheinander) und das Messprogramm ruft die Massenänderung 30× ohne
Pause auf. ngx hat keinen solchen Trigger und `bulk_update_documents` aktualisiert dort einzeln
(autocommit, je Dokument), daher 0 Fehler.

Gleiches Muster (unsortiertes Mehrzeilen-UPDATE auf `paperrust_document`, gleichzeitig mit dem
sortierenden Trigger): `pl-documents-write/src/docs.rs:151` (`touch_modified`), `cfi.rs:534`,
`bulk_edit.rs:736` (`set_correspondent/…`: `UPDATE paperrust_document SET <fk> … WHERE id IN (…)`),
`bulk_edit.rs:791` (`set_permissions`).

**Vorschlag.**
- Alle Mehrzeilen-Updates auf `paperrust_document` in id-Reihenfolge sperren, z. B.
  `UPDATE paperrust_document d SET modified = $2 FROM (SELECT id FROM paperrust_document WHERE id = ANY($1)
  AND deleted_at IS NULL ORDER BY id FOR NO KEY UPDATE) s WHERE d.id = s.id`
  (Fundstellen oben, am besten als gemeinsame Hilfsfunktion).
- Zusätzlich `run_method` und `bulk_update_documents` bei SQLSTATE `40P01`/`40001` 2–3× wiederholen
  (die Transaktion ist vollständig und idempotent).
- Optional: aufeinanderfolgende `bulk_update_documents` derselben Dokumente zusammenfassen (B6).

## B5 – Paralleles add_tag: duplicate key (mittel)

**Beobachtung.** s1 mit 2–8 gleichzeitigen `add_tag` auf dieselben Dokumente: 400,
`duplicate key value violates unique constraint "paperrust_document_tag_pkey"` (Lauf 0: 52×, Hauptlauf 47×).
**Ursache.** `execute_tag` (`bulk_edit.rs:826-860`) liest „Dokumente ohne Tag“ und fügt dann ohne
`ON CONFLICT` ein (`insert_tag_rows(conn, &pairs, false)`, Zeile 858). Die zweite Transaktion wartet
auf den Eindeutigkeitsindex und scheitert nach dem Commit der ersten. Upstream (`bulk_create` ohne
`ignore_conflicts`) hat dieselbe Lücke, aber ohne umgebende Transaktion mit Audit-Momentaufnahmen ist
das Fenster dort Millisekunden statt der ganzen Anfrage. **Vorschlag:** `ignore_conflicts = true`
(wie `modify_tags`, Zeile 785) – das Ergebnis ist identisch („Tag ist gesetzt“); als bewusste
Abweichung im Vertrag notieren. Besonders unangenehm im Hintergrundweg (`bulk_task`, ab
`async_min_docs` = 500 Dokumenten): die Anfrage antwortet 200 mit `task_id`, die Aufgabe
`documents.bulk_operations.execute_bulk_operation` scheitert dann still mit demselben Fehler
(Hauptlauf: 10 Aufgaben).

## B10 – Abgebrochene Verbindung bricht den Handler mitten in der Arbeit ab (hoch)

**Beobachtung.** Zusatzlauf s3 (4 Clients PATCH + 3 Versionen, eigener Workflow „Dokument geändert →
Tag `stress-wf-updated`“): vier PATCH hängen durch B1, der Client gibt nach 60 s auf
(`status=499 … outcome="client closed"`). Danach fehlt der Workflow-Tag am Dokument
(`tags=[2, 3, 20]`), obwohl jede erfolgreiche Änderung den Workflow auslöst.
**Ursache.** hyper/axum verwerfen die Handler-Future, sobald der Client die Verbindung schließt
(sichtbar am `AccessGuard`, `pl-server/src/frontline.rs:126-160`). Der PATCH-Handler
(`pl-documents-write/src/update.rs:147-256`) besteht aus mehreren einzeln committeten Schritten:
`apply_update` (gespeichert) → Serialisieren → `workflows::run_updated` → WebSocket → Suchindex. Wird
die Future nach dem Speichern verworfen, laufen Workflows, Index und Benachrichtigung nie. Gleiches gilt
für jeden schreibenden Handler mit mehreren Schritten (bulk_edit nach dem Commit: `p.publish` der
Nacharbeit; Upload; Notizen). Django arbeitet eine Anfrage immer zu Ende.
**Vorschlag.** Schreibende Handler vom Verbindungsleben entkoppeln (`tokio::spawn` des Handler-Körpers
und auf das `JoinHandle` warten, oder eine Tower-Schicht, die mutierende Methoden nicht abbrechen lässt);
Nacharbeiten (Workflow, Index) zusätzlich als Aufgabe in derselben Transaktion einreihen.

## B6 – Warteschlange wächst bei vielen Massenänderungen (mittel)

**Beobachtung.** Lauf 0 von s1 (≈ 1000 bulk_edit in 8 min, Testdaten-Workflow „Notify on update“
aktiv): 697 `bulk_update_documents` und 3463 `send_webhook` warten, Abbau ≈ 10 Aufgaben/min →
Warteschlange über eine Stunde voll, Suchindex/Dateinamen entsprechend lange veraltet.
**Ursache.** Jede Massenänderung reiht eine `bulk_update_documents` über **alle** betroffenen Dokumente
ein (upstream genauso); jede führt die Workflows „Dokument geändert“ je Dokument aus (hier: Webhook
an `127.0.0.1:9` mit Wiederholungen). Kein Zusammenfassen gleicher, noch nicht gestarteter Aufgaben.
**Vorschlag.** Noch wartende `bulk_update_documents` mit überlappenden ids zusammenführen (Debounce);
Fortschritt in der Oberfläche anzeigen. Kein Upstream-Fehler, aber auf dem Jetson spürbar.

## B7 – Nach hartem Abbruch: Einlese-Aufgabe „fehlgeschlagen“, obwohl das Dokument da ist (mittel)

**Beobachtung.** s5: 30 Uploads, Server nach 6 fertigen Aufgaben per `taskkill /F` beendet, neu
gestartet. 29 Aufgaben enden erfolgreich, jede Datei ist genau einmal als Dokument vorhanden, keine
fehlenden/verwaisten Dateien. Aber: zwei laufende Aufgaben werden erst nach Ablauf der Lease (≈ 60 s)
neu ausgeliefert (`pl_queue::pg: worker lost, task requeued`); eine davon (`stress_…_00432.pdf`) hatte
im ersten Versuch schon committet (Dokument 638, `added` 05:05:20, Abbruch 05:05:20) und scheitert
im zweiten mit `Cannot consume …\scratch\tmp…\stress_…_00432.pdf: File not found.` → Aufgabe
`FAILURE`, die Oberfläche meldet einen Fehler für ein vorhandenes Dokument.
**Ursache.** Die Aufgabe ist nicht idempotent: Commit des Dokuments und Löschen der Arbeitskopie
(`Deleting original file …`) geschehen vor `queue.complete()`; bei einem Absturz dazwischen liefert
`reap_expired_leases` (`pl-queue/src/pg.rs:435-470`) die Aufgabe erneut aus.
**Vorschlag.** Beim erneuten Ausliefern einer `consume_file` prüfen, ob schon ein Dokument aus dieser
Aufgabe existiert (z. B. Aufgaben-id am Dokument/`PaperlessTask.related_document` oder Prüfsumme der
Arbeitskopie vor dem Löschen merken) und dann `success` mit Verweis melden statt „File not found“.
Celery mit `acks_late` hätte dieselbe Lücke; upstream verliert die Aufgabe eher ganz.

## B8 – Suchindex: Schreibfehler unter Last (mittel, Windows)

**Beobachtung.** s2 (20 Nutzer, PATCH/Upload/Notizen gemischt): 4 `documents.tasks.index_document`
scheitern mit `tantivy: An IO error occurred: 'Zugriff verweigert (os error 5)'` bzw.
`tantivy: Failed to open file for write: 'FileAlreadyExists("46d6….732031.del")'`, der Consumer meldet
`Error while updating search index for 698: tantivy: … (os error 5)`. Zusätzlich 6×
`WARN log: couldn't find segment in SegmentManager` in s1.
**Ursache (vermutet, nicht bis ins Letzte belegt).** Schreiber sind über `.tantivy.lock` serialisiert
(`pl-indexer/src/lock.rs`), aber jede Batch öffnet einen frischen Index/Writer (`writer.rs:120-135`),
während Leser (Suche, `searcher`) Segment- und `.del`-Dateien geöffnet bzw. gemappt halten. Unter
Windows kann eine geöffnete Datei weder gelöscht noch neu angelegt werden → Garbage-Collection bzw. das
Schreiben einer neuen `.del`-Datei mit derselben Opstamp scheitert. Unter Linux (Docker/Jetson)
vermutlich nicht. Folge: Änderungen fehlen im Suchindex, bis die Aufgabe wiederholt wird
(`index_document` wird nicht automatisch wiederholt) oder neu indexiert wird.
**Vorschlag.** `index_document` bei IO-Fehlern mit Backoff wiederholen; einen langlebigen Writer je
Prozess statt je Batch; unter Windows Leser vor dem Commit neu laden/freigeben; die Konsistenzprüfung
(s7) nach jedem Lauf behalten.

## B9 – Speicher wächst im Dauerlauf (niedrig, beobachten)

s6, 30 min, 10 Nutzer + jede Minute „Tag an alle“ und ein Upload (33 772 Anfragen, **0 Fehler**):
private Bytes 356 MB (Mittel erste 5 min) → 449 MB (letzte 5 min), Steigung 3,6 MB/min gesamt,
6,4 MB/min in der zweiten Hälfte; Working Set 263 → 300 MB. Handles (270–380) und Threads (23–38)
schwanken ohne Trend, DB-Verbindungen konstant 10. In derselben Zeit wuchs der Bestand von 728 auf
982 Dokumente; ob das Wachstum Caches mit dem Bestand sind oder ein Leck, ist mit 30 min nicht zu
trennen. **Vorschlag:** Dauerlauf 2–4 h mit festem Bestand (ohne Uploads: Option in s6 nachrüsten)
bzw. Release-Build auf dem Jetson; erst dann bewerten.

## Ohne Befund geprüft

- **s2 viele Nutzer** (20 Nutzer, 5 min, 11 907 Anfragen; Liste, Filter, Suche, Detail, Vorschaubild,
  Vorschau, Metadaten, Notizen, PATCH, Upload): 0 Fehler, p95 je Operation 135–340 ms (Debug-Build).
  Weitere Läufe mit 30 Nutzern: siehe [Läufe](#läufe).
- **s4 Uploads** (20 gleichzeitig + Duplikate + 60-MB-PDF + kaputtes/abgeschnittenes PDF + leere
  Datei + `.exe`): 0 Fehler, keine hängende Aufgabe. Gültige Dateien und die große Datei werden
  eingelesen (Upload 60 MB: 0,57 s), Duplikate werden wie konfiguriert angenommen (mit
  `duplicate_documents`), kaputte PDFs enden `failure` mit Grund (`failed parsing cross reference table`,
  `PDFium: file not in PDF format or corrupted`), leere Datei und `.exe` → 400 beim Hochladen.
- **s5 Neustart**: keine verlorenen oder doppelten Dokumente, keine kaputten oder verwaisten Dateien,
  Warteschlange macht nach dem Neustart weiter (nur B7).
- **Konsistenz (s7) nach jedem Szenario**: Anzahl Dokumente = `all`, Detail und Vorschaubild jedes
  Dokuments 200, jede Original-/Archiv-/Vorschaudatei der DB vorhanden, keine verwaisten Dateien
  in `originals/`, `archive/`, `thumbnails/`, alle 254 Suite-PDFs des Dauerlaufs über ihr eindeutiges
  Suchwort genau einmal gefunden, Warteschlange am Ende leer, keine `PaperlessTask` ohne Grund
  fehlgeschlagen.

## Nicht geprüft / Grenzen

- Debug-Build auf Windows (kein frischer Build möglich, siehe oben); Zeiten sind nicht mit dem Jetson
  vergleichbar. B4 und B3 hängen vom Timing ab und treten auf dem langsamen Jetson eher häufiger auf.
- Keine Prüfung gegen ngx im selben Lauf; Vergleich nur über den Jetson-Bericht.
- Mail-Abruf, OCR schwerer Scans, Office-Dateien, KI-Funktionen, Sync, Freigabelinks, WebSocket-Last
  nicht belastet. `s6` nur 30 min (Auftrag: 30–60 min).
- Die Testdaten-Workflows wurden für die Läufe abgeschaltet (sie senden je Änderung Webhooks an
  `127.0.0.1:9` und Mails); im allerersten Lauf waren sie aktiv (B6).
- Ursache B8 ist eine Vermutung (Windows-Dateisemantik), nicht bis in tantivy verfolgt.

## Läufe

Alle gegen `http://127.0.0.1:58360`, Token-Anmeldung, 2 Task-Worker, Pool 10. Zeiten Ortszeit 2026-11-18.

| Lauf | Zeit | Inhalt | Anfragen | HTTP-Fehler | Wichtigste Befunde |
|---|---|---|---|---|---|
| Vorversuche | 04:17–04:27 | s4/s3/s2 klein; s3 mit 3/6/8 Clients, mit und ohne Versionen | ~1 500 | 6–8 je s3-Lauf (Timeout/500) | B1, B2 (erstmals gefunden) |
| Lauf 0 | 04:28–04:35 (abgebrochen) | s1 200 Dateien, Testdaten-Workflows aktiv | 991 bulk_edit | 72× 400, 2× Timeout | B3 (11 Importe FAILURE, 19× 400), B5 (52), B4 (1 Deadlock), B1, B6 |
| Hauptlauf | 04:36–05:12 (36 min) | s1 ×2 (je 200 Dateien), s4, s3 (8 Clients, 3 Versionen), s5, s2 (20 Nutzer, 5 min) | 22 400 | s1: 34 + 9, s3: 9, sonst 0 | s1: B4 (4 Deadlocks), B5 (47), B3 (1 Import), B1 (2× 60 s), B6; s3: B1, B2 (500); s5: B7 |
| Dauerlauf | 05:12–05:43 (30 min) | s6: 10 Nutzer + jede Minute „Tag an alle“ + Upload, danach s7 | 36 000 | 0 | B9, B8 (5 Index-Aufgaben) |
| Zusatz | 05:43–05:51 | s2 mit 30 Nutzern, Denkzeit ≤ 0,3 s, 2 min; s3 mit 4 Clients + 3 Versionen | 11 000 | s2: 35× 500, s3: 4 Timeouts | B2 (alle Endpunkte), B1, B10 |

Gesamtlaufzeit der Lastläufe ≈ 1 h 50 min. Rohdaten (summary.json/md, requests.jsonl, Server-Logs)
lagen in `%TEMP%/paperrust-stress-results/` und wurden mit der Wegwerf-DB gelöscht; mit
`python tests/stress/stress.py --local …` jederzeit neu erzeugbar.

**Reproduktion je Befund**
- B1: `stress.py --local --same-doc-clients 3 --versions 2 s3` (jeder Lauf)
- B2: `stress.py --local --same-doc-clients 8 --versions 0 s3` oder `--users 30 --think 0.3 s2`
- B3, B4, B5, B6: `stress.py --local --docs 200 s1` (zeitabhängig, meist jeder Lauf)
- B7: `stress.py --local s5`; B8/B9: `stress.py --local s2 s6`; B10: wie B1 mit Workflow (s3 legt ihn an)

---

## Stand nach Behebung (2026-11-18, Gesamtprüfung)

Geprüft wurde der kombinierte, noch nicht committete Stand beider Behebungs-Agenten plus die Restpunkte
dieser Prüfung. Debug-Build `target/debug/paperrust.exe` vom 2026-11-18 09:01 (frisch gebaut), eigener
Testserver (Port 58371, DB `pl_stress_g` frisch aus `paperless_snapshot`, 2 Task-Worker, Pool 10),
Windows-Laptop, PostgreSQL 18 lokal.

### Je Befund

| Nr. | Stand | Behebung (Kurz) | Beleg |
|---|---|---|---|
| B1 | **behoben** | Feste Sperrreihenfolge „erst Dokumentzeilen, dann MEDIA_LOCK, darunter nie auf fremde Zeilen warten“ (`pl-consumer/src/medialock.rs`, genutzt von pl-consumer, pl-documents-write und jetzt auch pl-cli); MEDIA_LOCK mit Zeitlimit (`PAPERRUST_MEDIA_LOCK_TIMEOUT`, 120 s) und prozessinterner Warteschlange | `lock_order_db.rs::b1_row_then_media_lock_no_deadlock` (grün, ohne Zeilensperre rot); s3 mit 8 Clients + 3 Versionen: 0 Fehler, keine Sperr-Wartezeit (LockWatch leer) – vorher in jedem Lauf 60-s-Hänger |
| B2 | **behoben** | Alle LISTEN-Verbindungen außerhalb des Pools (`pl_core::dbconn::listener`, jetzt auch pl-settings); Worker-Steuerung, Sync-Sequenzer, Long-Poll auf Nebenpools; keine zweite Pool-Verbindung, solange ein Handler eine hält (PATCH, Versionen, `selection::resolve` für `all=true`, Notizen, PDF-Werkzeuge, Vorschläge, `overrides_from_document`, Webhook-Einreihen, Besitzer-Laden beim Einlesen, Neuverarbeiten, Sync-Long-Poll) | s2 mit 30 Nutzern, Denkzeit ≤ 0,3 s: 44 747 Anfragen, **0×500** (vorher 35×500 in 2 min); s3 mit 8 Clients: 0 Fehler |
| B3 | **behoben** | ein gemeinsamer Baustein für den Tag-INSERT, ids immer `i32` | `concurrency_db.rs::b3_tag_insert_same_connection`; s1 zweimal: 0 fehlgeschlagene Importe, 0× bulk_edit 400 |
| B4 | **behoben** | Mehrzeilen-Updates auf `paperrust_document` sperren in id-Reihenfolge (`touch_modified_sorted`, `lock_documents_sorted`), Wiederholung bei 40P01/40001 | `concurrency_db.rs::b4_touch_modified_vs_bulk_edit_no_deadlock` (+ Gegenprobe `b4_control_unsorted_update_deadlocks`); s1 „Tag an alle“ 30× und parallel: 0× 400 |
| B5 | **behoben** | `add_tag` mit `ON CONFLICT DO NOTHING` (bewusste Abweichung, Vertrag pl-documents-write) | `concurrency_db.rs::b5_parallel_add_tag`; s1: 0× duplicate key, 0 gescheiterte Hintergrundaufgaben |
| B6 | **behoben** | wartende `bulk_update_documents` werden zusammengefasst (`coalesce.rs`, `PAPERRUST_BULK_UPDATE_COALESCE`) | `concurrency_db.rs::b6_coalesce_pending_bulk_update`; s1: Warteschlange nach jedem Szenario leer |
| B7 | **behoben** | die Einlese-Aufgabe merkt sich ihr Dokument in derselben Transaktion; erneute Zustellung meldet Erfolg statt „File not found“ | `lock_order_db.rs::b7_task_document_marker`; s5 (harter Abbruch nach 8 fertigen Aufgaben): 30/30 Erfolg, keine Fehlmeldung, keine Doppel |
| B8 | **behoben (Windows)** | jeder Index-Schreibvorgang bis 5× mit frischem Writer, danach `retry` statt endgültig | `pl-indexer writer::tests::commit_blocked_by_foreign_handle_is_retried`; in den Lastläufen tritt „Zugriff verweigert … retrying with a fresh writer“ weiter vereinzelt auf (1–2× je Lauf), **jede** Wiederholung gelang, s7: 0 Dokumente fehlen im Index |
| B9 | beobachtet | – | s6 60 min, siehe unten |
| B10 | **behoben** | pl-server-Middleware `detach`: nicht-sichere Methoden laufen als eigene Aufgabe zu Ende | `pl-server detach::tests::write_survives_client_abort_read_does_not`; **neues Szenario s8**: 122 hart abgebrochene PATCH/bulk_edit (RST nach 0–400 ms), alle 12 gespeicherten Titel im Index, Workflow-Tag an allen Dokumenten; das Zugriffs-Log zeigt `outcome="client closed"` für bulk_edit, die Nacharbeit lief trotzdem |

### Restpunkte dieser Prüfung

1. **Einstellungs-Listener** (`pl-settings/src/reload.rs`) lauscht über `pl_core::dbconn::listener`
   (eigene Verbindung). DB-Tests `pl-settings --test db` (7, u. a. `live_reload_two_processes`,
   `fallback_poll`) grün.
2. **Verschachtelte Pool-Anforderungen** systematisch gesucht (alle ~190 `acquire`/`begin` in
   `crates/*/src`, Aufrufe bis zur tatsächlichen Pool-Abfrage verfolgt) und behoben:
   `selection::resolve` (gibt die Verbindung für `all=true` zurück), `notes::doc_and_perm`,
   `pdftools::target_any`, `suggestions::view`, `pdf_edit::overrides_from_document`,
   `SharedEffects::webhook_in` (Einreihen immer im Hintergrund – deckt PATCH, Versionen, Einlesen und
   Massen-Nacharbeit ab), `Consumer::consumption_finished` (Besitzer über die Transaktion;
   `load_user_by_id` nimmt jetzt Pool oder Verbindung), `reprocess` (Verbindung vor Index/Cache frei),
   `pl_sync::wait_for_changes` (Prüfabfrage über die Listener-Verbindung; vorher konnten 8 wartende
   Sync-Clients den Long-Poll-Nebenpool verklemmen), Sicherung in pl-guard (Sperrverbindung vom Pool
   gelöst; nebenbei behoben: bei einem Fehler zwischen Sperre und Lauf blieb die Advisory-Sperre an
   einer Pool-Verbindung hängen). **Bewusst nicht geändert:** die Worker-Aufgaben in `bulk.rs`
   (Cache leeren und KI-Aufgaben bei gehaltener Verbindung), `signals.rs` (Einzelweg), `manual.rs`,
   `scheduled.rs`. Davon laufen höchstens so viele gleichzeitig wie Task-Worker (2), und Anfragen halten
   nie mehr als eine Verbindung. Damit bleibt immer eine Verbindung frei: keine Verklemmung möglich, nur
   kurzes Warten. KI-Aufgaben werden nur mit einer KI-Workflow-Aktion eingereiht.
3. **pl-cli** (`export/medialock.rs`): MEDIA_LOCK über `pl_consumer::medialock` (asynchron, Zeitlimit,
   Sperrdatei wird unter Windows wie bei filelock gelöscht). `filemove::update_filename_and_move_files`
   (Renamer, Retagger) sperrt erst die Dokumentzeile, dann MEDIA_LOCK. Exporter (nur lesende
   Transaktion), Importer und `empty_trash` (Dateien erst nach dem Commit) warten unter MEDIA_LOCK nie
   auf Zeilen. Test `export::medialock::tests::times_out_while_held_elsewhere`.
4. **s8 „Abbrechende Clients“** in `stress.py` (neues Szenario, bestehende Szenarien unverändert), Ergebnis
   oben.

### Neue Befunde dieser Prüfung (alle behoben)

| Nr. | Befund | Ursache | Behebung | Beleg |
|---|---|---|---|---|
| N1 | s2 mit 30 Nutzern (Denkzeit ≤ 0,3 s): nach 5 min Last war die Warteschlange auch nach 10 min Warten nicht leer (1 838 `index_document` + 188 `consume_file` wartend, 117 PaperlessTask „pending“) | Jede Änderung (PATCH, Notiz) reihte eine eigene `index_document`-Aufgabe ein, auch wenn für dasselbe Dokument schon eine wartete. Jede kostet einen Index-Commit (Debug ≈ 1 s) und verdrängte die Einlese-Aufgaben. | `pl-documents-write/src/index.rs`: nicht erneut einreihen, wenn eine noch nicht gestartete `index_document(id)` wartet (sie liest den Stand erst bei der Ausführung, die Änderung ist dann schon festgeschrieben); Known difference im Vertrag | Wiederholung von s2 mit gleicher Last: höchstens 1 wartende `index_document`, Warteschlange 50 s nach Lastende leer, 0 Befunde, Szenario 354 s statt 914 s |
| N2 | `pl-settings --test inventory` rot | neue Einstellungen der Behebungen (`PAPERRUST_DB_ACQUIRE_TIMEOUT`, `_DB_LONGPOLL_CONNECTIONS`, `_MEDIA_LOCK_TIMEOUT`, `_BULK_UPDATE_COALESCE`) ohne Schema-Eintrag; Test-Variablen `PAPERRUST_FIX1/2_SERVER` ohne Ausnahme | Schema, englische Hilfe, docs/SETTINGS.md (Zählung C 89 → 93, Summe 377 → 381), Ausnahmen im Test | `cargo test -p pl-settings` grün |
| N3 | Suite: s7 meldete nach s4 „Suchwort trifft mehrere Dokumente“ | s4 lädt dieselbe Datei absichtlich dreifach hoch (Duplikate erlaubt). Fehlalarm der Prüfung, kein Produktfehler. | s7 erwartet so viele Treffer wie erfolgreiche Uploads mit diesem Suchwort | Lauf 2: 0 Befunde |

### Läufe nach Behebung

| Lauf | Zeit | Szenario | Anfragen | HTTP-Fehler | Befunde | Dauer |
|---|---|---|---|---|---|---|
| 1 (vor N1/N3) | 08:32–09:00 | s1 / s4 / s3 (8 Clients, 3 Versionen) / s5 / s2 (30 Nutzer, ≤ 0,3 s) / s8 | 1 729 / 1 092 / 1 657 / 1 236 / 44 914 / 2 690 | 0 / 0 / 0 / 0 / 0 / 0 | s4–s8: N3 (Fehlalarm), s2: N1 | 190 / 27 / 82 / 78 / 914 / 369 s |
| **2 (Endstand)** | 09:01–09:15 | s1 / s4 / s3 (8 Clients, 3 Versionen) / s5 / s2 (30 Nutzer, ≤ 0,3 s) / s8 | 1 729 / 1 092 / 1 632 / 1 236 / 44 747 / 2 614 | **0** | **0** | 199 / 27 / 70 / 80 / 354 / 37 s |
| **3 (Endstand)** | 09:15–10:15 | s6 Dauerlauf 60 min | 64 699 | **0** | **0** | 3 621 s |

Nach jedem Szenario lief s7 (Konsistenz): Anzahl, Detail und Vorschaubild aller Dokumente, Dateien je
DB-Zeile, keine Waisen, Suchwort jedes eingelesenen PDFs genau einmal (bzw. so oft wie hochgeladen),
Warteschlange leer. Server-Log von Lauf 2: keine ERROR außer erwarteten (kaputtes PDF aus s4,
Testdaten-Mailkonto ohne Server); WARN nur langsame Anweisungen (Debug-Build), OCR-Hinweise, 2× „worker
lost, task requeued“ (der absichtliche Abbruch in s5) und 1× B8-Wiederholung (gelungen).

**B9 / Dauerlauf s6 (60 min, 10 Nutzer + jede Minute „Tag an alle“ und ein Upload):** 64 699 Anfragen,
0 Fehler, 0 Befunde, keine Sperr-Wartezeit. Private Bytes 514 MB (Mittel der ersten 5 min) → 594 MB
(letzte 5 min), Steigung der zweiten Hälfte **0,53 MB/min** (Schwelle 2 MB/min; Erstlauf 6,4 MB/min),
Working Set 317 → 349 MB, Handles 288–394 und Threads 29–45 ohne Trend. Der Bestand wuchs dabei von
≈ 880 auf 1 445 Dokumente. Kein Hinweis auf ein Leck; ein Lauf mit festem Bestand bleibt offen.
DB-Verbindungen des Servers jetzt konstant 17–18 statt 10: Hauptpool 10 plus eigene Verbindungen für
LISTEN (4) und die Nebenpools der Worker-Steuerung/des Sync-Sequenzers (B2). Bei knappem
`max_connections` in PostgreSQL (Standard 100) einplanen.

### Sonstige Prüfungen (Endstand)

- `cargo clippy --all-targets -j 4 -- -D warnings` für alle 43 Crates (Gruppen zu 8) und pl-pdftools
  `--features engine`: sauber.
- `cargo test --lib` für alle Crates: grün; Integrationstests pl-core, pl-queue, pl-indexer, pl-consumer,
  pl-documents-write, pl-server, pl-settings, pl-cli: grün (pl-settings nach N2).
- DB-Tests mit `--include-ignored` gegen Wegwerf-DBs aus frisch migrierten Vorlagen: `concurrency_db`
  5/5, `lock_order_db` 2/2, `pl-settings --test db` 7/7.
- Oberfläche (frontend-light): vitest 653/653 (73 Dateien), `svelte-check --threshold error` 0 Fehler,
  `npm run build` erfolgreich.

### Weiterhin nicht geprüft

- Release-Build und Linux/Jetson (B8 ist Windows-spezifisch; B3/B4 hängen vom Timing ab – auf dem Jetson
  erneut laufen lassen: `stress.py --url … s1 s3 s8`).
- Gegenprobe für s8 mit einem Build ohne `detach` (B10 war im Erstlauf 1/1 reproduziert; die Wirksamkeit
  belegt der Unit-Test, nicht ein zweiter Lastlauf ohne die Middleware).
- Mail-Abruf, schwere OCR-Scans, Office-Dateien, KI, Sync-Last und WebSocket-Last wurden wie im Erstlauf
  nicht belastet.

## N4 – Statement-Cache bei dynamisch gebauten Abfragen (2026-11-18, behoben)

**Beobachtung.** Klicktest der neuen Automatik-API (`/rs/api/v2/automation`): Testlauf → `PUT /workflows/{id}` →
`POST /workflows/{id}/apply` mit `document_ids: [192]` → 500, ab und zu auch `GET /workflows/{id}`. PostgreSQL-Log
(DB `pl_auto_ui`, 11:17–11:27, 11× ): `incorrect binary data format in bind parameter 1` bzw. `insufficient data
left in message`, jeweils `STATEMENT: SELECT "id", "name" FROM "documents_correspondent" WHERE "id" IN ($1)`.

**Ursache.** Dieselbe Klasse wie B3, aber bei **sea-query**-Abfragen, die der statische Wächter
(`pl-documents-write/tests/sql_cache_guard.rs`) nicht sieht: `pl-automation/src/common.rs` `Names::resolve` bindet
die ids als `i32`, andere Crates bauen denselben kurzen Text (`… WHERE "id" IN ($1)`) mit `i64`. Bei genau einem
Wert ist der Text gleich (bei drei Werten `IN ($1, $2, $3)` – deshalb klappte es mit drei Dokumenten). sqlx
(`sqlx-postgres` 0.8.6 `get_or_prepare`) sucht im Cache nur nach dem Text, auch bei `persistent(false)`.
Nachgestellt in `crates/pl-core/tests/sqlbind_db.rs` (ein Text, `i32` dann `i64` auf derselben Verbindung →
„incorrect binary data format“). Zusätzlich (Prüfung aller mit `format!` gebauten Texte):
`SELECT EXISTS(SELECT 1 FROM auth_user WHERE id = $1)` aus `pl-documents-write/src/serializer.rs` `exists` (`i32`)
und `pl-authz/src/owned.rs` `parse_owner` (`i64`) – gleiche Kollision.

**Behebung.**
1. `pl_core::sqlbind::SqlxBinder` (gleiche Signatur wie `sea_query_binder::SqlxBinder::build_sqlx`) hängt an jede
   sea-query-Anweisung mit Parametern ` /*t:<hash>*/` an: Hash über Anzahl und Art der Werte (keine Inhalte).
   Anderer Bindungstyp → anderer Cache-Schlüssel; gleiche Typen teilen weiter eine vorbereitete Anweisung.
   Alle 69 Stellen in 19 Crates (`src` und `tests`) umgestellt (nur der `use`). Statement-Cache bleibt an –
   `statement_cache_capacity(0)` war nicht nötig (Kosten: ein Hash über die Wertarten je Abfrage).
2. `pl-authz` `parse_owner` und `pl-objects` `load_parent` binden `int4`-Spalten jetzt als `i32` (Regel
   „`integer` → `i32`, `bigint` → `i64`“).
3. **N4b** (vom Klicktest gefunden): zwei gleichzeitige `apply` auf dasselbe Dokument → Verlaufseintrag „error“
   `duplicate key … paperrust_document_tag_pkey` (Workflow-Tag: beide Läufe lesen „fehlt“, beide fügen ein).
   `pl-consumer/src/db.rs` `add_tags` fügt jetzt mit `ON CONFLICT DO NOTHING` ein – wie Django `_add_items`
   (`bulk_create(ignore_conflicts=True)`).

**Dauerhafter Schutz.** `crates/pl-core/tests/sqlbind_guard.rs` schlägt fehl, sobald irgendwo in `crates/*/src`
oder `crates/*/tests` `sea_query_binder::SqlxBinder` oder `build_any_sqlx` direkt verwendet wird. Beleg-Test
`sqlbind_db.rs` (`#[ignore]`, Wegwerf-DB `pl_sqlbind_test`). Nicht automatisch geschützt: mit `format!` gebaute
Texte mit `.bind(…)` (≈ 120 Stellen geprüft, nur die eine Kollision oben); dort gilt weiter die Typregel.

**Belege (eigener Server :58371, DB `pl_stress_n4`, Binary kopiert).**
- Klicktest (3 Workflows je Runde: GET, Testlauf mit 20 bzw. 1 Dokument, PUT, apply mit `[192]`, `[190,191,192]`,
  `[5]`, GET ×3 als admin und alice, Verlauf): 8 Runden 345 Anfragen, danach 2 Läufe parallel + v1-Abfragen
  (`/api/correspondents/?id__in=1`, `/api/documents/192/` …) 346 + 360 Anfragen: **0 Fehler**; 144
  `run_workflows_for_documents` erledigt, Verlauf 113 ok / 127 nicht zutreffend / **0 error**; PostgreSQL-Log ohne
  Bindungsfehler (nur fehlgeschlagene Webhooks der Testdaten an `127.0.0.1:9`, erwartet).
- `stress.py --local s1 s3`: s1 1 729 Anfragen, s3 1 854 Anfragen, **0 HTTP-Fehler, 0 Befunde**, s7 konsistent,
  keine Sperr-Wartezeiten; Log-Warnungen nur langsame Anweisungen (Debug-Build) und OCR-Hinweise.
- `cargo clippy --all-targets -- -D warnings` und `cargo test` für alle 19 umgestellten Crates grün (zwei
  Unit-Tests in pl-search/pl-ai prüften das Textende und berücksichtigen jetzt die Signatur);
  `pl-automation --test automation_db --ignored` 3/3.
