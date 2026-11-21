# Vertrag pl-indexer (tantivy-Schreiber, Worker-Baustein)

Quellen (alle `upstream/src/`, v3.4.2): `documents/search/_backend.py` (`WriteBatch`,
`TantivyBackend.open/_build_tantivy_doc/add_or_update/remove/batch_update/rebuild`,
`_DocumentViewerStream`, `_bulk_get_viewer_permissions`, `_is_indexable_date`,
`_extract_autocomplete_words`, Lock-Konstanten), `_schema.py` (`field_descriptors`,
`schema_fingerprint`, `needs_rebuild`, `_settings_mismatch`, `wipe_index`, `_write_sentinels`,
`open_or_rebuild_index`, `rebuild_in_progress`), `_fields.py`, `_tokenizer.py`;
`documents/models.py` (`Document.get_effective_content/get_effective_barcodes`,
`CustomFieldInstance.value/value_for_search`, Meta-Ordnungen, `SoftDeleteModel`),
`documents/versioning.py` (`root_document_ids`, `annotate_effective_content`,
`versions_newest_first`, `sort_versions_newest_first`, `latest_version`),
`documents/permissions.py` (`get_permission_grantees`), `documents/tasks.py`
(`index_optimize`, `index_document`, `remove_document_from_index`, `bulk_update_documents`),
`documents/signals/handlers.py` (`add_to_index`), `documents/management/commands/document_index.py`.
Bibliotheken: **tantivy-py 0.26.2** (`uv.lock`; sdist geprüft, SHA-256 aus dem Lock:
`src/document.rs`, `src/index.rs`, `src/schemabuilder.rs`), **filelock 4.2.6**.

Kein HTTP: pl-indexer hat keine Routen. Er ist die Rust-Hälfte von „Index schreiben“ für den
späteren Rust-Worker (pl-queue) und für eine Übernahme des Index von Python.

## Index-Format

- tantivy-py 0.26.2 bettet **tantivy 0.26.2** ein (Pfadstrings im `.pyd` und `Cargo.lock` des
  sdist). Der Workspace pinnt `tantivy = "=0.26.0"`. Diff 0.26.0 → 0.26.2 (`src/`): nur
  Aggregationen und `query/{disjunction,union}` (Seek-Fix); Indexer, Doc-Store, Postings,
  Fast-Fields, Fieldnorms, Merge-Policy unverändert → gleiches On-Disk-Format (index_format v7).
  Paritätslauf bestätigt: Inhalt byte-gleich je Dokument (siehe unten). CORE-REQUEST: auf
  `=0.26.2` heben.
- Schema/Tokenizer: aus pl-search (`schema::build_schema`, `FIELD_DESCRIPTORS`,
  `tokenizer::register_tokenizers`), keine Duplikate.
- `tantivy.Index(build_schema(), path=…)` (reuse=True) = `Index::open_or_create(MmapDirectory)`,
  `IndexSettings::default()`, Tokenizer registriert. Schreiber: `Index::writer(heap)` (Threads
  automatisch, wie `num_threads=0`), Standard-Merge-Policy (LogMergePolicy).
- Heap: `WriteBatch` 128 000 000 B (tantivy-py-Default), `rebuild` 512 000 000 B
  (`--heap-size-mb`·10⁶ überschreibbar).

## `.index_settings.json` und Lebenszyklus

- Inhalt exakt wie `_write_sentinels`: `{"schema_version": 4, "language": <SEARCH_LANGUAGE|null>,
  "schema_fingerprint": "<blake2b-512 hex>"}`; Fingerprint = `blake2b(json.dumps([list(d) for d
  in field_descriptors()]))` (lokale BLAKE2b-Implementierung, gegen RFC-Vektoren und den
  Fingerprint des Referenz-Index getestet).
- `needs_rebuild`: `.rebuilding` vorhanden oder `_settings_mismatch` (Datei fehlt/unlesbar,
  `schema_version != 4`, `language` fehlt/≠, Fingerprint ≠).
- `Indexer::open` = `TantivyBackend.open()`-Nebenwirkungen (`open_or_rebuild_index`):
  Verzeichnis fehlt → nichts; Mismatch → `wipe_index`, leer neu anlegen, Sentinels; `Index.open`
  scheitert → ebenso (Log „corrupted or incomplete“).

