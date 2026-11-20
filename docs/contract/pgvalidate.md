# Phase 6 – Postgres-Validierung (pgvalidate)

Stand 2026-11-08, Coder 20. Ausstiegskriterium laut docs/PLAN.md: *`EXPLAIN` aller Listen,
Abfragen ↔ Schema geprüft*. Ergebnis: **EXPLAIN (ANALYZE, BUFFERS) aller mitgeschnittenen
Abfragen liegt vor, Schema-Querprüfung ohne Befund – aber die Phase ist inhaltlich NICHT bestanden:**
mehrere Listen- und Zählabfragen von PaperRust sind bei 100 000 Dokumenten um Faktor 5–60
langsamer als Django (bis 260 s je Anfrage). Ursache ist durchweg die Abfrageform
(korrelierte Unterabfragen je Zeile), nicht ein fehlender Index. Upstream hat genau diese Formen in
v3.4.2 bewusst ersetzt (Kommentar in `src/documents/permissions.py::annotate_document_count_by_ids`:
„A per-outer-row correlated subquery … catastrophic once the queryset has hundreds/thousands of rows“).

Kein Crate-Code wurde geändert. Alle Vorschläge unten sind gemessen (vorher/nachher auf derselben DB).

> **Nachtrag:** P1–P3 sind inzwischen umgesetzt und gemessen, siehe **Abschnitt 7 „Korrekturen“**
> (offen blieb `has_duplicates`, behoben in **Abschnitt 8**). Teilstringsuche im Inhalt und Serializer: **Abschnitt 9**.

## 1. Aufbau

| | |
|---|---|
| DB-Kopie | **`pl_pgvalidate`** (Template `paperless_snapshot`, Owner `paperless`) – **bleibt bestehen** (746 MB). Referenz-DB `paperless` unberührt. |
| Hochskalierung | `tests/pgvalidate/scale_100k.sql` (deterministisch, ohne `random()`): die 200 Seed-Wurzeldokumente 500× geklont, Verhältnisse des Seeds bleiben (Besitzer, Papierkorb, Rechte, Notizen, CF), dazu mehr Stammobjekte und Tasks. Log: `results/pgvalidate/logs/scale_100k.log` (~75 s). |
| Datenmenge | 100 204 Dokumente (97 695 sichtbar für admin, 2 505 im Papierkorb), 256 204 Dokument-Tags, 20 040 Notizen, 203 907 CF-Instanzen, 41 589 + 21 044 guardian-Objektrechte, 411 418 Audit-Log-Einträge, 20 012 Tasks, 200 Tags, 100 Korrespondenten, 30 Dokumenttypen. Inhalte ~2,4 kB Text je Dokument. |
| PaperRust | Kopie `target/debug/paperless-rs.exe` (Build 02:11) → `results/pgvalidate/bin/paperrust-pgv.exe`, 127.0.0.1:8090, `PAPERLESS_DBNAME=pl_pgvalidate`, **alle 141 Routen auf `rust`** (`results/pgvalidate/routes-all-rust.toml`, über `PAPERLESS_RS_ROUTES`), Log-Filter über **`PAPERLESS_RS_LOG`** (nicht `RUST_LOG`, siehe `crates/pl-server/src/main.rs`) = `info,sqlx::query=debug`. Start/Stopp per PID: `tests/pgvalidate/rs-start.sh`/`rs-stop.sh`. |
| Django | eigener granian auf 127.0.0.1:8091, upstream unverändert, Settings-Overlay `tests/pgvalidate/pgv_settings.py` (= `reference_settings` + SQL-Logger über `force_debug_cursor`, kein DEBUG), Redis-Präfix `pgv`, Logs nach `results/pgvalidate/logs/django`. `tests/pgvalidate/dj-start.sh`/`dj-stop.sh`. Beide Server sind beendet. |
| Pläne | `auto_explain` (log_analyze, log_buffers, log_min_duration=0, log_level=notice) **nur für `pl_pgvalidate`** per `ALTER DATABASE` – die Pläne kommen als Notice zum Client und landen über `sqlx::postgres::notice` im Rust-Log, mit den echten Parametern. Nach dem Mitschnitt wieder `RESET`. |
| Anfragen | `tests/pgvalidate/pgv_requests.py`: 61 Anfragen wie das Angular-Frontend (App-Start-`listAll`, Dashboard, Dokumentliste mit 10 Sortierungen/16 Filtern, `selection_data`, Detail, Verwaltung mit `full_perms`, Tasks-Abschnitte, Suche) als **admin, alice, bob**. Treiber `tests/pgvalidate/capture.py`, Auswertung `analyze.py`. |
| Läufe | (a) Mitschnitt mit auto_explain (Rust + Django) → Roundtrips, SQL, Pläne; (b) Zeitmessung ohne auto_explain, je Anfrage 2×, gewertet das Minimum (warm), Server nacheinander (nie gleichzeitig). |

Hinweis Methode: Das Frontend sendet in der Dokumentliste **immer** `include_selection_data=true`.
Weil das in Rust Minuten kostet, laufen die Sortier-/Filtervarianten ohne und zwei Varianten
(`+sel`) mit selection_data. Echte UI-Kosten einer Listenansicht = Variante + selection_data-Anteil.

## 2. Ergebnis in Zahlen (warm, ms, Auszug; vollständig in `results/pgvalidate/summary.md`)

| Anfrage | Nutzer | Rust | Django | RT Rust/Django |
|---|---|---:|---:|---|
| `GET /api/tags/?page_size=100000` (App-Start) | admin | 13 959 | 2 429 | 5 / 9 |
| | alice | **260 031** | 4 251 | 5 / 11 |
| | bob | 113 280 | 4 172 | 5 / 11 |
| `GET /api/correspondents/?page_size=100000` | alice | 43 465 | 1 708 | 5 / 7 |
| `GET /api/document_types/?page_size=100000` | alice | 14 191 | 1 617 | 5 / 7 |
| `GET /api/tags/?page_size=25&ordering=name&full_perms=true` (Verwaltung) | alice | 133 902 | 4 017 | 7 / 14 |
| `GET /api/documents/…&include_selection_data=true` (Standardliste) | admin | **157 195** | 13 093 | 17 / 22 |
| | alice | 38 349 | 9 369 | 17 / 25 |
| `…&tags__id__all=6&include_selection_data=true` | admin | 8 217 | 4 866 | 17 / 22 |
| `…&has_duplicates=true` | admin | 10 811 | 1 796 | 3 / 2 |
| `…&ordering=correspondent__name` | admin | 4 840 | 1 288 | 11 / 14 |
| `…&title_content=rechnung` | admin | 19 106 | 15 731 | 11 / 14 |
| Dokumentliste Standard (ohne sel) | admin | 852 | 688 | 11 / 14 |
| `…&shared_by__id=2` | admin | 410 | 5 416 | 11 / 14 |
| `…&ordering=-custom_field_7` | admin | 2 315 | 7 198 | 12 / 15 |
| `…&page_size=100000&fields=id` (Alle auswählen) | admin | 1 949 | 12 879 | 3 / 13 |
| `GET /api/trash/` | admin | 631 | 1 584 | 10 / **143** |
| `GET /api/documents/?query=…` (Volltext) | admin | 170 | 638 | 10 / **218** |
| `GET /api/search/?query=…&db_only=true` | admin | 638 | 1 294 | 2 / **73** |
| `GET /api/documents/{id}/notes/` | admin | 8 | 352 | 3 / 31 |

Gesamt (61 Anfragen je Nutzer, Summe warm): admin Rust 262 s / Django 107 s, alice 662 s / 112 s,
bob 399 s / 100 s. **Ohne die 11 Ausreißer-Anfragen** (Objektlisten mit document_count,
selection_data, has_duplicates, Korrespondent-Sortierung): Rust 48 / 41 / 39 s gegen Django
75 / 78 / 67 s, Rust schneller in 40–49 von 61 Anfragen. Roundtrips gesamt Rust 450–461 gegen
Django 1 030–1 092 – **Rust hat kein N+1** (Dokumentliste konstant 11 Roundtrips je Seite: Nutzer,
Seite+COUNT, Dokumente, 2× Versionen, Tags, Notizen, CF, 3× Rechte), Django hat N+1 bei Papierkorb,
Volltext, globaler Suche und Notizen. Alle 183 Statuscodes Rust = Django.