## Dokument → tantivy-Dokument (`doc::build_tantivy_doc`, `_build_tantivy_doc`)

Werte werden wie tantivy-py gesammelt (`BTreeMap<Feldname, Vec<Wert>>`) und mit
`TantivyDocument::convert_named_doc` umgewandelt → Feldwerte in Feldnamen-Reihenfolge, je Feld
in Einfüge-Reihenfolge. Alle Strings `normalize_search_text` (NFC).

| Feld | Wert |
|---|---|
| `id` | pk (u64) |
| `checksum` | unverändert |
| `title`, `title_sort`, `simple_title` | Titel |
| `content`, `simple_content` | `get_effective_content() or ""` |
| `bigram_title` / `bigram_content` | `extract_cjk_text(...)`, nur wenn nicht leer (content nur wenn content nicht leer) |
| `original_filename` | nur wenn nicht leer |
| `correspondent`, `correspondent_sort`, `bigram_correspondent` | Name, wenn gesetzt (bigram nur CJK) |
| `document_type`, `type_sort`, `bigram_document_type` | dito |
| `storage_path` | Name, wenn gesetzt |
| `tag` (+ `bigram_tag`) | je Tag, Ordnung `Tag.Meta.ordering=("name",)` |
| `notes` | JSON je Notiz (nicht gelöscht, Ordnung `created`): `{"note": …, "user": username|null}` |
| `notes_text` | alle Notiztexte mit `" "` verbunden, nur wenn ≥1 Notiz |
| `custom_fields` | JSON je Instanz (nicht gelöscht, Ordnung `created`) mit `value_for_search != None`: `{"name", "value"}` |
| `barcodes` | JSON je Barcode des effektiven Dokuments (Ordnung `page, id`): `{"format", "value"}` |
| `created` | `datetime(y, m, d, UTC)`; außerhalb i64-Nanosekunden → weggelassen + Warnung |
| `modified`, `added` | UTC, Mikrosekunden |
| `asn`, `page_count` | wenn nicht NULL |
| `num_notes` | Anzahl Notizen |
| `owner_id` | wenn truthy (≠ NULL, ≠ 0) |
| `viewer_id` / `viewer_group_id` | direkte `view_document`-Objektrechte (guardian), sortiert |
| `autocomplete_word` | `sorted(set(autocomplete_tokens(t) for t in [title, content, correspondent?, type?, *tags] if t))` |

JSON-Objekte: Schlüssel sortiert (tantivy-py nutzt serde_json ohne `preserve_order`), jeder Wert
über `OwnedValue::from(serde_json::Value)` – inkl. tantivys Erkennung von RFC-3339-Strings als
Datum (eine Notiz „2025-01-06T10:00:00Z“ wird im JSON zum Datum, wie upstream).

`value_for_search`: string/url/monetary/longtext → Text; date → `YYYY-MM-DD`; boolean →
`True`/`False`; integer → Dezimal; float → Python-`repr` (kürzeste Darstellung, `1e+16`,
`1e-05`, `100.0`); documentlink → Python-`str` der JSON-Liste (`[1, 2]`); select → `label` der
Option mit `id == value_select` (sonst None).

Effektiver Inhalt/Barcodes je Aufrufpfad (upstream hat zwei Ordnungen, Unterschied nur bei
`version_index IS NULL`):

| Pfad | Inhalt | Barcodes | Wurzel |
|---|---|---|---|
| `rebuild` (reindex) | prefetch `versions` → `(version_index or 0, id)` desc | prefetch → ebenso | `Document.objects` (ohne Papierkorb), `root_document IS NULL` |
| `add_or_update_ids` | `annotate_effective_content` → SQL `version_index DESC NULLS LAST, id DESC` | prefetch `versions__barcodes` → Python-Ordnung | `root_document_ids(ids)` über `Document.objects`; Unbekanntes/Papierkorb still übersprungen |
| `add_or_update(document)` | Abfrage, SQL-Ordnung | `get_latest_version_for_root`, SQL-Ordnung | Version → `root_document` (FK, auch Papierkorb) |

Versionen immer nur nicht gelöschte (`versions` = Default-Manager).

## Schreiber-API (`writer.rs`)

```rust
let (ix, _outcome) = Indexer::open(IndexerConfig::from_env())?;     // TantivyBackend.open()
let mut b = ix.batch(Duration::from_secs(30)).await?;               // batch_update()
b.add_or_update_ids(&pool, &ids).await?;  b.add_or_update(&pool, id).await?;  b.remove(id)?;
b.add_or_update_record(&record)?;         // bereits geladener Datensatz
b.commit()?;                              // __exit__: commit + wait_merging_threads + Sperren frei
// drop(b) ohne commit = verwerfen (Ausnahme im with-Block)
ix.add_or_update(&pool, id).await? / ix.remove(id).await? -> WriteOutcome::{Written,Skipped,Deferred}
ix.index_document_task(&pool, id) / ix.remove_document_task(id)     // tasks.index_document / remove_document_from_index
ix.rebuild(&pool, &RebuildOptions{..}).await?                       // document_index reindex [--recreate] [--if-needed] [--heap-size-mb]
ix.optimize()                                                       // no-op wie upstream
```

- `remove` = `delete_query(TermQuery(id))` (wie `delete_documents_by_query(term_query)`).
- `add_or_update*` = `remove(pk)` + `add_document`.
- `rebuild`: (Sperre, s. u.) → `wipe_index` → `.rebuilding` anlegen → Index anlegen → Sentinels →
  Writer(512 MB) → Dokumente in Blöcken zu 1000 (`ORDER BY created DESC`, je Block 8 feste
  Abfragen, kein N+1) → `commit` → `wait_merging_threads` → Marker entfernen. Fehler: Writer
  verworfen, Marker bleibt (nächstes `needs_rebuild` → true), wie upstream.
- DB-Zugriff nur lesend (sqlx, gebundene Parameter; `$n`-Platzhalterlisten für `IN`).

## Sperren (Kompatibilität mit dem Python-Writer)

1. `INDEX_DIR/.tantivy.lock` wie `filelock.FileLock`: Rust `File::try_lock` → Unix
   `flock(LOCK_EX|LOCK_NB)` (= filelock UnixFileLock), Windows `LockFileEx(EXCLUSIVE|
   FAIL_IMMEDIATELY)` über die ganze Datei (überlappt filelocks Byte 0 → gegenseitiger
   Ausschluss). Polling 50 ms (filelock-Default), Zeitlimit je Versuch 30 s (`batch_update`)
   bzw. 10 s (`add_or_update`/`remove`), 4 Versuche, dazwischen `uniform(0, min(10, 2**attempt))` s.
   Erschöpft → `IndexerError::LockExhausted` (= `SearchIndexLockError`); die Komfort-Methoden
   liefern `WriteOutcome::Deferred` – der Aufrufer muss wie upstream `index_document` /
   `remove_document_from_index` mit `countdown=60` einplanen (kein Queue-Zugriff in diesem Crate).
   Lock-Datei wird nicht gelöscht (Unix-filelock löscht auch nicht).
2. Zusätzlich tantivys eigene `.tantivy-writer.lock` über `Index::writer` (gleiche tantivy-Version
   wie tantivy-py → kompatibel; zweiter Writer scheitert sofort mit `LockBusy`).
3. Jede Batch öffnet einen **frischen** Index (frische ManagedDirectory, wie upstream begründet).
   Reihenfolge beim Ende: commit → Merges abwarten → Writer frei → `.tantivy.lock` frei.

## Tests

- Unit (`cargo test -p pl-indexer`, 14 Tests): BLAKE2b-Vektoren, Fingerprint = Referenz-Index,
  Sentinel-Datei byte-gleich, `needs_rebuild`/Marker, `open_or_rebuild_index`, Python-`repr`
  für float/JSON, `value_for_search` je Typ, Versionsordnungen, Dokumentaufbau (Feldreihenfolge,
  JSON-Schlüssel, RFC-3339-Datum in JSON, Bigramme, owner 0, Datumsgrenzen), Sperre exklusiv,
  Rebuild + Batch-Upsert/Remove + Lesen über pl-search, verworfene Batch.