Django-Grundlast hier ~200 ms je Anfrage (granian/asyncio unter Windows), daher sind kleine
Rust-Vorteile (<200 ms) nicht aussagekräftig; die großen Abweichungen sind es.

## 3. Befunde (nach Schwere)

### P1 – Objektlisten: `document_count` als korrelierte Unterabfrage je Objekt (Tags, Korrespondenten, Dokumenttypen, Speicherpfade)

- Code: `crates/pl-objects/src/perms.rs::document_count_expr` (Zeile 49), benutzt von
  `listing::select`. Für Nicht-Superuser enthält die Unterabfrage je Objekt die komplette
  Rechte-Unterabfrage über `documents_document` (inkl. LEFT JOIN auf die Elterndokumente).
- Plan (alice, `results/pgvalidate/explain/alice__tags.listAll__01.txt`): `SubPlan 2` mit
  `loops=158`, darin `Seq Scan on documents_document … loops=158` und Hash über 100 204 Zeilen –
  pro Tag einmal die ganze Rechteprüfung über 100k Dokumente; 2,8 Mio. gelesene Puffer.
- Verstärkt in `crates/pl-objects/src/handlers.rs::tag_list` (Zeile 64): die teure Abfrage läuft
  **zweimal** (alle Tags ungepaged + noch einmal für `children_source` mit denselben Zählungen),
  und für `page_size=25` werden trotzdem die Zählungen aller Tags berechnet.
- Upstream v3.4.2: `annotate_document_count_for_related_queryset` = **ein** `GROUP BY tag_id`
  über `documents_document_tags` mit `document_id IN (permitted_document_ids)`, dann `CASE WHEN`
  (Django-Mitschnitt `results/pgvalidate/capture/dj-sql.json`).
- **Messung E2** (`tests/pgvalidate/experiments_e2.sql`, aus dem Mitschnitt erzeugt mit
  `make_e2.py`; Ergebnis `results/pgvalidate/explain/experiments_e2.out.txt`): heutige Abfrage
  **81 093 ms**, Umformulierung als LEFT JOIN auf ein GROUP BY **1 328 ms** (Faktor 61),
  Ergebnisgleichheit per `EXCEPT` geprüft (0 Zeilen). Für FK-Objekte (Korrespondent, Typ, Pfad)
  gilt dasselbe Muster (`GROUP BY correspondent_id` …).
- Vorschlag an pl-objects: document_count wie upstream als eine gruppierte Abfrage (oder LEFT JOIN
  auf eine gruppierte Unterabfrage), nur für die ausgelieferten Objekte; in `tag_list` die Zählung
  nicht doppelt berechnen. Kein Index nötig.

### P1 – `selection_data` der Dokumentliste: korrelierte Zählung je Objekt über `string_to_array` mit allen IDs

- Code: `crates/pl-documents/src/views.rs::selection_data_list` (Zeile 112) mit
  `crates/pl-documents/src/db.rs::id_in_list` (`$1 = ANY(string_to_array($2, ',')::bigint[])`).
  Die IDs aller Treffer (97 695 für admin) gehen als ein Text-Parameter; jede der 5 Abfragen
  zählt per korrelierter Unterabfrage je Korrespondent/Tag/Typ/Pfad/Feld.
- Plan (`admin__doclist.default+sel__02.txt`): `Index Scan using documents_document_pkey …
  Index Cond: (id = ANY('{…97 695 …}')) … loops=100`, `Rows Removed by Filter: 96 853` je Schleife
  → 9,7 Mio. Index-Zugriffe, 110 s für die Korrespondenten allein.
- Upstream: IDs einmal materialisieren (wie Rust), dann je Beziehung **ein** `GROUP BY`
  (`Count(… filter=Q(documents__id__in=ids))` bzw. `annotate_document_count_by_ids`).
- **Messung E1** (`tests/pgvalidate/experiments.sql`, `results/pgvalidate/explain/experiments.out.txt`):
  Korrespondenten heute **58 711 ms** → `JOIN unnest(…::bigint[]) … GROUP BY` **464 ms**;
  Tags heute **15 965 ms** → **692 ms**; Ergebnisgleichheit geprüft (0 Zeilen). Alle fünf
  Beziehungen in einer einzigen Abfrage (CTE): 2 705 ms – kein Vorteil gegenüber fünf
  getrennten GROUP BY, also verworfen.
- Vorschlag an pl-documents: je Beziehung LEFT JOIN auf eine gruppierte Unterabfrage mit
  `unnest($1::bigint[])` (ein Array-Parameter statt Text), Reihenfolge der Ausgabe wie bisher
  per `ORDER BY` auf der äußeren Tabelle.

### P2 – `has_duplicates`: doppelte Ausführung bei leerer erster Seite

- Code: `crates/pl-core/src/pagination.rs::fetch_page` (Zeile 212): Seite+COUNT in einer Abfrage;
  ist die Seite leer, folgt ein **zweiter** `COUNT` über dieselbe teure Bedingung.
  Bei `has_duplicates` (im Bestand keine Duplikate) laufen damit 13,7 s + 3,4 s. Zusätzlich wählt
  der Planer für die Seitenabfrage `Index Scan Backward using documents_document_created…`
  über alle 97 695 Zeilen mit je einem BitmapOr (`loops=97695`), weil er Treffer erwartet.
- Django zählt zuerst und spart die Seitenabfrage bei 0 Treffern: 1 796 ms.
- Vorschlag an pl-core: Ist `offset == 0` und die Seite leer, ist `count = 0` – kein zweiter
  COUNT nötig (gilt für alle Listen). Die Umformulierung in vier EXISTS (E5C) war langsamer
  (3 331 ms gegen 2 598 ms) → verworfen; Django-Form (E5B, 2 468 ms) ≈ Rust-Form (E5A, 2 598 ms).

### P2 – Sortierung `correspondent__name` (ebenso `document_type__name`, `storage_path__name`) als korrelierte Skalar-Unterabfrage im ORDER BY

- Code: `crates/pl-docfilter/src/ordering.rs::field_expr` Zeilen 62–64 (`related_name(… "oc"/"odt"/"osp")`);
  gemessen nur `correspondent__name`, die anderen beiden haben dieselbe Form.
- **Messung E3** (`experiments2.out.txt`): heute 2 114 ms → LEFT JOIN + `ORDER BY oc.name` (Django-Form)
  387 ms. Im Server: 4 840 ms Rust gegen 1 288 ms Django (admin).
- Vorschlag: LEFT JOIN wie Django (NULL-Sortierung beachten: Django `ASC` = NULLS LAST, wie heute).

### P3 – Sortierung `num_notes` als korrelierte COUNT je Zeile

- Code: `crates/pl-docfilter/src/ordering.rs` Zeile 48 (`"nn"`).
- **Messung E4**: 1 124 ms → LEFT JOIN auf vorab gruppierte Notizen 503 ms. Im Server (admin)
  2 332 ms Rust gegen 1 470 ms Django.

### P3 – COUNT der Dokumentliste (betrifft Rust und Django gleich)

- Jede Dokumentliste zählt `root_document_id IS NULL AND deleted_at IS NULL` per Seq Scan über
  `documents_document` (~20 000 Blöcke, 290–530 ms); tiefe Seiten (`page=200`) zusätzlich ~800 ms.
- **Index-Kandidat (gemessen, nur in der Kopie, wieder entfernt):**
  `CREATE INDEX paperrust_doc_live_created ON documents_document (created, id) WHERE root_document_id IS NULL AND deleted_at IS NULL;` (2,1 MB)
  - auf den **echten** Rust-Abfragen (`experiments3.sql`, Replay des Mitschnitts): admin
    Standardliste 289 → **27 ms**, admin Seite 200: 795 → **27 ms** (Index-Only-Scan für COUNT und
    Seite); isoliert COUNT 527 → 42 ms, `OFFSET 9950` 305 → 4 ms.
  - **alice (Nicht-Superuser): kein Gewinn** (489 → 432 ms, 598 → 648 ms) – die Rechte-
    Unterabfrage braucht `owner_id`/guardian und geht weiter über den Heap.
  - Empfehlung: nur als expand-only-Migration `paperrust_…`, wenn Admin-/Einzelnutzer-Installationen
    (Superuser) das Ziel sind; vorher P1/P2 umsetzen. Kein Produkt-Schema geändert.

### Gleichstand / kein Handlungsbedarf