- Parität (`tests/parity/pl-indexer/gen_reference.py`, `crates/pl-indexer/tests/parity.rs`,
  `#[ignore]`, braucht Referenz-DB): upstream baut mit unverändertem Code
  (`call_command("document_index","reindex")`, nur `PAPERLESS_DATA_DIR`/`PAPERLESS_LOGGING_DIR`
  per Umgebung auf einen Temp-Ordner, DB-Sitzung `READ ONLY`); Rust baut aus derselben DB.
  Verglichen: Settings-Datei, jedes gespeicherte Dokument, jeder Term mit Postings
  (id, tf, Positionen), Fieldnorms, Fast-Field-Werte; über pl-search 61 Suchen × 4 Nutzer
  (Score, Highlights, Reihenfolge), Autocomplete, More-like-this. Optional `--incremental`:
  gleiche Schreib-Batches (add_or_update_ids mit Version/Papierkorb/unbekannt, remove,
  add_or_update einer Version, Komfort-remove/add) auf je einer Kopie desselben upstream-Index.

## Known differences

0. **Frisch geleerter Index bleibt „neu aufzubauen“** (Jetson-Prüfung B1, 2026-11-19):
   `open_or_rebuild_index` setzt nach dem Leeren (Einstellungen passen nicht / Index kaputt) zusätzlich
   die Neuaufbau-Marke. Upstream verlässt sich darauf, dass der Docker-Start `document_index reindex
   --if-needed` vor jedem anderen Öffnen ausführt; bei PaperRust öffnet der Server zuerst, und der
   Neuaufbau des Workers meldete danach „Search index is up to date.“ – Suche nach Umstieg von 2.x
   leer. Test `settings::tests::recreated_index_still_needs_rebuild_until_a_rebuild_finishes`.

1. **Reihenfolge gleich bewerteter Treffer** (gleicher Score bzw. gleicher Sortierwert, MLT-Ties)
   folgt der internen Doc-Reihenfolge. Die ist schon **upstream zwischen zwei Rebuilds nicht
   stabil** (mehrere Writer-Threads + Merge; Kontrolllauf upstream-vs-upstream: 78 identisch,
   166 nur Tie-Reihenfolge, 0 verschieden – Rust-vs-upstream: 80–88/156–164/0). Kein
   Rust-Unterschied, aber nicht „identisch“ im Sinne gleicher Bytes. More-like-this wählt seine
   12 Terme bei gleichem tf-idf zufällig (tantivy: `HashMap` + Heap nur nach Score) – auch
   upstream schwankt die Treffermenge; der Test akzeptiert eine Rust-Menge nur, wenn eine
   Wiederholung auf dem upstream-Index genau sie liefert.

2. **`reindex --if-needed` prüft auch den Inhalt** (2026-11-21): upstream baut nur bei abweichenden Einstellungen (Schema/Sprache) oder liegengebliebener Markierung neu. PaperRust zusätzlich, wenn der Index leer ist, die Datenbank aber Dokumente hat, oder wenn er weniger als die Hälfte der Dokumente enthält (`writer::index_incomplete`, `db::count_root_docs`). Anlass: NAS-Paralleltest, Index als „aktuell“ gestempelt aber leer → 0 Suchtreffer bis zum manuellen Reindex.

## Ergebnis Paritätslauf (2026-11-08, Referenz-DB, 195 Dokumente)

- Neuaufbau: 0 Unterschiede in Settings-Datei, 195 gespeicherten Dokumenten, 4324 Termen mit
  Postings/Positionen, 3900 Fieldnorms, 2535 Fast-Field-Werten; 244 Suchen: 0 verschieden
  (Rest identisch oder nur Tie-Reihenfolge, s. o.); Autocomplete und MLT: 0 verschieden.
- Inkrementell (gleiche Batches auf Kopien desselben upstream-Index, 4 Segmente): 0
  Index-Unterschiede, 244/244 Suchen identisch, Autocomplete/MLT identisch.
2. `rebuild` hält `.tantivy.lock` (upstream nicht: `document_index reindex` wischt ohne Sperre);
   abschaltbar mit `RebuildOptions::take_lock = false`. `wipe_index` lässt `.tantivy.lock` stehen
   (Löschen einer gehaltenen Sperrdatei würde den Ausschluss brechen).
3. `rebuild` legt ein fehlendes `INDEX_DIR` an (upstream: `wipe_index` scheitert dann mit
   `FileNotFoundError`; paperless legt das Verzeichnis beim Start an).
4. Gleichstände in DB-Ordnungen (`created` bei Notizen/Custom Fields, `-created` der Dokumente,
   Tag-Namen) sind wie upstream nicht deterministisch; Rust nutzt dieselbe `ORDER BY`-Klausel.