- `title_content`/`content__icontains`: Rust erzeugt dieselbe Form wie Django
  (`UPPER(COALESCE((SELECT neueste Version.content …), content)) LIKE`) – beide 10–20 s,
  Seq Scan + Unterabfrage je Zeile. Ein pg_trgm-Index hilft wegen der Versions-Unterabfrage nicht
  ohne Umbau der Semantik; nicht gemessen, kein Vorschlag.
- `custom_fields` (Liste/Verwaltung): Rust nutzt bereits das gruppierte Muster (1,1–1,9 s,
  Django 1,7–2,8 s). `statistics` für Nicht-Admins 3,4–4,5 s (Django 1,5 s): drei getrennte
  Rechte-Unterabfragen über alle Dokumente – P3, nicht weiter gemessen.
- Rust deutlich schneller als Django: `shared_by__id`, `custom_field`-Sortierung, „Alle auswählen“
  (`fields=id`), Papierkorb, Volltextliste, globale Suche, Detail/Notizen, Tasks.
- `custom_field_query=["7","gt",100]` liefert in beiden 400 (mein Testwert ungültig) – nicht gemessen.

## 4. Schema-Querprüfung

`tests/pgvalidate/schema_check.py` (statisch über alle String-Literale in `crates/*/src`, Tabellen in
FROM/JOIN/INTO/UPDATE und `Alias::new`, Spalten über aufgelöste Aliase in rohem SQL und
`(Alias::new(tabelle), Alias::new(spalte))`) gegen `reference/schema/postgres.sql` (75 Tabellen):
Ergebnis `results/pgvalidate/schema_check.json`.

- **Unbekannte Spalten: keine.** Unbekannte Tabellen: nur `paperrust_task_queue`,
  `paperrust_task_group`, `paperrust_beat_state` (pl-queue, eigene Tabellen mit Präfix, Migration
  `crates/pl-queue/migrations/0001_paperrust_task_queue.sql`) – erwartet, kein Befund.
- 72 der 75 Tabellen werden angesprochen; ungenutzt: `account_emailaddress`,
  `account_emailconfirmation`, `django_admin_log`.
- Nicht auflösbare Bezeichner (19) sind Ausgabe-Aliase/Casts (`document_count`,
  `last_correspondence`, `bigint`, `cfo` …), geprüft, kein Befund.
- Dynamisch: alle 1 366 mitgeschnittenen Rust-Abfragen (61 Anfragen × 3 Nutzer) liefen ohne
  SQL-Fehler gegen das Schema (kein 500, alle Statuscodes = Django).
- Grenzen: dynamisch zusammengesetzte Namen (`format!`, Variablen) prüft die Heuristik nicht.

## 5. Rohdaten (`results/pgvalidate/`)

- `capture/rs-explain.json`, `capture/dj-sql.json` – Mitschnitt je Nutzer/Anfrage: SQL, Zeilen,
  Dauer, bei Rust der auto_explain-Plan mit Parametern.
- `capture/rs-time.json`, `capture/dj-time.json` – Zeitmessung (2 Läufe je Anfrage).
- `explain/<nutzer>__<anfrage>__<nr>.txt` – EXPLAIN (ANALYZE, BUFFERS) jeder Rust-Abfrage (1 366).
- `explain/experiments*.out.txt` – Vorher/Nachher-Messungen E1–E6 und Index-Replay.
- `summary.md` – Tabelle aller 183 Anfragen (Roundtrips, Zeiten, Plan-Merkmale SEQ/LOOP/SORT/N+1).
- `logs/` – Server-Logs, Treiber-Ausgaben, Skalierungslog. `bin/paperrust-pgv.exe` – gemessene Binary.

## 6. Einschränkungen

- Laptop, `shared_buffers=32MB` der Referenz-PG (Produktion 128 MB+) – viele Puffer kommen aus dem
  OS-Cache („read“), absolute Zeiten sind höher als auf einem Server; Verhältnisse gelten.
- Zeitmessung mit 2 Läufen (Minimum), nicht verschränkt; Mitschnitt-Zeiten mit auto_explain
  (Timing-Overhead) nur für Pläne verwenden. Django-Zeiten in `dj-sql.json` haben 15,6-ms-Auflösung
  (Windows `time.monotonic`).
- Volltext (`query=`, Autocomplete) liest den tantivy-Index der Referenz (200 Dokumente) – SQL-Teil
  realistisch, Index-Teil nicht.
- Binary-Stand 02:11; die Befund-Codestellen wurden am aktuellen Quelltext (≈03:40) gelesen und
  stimmen mit dem mitgeschnittenen SQL überein.
- Nicht erfasst: Schreibpfade, Mail-/Workflow-Listen, `metadata`, `suggestions`, Dateien.

## 7. Korrekturen (2026-11-08, Performance-Fix nach Phase 6)

Umgesetzt nach der Regel „Profil vor Maßnahme, eine Maßnahme pro Messpaar, nur behalten, was über
dem Rauschen gewinnt; nach jeder Stufe Parität“. Index `paperrust_doc_live_created` **nicht** eingebaut
(bleibt Vorschlag, siehe P3 oben). Kein Schema geändert, Referenz-DB `paperless` nur lesend benutzt.

### 7.1 Maßnahmen