5. Python-`repr` von Strings (documentlink-JSON, nicht-String-Select-Label) nur für gängige
   Escapes nachgebildet; upstream würde bei nicht-String-Label mit `TypeError` abbrechen, Rust
   nimmt dessen `repr`.
6. `value_for_search` für unbekannten `data_type`: upstream `NotImplementedError`, Rust None
   (Feld übersprungen).
7. Ungültige Werte, die tantivy-py ablehnen würde (negatives u64, Datum außerhalb i64-ns bei
   modified/added), führen wie upstream zum Abbruch der Batch (`IndexerError::InvalidValue`).

## Nicht durch die Referenzdaten abgedeckt

Barcodes (0 in der Referenz-DB) und CJK-Texte kommen in den Referenzdaten nicht vor → nur
Unit-Tests. `\p{Han}` u. a. in `extract_cjk_text` kommt aus pl-search (dort Known difference 8).

## CORE-REQUEST

1. `[workspace.dependencies]`: `tantivy = "=0.26.2"` (tantivy-py 0.26.2 bettet 0.26.2 ein;
   Format gleich, aber Abfrage-Fixes in `query/union`). Betrifft auch pl-search.
2. pl-indexer nutzt `pl-search` per `path` mit `default-features = false` (nur Engine); bitte so
   lassen oder als Workspace-Dependency mit `default-features = false` anbieten.
3. pl-queue: Aufgaben `index_document(document_id)` / `remove_document_from_index(doc_id)` mit
   `countdown=60` einplanen, wenn `WriteOutcome::Deferred` zurückkommt; Signale (Dokument
   gespeichert/gelöscht) → `Indexer::add_or_update` / `remove`.
4. Architekturregel „Rust liest nur“ (docs/ARCHITECTURE.md) gilt bis zur Umschaltung des
   Workers; pl-indexer darf erst schreiben, wenn der Python-Worker nicht mehr indexiert oder
   (Sperren kompatibel) beide abwechselnd schreiben.

## Last-Befund B8 (2026-11-18): Schreibfehler unter Windows

Quelle: tests/stress/BEFUNDE.md B8. Nachgestellt in
`writer::tests::commit_blocked_by_foreign_handle_is_retried`: ein fremder Griff auf `meta.json` ohne
`FILE_SHARE_DELETE` (Virenscanner, Windows-Suche) lässt den Commit beim Ersetzen von `meta.json` mit
„Zugriff verweigert (os error 5)“ scheitern, **nachdem** Segment- und `.del`-Dateien geschrieben sind;
der nächste Writer startet mit derselben Opstamp und scheitert an der verwaisten Datei mit
`FileAlreadyExists("<segment>.<opstamp>.del")` – genau die beiden Meldungen aus dem Lastlauf. Welcher
Prozess im Lastlauf den Griff hielt, ist nicht belegt (Leser von pl-search öffnen mit
`FILE_SHARE_DELETE` und blockieren das Ersetzen nicht; 3000 Commits mit lebendem Leser ohne Fehler).

Behebung:
- `IndexerError::is_transient` (IO-, Öffnen-/Schreib-, Sperr- und Thread-Fehler von tantivy).
- Jeder vollständige Schreibvorgang (`add_or_update`, `remove`, `index_document_task`,
  `remove_document_task`, neu `add_or_update_ids_task`, `write_records`, L3-Abbau `dirty::drain_once`)
  wird bei solchen Fehlern bis zu 5-mal mit frischem Writer wiederholt (0,25/0,5/1/2 s). Ab dem
  zweiten Versuch räumt der Writer Waisen weg (`garbage_collect_files`) und verschiebt die Opstamp-Folge
  (Löschung einer nie vergebenen id), damit keine Dateinamen des gescheiterten Commits mehr entstehen.
- Bleibt der Fehler: `add_or_update`/`remove` melden `Deferred` (Aufrufer reiht die Aufgabe mit
  countdown 60 ein); die Aufgaben selbst werden in pl-server als `retry` (OSError, 60 s) statt
  endgültig fehlgeschlagen behandelt.

Known difference: upstream (tantivy-py) wiederholt nur bei belegter Sperre (`SearchIndexLockError`).