| Nr. | Crate / Stelle | Vorher | Nachher (upstream-Bezug) |
|---|---|---|---|
| M1 | pl-objects `perms.rs::document_count_source`, `listing::select_counted` | `document_count` (und `last_correspondence`) als korrelierte Unterabfrage je Objekt, darin die volle Rechte-Unterabfrage | **ein** `GROUP BY` über `documents_document` (FK, `COUNT(id)`, ggf. `MAX(created)`) bzw. `documents_document_tags` (`COUNT(DISTINCT document_id)`), als `LEFT JOIN (…) pl_dc ON pl_dc.k = o.id`, Wert `COALESCE(n, 0)` – wie upstream `Count("documents", filter=…, distinct=True)` bzw. `annotate_document_count_for_related_queryset`. Filter unverändert (Superuser `deleted_at IS NULL`, sonst `id IN permitted_document_ids`). Einzelobjekt/Kinder: Aggregation auf die bekannten IDs beschränkt (upstream `tag_id__in=queryset.values("pk")`). |
| M1b | pl-objects `perms.rs` (Liste, Nicht-Superuser) | M1 zählte für alle Objekte | Aggregation zusätzlich auf die für den Nutzer sichtbaren Objekte beschränkt (`k IN (SELECT id … PermittedObjectsFilter view_*)`, dieselbe Bedingung wie die äußere Liste). Grund: nach M1 war bob `storage_paths.listAll` (3 sichtbare Pfade, 0 Dokumente) von 51 ms auf ~690 ms **langsamer** geworden. |
| M2 | pl-objects `handlers.rs::tag_list` | `children_source` berechnete alle Zählungen ein zweites Mal | Zeilen ohne Zählung holen, Zählungen aus `all_tags` übernehmen, nur fehlende Tags (Kinder außerhalb des Filters) gezielt zählen (`listing::fetch_counts`); bei `ordering=document_count` weiter mit Zählung in der Abfrage (die Sortierung braucht sie). |
| M3 | pl-documents `views.rs::selection_data_list` | korrelierte Zählung je Objekt mit `= ANY(string_to_array($ids))` | je Beziehung `LEFT JOIN (SELECT fk AS k, COUNT(DISTINCT doc) FROM <join> JOIN unnest(string_to_array($1, ',')::bigint[]) … GROUP BY fk)`; `ORDER BY` wie bisher explizit (FK: `id`, Tags: `name`, Felder: `created`). Text- statt Array-Parameter, weil sea-query ohne `postgres-array`-Feature gebaut wird (Workspace-Cargo.toml). |
| M4 | pl-core `pagination.rs::fetch_page` | leere Seite ⇒ zweiter `COUNT` | leere Seite bei `OFFSET 0` ⇒ `count = 0` ohne weitere Abfrage; spätere leere Seiten wie bisher (404-Prüfung braucht die Zahl). |
| M5 | pl-docfilter `ordering.rs` | `correspondent__name`/`document_type__name`/`storage_path__name` als Skalar-Unterabfrage im ORDER BY | `LEFT JOIN <tabelle> AS oc/odt/osp ON id = fk`, `ORDER BY oc.name` – Form des Django-Mitschnitts (`ORDER BY "documents_correspondent"."name" ASC`, kein Tiebreaker; PG-Standard ASC = NULLS LAST / DESC = NULLS FIRST, wie vorher). Ein Join je Tabelle auch bei doppeltem Term. `owner` sortiert wie Django nach `owner_id` (kein Join nötig; `owner__username` ist kein `ordering_field`). |
| M6 | pl-docfilter `ordering.rs` (`num_notes`) | korrelierte `COUNT` je Zeile | `LEFT JOIN (SELECT document_id, COUNT(*) n FROM documents_note WHERE deleted_at IS NULL GROUP BY document_id) onn`, `ORDER BY COALESCE(onn.n, 0)` |
| M7 | pl-system `statistics.rs` (Nicht-Admins) | drei Durchläufe über die Rechte-Unterabfrage (Inbox, MIME-Statistik, ASN-Max) | ASN-Maximum (gefiltert, Known difference #14710 von pl-system) im selben `GROUP BY mime_type` als `MAX(asn)` je Gruppe; Maximum der Gruppen = Maximum der Menge, keine Dokumente ⇒ 0. Ein Durchlauf/Roundtrip weniger. Inbox-Zählung bleibt getrennt (andere Join-Form). |

Count-Abfragen (`count_statement`) und `fetch_all_ids` behalten die neuen LEFT JOINs; sie ändern die
Zeilenzahl nicht (Join auf Primärschlüssel bzw. auf eine nach dem Join-Schlüssel gruppierte Tabelle),
PostgreSQL entfernt sie dort (join removal; Kontrolle `doclist.default` unten unverändert).

### 7.2 Messung

Skripte: `tests/pgvalidate/fix_bench.sh` + `fix_bench.py` (Wanduhr, verschränkt: Runde r = Rust vorher →
Rust nachher → Django, 3 Runden, Median; Rust nacheinander auf :8090 per PID gestartet/beendet, Log
`warn`; Django eigener granian :8091), `fix_compare.py` (Antwortkörper vergleichen), `fix_table.py`,
`fix_dbtime.sh` + `fix_buffers.py` (DB-Zeit und Puffer aus auto_explain, nur für diesen Lauf auf
`pl_pgvalidate` gesetzt, danach `ALTER DATABASE pl_pgvalidate RESET ALL`, geprüft: 0 Einstellungen).
Binaries (`results/pgvalidate/bin/`): `paperrust-before.exe` (Stand vor den Korrekturen),
`paperrust-after.exe` (M1–M7 ohne M1b), `paperrust-after2.exe` (alle), `paperrust-nodedupe.exe` (M1 ohne M2).
Rohdaten: `results/pgvalidate/fix/{main,restrict,dedupe,dbtime,smoke}/` (runs.jsonl, bodies/, table.md).

**Rauschen:** Während der Messung lief der Laptop unter fremder Dauerlast (u. a. mehrere `find`-Prozesse
anderer Agenten mit je >20 000 s CPU); Runde 2 war durchgehend 2–5× langsamer (auch Django). Die
Wanduhr ist daher nur für Effekte ≫2× aussagekräftig; Kontrollen ohne Codeänderung
(`selection_data` POST, `doclist.default`) schwanken zwischen 0,8× und 1,8×. Für die kleinen
Maßnahmen (M4–M7) gilt deshalb die lastunabhängige Zahl der gelesenen Puffer (7.2.3).

#### 7.2.1 Wanduhr, ms (Median aus 3 verschränkten Läufen; „nachher“ = after.exe, M1–M7 ohne M1b)

| Anfrage | Nutzer | Django | Rust vorher | Rust nachher |
|---|---|---:|---:|---:|
| `tags.listAll` (`/api/tags/?page_size=100000`) | admin | 5 229 | 15 828 | 1 147 |
| | alice | 6 333 | **220 574** | 1 730 |
| | bob | 6 779 | 152 004 | 1 235 |
| `correspondents.listAll` | admin | 1 848 | 3 379 | 377 |
| | alice | 3 144 | 89 665 | 3 044 |
| | bob | 2 490 | 28 203 | 3 111 |
| `document_types.listAll` | admin | 2 745 | 1 841 | 513 |
| | alice | 4 189 | 23 421 | 814 |
| | bob | 3 460 | 9 927 | 1 342 |
| `storage_paths.listAll` | admin | 1 317 | 463 | 564 |
| | alice | 5 998 | 952 | 764 |
| | bob | 3 150 | 51 | 691 (mit M1b: 191, s. 7.2.2) |
| `tags.manage` (`page_size=25&ordering=name&full_perms=true`) | admin | 3 864 | 12 174 | 825 |
| | alice | 10 843 | 250 936 | 1 104 |
| | bob | 4 560 | 209 616 | 1 048 |
| `tags.manage.filter` (`…&name__icontains=tag`) | admin | 2 503 | 3 913 | 891 |
| | alice | 7 064 | 107 760 | 2 714 |
| | bob | 3 475 | 96 293 | 961 |
| `correspondents.manage` | admin | 1 291 | 1 111 | 358 |
| | alice | 2 314 | 18 444 | 850 |
| | bob | 2 094 | 24 355 | 2 710 |
| `correspondents.manage.lc` (`…&last_correspondence=true`, wie das Frontend) | admin | 1 513 | 412 | 363 |
| | alice | 4 859 | 37 469 | 1 852 |
| | bob | 4 012 | 53 761 | 858 |
| `document_types.manage` | admin | 1 300 | 1 755 | 365 |
| | alice | 1 564 | 18 430 | 1 708 |
| | bob | 3 652 | 19 816 | 1 087 |
| `doclist.default+sel` (Standardliste mit `include_selection_data=true`) | admin | 18 844 | **169 736** | 5 300 |
| | alice | 13 845 | 49 341 | 6 880 |
| | bob | 12 190 | 25 435 | 5 892 |
| `doclist.f.tag_all+sel` | admin | 5 866 | 9 180 | 2 757 |
| | alice | 5 796 | 8 914 | 3 975 |
| | bob | 6 674 | 6 424 | 3 038 |
| `doclist.f.has_duplicates` | admin | 3 570 | 15 375 | 16 678 |
| | alice | 2 642 | 13 214 | 13 710 |
| | bob | 2 634 | 13 137 | 15 630 |
| `doclist.correspondent` (`ordering=correspondent__name`) | admin | 1 854 | 3 974 | 1 010 |
| | alice | 2 389 | 2 590 | 1 796 |
| | bob | 2 783 | 2 213 | 1 345 |
| `doclist.document_type` (`ordering=document_type__name`) | admin | 1 692 | 2 826 | 738 |
| | alice | 2 439 | 1 591 | 1 746 |
| | bob | 2 869 | 1 646 | 1 393 |
| `doclist.-storage_path` (`ordering=-storage_path__name`) | admin | 1 381 | 2 361 | 906 |
| | alice | 3 181 | 1 294 | 1 837 |
| | bob | 2 247 | 1 568 | 1 250 |
| `doclist.-num_notes` | admin | 2 432 | 2 241 | 973 |
| | alice | 3 292 | 1 049 | 1 249 |
| | bob | 1 994 | 1 475 | 1 194 |
| `statistics` | admin | 1 094 | 1 151 | 768 |
| | alice | 3 951 | 5 395 | 4 170 |
| | bob | 4 060 | 4 116 | 3 458 |
| Kontrolle `doclist.default` (unverändert) | admin / alice / bob | 911 / 1 463 / 1 581 | 1 152 / 733 / 790 | 636 / 438 / 678 |
| Kontrolle `selection_data` POST (unverändert) | admin / alice / bob | 2 341 / 4 217 / 3 570 | 3 164 / 2 613 / 2 475 | 2 121 / 3 347 / 2 687 |

#### 7.2.2 Einzelmessungen M1b und M2 (eigene verschränkte Läufe, stärkere Fremdlast, Median ms)

| Messpaar | Anfrage | Nutzer | Django | ohne Maßnahme | mit Maßnahme |
|---|---|---|---:|---:|---:|
| M1b (after → after2) | `storage_paths.listAll` | bob | 7 121 | 2 079 | **191** |
| | `storage_paths.listAll` | alice | 7 621 | 9 748 | 1 597 |
| | `tags.manage.filter` | bob | 6 826 | 10 431 | 1 473 |
| | `correspondents.manage.lc` | bob | 11 965 | 5 532 | 1 093 |
| | übrige 23 Objektlisten | alle | | 0,2–1,6× (Rauschen, s. o.) | |
| M2 (nodedupe → after) | `tags.listAll` | admin / alice / bob | 3 568 / 16 456 / 18 621 | 2 363 / 5 500 / 10 193 | 1 482 / 3 141 / 1 746 |
| | `tags.manage` | admin / alice / bob | 3 786 / 13 745 / 15 855 | 2 308 / 5 652 / 7 105 | 1 076 / 4 898 / 4 431 |
| | `tags.manage.filter` | admin / alice / bob | 3 033 / 9 402 / 9 336 | 849 / 1 504 / 1 478 | 920 / 4 895 / 4 531 |

M1b: deutlicher Gewinn dort, wo der Nutzer wenige Objekte sieht, sonst im Rauschen → behalten.
M2: `tags.listAll`/`tags.manage` 1,2–5,8× (eine von zwei vollen Aggregationen entfällt) → behalten;
`tags.manage.filter` hier ohne Gewinn (die nicht passenden Kinder fehlen in `all_tags` und werden
gezielt nachgezählt; der Rückgang alice/bob 1,5 → 4,9 s liegt in einem Lauf mit 2–5× Fremdlast,
die Puffer in 7.2.3 sinken trotzdem von 3,0 Mio. auf 22 000).

#### 7.2.3 DB-Arbeit (lastunabhängig): Puffer shared hit+read aller Abfragen, Roundtrips (2 Läufe, identisch)

| Anfrage | Nutzer | vorher Puffer / RT | nachher (after2) Puffer / RT | Maßnahme |
|---|---|---:|---:|---|
| `doclist.f.has_duplicates` | admin | 2 630 053 / 3 | 1 355 100 / 2 | M4 |
| | alice | 2 696 815 / 3 | 1 915 513 / 2 | M4 |
| `doclist.correspondent` | admin | 139 651 / 11 | 41 965 / 11 | M5 |
| | alice | 100 282 / 11 | 62 541 / 11 | M5 |
| `doclist.-num_notes` | admin | 256 784 / 11 | 41 649 / 11 | M6 |
| | alice | 162 686 / 11 | 63 496 / 11 | M6 |
| `statistics` | admin | 41 002 / 8 | 40 995 / 7 | M7 |
| | alice | 123 363 / 8 | 82 377 / 7 | M7 |
| `tags.manage.filter` | admin | 246 291 / 6 | 21 856 / 6 | M1+M1b+M2 |
| | alice | 3 003 676 / 6 | 22 114 / 6 | M1+M1b+M2 |
| `storage_paths.listAll` | admin / alice | 5 607 / 26 247 | 5 599 / 26 235 | M1+M1b |
| Kontrolle `doclist.default` | admin / alice | 21 799 / 22 111 | 21 799 / 22 105 | – |

(`results/pgvalidate/fix/dbtime/summary.txt`.)

**Bewertung:** M1/M3 Faktor 7–230 (alice/bob App-Start-Tags 220 s / 152 s → 1,2–1,7 s; Standardliste
mit selection_data admin 170 s → 5,3 s); alle Objektlisten und `+sel`-Listen jetzt schneller als Django.
M5/M6 −40 … −84 % DB-Arbeit, Wanduhr admin 2,3–3,9×, Nicht-Admins im Rauschen (dort dominiert die
Rechte-Unterabfrage). M7 −33 % DB-Arbeit für Nicht-Admins, ein Roundtrip weniger. M4: ein Roundtrip
und 0,8–1,3 Mio. Puffer weniger, aber **Wanduhr ohne messbaren Gewinn**: die Seitenabfrage selbst
(Index Scan Backward über `created` mit BitmapOr je Zeile, s. P2 oben) bleibt 13–17 s gegen Django
2,6–3,6 s. Behalten, weil die Arbeit messbar sinkt und die Semantik gleich ist; **`has_duplicates`
bleibt offen** (Ansatz: Django-Reihenfolge „erst zählen, bei 0 keine Seite“ nur für diesen Filter, oder
die Duplikatbedingung als Semi-Join auf eine vorab gruppierte Prüfsummenmenge – nicht umgesetzt).

### 7.3 Parität

- **Referenz (200 Dokumente, Django :8000, DB `paperless` nur lesend):**
  - pl-objects `reference_parity`: 0 Abweichungen in 1 256 Anfragen (nach M1–M7 und erneut nach M1b, jeweils 0).
  - pl-docfilter `parity`: same 318, tie_only 44, different 0 (= Stand vor den Korrekturen; die 44 sind
    Gleichstände bei `custom_field_*`-Sortierung, upstream ohne Tiebreaker).
  - pl-documents `parity`: routes 1 398 / 0, docfilter 188 / 0, search 41 / **6** – die 6 sind
    `more_like_id`, das pl-documents bewusst an Django delegiert (501 im In-Process-Test, Commit
    20e977c), unabhängig von diesen Korrekturen.
  - pl-system `reference_parity`: 6 Abweichungen, alle `current_asn` in `/api/statistics/` für
    alice/bob/carol (Known difference 1 von pl-system: gefiltertes ASN-Maximum wie upstream-Fix #14710,
    1193 statt 1194) – derselbe Wert wie vor M7 (Maximum der Gruppenmaxima = Maximum der Menge);
    vor/nach M7 auch bei 100k identisch (fix_compare rs-before ↔ rs-after).
- **100k (`pl_pgvalidate`), Antwortkörper Rust ↔ Django (`fix_compare.py`, Host normalisiert):**
  identisch in allen 27 Objektlisten (inkl. `document_count`, Inbox-Tags, Kind-Tags/`children`,
  `last_correspondence`, `full_perms`; vor und nach M1b), `doclist.default+sel` und `tag_all+sel` (inkl.
  Reihenfolge der `selection_data`), `has_duplicates`, `statistics` admin. Abweichend nur:
  - Sortierungen nach `correspondent__name`, `document_type__name`, `-storage_path__name`, `-num_notes`:
    andere Dokumente auf Seite 1 **bei Gleichstand** des Sortierschlüssels (tausende Dokumente je
    Name). Verglichen als Menge + Regel: Folge der Sortierschlüssel und `count` identisch mit Django,
    vorher wie nachher; Django selbst liefert zwischen den Runden andere Gleichstand-Dokumente
    (upstream ohne Tiebreaker → planabhängig).
  - `statistics` alice/bob: `current_asn` (Known difference #14710, s. o.).
  - `selection_data` POST (nicht geändert, `selection_data_post`): Reihenfolge der Korrespondenten
    planabhängig – Django-Form `LEFT JOIN … GROUP BY` mit `COUNT(CASE …)` ohne `ORDER BY` (Hash-Aggregat
    möglich); Rust nutzt dieselbe Form und liefert bei 100k schon vor den Korrekturen eine andere
    Reihenfolge als Django. Offener Punkt für pl-documents (als Menge vergleichen oder dokumentieren).
- **Reihenfolge der Listen-`selection_data` (M3):** upstream zählt FK-Beziehungen mit
  `COUNT(DISTINCT …) FILTER (…)` und `GROUP BY "documents_correspondent"."id"` ohne `ORDER BY`; ein
  DISTINCT-Aggregat kann PostgreSQL nicht hash-aggregieren, es sortiert nach `id` (GroupAggregate) →
  faktisch `id`-Reihenfolge (bestätigt bei 200 und bei 100k Dokumenten). Tags/Felder: `Meta.ordering`
  (`name`/`created`). Rust sortiert explizit genauso; die Rust-Reihenfolge ist damit nicht planabhängig.
- `cargo test` (pl-core, pl-objects, pl-documents, pl-docfilter, pl-system) grün, `cargo clippy
  --all-targets -- -D warnings` sauber; Erwartungs-SQL in `pl-docfilter/tests/ordering.rs` an die
  Join-Form angepasst (plus Fall mit doppeltem Term).

## 8. Korrekturen, Runde 2 (2026-11-08, W16: `has_duplicates`, Nachmessung der übrigen Listen)

Gleiche Regeln wie Abschnitt 7 (eine Maßnahme, verschränkt, Median aus 3 Runden, lastunabhängig
über Puffer). Kein Schema geändert; `pl_pgvalidate` nur gelesen (auto_explain nur für die
Puffer-Messung per `ALTER DATABASE`, danach `RESET ALL`, geprüft: 0 Einträge in
`pg_db_role_setting`). Die Korrektheitsprüfung mit künstlichen Duplikaten lief auf einer
**temporären** Sitzungs-Kopie (`CREATE TEMP TABLE`), nicht auf der DB-Kopie.

### 8.1 Maßnahme M8 – `has_duplicates=true`: Kandidaten-Vorfilter

- Ursache (P2 oben): Der Planer erwartet Treffer und liest `ORDER BY created DESC LIMIT 50` per
  `Index Scan Backward` über alle 97 695 Zeilen, je Zeile eine BitmapOr-Unterabfrage (4 Index-Suchen,
  1,35 Mio. Puffer). Im Bestand gibt es keine Duplikate, also wird alles geprüft.
- Neu (`crates/pl-docfilter/src/filterset.rs::duplicate_candidates`): vor die unveränderte exakte
  Bedingung (`EXISTS(... dup ...)`, Sichtbarkeit, `id <>`, `NULLIF`) kommt
  `documents_document.id = ANY(ARRAY(<Kandidaten>))`. Kandidaten = Dokumente, deren `checksum` oder
  `archive_checksum` einem Schlüssel entspricht, der im Gesamtbestand ≥ 2× als Prüfsumme vorkommt
  (`checksum` doppelt ∪ `archive_checksum` doppelt ∪ `checksum` = `archive_checksum` eines
  Dokuments). Jedes Dokument mit Duplikat hat einen solchen Schlüssel (er steht in beiden Dokumenten)
  → **Obermenge**, die exakte Bedingung entscheidet weiter. Die drei Teile laufen als sortierte
  Index-Only-Scans (GroupAggregate bzw. Merge Semi Join, kein Hash/Sort im `work_mem`), `ARRAY()`
  ist ein InitPlan, die Kandidaten werden über den Primärschlüssel gelesen.
- `has_duplicates=false` bleibt `NOT EXISTS(...)` (Django-Form): die Negation einer großen
  Kandidatenliste müsste je Zeile das ganze Array prüfen.
- Verworfen (gemessen in `psql`): Semi-Join auf alle Prüfsummen mit `GROUP BY`/MIN/MAX (5,1 s,
  4 Seq Scans + Spill), `id IN (Kandidaten)` als Semi-Join (Planer bleibt beim Index Scan Backward,
  13 s); erst `ARRAY(...)` trennt die Kandidaten sicher von der Sortierung.
- Korrektheit: auf einer temporären Kopie mit eingestreuten Fällen (checksum doppelt, archive
  doppelt, checksum = fremdes archive, `archive_checksum = ''` mehrfach plus `checksum = ''`, eigene
  checksum = eigenes archive, Version und gelöschtes Dokument als Duplikat, `archive_checksum NULL`)
  liefern alte und neue Form dieselben 12 IDs. Antworten bei 100k vorher = nachher = Django (`count 0`).

### 8.2 Messung (Wanduhr, Median aus 3 verschränkten Runden, ms)

Runde = Rust vorher (`paperrust-w16-before.exe`, Build 14:16 ohne M8) → Rust nachher
(`paperrust-w16-after.exe`) → Django (eigener granian, **je Runde gestartet und per PID beendet**, nie
gleichzeitig mit Rust). Ports 8092/8093 (`PGV_RS_PORT`, `PGV_DJ_PORT`, `PGV_RS_PIDFILE`,
`PGV_DJ_PIDFILE`, `PGV_DJ_PER_ROUND=1` in `fix_bench.sh`). Rohdaten `results/pgvalidate/fix/w16/`
(runs.jsonl, bodies/, table.md). **Fremdlast:** während der ganzen Messung liefen ≥ 10 `find`-Prozesse
anderer Agenten mit je > 30 000 s CPU; Abweichungen < 2× sind Rauschen (vorher/nachher sind außer
`has_duplicates` dieselbe SQL – ihre Streuung 0,4×–1,7× zeigt das Rauschen).

| Anfrage | Nutzer | Django | Rust vorher | Rust nachher |
|---|---|---:|---:|---:|
| `doclist.f.has_duplicates` | admin | 7 597 | 20 516 | **3 316** |
| | alice | 6 093 | 17 392 | **2 396** |
| | bob | 6 120 | 16 296 | **1 198** |
| `doclist.correspondent` | admin / alice / bob | 3 893 / 4 363 / 3 961 | 2 962 / 2 572 / 2 476 | 3 242 / 2 701 / 1 872 |
| `doclist.default` | admin / alice / bob | 1 655 / 2 643 / 2 595 | 975 / 938 / 830 | 800 / 793 / 805 |
| `doclist.title` | admin / alice / bob | 2 125 / 1 983 / 2 423 | 994 / 718 / 1 038 | 746 / 658 / 667 |
| `doclist.page200` | admin / alice / bob | 4 582 / 4 188 / 3 427 | 2 340 / 2 656 / 2 385 | 2 241 / 2 250 / 3 462 |
| `doclist.f.untagged` | admin / alice / bob | 3 074 / 4 218 / 3 941 | 2 753 / 2 599 / 1 997 | 2 812 / 1 566 / 2 596 |
| `doclist.f.title_content` | admin / alice / bob | 24 347 / 19 950 / 17 080 | 28 511 / 24 762 / 16 946 | 28 613 / 20 697 / 16 474 |
| `doclist.f.saved_view3` | admin / alice / bob | 11 100 / 9 824 / 3 276 | 4 956 / 5 467 / 3 922 | 12 173 / 3 841 / 6 663 |
| `statistics` | admin / alice / bob | 2 270 / 5 968 / 4 424 | 1 277 / 6 291 / 3 090 | 1 127 / 5 174 / 5 912 |
| `dashboard.widget.recent` | admin / alice / bob | 1 616 / 2 193 / 2 161 | 559 / 687 / 544 | 616 / 648 / 687 |
| `dashboard.widget.inbox` | admin / alice / bob | 1 446 / 1 931 / 3 124 | 633 / 659 / 765 | 785 / 588 / 748 |
| `selection_data` (POST) | admin / alice / bob | 4 491 / 5 508 / 4 854 | 2 792 / 3 956 / 3 586 | 3 674 / 3 307 / 3 524 |

Lastunabhängig (`fix_dbtime.sh` + `fix_buffers.py`, 2 Läufe, `results/pgvalidate/fix/w16-dbtime/`):
`has_duplicates` Puffer shared hit+read **1 355 103 → 6 153** (admin), 1 915 513 → 6 152 (alice),
1 590 013 → 6 149 (bob), Roundtrips 2 → 2; DB-Zeit (sqlx) 8,1–20,1 s → 1,3–2,8 s.

### 8.3 Bewertung der übrigen Punkte (keine Codeänderung)

- **`ordering=correspondent__name`** (nach M5): Rust 1,9–3,2 s gegen Django 3,9–4,4 s – erledigt.
- **Standardliste, `title`, `page200`, `untagged`, Dashboard-Widgets:** Rust ≤ Django (meist 2–3×
  schneller). Pläne verglichen (Mitschnitt Abschnitt 1): Superuser ohne Rechte-Unterabfrage, ein
  COUNT (als InitPlan in der Seitenabfrage, Django: getrennte Abfrage), dieselbe Seq-Scan-Zählung
  wie Django (P3), Seite per Index Scan Backward wie Django. Keine überflüssige Arbeit gefunden;
  die Einzelwerte 852 vs. 688 ms aus Abschnitt 2 waren Rauschen (2 Läufe, nicht verschränkt).
  `page200` bob 3 462 vs. 3 427: Gleichstand im Rauschen (vorher 2 385 mit gleicher SQL).
- **`title_content` / `saved_view3`:** gleiche Arbeit wie Django (Rust: COUNT als InitPlan + Seite
  per Seq Scan + Top-N; Django: COUNT + Seite getrennt, beide je ein voller Scan mit
  `UPPER(COALESCE((neueste Version), content)) LIKE`). Kosten ~90 % Dekomprimieren/`UPPER` des
  Inhalts (gemessen: ohne Versions-Unterabfrage nur ~10 % schneller). Wanduhr im Rauschen um Django
  (admin 28,6 vs. 24,3 s, bob 16,5 vs. 17,1 s; saved_view3 admin 12,2 vs. 11,1 s bei vorher 5,0 s mit
  gleicher SQL). **Nicht umgesetzt:** ein einziger Scan für Seite und Zählung (`COUNT(*) OVER ()`)
  würde die Arbeit halbieren, ändert aber den Plan der Seite (Top-N statt Index-Reihenfolge) und
  damit bei Gleichständen (`created` hat bei 100k nur 3 650 verschiedene Werte) die Auswahl der
  Dokumente gegenüber Django – das verletzt „Antworten unverändert“; außerdem pl-core-Sache.
- **`statistics`:** gehört pl-system; Rust admin 1,1 s vs. Django 2,3 s, Nicht-Admins im Rauschen
  um Django (alice 5,2 vs. 6,0 s, bob 5,9 vs. 4,4 s bei vorher 3,1 s mit gleicher SQL).
- **`selection_data` POST, Reihenfolge der Korrespondenten:** nicht herstellbar, weil upstream
  selbst nicht deterministisch ist. Die Django-Abfrage (`LEFT JOIN … GROUP BY id`, ohne ORDER BY)
  läuft als HashAggregate; dessen Ausgabe hängt von der Reihenfolge der Eingabezeilen ab
  (Kollisionen), gezeigt in `psql`: derselbe Join über `documents_document` in `id DESC`-,
  `id`- und `created`-Reihenfolge gibt drei verschiedene Korrespondenten-Reihenfolgen. Die
  Scan-Reihenfolge hängt bei großen Tabellen von `synchronize_seqscans` (Startblock = zuletzt
  gemeldete Position) und Parallelität ab. Django lieferte in den drei Runden dieser Messung **drei
  verschiedene** Antworten (je Nutzer 3 verschiedene sha256), ebenso in Abschnitt 7 (Runde 1: Rust =
  Django byte-gleich, Runde 2 für alice/bob gleich, sonst verschieden). Rust setzt dieselbe SQL ab (in
  `psql` mit gleicher Eingabe identische Reihenfolge wie Djangos SQL) – mehr ist nicht möglich.
  Inhalte (Menge von `id`/`document_count`) sind gleich. Vermerkt in `docs/contract/pl-documents.md`.

### 8.4 Parität

- 100k, `fix_compare.py` (Host:Port jetzt allgemein normalisiert): Rust vorher ↔ nachher: 34 gleich,
  2 nur Reihenfolge (`selection_data` POST, s. o.), 0 verschieden. Rust nachher ↔ Django: 24 gleich,
  4 nur Reihenfolge (`selection_data` ×3, `correspondent` alice), 8 verschieden – davon 6 Gleichstände
  der Sortierung (`title_content` ×3, `page200` alice/bob, `correspondent` bob: `count` gleich, Folge
  der Sortierschlüssel `created` bzw. Korrespondent auf der Seite identisch; Django selbst lieferte
  hier 2–3 verschiedene Antworten in 3 Runden) und 2 × `statistics.current_asn` (Known difference
  #14710 von pl-system). `has_duplicates` für alle 3 Nutzer gleich.
- Referenz (200 Dokumente, Django :8000, nur lesend): pl-docfilter `parity` same 318 / tie_only 44 / different 0 (unverändert, inkl. `has_duplicates=1`/`=0`); pl-documents `parity` routes 1 398 / 0, docfilter 188 / 0, search 41 / **6** – die 6 sind wie zuvor `more_like_id` (bewusst an Django delegiert, 501 im In-Process-Test; deshalb meldet der Test `FAILED`, unabhängig von M8). Logs `results/pgvalidate/fix/w16-parity-*.log`.
- `cargo test -p pl-docfilter -p pl-documents -j 1` grün, `cargo clippy -p pl-docfilter -p
  pl-documents --all-targets -- -D warnings` sauber. Erwartungs-SQL in
  `pl-docfilter/tests/filterset.rs::shared_by_and_duplicates` um den Vorfilter ergänzt.

## 9. Korrekturen, Runde 3 (2026-11-08/09, O2: Teilstringsuche im Inhalt, Serializer)

Gleiche Regeln wie Abschnitte 7/8. Kein Schema geändert, **keine Migration** (s. 9.2),
`pl_pgvalidate` nur gelesen (auto_explain nur für die Puffer-Messung per `ALTER DATABASE`, danach
`RESET ALL`, geprüft: 0 Einträge in `pg_db_role_setting`). Ports 8098/8099, eigene PID-Dateien.
Binaries `results/pgvalidate/bin/paperrust-o2-{before,m9,after,after2}.exe` (Debug; `before` =
Arbeitsbaum 22:50 vor O2, enthält aber Zwischenstände anderer Agenten, ebenso `after2`).

### 9.1 Profil `title_content` (100k, ICU `en-US`)

`UPPER(content)` der Kollation ≈ 2/3 der Zeilenkosten, Dekomprimieren (pglz, ~2,4 kB) ≈ 1/3
(`count(*) … WHERE length(content)>=0` 2,5 s, `UPPER(content COLLATE "C") LIKE` 2,9–4,2 s,
`UPPER(content) LIKE` 6,7–17 s je nach Fremdlast). Rust brauchte **zwei** volle Scans (COUNT als
InitPlan + Seite per Seq Scan/Top-N), Django ebenso (COUNT + Seite getrennt). Treffer: `rechnung`
55 % aller Dokumente, `rent` 99 %, `strasse` 0.

### 9.2 Bewertete Optionen

| Option | Bewertung |
|---|---|
| pg_trgm-GIN auf `UPPER(content)` (`paperrust_`-Index, expand-only; `pg_trgm` 1.6 ist verfügbar) | **nicht umgesetzt.** LIKE über Trigramme ist immer verlustbehaftet → Recheck mit dem Indexausdruck (`UPPER` der Kollation) für jede Kandidatenzeile; bei den typischen dichten Suchbegriffen (55–99 % Treffer) ist das teurer als der Seq Scan. `effective_content` (`COALESCE(neueste Version, content)`) ist nicht indexierbar, bräuchte einen Kandidaten-Vorfilter. Hilft nur seltenen Begriffen; dafür ~1000 Trigramm-Einträge je Dokument Schreiblast. Bleibt Option für seltene Begriffe (offener Punkt). |
| Seite und Zählung getrennt mit LIMIT-Abbruch | hilft der Seite nur bei dichten Treffern und nur, wenn der Planer den Index auf `created` wählt (er schätzt das LIKE auf 20 Zeilen → Seq Scan); die Zählung bleibt ein voller Scan. Verworfen. |
| **M9** ergebnisgleiche Faltung `UPPER(x COLLATE "C")` + `replace()` der Sonderzeichen | umgesetzt (`pl-docfilter/src/textfold.rs`, Begründung der Gleichheit im Modulkopf, Vertrag pl-docfilter „Schnelle Textsuche“). Nur ASCII-Suchtexte, Tabelle der Sonderzeichen aus der DB kalibriert, ≤ 4 Ersetzungen (sonst Django-Form). |
| **M10** `COUNT(*) OVER ()` statt InitPlan, nur bei Teilstringsuche im Inhalt | umgesetzt (`pl_core::pagination::fetch_page_windowed`, Aufruf in `pl-documents/src/views.rs::list` über `pl_docfilter::has_content_text_filter`). Ein Scan statt zwei; Plan der Seite bleibt Seq Scan → (WindowAgg) → Top-N, Gleichstände wie bisher planabhängig (s. 9.5). |

Abschnitt 8.3 hatte `COUNT(*) OVER ()` verworfen, weil es den Plan von Index-Reihenfolge auf Top-N
ändere – das gilt für Listen ohne Inhaltsfilter (dort weiter `fetch_page`), nicht für die
Inhaltssuche, deren Seite bei Rust und Django schon vorher Seq Scan + Top-N war.

### 9.3 Maßnahme M11 – Serializer der Dokumentliste/-detail (`pl-documents/src/serializer.rs`)

- Eine Dokumentabfrage liest Tags (`ARRAY(… ORDER BY name)`), Notizen (`json_agg(… ORDER BY
  created)`, Zeit als Epoch-µs) und „hat Versionen“ (`EXISTS`) als korrelierte Index-Unterabfragen mit;
  `LEFT(content, 550)` bei `truncate_content` (statt `chars().take(550)` in Rust); Versionen und
  Inhalt der neuesten Version nur für Dokumente mit Versionen. Custom Fields bleiben eigene Abfrage
  in Djangos Form (104 Gleichstände `created` je Dokument in der Referenz – nur so gleiche Reihenfolge;
  ein erster Versuch mit `json_agg` lieferte 4 Abweichungen im Paritätslauf und wurde zurückgenommen).
- Werte werden aus den Zeilen verschoben statt geklont, `Value` direkt statt `json!`-Umwege.
- Roundtrips (auto_explain-Mitschnitt): Dokumentliste **11 → 7**, Detail **12 → 8**, Puffer +2 %
  (Index-Proben der Unterabfragen; `doclist.default` admin 21 787 → 22 199).

### 9.4 Messung

Wanduhr, Median aus 3 verschränkten Runden (Rust vorher → Rust nachher → Django, Django je Runde
gestartet/per PID beendet), ms, `results/pgvalidate/fix/o2/final/` (Rohdaten, `table.md`). Ruhigere
Phase als in Abschnitt 8 (ein anderer Agent benchmarkte zeitweise auf `pl_bench`). Ein erster Lauf
mit stärkerer Fremdlast (`results/pgvalidate/fix/o2/main/`) zeigt dieselbe Richtung.

| Anfrage | Nutzer | Django | Rust vorher | Rust nachher |
|---|---|---:|---:|---:|
| `doclist.f.title_content` (`rechnung`, 55 % Treffer) | admin / alice / bob | 5 866 / 3 739 / 2 927 | 6 079 / 4 122 / 3 426 | **2 173 / 1 503 / 1 138** |
| `doclist.f.title_content+sel` (wie das Frontend) | admin / alice / bob | 14 223 / 9 709 / 7 191 | 11 294 / 7 136 / 5 916 | **6 033 / 3 819 / 2 953** |
| `doclist.f.title_content.p2` (Seite 2) | admin / alice / bob | 5 466 / 4 034 / 3 010 | 6 849 / 4 241 / 3 342 | **2 584 / 1 504 / 1 033** |
| `doclist.f.content_icontains` (`quarterly`) | admin / alice / bob | 5 522 / 3 481 / 2 860 | 6 087 / 3 660 / 3 106 | **3 443 / 1 876 / 1 512** |
| `doclist.f.search` (`search=rechnung`) | admin / alice / bob | 5 916 / 6 284 / 6 150 | 3 496 / 2 193 / 1 794 | **2 619 / 1 496 / 1 195** |
| `doclist.f.saved_view3` (`storage_path` + `rent`) | admin / alice / bob | 780 / 781 / 556 | 1 034 / 910 / 509 | 872 / 749 / 386 |
| `doclist.f.title_content.sparse` (`strasse`, 0 Treffer, Django-Form) | admin / alice / bob | 2 465 / 1 728 / 1 418 | 3 382 / 1 998 / 1 692 | 4 012 / 2 178 / 1 416 |
| `doclist.f.title_content.umlaut` (`gebühr`, Django-Form) | admin / alice / bob | 2 475 / 1 673 / 1 386 | 3 368 / 1 961 / 1 653 | 3 906 / 2 086 / 1 480 |
| `doclist.default` | admin / alice / bob | 328 / 476 / 457 | 206 / 234 / 225 | 205 / 224 / 242 |
| `doc.retrieve` | admin / alice / bob | 134 / 161 / 156 | 31 / 32 / 43 | 62 / 14 / 33 |
| `dashboard.widget.recent` | admin / alice / bob | 290 / 428 / 411 | 165 / 224 / 212 | 178 / 239 / 196 |

Lastunabhängig (`results/pgvalidate/fix/o2/dbtime/summary.txt`, Puffer shared hit+read / Roundtrips):
`title_content` admin 433 077 / 11 → 217 650 / 9 (2 davon einmalige Kalibrierung), alice
274 925 / 11 → 138 573 / 7; `+sel` 698 557 / 17 → 483 125 / 13; `content_icontains` 433 022 → 217 589;
`search` und `saved_view3` (Index-Scan über `storage_path`) Puffer gleich, Roundtrips 11 → 7.
CPU-Anteil M9 allein (psql, verschränkt, `count(*)` über 100k, ms): `rechnung` Kollation
3 620 / 3 544 / 3 255 gegen Faltung (2 Ersetzungen) 2 511 / 2 511 / 2 614 (−25…−30 %); `strasse`
Kollation 2 851 / 3 632 / 3 657 gegen Faltung mit 6 Ersetzungen 3 889 / 3 996 / 3 807 (+5…+35 %) →
Grenze `MAX_REPLACES = 4`, `strasse` nimmt seitdem die Django-Form.

**Bewertung:** Inhaltssuche mit Treffern jetzt **2,4–3,2× schneller als vorher und 2,5–3× schneller
als Django**, mit selection_data (reale UI-Anfrage) 2,4–2,5× schneller als Django. `saved_view3`
schneller als vorher und als Django (alice/bob), admin im Rauschen. **Nicht schneller als Django:**
Suchbegriffe ohne Treffer bzw. ohne Faltung (`strasse`, `gebühr`): gleiche SQL wie Django, in `psql`
gleich schnell (Zählung 2,5–3,1 s vs. Fensterform 2,6–3,4 s, verschränkt); die Wanduhrdifferenz
liegt in Reihenfolge-/Cache-Effekten (Wiederholungen derselben Abfrage schwanken 2,5 ↔ 3,9 s, bei
Rust vorher wie nachher). Nur ein Index (9.2) würde hier helfen. Listen ohne Inhaltsfilter
unverändert schnell (Roundtrips −4).

`tests/bench/bench.py` (`pl_bench`, 195 Dokumente, 8 Clients, 8 s, 3 verschränkte Runden, nur
Rust, `results/pgvalidate/fix/o2/bench/summary.txt`): `documents_list` 251 → 310 Anfr./s (p50
31,0 → 25,1 ms), `document_retrieve` 549 → 602 (p50 14,4 → 13,2 ms) – aber Kontrollen ohne
O2-Änderung stiegen ebenfalls (`tags_all` 567 → 636, `correspondents_all` 893 → 1 084;
`ui_settings`/`statistics` gleich), weil `after2` auch spätere Änderungen anderer Agenten enthält.
Der Netto-Anteil von M11 ist daher nur über die Roundtrips belegt (11 → 7 bzw. 12 → 8).

### 9.5 Parität

- 100k, gleichstandsbewusst (`tests/pgvalidate/fix_compare_ties.py`: `count`, Folge der
  Sortierschlüssel, innere Gleichstandsgruppen und gleiche Dokumente identisch; nur die Gruppe am
  Seitenrand darf andere Dokumente enthalten): Rust nachher ↔ Django **30 gleich, 3 Gleichstand**
  (`saved_view3` ×3), 0 verschieden; Rust vorher ↔ Django 30 gleich, 3 Gleichstand (`search` ×3);
  vorher ↔ nachher 27 gleich, 6 Gleichstand. `title_content` (auch Seite 2, `+sel`),
  `content_icontains`, `doclist.default`, `doc.retrieve` byte-gleich mit Django. Jede Binary liefert
  in allen Runden dieselbe Antwort (sha256 je Runde gleich).
- Referenz (Django :8000, nur lesend): pl-docfilter `parity` same **334** (318 + 16 neue Fälle:
  `title_content=strasse|fi|ß|a_b%`, `content__iexact/iendswith/icontains=SS`, `search=ss`) /
  tie_only 44 / different **0**; pl-documents `parity` routes 1 398 / 0, docfilter 188 / 0,
  search 41 / 6 (die 6 wie bisher `more_like_id`, an Django delegiert). Logs
  `results/pgvalidate/fix/o2/parity-*.log`.
- Faltung gegen die echte Kollation: `crates/pl-docfilter/tests/textfold_db.rs` (12 168 Vergleiche
  Text × Muster × Lookup, unbegrenzte Ersetzungen) gleich; Zählungen bei 100k alte = neue Form
  (`rechnung` 53 936, `strasse`/`fil` 0).
- `cargo test -p pl-docfilter -p pl-documents -j 1`, `cargo test -p pl-core --lib pagination`
  grün; `cargo clippy -p pl-docfilter -p pl-documents -p pl-core --all-targets -- -D warnings` sauber.

### 9.6 Migrationen / Rückbau

Keine. M9–M11 sind reine Abfrageformen; Rückbau = Code zurücknehmen. Die Kalibrierung liest nur
(`pg_database`, `pg_attribute`, `upper(chr(i))`).
