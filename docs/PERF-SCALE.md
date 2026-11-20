# PaperRust – Messung mit 1 Mio. Dokumenten (Leistungsbasis vor dem Altlasten-Abbau)

Stand 2026-11-10. Ziel: herausfinden, wo das Backend bei großen Beständen wirklich langsam wird,
**bevor** die Altlasten L1–L6 (docs/ARCHITECTURE-V2.md §5) ersetzt werden. Werkzeuge und
Reproduktion: [`tests/bench/scale/`](../tests/bench/scale/README.md); Rohdaten:
`results/bench/scale/2026-11-10/` (`results.json`, `summary.md`, `profile.json`, `explain.txt`,
`idx-with/`, `idx-without/`, `scenarios.json`).

## 1. Kurzfassung

1. **Die Angular-UI ist bei 1 Mio. Dokumenten in den Grundansichten unbenutzbar** – nicht wegen der
   Liste selbst (≈ 1 s), sondern wegen Beiwerk, das jede Ansicht mitlädt:
   `include_selection_data=true` (jede Dokumentliste der UI) **31 s** (admin) / 13 s (eingeschränkt),
   mit Volltextsuche **92 s**; `POST selection_data` beim Markieren 9–43 s (100 Dok.) bzw.
   **> 2 min** (10 000 Dok. / „alle auswählen“); Tag-Verwaltung 10–14 s; Custom-Field-Liste 4–11 s.
2. Ursache fast immer **dieselbe Form**: Zählungen/Aggregate über die **ganze** Dokumentmenge bzw.
   die 5-Mio.-Zeilen-Zwischentabelle, wo upstream (Django-ORM) sie vorgibt (`Count(Case(When(id__in)))`,
   `Count(distinct)`, korrelierte Unterabfragen je Zeile, ID-Listen als Text-Parameter). Das ist
   Altlast **L1/L5/L6** (Abfrageformen des ORMs 1:1 nachgebaut), nicht Rust.
3. **Kein N+1**: 1–21 SQL-Abfragen je Request (Dokumentliste 5), die Zeit steckt zu > 95 % in
   einzelnen schweren Abfragen. Rust-Anteil nur bei Volltext (tantivy ≈ 1,9 s für „alle Treffer
   sortiert“), `search/` (1,3 s), Vorschlägen (0,7 s) und dem v2-Zeitstrahl (1 s) nennenswert.
4. **Ein einziger Teilindex** (`paperrust_doc_live_created`, bereits vom L1-Agenten als
   pl-store-Migration vorbereitet) macht Liste/Paginierung/Dashboard **2,5–7× schneller und
   5–13× durchsatzstärker** (admin `docs_default` 771 → 263 ms, 2,6 → 27 req/s). Parität-neutral.
5. Volltext (L3) skaliert ordentlich für seltene Begriffe (< 10 ms), aber häufige Begriffe kosten
   1,6–5 s, weil upstreams „alle Treffer ranken, dann mit allen sichtbaren IDs schneiden“
   nachgebaut ist. Klassifikator (L4) ist bei 1 Mio. nicht trainierbar (Abschnitt 4.3).
6. Die **v2-API mit Cursor-Paginierung ist schnell** (20–60 ms für `/rs/api/v2/documents`,
   Sync-Feed 70–110 ms je 500er-Seite) – sie zahlt sich aus, sobald `count=true` weggelassen wird.
   Ausnahme: `sync/ids` (13–25 s, schreibt bei jedem Aufruf 1 Mio. Zeilen-Konfliktprüfungen).
7. **Massenänderungen laufen Stunden**: Der Nachlauf von `bulk_edit` (122 ms/Dokument → 20 min
   für 10 000) und das Umbenennen eines Speicherpfads (260 ms/Dokument → ≈ 3,6 h für 50 000)
   verarbeiten jedes Dokument einzeln (Workflows, Vorlage, Datei, Index). Der Sync-Sequenzer kam
   danach > 25 min nicht hinterher und hielt dabei eine Cluster-weite Sperre (`ProcSignalBarrier`)
   fest – das betraf auch andere Datenbanken im Cluster.

## 2. Aufbau

### 2.1 Rechner und Instanz

- Windows-11-Laptop, 12 Threads, 16 GB RAM, **geteilt mit 1–2 anderen Agenten** (Streuung der
  c=8-Werte ±30 %; c=1-Mediane sind belastbarer). Kein Docker.
- PostgreSQL 18.6 = Referenz-Cluster (`reference/`), **unveränderte Clustereinstellungen**:
  `shared_buffers` 32 MB, `work_mem` 4 MB, `max_parallel_workers_per_gather` 1, `jit` an,
  `max_wal_size` 256 MB. Nur für `pl_scale_1m`: `effective_cache_size = 4GB` (wie das Docker-Image
  `postgres`-Standard). Kleine `shared_buffers` → alles liest aus dem OS-Cache (Windows), große
  Scans sind CPU-gebunden. Docker-Standard wäre 128 MB – Größenordnungen ändern sich dadurch nicht.
- Binärdatei: `cargo build --release -p pl-server` (Profil `release`, LTO wie Workspace), gebaut aus
  dem `main`-Arbeitsbaum 2026-11-10 15:17 (HEAD 5844ec8 = v0.1.0 + Light-UI-Fix bfb0c4d). Die zu
  diesem Zeitpunkt unfertigen L1-Dateien eines anderen Agenten (`pl-store/src/docsql.rs`,
  `pl-docfilter/src/ast.rs`) sind **nicht** enthalten (geprüft: keine Quellpfade im Binary) →
  gemessen ist der **bisherige Filterweg (legacy)**.
- `paperrust serve` ohne Django (`results/parity/routes-all.toml`), Queue `postgres`, Cron-Aufgaben
  aus; Szenarien mit `serve --worker`.

### 2.2 Datensatz (`tests/bench/scale/gen.py`, Seed 20261110)

| Objekt | Menge | Verteilung |
|---|---:|---|
| Dokumente | 1 000 000 (2 948 im Papierkorb) | `created` 2011-11 … 2026-11, Bestand wächst linear (neuere Jahre dichter) |
| Inhalt | 2,22 GB, Ø 2 212 Zeichen (0,5–5 KB, log-gleichverteilt) | 60 % de / 40 % en; Grundwortschatz + 2 × 40 000 Pseudowörter, Zipf 1,05; Kopf mit Absender, Adresse, Datum, Rechnungs-/Kundennummer, Betrag, IBAN-artige Nummer |
| Korrespondenten | 5 000 | Zipf 1,1 über Dokumente (Top-1 ≈ 10 %), 8 % ohne |
| Dokumenttypen | 60 | Zipf 1,0, 12 % ohne |
| Tags | 800, 100 Wurzeln, bis Ebene 4 (700 verschachtelt) | 0–8 je Dokument (Zipf), Vorfahren mitgesetzt wie upstream → **5,05 Mio.** Zuordnungen (Ø 5); Inbox-Tag auf 22 629 Dok. |
| Speicherpfade | 200 | 45 % ohne; Pfad 1 „Scale rename“ = genau 50 000 Dokumente mit echten Dateien |
| Custom Fields | 30 (alle 10 Typen) | **3,0 Mio.** Instanzen (Ø 3, Zipf über Felder, 8 % leer) |
| Notizen | 99 792 auf 5 % der Dokumente | 1–3 je Dokument |
| Nutzer / Gruppen | 200 + admin / 30 | Nutzer in 1–3 Gruppen; Modellrechte über Gruppen |
| Besitzer | 30 % admin, 62 % Zipf über Nutzer, 8 % ohne (79 813) | |
| guardian-Objektrechte | 239 709 Nutzer- + 356 471 Gruppen-Zeilen auf 200 344 Dokumenten (20 %) | 1–3 Freigaben, 60 % Gruppen, view (+ change bei 50 %); dazu Freigaben auf eigene Labels |
| ASN | 600 000 (60 %) | steigend mit der id |
| Saved Views | 300 (1–3 Regeln: Tags, Korrespondent, Typ, Datum, Inbox, title_content, custom_field_query, Besitzer) | Dashboard: admin 8, eingeschränkt 6 |
| Workflows | 50 (Trigger Consumption/Added/Updated; Scheduled deaktiviert), nur Zuweisen/Entfernen | |
| Mail | 5 Konten (`*.invalid`), 40 Regeln | Abruf aus |
| Dateien | 52 000 Kleinst-PDFs + WebP-Vorschau (2 000 zufällig + 50 000 im Speicherpfad 1), 138 MB | übrige Dokumente ohne Datei |

Messnutzer: **admin** (Superuser) und **restricted** (`user022`, Gruppen 6 und 9, ≈ 4 900 eigene
Dokumente, sieht ≈ 100 000 Dokumente über „ohne Besitzer“ + Gruppen-/Nutzerfreigaben).

### 2.3 Aufbauzeiten und Größen

| Schritt | Zeit | Ergebnis |
|---|---:|---|
| `paperrust manage migrate` (leere DB) | 6 s | Schema v3.4.2 + 9 `paperrust_*`-Migrationen |
| `gen.py` (Erzeugen + COPY, Trigger aus) | 26 min (Dokumente 23 min, davon Python-Texterzeugung > 50 %) | 4,47 GB DB |
| `paperrust_sync_rebuild()` | 102 s | 1 003 674 Sync-Objekte |
| `VACUUM ANALYZE` | 28 s | |
| `document_index reindex` (tantivy) | **911 s (15,2 min)**, Peak 2,2 GB Working Set / 0,6 GB privat | Index **3,6 GB** |
| `document_create_classifier` | siehe 4.3 | |

DB-Größe 4 465 MB: `documents_document` 2 647 MB (Heap 931 MB – Inhalt bleibt komprimiert im Heap,
weil < 2 KB), Custom-Field-Instanzen 697 MB, Tag-Zuordnungen 513 MB, Sync-Tabellen 422 MB,
guardian 141 MB.

## 3. Methode

`tests/bench/scale/run.py`: je Endpunkt und Nutzer ein Aufwärmaufruf, dann **c=1** (bis 8 Aufrufe /
40 s, p50/p95) und **c=8** (8 Keep-Alive-Clients, 10 s, p50 + req/s). Über 8 s Median wird c=8
ausgelassen, über 120 s abgebrochen (Server-Abfrage per `pg_cancel_backend` beendet). API-Version
10 wie die Angular-UI; `ui_*` = genau die Parameter der UI (`truncate_content=true&include_selection_data=true`),
alle übrigen Listen **ohne** `selection_data`, damit die Filterkosten isoliert sichtbar sind.
Abfragen je Request: zweiter Lauf mit `PAPERRUST_LOG=…,sqlx::query=debug` (Profilmodus, je Endpunkt
ein Aufruf, Hintergrundabfragen von Queue/Sequenzer herausgefiltert) → `profile.json`.
EXPLAIN (ANALYZE, BUFFERS) der schwersten Formen: `tests/bench/scale/explain.sql` → `explain.txt`.

## 4. Ergebnisse

### 4.1 Endpunkte (Median ms, c=1; c=8-Durchsatz req/s) – ohne neuen Index

`–` = c=8 ausgelassen (Median > 8 s). Vollständige Tabelle: `results/bench/scale/2026-11-10/summary.md`.

| Endpunkt | admin p50 | admin p95 | admin c8 req/s | restricted p50 | restricted p95 | restricted c8 req/s | SQL/Req |
|---|---:|---:|---:|---:|---:|---:|---:|
| **UI-Liste Standard** (`include_selection_data`) | **31 238** | 31 733 | – | **12 783** | 15 543 | – | 33 |
| UI-Liste mit Korrespondent-Filter | 72 | 157 | 70.8 | 234 | 272 | 23.2 | 11 |
| **UI-Liste + Volltext „rechnung“** | **91 570** | 93 237 | – | **11 269** | 11 402 | – | – |
| Liste Standard (`-created`) | 992 | 1 202 | 2.1 | 1 203 | 1 246 | 2.4 | 5 |
| Liste API v9 (Feld `all` = alle IDs, 6,9 MB) | 5 404 | 5 685 | 0.6 | 4 332 | 4 940 | 0.7 | 6 |
| Sortierung Titel / Korrespondent | 1 144 / 1 837 | | 2.1 / 1.1 | 1 197 / 2 672 | | 2.0 / 0.7 | 5–7 |
| Sortierung Notizanzahl | 2 162 | 2 250 | 1.0 | 2 563 | 2 965 | 0.9 | 5 |
| **Sortierung Custom Field** | **8 961** | 9 072 | – | 4 431 | 4 820 | 0.5 | 8 |
| Seite 2000 | 1 642 | 1 803 | 2.1 | 5 141 | 5 388 | 0.6 | 5 |
| `fields=id&page_size=100000` | 3 289 | 3 688 | 1.1 | 11 184 | 12 153 | – | 3 |
| Filter Tag any / all / none | 674 / 1 729 / 1 579 | | 6.4 / 1.4 / 1.8 | 3 524 / 2 428 / 3 904 | | 0.8 / 0.9 / 0.5 | 5 |
| Filter verschachtelter Wurzel-Tag | 408 | 669 | 16.5 | 1 953 | 2 267 | 1.1 | 5 |
| Filter Korrespondent | 28 | 38 | 141 | 208 | 237 | 17.4 | 5 |
| Filter Datumsbereich (1 Jahr) | 255 | 657 | 17.2 | 494 | 552 | 9.6 | 5 |
| `custom_field_query` AND (int > , date ≥) | 643 | 1 122 | 7.4 | 1 128 | 1 309 | 4.3 | 6 |
| `custom_field_query` select exact | 1 957 | 2 054 | 5.2 | 1 134 | 1 260 | 3.0 | 6 |
| `custom_field_query` OR mit icontains | 3 331 | 3 607 | 1.3 | 5 887 | 7 782 | 0.4 | 8 |
| `custom_fields__icontains` | 2 717 | 2 861 | 0.8 | 10 140 | 12 031 | – | 6 |
| Besitzer-Filter / „von mir geteilt“ | 95 / 907 | | 55.9 / 6.7 | 182 / 1 135 | | 26.9 / 3.8 | 5 |
| **`title_content` häufig / nicht vorhanden** | **38 638 / 42 326** | | – | **9 087 / 24 746** | | – | 5 |
| Inbox | 646 | 686 | 3.1 | 2 682 | 3 300 | 0.9 | 5 |
| Kombination (Korr. + Tag + Datum) | 1 031 | 1 299 | 3.5 | 2 141 | 2 333 | 2.3 | 7 |
| Volltext häufiges Wort / 3 Wörter | 1 904 / 1 595 | | 1.5 / 1.9 | 3 382 / 4 677 | | 1.0 / 0.7 | 6 |
| Volltext seltenes Wort (0 Treffer) | 0.9 | 1.2 | 3 079 | 5.3 | 5.6 | 962 | 1 |
| `more_like_id` | 1 798 | 2 038 | 1.6 | 5 188 | 5 650 | 0.5 | 7 |
| Globale Suche `/api/search/` | 426 | 487 | 10.2 | 1 416 | 1 531 | 4.1 | 11–15 |
| **`selection_data` 100 Dok.** | **9 080** | 9 220 | – | **43 398** | 48 253 | – | 10 |
| **`selection_data` 10 000 Dok. / alle** | **> 120 000** | | – | **> 120 000** | | – | – |
| **Statistik** | 1 809 | 1 886 | 1.1 | **9 479** | 10 352 | – | 3–5 |
| **Tags Seite (25) / alle** | **14 047 / 9 787** | | – / – | 7 484 / 7 285 | | 0.5 | 4–6 |
| Korrespondenten Seite / alle (5 000) | 602 / 809 | | 3.0 / 3.0 | 4 029 / 3 785 | | 0.5 | 4 |
| Dokumenttypen / Speicherpfade alle | 570 / 588 | | 3.6 / 3.2 | 4 229 / 6 281 | | 0.4 / 0.4 | 4 |
| **Custom Fields alle (30)** | **4 221** | 5 142 | 0.8 | **11 262** | 12 407 | – | 3–5 |
| Saved Views / UI-Settings / Nutzer | 10 / 2 / 28 | | 544 / 2 678 / 153 | 14 / 5 / 403 | | 571 / 1 619 / – | 2–5 |
| Dokument abrufen / Notizen / Metadaten | 14 / 2 / 8 | | 699 / 2 399 / 1 364 | 22 / 5 / 16 | | 200 / 1 293 / 737 | 3–7 |
| Vorschläge (ohne Klassifikator) | 601 | 672 | 7.0 | 1 052 | 1 150 | 5.2 | 9 |
| Vorschaubild / Download | 3 / 2 | | 1 894 / 2 398 | 6 / 4 | | 1 082 / 1 426 | 3–5 |
| v2 `/documents` (Cursor, 50) | 19 | 21 | 325 | 57 | 62 | 92 | 15 |
| v2 `/documents?count=true` | 618 | 759 | 2.9 | 1 762 | 1 841 | 0.9 | 16 |
| v2 `/documents/query` Tag + Datum (+count) | 391 | 441 | 8.0 | 832 | 866 | 4.1 | 17 |
| v2 `/documents/query` Text | 59 | 65 | 95 | 61 | 65 | 76 | 15 |
| v2 `/timeline` (1 Jahr) | 2 441 | 3 101 | 1.4 | 7 009 | 7 764 | 0.5 | 16 |
| v2 Sync-Feed erste Seite (500, embed) | 113 | 129 | 48.8 | 66 | 78 | 85.1 | 20 |
| **v2 `sync/ids?kind=document`** | **12 872** (42,9 MB) | 13 198 | – | **25 134** (4,3 MB) | 26 859 | – | 5–7 |
| Dashboard admin (8 Widgets) | 4–922 je Widget, Summe ≈ 4,0 s | | | | | | 3–5 |
| Dashboard restricted (6 Widgets) | | | | 522–2 656 je Widget, Summe ≈ 9,2 s | | | 5–6 |

Beobachtungen:
- **Eingeschränkter Nutzer** ist bei Filtern 2–5× langsamer als admin (Sichtbarkeit
  `owner = u OR owner IS NULL OR CAST(id AS varchar) IN (guardian…)` erzwingt einen Scan über
  alle Dokumente, weil sie nicht indexierbar ist) und bei Label-Listen 5–10× (Zählung je Label mit
  derselben Sichtbarkeitsbedingung). Umgekehrt ist er bei Gesamt-Scans (title_content, UI-Liste)
  schneller, weil nur 10 % der Zeilen übrig bleiben.
- c=8 bringt bei den schweren Endpunkten **kaum Durchsatz** (2–3 req/s): CPU-gebunden in PostgreSQL
  (ein Kern je Abfrage, `max_parallel_workers_per_gather = 1`).

### 4.2 Schreib-Szenarien (`--scenarios`, `serve --worker`, umkehrbar)

| Szenario | Antwort der API | Nachlauf (Worker) | je Dokument |
|---|---:|---:|---:|
| Einlesen 50 kleine Textdateien (`post_document`, kein OCR, 50 Workflows aktiv) | Upload aller 50: 1,1 s | bis alle `SUCCESS`: **39,5 s** (Aufgaben einzeln 0,37–0,45 s, nacheinander) | **790 ms** (10 000-Dok.-DB: 1 013 ms – unabhängig von der Bestandsgröße) |
| `bulk_edit add_tag` auf 9 980 Dokumente per „alle auswählen“ (`all=true` + Filter) | **6,7 s** | `bulk_update_documents`: **20 min 19 s** | **122 ms** |
| `bulk_edit remove_tag` (gleiche Auswahl) | **8,2 s** | abgebrochen (Abschnitt „Sequenzer“ unten); Rate wie add_tag | – |
| `selection_data` für dieselbe Auswahl (`all=true` + Filter, ruft die UI beim Markieren) | **> 6,8 min** (vom Client abgebrochen) | – | – |
| Speicherpfad mit 50 000 Dokumenten (mit Dateien) umbenennen (`PATCH /api/storage_paths/1/`, neue Vorlage `renamed/{{ created_year }}/{{ correspondent }}/{{ title }}`) | **1,1 s** | nach 10 min erst **2 300 von 50 000** verschoben (7,5 Dok./s in der ersten halben Minute, fallend auf 2,3 Dok./s); hochgerechnet **≈ 3,6 h** (eher mehr, Rate sinkt) | **260 ms** (Mittel) |

Umbenennung danach per `terminate_requested` abgebrochen und der Ausgangszustand direkt
wiederhergestellt (Pfad per SQL, 2 340 Dateien zurückverschoben, 11 s) – ein Rück-PATCH hätte
weitere Stunden gedauert.

Was der Nachlauf je Dokument tut (upstream `bulk_update_documents`, in PaperRust 1:1): Cache leeren,
**alle „Document Updated“-Workflows auswerten und ggf. Aktionen anwenden** (im Datensatz 13 von 50
Workflows mit Update-Trigger; Aktionen ändern dabei weitere Felder), Dateinamen aus der Vorlage neu
rendern und ggf. Datei verschieben, danach Suchindex für alle IDs. Zeit geht überwiegend in die
sequentielle Einzelverarbeitung (Workflow-Matching + Einzel-SQL je Dokument), nicht in PostgreSQL-
Scans – darum unabhängig von der Bestandsgröße, aber linear in der Auswahl.

**Sync-Sequenzer unter Last:** nach dem 10 000-Dokumente-`bulk_edit` lagen 131 227 Zeilen in
`paperrust_sync_pending`; ein Aufruf von `paperrust_sync_sequence()` lief **> 25 min** (IO-gebunden)
ohne fertig zu werden. Jeder Lauf verbindet die Warteliste per `LEFT JOIN` mit der Sicht
`paperrust_sync_live` (UNION über alle Tabellen) → **Seq Scan über `documents_document` (931 MB)
je Lauf**. Nebenwirkung auf dem geteilten Cluster: Während dieser Abfrage blieb eine
`ProcSignalBarrier` hängen – `DROP DATABASE … WITH (FORCE)` anderer Agenten wartete 15 min, ≈ 90 neue
Verbindungen hingen in `starting`. Erst das Beenden der Messinstanz löste die Sperre; danach
`paperrust_sync_rebuild()` (39 s) zum Aufräumen. → eigener Engpass #10.

### 4.3 Suchindex und Klassifikator

| Bestand | Suchindex `reindex` | Klassifikator `document_create_classifier` |
|---|---|---|
| 10 000 Dok. (Probelauf) | 9,8 s, 43 MB | 107,5 s, Modell 6,9 MB |
| 100 000 Dok. (`pl_scale_1m_t100k`, gleicher Generator) | – | **> 17 min, nicht fertig** (97 451 Trainingsdokumente, Vektorisieren ≈ 2 min, danach MLP-Training; 1,7 GB privat, 1 175 CPU-s) – abgebrochen, weil der geteilte Rechner unter 1,5 GB freien RAM fiel |
| 1 000 000 Dok. | **911 s, 3,6 GB**, Peak 2,2 GB WS / 0,6 GB privat (Puffer 6 650 Dok./Segment) | **nicht trainierbar**: nach 303 s noch in „Vectorizing“ (974 475 Trainingsdokumente, Inhalte in 1 000er-Blöcken), **5,6 GB privat** → vom Messskript beim Deckel 5 GB abgebrochen |

Ursache (L4): Nachbau von sklearn `CountVectorizer(ngram_range=(1,2))` + `MLPClassifier` über
**alle** Dokumente; die Bigramm-Vokabel und die Dokument-Term-Matrix wachsen mit dem Bestand, das
MLP-Training (bis 200 Epochen, je Ziel ein Netz – 3 182 Korrespondenten-Klassen) ist superlinear.
Upstream (Python) hat dasselbe Verhalten. Vorschlag L4: Training auf eine Stichprobe (z. B. jüngste
50 000 je Klasse begrenzt), Hashing-Vectorizer mit fester Breite, inkrementelles Nachtrainieren
(SGD/Naive Bayes, `pl-classifier` hat `nb` bereits), oder KI-gestützte Zuordnung (`pl-ai`).
Vorschläge (`/suggestions/`) ohne Modell: 0,6–1 s (Matching über 5 000 Korrespondenten).

### 4.4 Abfragen je Request (N+1-Prüfung)

`profile.json`: Dokumentliste 5 Abfragen (Benutzer, Seite + COUNT-InitPlan, Objektrechte der 25
Treffer, Serialisierung in einer Abfrage mit Unterabfragen für Tags/Notizen), Label-Listen 3–6,
UI-Liste mit `selection_data` 33 (5 Aggregate + Liste), v2 15–21 (Principal, Rechte, Uuid-Auflösung
je Art, Seite). **Kein N+1** gefunden; die größte Abfragezahl hat der v2-Sync-Feed (21 bei 500
Änderungen – gebündelt je Art). App-Zeit (Rust) > 10 % nur bei: Volltext (tantivy, 1,9 s admin),
`/api/search/` (1,3 s), Vorschläge (0,5–0,7 s, Matching über 5 000 Korrespondenten/800 Tags +
Datumsparser), v2-Zeitstrahl (1,1 s), `sync/ids` (1,2 s JSON für 1 Mio. Paare), API v9 `all`
(0,3 s JSON).

## 5. Top-10-Engpässe (nach Wirkung)

Gewinn = erwartete Verbesserung am gemessenen Endpunkt; Aufwand S (< 1 Tag), M (1–3 Tage), L (> 3 Tage).
EXPLAIN-Belege: `results/bench/scale/2026-11-10/explain.txt` (E1–E8).

| # | Engpass (Messwert) | Ursache | Schicht | Lösung | Gewinn | Aufwand |
|---|---|---|---|---|---|---|
| 1 | **`selection_data`** – UI-Liste 31 s / 13 s, Volltext-UI 92 s, POST 9–43 s (100 Dok.), > 2 min ab 10 000 | Django-Form 1:1: `COUNT(CASE WHEN j.document_id = ANY(string_to_array($1,',')::bigint[]))` über **LEFT JOIN Objekt × ganze Zwischentabelle** (5 Mio. Tag-Zeilen, 3 Mio. CF-Zeilen, 1 Mio. Dokumente), `string_to_array` wird **je Zeile** neu geparst (E4: 63 s für 100 IDs); in der Liste zusätzlich vorher **alle** gefilterten IDs (1 Mio.) als Text-Parameter zurück an die DB | L1 / L6 (ORM-Form) | Aggregat nur über die Auswahl: `SELECT tag_id, count(*) FROM documents_document_tags WHERE document_id IN (<gefilterte Abfrage als Unterabfrage>) GROUP BY tag_id`, dann rechts an die Objekte (E4b: **17 ms** statt 63 s für 10 000 IDs); keine ID-Liste über die Leitung; 5 Aggregate parallel bzw. in einer Abfrage. Für „alles ungefiltert“ (Standardansicht) dieselben Zahlen wie `document_count` (ggf. gecacht, Invalidierung über Sync-Log) | 31 s → < 1 s (UI-Liste), POST 100–1000× | M (Reihenfolge der Objekte war bisher „wie der Plan“ → feste `ORDER BY` + Paritätsregel) |
| 2 | **`title_content` / `content__icontains`** 25–42 s, c=8 unmöglich | `UPPER(content) LIKE UPPER('%x%')` über 2,2 GB Text: voller Scan + Faltung je Zeile (E6: 30 s), kein Index möglich | L1 | kurzfristig Trigramm-GIN auf `UPPER(content::text)` (Quick Win Q4, Größe/Ladezeit abwägen); langfristig `title_content` auf tantivy (Phrase/Teilwort) umleiten, wie v2 `text` bereits (59 ms) | 30 s → < 0,5 s (Index) / < 0,1 s (tantivy) | S (Index) / M (Umleitung, Parität: Teilwort-Semantik prüfen) |
| 3 | **Label-Listen mit `document_count`**: Tags 10–14 s, Custom Fields 4–11 s, Korrespondenten/Typen/Pfade 0,6 s (admin) bzw. 4–6 s (restricted) | `COUNT(DISTINCT dt.document_id)` über alle 5 Mio. Zuordnungen mit Sort-Spill (E3: 9,5 s, `external merge Disk 88 MB` bei `work_mem` 4 MB), für die **ganze** Tabelle auch wenn nur 25 Tags angezeigt werden; Sichtbarkeitsunterabfrage mit überflüssigem Self-Join auf `root_document_id`; restricted: Sichtbarkeit je Dokument | L1 / L5 | (a) `COUNT(*)` statt `COUNT(DISTINCT)` (Paar ist UNIQUE, Ergebnis identisch; E3b 2,5 s) (b) Zählung nur für die Seiten-IDs, (c) Self-Join weglassen, wenn kein Versionsdokument sichtbar sein kann, (d) L5: materialisierte Sichtbarkeit (`paperrust_grant` + Zähler je Label, über Sync-Trigger gepflegt) | 14 s → 2,5 s (a), → < 0,2 s (b+c), restricted → < 0,3 s (d) | S (a,c) / M (b) / L (d) |
| 4 | **Dokumentliste/COUNT** – jede Liste ≈ 1 s, Seite 2000 1,6–5 s, Dashboard-Widgets 0,5–2,7 s, c=8 nur 2 req/s | exakte Gesamtzahl (`COUNT(*)` InitPlan) und `ORDER BY created` lesen den **931-MB-Heap** (Inhalt liegt inline), Index `created` filtert `root_document_id/deleted_at` erst im Heap | L1 / L6 | Teilindex `paperrust_doc_live_created (created DESC, id DESC) INCLUDE (owner_id) WHERE root_document_id IS NULL AND deleted_at IS NULL` → Index-Only-Scan (30 MB) – **gemessen** (Abschnitt 6); später v2-Cursor ohne `count` (19 ms) als Standard der neuen UI | 2,5–7× Latenz, 5–13× Durchsatz | **S** (Migration liegt beim L1-Agenten bereit) |
| 5 | **Volltext** häufige Begriffe 1,6–5 s, `more_like_id` 1,8–5 s, UI-Variante + selection 92 s | upstream-Ablauf: tantivy rankt **alle** Treffer (`TopDocs::with_limit(num_docs)`), dann SQL holt **alle sichtbaren IDs** (1 Mio., E8: 1,4 s) für die Schnittmenge, dann `selection_data` über alle Treffer | L3 | Rechte als Filter **im** Index (Besitzer/Freigabe-Felder wie upstream `viewer_id` – existiert bereits als `apply_permission_filter`) und nur Top-N ranken; Schnittmenge nur für die angezeigte Seite + Trefferzahl aus tantivy; selection_data siehe #1 | 2–5 s → < 0,2 s | M (L3) |
| 6 | **Statistik** restricted 9,5 s (admin 1,8 s) | 6 Teilabfragen mit derselben Sichtbarkeitsbedingung; `inbox_docs` = `COUNT(DISTINCT)` über JOIN Tags × sichtbare Dokumente; Summe `content_length` scannt Heap | L5 / L6 | Sichtbarkeit einmal als CTE/Temp-ID-Menge; Index-Only über Teilindex mit `INCLUDE (owner_id, mime_type, content_length, archive_serial_number)`; L5-Grant-Tabelle | 9,5 s → < 1 s | M |
| 7 | **Sortierung nach Custom Field** 9 s (admin), Notizanzahl 2,2 s, Korrespondent 1,8–2,7 s | zwei **korrelierte Unterabfragen je Dokument** (EXISTS + Wert, E5: 9,5 s für 1 Mio. Zeilen); Notizen per `LEFT JOIN (SELECT … GROUP BY)` über alle Notizen; Korrespondent-Name per JOIN + Sort über 1 Mio. | L1 | `LEFT JOIN documents_customfieldinstance cf ON (document_id, field_id)` + `ORDER BY cf.value_x NULLS LAST` (gleiche Semantik wie EXISTS-DESC + Wert), Index `(field_id, value_int)` existiert bereits; Notizzahl als Spalte/Zähler (L6) | 9 s → < 1 s | M |
| 8 | **`sync/ids`** 13 s (admin, 42,9 MB) / 25 s (restricted) | jeder Aufruf `INSERT INTO paperrust_sync_object … SELECT <alle sichtbaren> ON CONFLICT DO NOTHING` (8 s Konfliktprüfung für 1 Mio. bestehende Zeilen), restricted zusätzlich Sichtbarkeits-Scan; Antwort ungepaged | Sync (eigene Schicht) / L5 | `INSERT … WHERE NOT EXISTS` nur für fehlende (oder ganz weglassen – Trigger + Rebuild halten die Tabelle vollständig); `ids` paginieren bzw. als Hash-Bereiche (Merkle) abgleichen | 13 s → < 1,5 s (nur Lesen), mit Hash-Bereichen ≪ 1 s | S / M |
| 9 | **Eingeschränkte Sichtbarkeit allgemein** (Filter 2–5× langsamer, Label-Listen 5–10×) | `CAST(id AS varchar) IN (UNION guardian-Nutzer + Gruppen)` – Text-Schlüssel `object_pk`, Disjunktion mit `owner_id` verhindert Indexnutzung, wird je Abfrage neu berechnet (bei Labels je Label-Typ) | **L5** | `paperrust_grant (doc_id int, principal, level)` mit Index (doc_id) und (principal); Sichtbarkeit als `EXISTS`/Semi-Join auf int; optional pro Nutzer materialisierte Sichtbarkeitsmenge (Bitmap) mit Epoche aus `perm_epoch` | restricted ≈ admin-Niveau (2–5×) | L |
| 10 | **Hintergrund-Massenänderungen**: `bulk_edit` auf 10 000 Dok. → 20 min Nachlauf (122 ms/Dok.), Speicherpfad mit 50 000 Dok. umbenennen → ≈ 3,6 h (260 ms/Dok., fallend), Einlesen 0,8 s/Dok.; dazu Sync-Sequenzer > 25 min nach dem 10 000er-Bulk (blockierte eine `ProcSignalBarrier` im geteilten Cluster) | `bulk_update_documents` verarbeitet **jedes Dokument einzeln** wie upstream (Cache, alle „Updated“-Workflows mit Matching, Vorlage rendern, Datei verschieben, Einzel-SQL), danach Index; ein Worker, nacheinander. Sequenzer: `LEFT JOIN paperrust_sync_live` (UNION-Sicht) → Seq Scan über alle Dokumente je Lauf, ein Lauf über 50 000 Wartelisten-Zeilen | L2 (Vorlagen) / Worker / Sync | (a) Vorlagen + Workflow-Trigger einmal vorkompilieren, Dokumentkontexte **gebündelt** laden (eine Abfrage je 500 Dok.), Dateioperationen gesammelt, Index-Commit am Ende (b) Nachlauf parallel in N Worker-Teilaufgaben (Gruppen-Chunks), (c) bei reinen Tag-Änderungen Dateinamen nur neu rendern, wenn die Vorlage Tags verwendet (d) Sequenzer: Existenzprüfung je Art per Index (`EXISTS` auf PK), kleinere Stapel (≤ 5 000), `statement_timeout` | Nachlauf 5–20× (≈ 10–25 ms/Dok.), Sequenzer < 1 s je Stapel | M (a,c,d) / M (b) |

Nicht unter den Top 10, aber notiert:
- API v9 `all` (5,4 s, 6,9 MB) – nur Altclients; v10-UI nutzt es nicht. Ggf. Hinweis in der
  Doku, keine Arbeit.
- v2 `/timeline` 2,4–7 s: lädt ein Jahr Datumsfelder aller sichtbaren Dokumente (1,2 MB); mit
  Fenster/Cursor oder Aggregation je Monat lösbar (pl-timeline, nicht L1–L6).
- Vorschläge 0,6–1 s ohne Klassifikator: Matching aller 5 000 Korrespondenten (Regex/Fuzzy) je
  Aufruf, Modell wird bei jedem Aufruf von der Platte geladen (mit Klassifikator siehe 4.3) →
  vorkompilierte Matcher/Modell im Speicher halten (mtime-Cache).
- `docs_fields_id_all` (Massen-IDs) 3–11 s – Folge von #4/#9.
- `paperrust_sync_sequence()` brauchte nach Löschen + Papierkorb-Leeren von 50 Dokumenten einmal
  81 s (vermutlich Wartezeit auf eine lange laufende Abfrage/Sperre) – beobachten.

## 6. Quick Wins ohne Architekturänderung

| # | Maßnahme | Parität | gemessen | Wirkung |
|---|---|---|---|---|
| Q1 | **Teilindex `paperrust_doc_live_created`** (`created DESC, id DESC` INCLUDE `owner_id`, `WHERE root_document_id IS NULL AND deleted_at IS NULL`, 30 MB) | neutral (nur Plan) | **ja** (A/B gleiche Bedingungen, `idx-with/` vs `idx-without/`) | admin Liste 771 → 263 ms (2,6 → 27,1 req/s), Seite 2000 1 184 → 159 ms, Sort `added` 821 → 197 ms, Inbox 765 → 301 ms, v2 count 742 → 271 ms, Dashboard-Widgets 790 → 272 ms; restricted Liste 1 017 → 397 ms (2,2 → 13 req/s), Seite 2000 2 872 → 663 ms. **Regressionen**: `owner__id__in` 72 → 160 ms (Planner wählt den neuen Index statt `owner_id`), restricted `tags__id__in` 1,7 → 2,5 s – beide mit `ANALYZE`/Statistikziel prüfen |
| Q2 | Tag-Zählung `COUNT(DISTINCT dt.document_id)` → `COUNT(*)` (UNIQUE (document_id, tag_id)) | neutral (gleiches Ergebnis) | EXPLAIN E3/E3b | 9,5 s → 2,5 s |
| Q3 | `selection_data`: Auswahl-Aggregat statt CASE über LEFT JOIN, ID-Liste als `int8[]`-Parameter (`= ANY($1::int8[])`) statt `string_to_array` je Zeile | Reihenfolge der Objekte festlegen (Regel) | EXPLAIN E4/E4b | 63 s → 17 ms (POST); UI-Liste 31 s → ≈ 1 s |
| Q4 | Trigramm-GIN `USING gin (UPPER(content::text) gin_trgm_ops)` + gleiches für `title` (`pg_trgm` ist verfügbar) | neutral | nein (Aufbau auf 2,2 GB Text geschätzt 20–40 min, Größe ≈ 1–2× Text; nur mit genug Platte) | `title_content` 30–40 s → < 1 s für Begriffe ≥ 3 Zeichen; seltene/fehlende Begriffe am meisten |
| Q5 | `work_mem` erhöhen | neutral | E3 mit 64 MB: Spill bleibt (90 MB), **kein Gewinn** | verworfen – Q2 beseitigt die Sortierung ganz |
| Q6 | `sync/ids`: `INSERT … ON CONFLICT` nur für fehlende Objekte | neutral | nein | 13 s → ≈ 5 s |
| Q7 | Sichtbarkeits-Unterabfrage ohne Self-Join `LEFT JOIN documents_document pl_authz_parent`, wenn nur Wurzeldokumente gezählt werden | neutral | nein | −10–20 % bei Label-Zählungen |
| Q8 | Klassifikator/Matcher im Speicher halten (mtime-Cache) für `suggestions` | neutral | nein | −0,3…0,5 s je Vorschlag |

**Umgesetzt in diesem Auftrag: nichts im Code.** Q1 ist bereits als
`crates/pl-store/migrations/0001_paperrust_docquery_indexes.sql` vom L1-Agenten im Arbeitsbaum
(nicht committet); ich habe ihn nur auf `pl_scale_1m` angelegt, gemessen und wieder entfernt
(Basiszustand = v0.1.0). Eine eigene Migration hätte dieselbe Datei/Crate berührt → bewusst
unterlassen, keine Paritätsläufe nötig.

## 7. Empfohlene Reihenfolge

1. **Q1 + Q2 + Q3 + Q6 + Q7 (+ Sequenzer-Fix aus #10d)** sofort (S, parity-neutral bzw. mit kleiner Ordnungsregel):
   UI wird von „unbenutzbar“ (31 s je Liste) auf ≈ 0,3–1 s gebracht.
2. **L1 (Filter-AST)** wie geplant, aber mit den Abfrageformen aus #1, #4, #7 als Abnahmekriterium
   (Messlauf `run.py --only 'docs_|f_|ui_'`, Ziel p50 < 300 ms admin / < 600 ms restricted).
   `title_content` auf tantivy oder Trigramm (#2) gleich mit.
3. **L5 (Rechte → `paperrust_grant`)** vorziehen vor L3: restricted ist überall 2–10× langsamer,
   und L3-Rechte im Index (#5) sowie Statistik (#6) und Label-Zählungen (#3d) bauen darauf auf.
4. **L3 (Suche)**: Rechte als Index-Filter, Top-N statt alle Treffer, Trefferzahl aus tantivy.
5. **L4 (Klassifikator)**: Training begrenzen (Stichprobe/zeitlich jüngste N, inkrementell) –
   heutiges Verfahren ist ab einigen 100 000 Dokumenten nicht mehr praktikabel (4.3).
6. **L2 (Vorlagen)** und **L6 (Datenmodell)** zuletzt; L6 bringt Zähler-Spalten (Notizen, Tags je
   Dokument) und schmale Heap-Tabelle (Inhalt ausgelagert) für #4/#6/#7 dauerhaft.

## 8. Reproduktion, Dateien, Aufräumen

- Generator, Messskripte, EXPLAIN: `tests/bench/scale/` (README mit Befehlen, DB-Dump/Restore).
- Ergebnisse: `results/bench/scale/2026-11-10/`.
- Messdatenbank `pl_scale_1m` (4,5 GB) im Referenz-Cluster und Arbeitsordner (Index 3,6 GB,
  Medien 138 MB) bleiben bis zur Freigabe bestehen; siehe Abschlussbericht für die Aufräumbefehle.

## 9. Nachher (2026-11-13)

Nachmessung nach den schnellen Lösungen (Q1–Q3, Q6–Q8), L3 (native Suche, Trigramm, Sortierung
nach Custom Field), Statistik, Objektlisten **und** den beim Nachmessen gefundenen Korrekturen
(9.3). Der Stand enthält auch L1 (Filter-AST, 40b50c5) und L5 (`paperrust_grant`, 201b358), die
nach der Basis (5844ec8) kamen: Die Tabelle zeigt die **gemeinsame** Wirkung.

### 9.1 Bedingungen

- Gleich wie die Basis: Datenbank `pl_scale_1m` (1 Mio. Dokumente), gleicher Arbeitsordner und
  tantivy-Index, `run.py --n1 8 --dur8 10 --timeout 120`, Nutzer admin und eingeschränkt (id 22).
  Release-Build von `pl-server` (`-j 2`): bewusste Ausnahme zu AGENTS.md („nie `--release`“),
  weil die Basis auch mit Release gemessen wurde.
- Vorbereitung auf `pl_scale_1m`: Trigramm-Index von Hand (17:09 min; Größe `title` 65 MB,
  `content` 1025 MB), danach `manage migrate` in 31 s (Teilindex Q1 6 s, Freigabe-Index L5 15 s,
  `paperrust_search_dirty`), `VACUUM ANALYZE` 1:53 min.
- Lauf `results/bench/scale/2026-11-13-after/` (13:56–14:40, Binary mit Stand d4579d2): Rechner ruhig (andere Sitzungen pausiert, CPU 7 % beim Start). Endpunkte, die die
  letzten zwei Korrekturen betreffen (mit ² markiert), wurden mit dem Endstand (08280e2)
  unter gleichen Bedingungen nachgemessen: `results/bench/scale/2026-11-13-after-fix2/`.
- Der erste Lauf vom 2026-11-11 (`2026-11-11-after/`) ist **verworfen**: Er lief neben Builds
  anderer Sitzungen. Live liefen dieselben Abfragen 2–4× schneller als dort gemessen.

### 9.2 Ergebnis

- 144 vergleichbare Messpunkte: im Mittel (geometrisch) **6,6× schneller**, Median 4,5×.
  Zwei Punkte liegen bei 0,9× (Messrauschen: 0,9 → 1,0 ms; 1,33 → 1,43 s).
- Über 1 s: vorher 78 Messpunkte (davon 6 Abbrüche nach 2 min), nachher 25, keiner bricht mehr ab.
- Angular-Grundansicht (Liste mit `selection_data`): 31 s → 5,3 s (admin), 12,8 s → 3,6 s.
  Markieren (`selection_data`): 100 Dok. 9–43 s → 12–14 ms, 10 000 Dok. > 2 min → 0,3 s,
  „alle“ > 2 min → 14 s / 3,7 s.
- Seltene Suchwörter in Titel/Inhalt: 42 s → 3 ms. Häufige („rechnung“): 39 s → 17 s. Grund: Die
  Gesamtzahl verlangt weiter einen Durchlauf über alle Inhalte.
- Noch langsam (> 3 s): Titel/Inhalt mit häufigem Wort, „alle auswählen“, Angular-Liste (admin)
  und Volltextliste mit `selection_data` (admin 4,6 s).

Werte: Median c=1 in ms, „Faktor“ = vorher/nachher (fett: langsamer). Rohdaten: `results.json`
und `summary.md` der Ordner.

| Endpunkt | admin vorher | admin nachher | Faktor | eingeschr. vorher | eingeschr. nachher | Faktor |
|---|---:|---:|---:|---:|---:|---:|
| ui_docs_default | 31 238 | 5 336 | 5,9× | 12 783 | 3 572 | 3,6× |
| ui_docs_correspondent | 72 | 24 | 2,9× | 234 | 27 | 8,6× |
| ui_fts_query_word | 91 570 | 4 569 | 20× | 11 269 | 1 660 | 6,8× |
| docs_default | 992 | 72 | 14× | 1 203 | 144 | 8,3× |
| docs_default_v9_all_ids | 5 404 | 480 | 11× | 4 332 | 358 | 12× |
| docs_sort_title | 1 144 | 193 | 5,9× | 1 197 | 141 | 8,5× |
| docs_sort_correspondent | 1 837 | 1 198 | 1,5× | 2 672 | 1 399 | 1,9× |
| docs_sort_asn | 884 | 95 | 9,3× | 1 105 | 145 | 7,6× |
| docs_sort_num_notes | 2 162 | 305 | 7,1× | 2 563 | 340 | 7,5× |
| docs_sort_custom_field | 8 961 | 388 | 23× | 4 431 | 419 | 11× |
| docs_sort_added | 972 | 81 | 12× | 1 953 | 139 | 14× |
| docs_page_2000 | 1 642 | 87 | 19× | 5 141 | 188 | 27× |
| docs_fields_id_all | 3 289 | 1 037 | 3,2× | 11 184 | 1 542 | 7,3× |
| f_tag_any | 674 | 164 | 4,1× | 3 524 | 192 | 18× |
| f_tag_all | 1 729 | 503 | 3,4× | 2 428 | 447 | 5,4× |
| f_tag_none | 1 579 | 325 | 4,9× | 3 904 | 300 | 13× |
| f_tag_nested_root | 408 | 199 | 2,0× | 1 953 | 186 | 11× |
| f_correspondent | 28 | 6,0 | 4,7× | 208 | 11 | 18× |
| f_created_range | 255 | 12 | 21× | 494 | 20 | 25× |
| f_cfq_and | 643 | 427 | 1,5× | 1 128 | 463 | 2,4× |
| f_cfq_select | 1 957 | 786 | 2,5× | 1 134 | 476 | 2,4× |
| f_cfq_or_icontains | 3 331 | 875 | 3,8× | 5 887 | 920 | 6,4× |
| f_cf_icontains | 2 717 | 1 478 | 1,8× | 10 140 | 1 189 | 8,5× |
| f_owner_in | 95 | 32 | 2,9× | 182 | 32 | 5,8× |
| f_shared_by_me | 907 | 207 | 4,4× | 1 135 | 93 | 12× |
| f_title_content_common ² | 38 638 | 16 860 | 2,3× | 9 087 | 3 249 | 2,8× |
| f_title_content_rare ² | 42 326 | 3,0 | 14109× | 24 746 | 5,2 | 4759× |
| f_inbox | 646 | 72 | 9,0× | 2 682 | 139 | 19× |
| f_combo | 1 031 | 849 | 1,2× | 2 141 | 914 | 2,3× |
| fts_query_word | 1 904 | 81 | 24× | 3 382 | 37 | 91× |
| fts_query_multi | 1 595 | 50 | 32× | 4 677 | 63 | 74× |
| fts_query_rare | 0,9 | 1,0 | **0,9×** | 5,3 | 1,3 | 4,1× |
| fts_more_like | 1 798 | 86 | 21× | 5 188 | 41 | 126× |
| search_autocomplete | 1,2 | 0,8 | 1,5× | 36 | 9,2 | 3,9× |
| search_global | 426 | 386 | 1,1× | 1 416 | 364 | 3,9× |
| selection_data_100 | 9 080 | 12 | 783× | 43 398 | 14 | 2993× |
| selection_data_10k | >120 000 | 265 | > 453× | >120 000 | 342 | > 351× |
| selection_data_all | >120 000 | 14 251 | > 8,4× | >120 000 | 3 694 | > 32× |
| statistics | 1 809 | 960 | 1,9× | 9 479 | 1 110 | 8,5× |
| tags_page | 14 047 | 667 | 21× | 7 484 | 2 856 | 2,6× |
| tags_all | 9 787 | 1 519 | 6,4× | 7 285 | 2 013 | 3,6× |
| correspondents_page ² | 602 | 6,7 | 90× | 4 029 | 9,1 | 443× |
| correspondents_all ² | 809 | 584 | 1,4× | 3 785 | 665 | 5,7× |
| correspondents_by_count ² | 674 | 497 | 1,4× | 3 783 | 566 | 6,7× |
| document_types_all ² | 570 | 522 | 1,1× | 4 229 | 549 | 7,7× |
| storage_paths_all ² | 588 | 468 | 1,3× | 6 281 | 589 | 11× |
| custom_fields_all | 4 221 | 1 321 | 3,2× | 11 262 | 2 189 | 5,1× |
| saved_views_all | 10 | 6,0 | 1,7× | 14 | 4,9 | 2,9× |
| users_all | 28 | 18 | 1,5× | Fehler 403 | Fehler 403 | – |
| ui_settings | 1,7 | 1,5 | 1,1× | 5,3 | 1,1 | 4,8× |
| doc_retrieve | 14 | 3,6 | 3,8× | 22 | 5,7 | 3,8× |
| doc_notes | 2,1 | 1,4 | 1,5× | 4,7 | 1,5 | 3,1× |
| doc_suggestions | 601 | 156 | 3,8× | 1 052 | 106 | 9,9× |
| doc_metadata | 7,5 | 4,4 | 1,7× | 16 | 4,1 | 3,9× |
| doc_thumb | 3,2 | 1,9 | 1,7× | 6,2 | 2,1 | 3,0× |
| doc_download | 1,9 | 1,1 | 1,7× | 3,7 | 1,2 | 3,1× |
| tasks | 1,8 | 1,7 | 1,1× | 4,6 | 2,0 | 2,3× |
| status | 11 | 7,8 | 1,4× | Fehler 403 | Fehler 403 | – |
| v2_documents | 19 | 10 | 1,8× | 57 | 18 | 3,2× |
| v2_documents_count | 618 | 88 | 7,0× | 1 762 | 141 | 12× |
| v2_query_tag_date | 391 | 179 | 2,2× | 832 | 209 | 4,0× |
| v2_query_text ² | 59 | 18 | 3,4× | 61 | 15 | 4,2× |
| v2_timeline | 2 441 | 1 610 | 1,5× | 7 008 | 359 | 20× |
| v2_upcoming | 2,3 | 1,4 | 1,6× | 10 | 1,6 | 6,6× |
| v2_sync_changes_first | 113 | 80 | 1,4× | 66 | 19 | 3,4× |
| v2_sync_ids_documents | 12 872 | 2 981 | 4,3× | 25 134 | 1 475 | 17× |
| dashboard_admin_w0 ² | 874 | 93 | 9,4× | – | – | – |
| dashboard_admin_w1 ² | 571 | 306 | 1,9× | – | – | – |
| dashboard_admin_w2 ² | 896 | 78 | 12× | – | – | – |
| dashboard_admin_w3 ² | 3,5 | 2,7 | 1,3× | – | – | – |
| dashboard_admin_w4 ² | 62 | 34 | 1,9× | – | – | – |
| dashboard_admin_w5 ² | 922 | 586 | 1,6× | – | – | – |
| dashboard_admin_w6 ² | 164 | 44 | 3,7× | – | – | – |
| dashboard_admin_w7 ² | 458 | 189 | 2,4× | – | – | – |
| dashboard_restricted_w0 ² | – | – | – | 1 327 | 1 432 | **0,9×** |
| dashboard_restricted_w1 ² | – | – | – | 617 | 163 | 3,8× |
| dashboard_restricted_w2 ² | – | – | – | 2 656 | 446 | 6,0× |
| dashboard_restricted_w3 ² | – | – | – | 522 | 41 | 13× |
| dashboard_restricted_w4 ² | – | – | – | 1 914 | 1 240 | 1,5× |
| dashboard_restricted_w5 ² | – | – | – | 2 162 | 369 | 5,9× |

**Nachmessung 2026-11-16** (Testprotokoll D-011/D-012/D-015, `results/testprotokoll/2026-11-16/D-betrieb/report.json`):
Kopie von `pl_scale_1m` nach `manage migrate` (eigenes Datenmodell, Stufen 1–5), Release-Build
cea93f7, `tests/bench/scale/run.py --n1 5 --dur8 3`, 12 Threads/15 GB unter Fremdlast, PostgreSQL 18.6.
Die „nachher“-Werte oben gelten für das **alte Schema**; mit dem eigenen Datenmodell (siehe auch §13)
liegen sie höher: `docs_default` admin 240 ms / eingeschr. 244 ms (oben 72 / 144),
`docs_page_2000` 172 / 362 ms (oben 87 / 188), `docs_sort_custom_field` 806 / 601 ms (oben 388 / 419).
Schneller geworden sind u. a. `tags_page` 17,6 / 186 ms und `statistics` eingeschr. 646 ms.

### 9.3 Beim Nachmessen gefunden und behoben

Alle Korrekturen sind mit Test und Vertragsnotiz versehen und haben das gleiche Ergebnis für
Nutzer (nur schneller); einzig 1. liefert bewusst mehr als upstream (upstream gibt in dem Fall `[]` zurück).

| # | Problem | vorher | nachher (Endstand) | Commit |
|---|---|---:|---:|---|
| 1 | „Ähnliche Dokumente“: Superuser Fehler 500; danach **immer leer** bei gelöschten Altfassungen im Index (tantivy 0.26 zählt sie bei `doc_freq` mit) | 500 / leer | 86 ms / 41 ms, mit Treffern | 0088803, 93a403a |
| 2 | Trigramm-Index bei häufigen Wörtern langsamer als der Scan (jede Kandidatenzeile wird nachgeprüft, kein früher Abbruch bei `LIMIT`): `title_content=rechnung` | 39 s (Basis) → 80 s | 17 s | f7dc049, d23b0c7 |
| 2 | dasselbe: v2-Text ohne Zählung | 59 ms → 96 s | 18 ms | dito |
| 2 | dasselbe: Dashboard-Widgets mit Textfilter (admin w4/w7, eingeschr. w3) | 0,06–0,5 s → 12–20 s | 34 ms / 189 ms / 41 ms | dito |
| 3 | Objektliste `page_size=100000`: eine Bind-Variable je Id (5000 Korrespondenten) | 0,8 s (Basis) → 9,4 s | 0,58 s | 7b57696 |
| 4 | Objektlisten eingeschränkt: Planer wählt wegen des Versions-`EXISTS` einen Index-Scan (Korrespondenten nach Anzahl) | 3,8 s (Basis) → 7,3 s | 0,57 s | 08280e2 |
| 5 | `selection_data` eingeschränkt: `NOT IN` über ~600 000 erlaubte ids läuft je Zeile | 43 s (100) / > 2 min (10 000) | 14 ms / 0,3 s | d4579d2 |

Zu 2.: Der erste Versuch, nur bei kleiner Planer-Schätzung den Index zu nehmen, reichte nicht, weil
ANALYZE breite Werte (> 1 kB) auslässt („Beitrag“ geschätzt 100, tatsächlich 70 310). Jetzt wird
bis 1001 gezählt (seltene Wörter ~20–60 ms, häufige 0,1–0,6 s, 5 min zwischengespeichert).

### 9.4 Parität

Voller Lauf gegen die Paritätskopie `w2perf` **mit** eingespielten PaperRust-Migrationen
(Trigramm-Indizes, `paperrust_grant`, native Suche aktiv), Binary 08280e2:
`results/parity/perf-final/2026-11-13_152325.md`: **2024 verglichen, 2001 identisch, 0 verschieden, 0 Fehler,
23 akzeptiert** (bestehende Regeln). Neu akzeptiert ist nur `search.documents.more_like@admin`
(vorher Fehler 500, Regel `more-like-this-upstream-nondeterministic`: upstream liefert bei zwei
gleichen Anfragen schon unterschiedliche Treffer). Neu im Harness: Regel „Gleichstand bei
`ordering=±document_count` nach id“ (0088803, docs/contract/testing.md), weil upstream dort
keinen Gleichstandsschlüssel hat.

Ein erster Lauf (15:12) zeigte 50 Abweichungen und 41 Fehler. Ursache: Redis der
Referenzumgebung lief nach dem Ruhezustand nicht, Django (A) antwortete deshalb mit 500. Nach dem
Neustart von Redis war der Lauf sauber (siehe oben). Frühere Läufe: `qw-after` (ohne
PaperRust-Migrationen, 0 verschieden) und `qw-after-migrated` (3 verschieden, alle behoben bzw.
als Gleichstand geregelt).

### 9.5 Offen

- `title_content` mit häufigem Wort (17 s admin, 3 s eingeschränkt) und „alle auswählen“ (14 s
  admin): die Gesamtzahl über alle Inhalte bleibt teuer. Weg: `title_content` auf tantivy umleiten
  (Engpass #2 langfristig) bzw. „alle“ ohne Id-Liste über die Leitung.
- Angular-Liste admin 5,3 s: der Rest ist die Gesamtzählung der Auswahl über alle Dokumente.
- Nicht geprüft: die `#[ignore]`-Differenztests gegen Referenz-DB/Index
  (`pl-server/tests/search_l3_diff.rs` u. a.) wurden nicht ausgeführt.
- Aufräumen: `pl_scale_1m` (jetzt mit Trigramm-Index, +1,1 GB) und der Arbeitsordner bleiben bis zur
  Freigabe bestehen.

## 10. Massenänderungen gebündelt (#10) – umgesetzt (2026-11-13)

**Messaufbau.** Eigene Kopie `pl_bulk_100k` (gen.py, Seed wie oben, **100 000 Dokumente**, 5 000 mit
Dateien in Speicherpfad 1, 2 000 Korrespondenten, 50 Workflows, davon 10 „Dokument geändert“; **4
davon ohne jeden Filter** – sie passen auf jedes Dokument und weisen bei jedem Lauf Korrespondent,
Typ und Tags neu zu). Vor jedem Lauf DB und Medien aus der Vorlage `pl_bulk_100k_tpl` zurückgesetzt;
Binärdatei Profil `perf` (release mit thin LTO, gleiche Einstellungen vorher/nachher); Messinstanz
`serve --worker` auf Port 58191. vorher = fe4a63f, nachher = dieser Stand. Alle unten stehenden
Läufe **ohne Fremdlast** (Tempo-Sitzung pausiert). Skript und Rohdaten:
`results/bench/scale/2026-11-11-bulk/` (`measure.sh`; `before/`, `after/`, `*-nowf/`;
`before-loaded/` = erster vorher-Lauf am 11.11. unter Last der Tempo-Messung, nur zur Info).

| Szenario | vorher | nachher | Faktor |
|---|---:|---:|---:|
| `bulk_edit add_tag`, 997 Dok., Nachlauf (mit Workflows) | 86,7 s = **87 ms/Dok.** | 21,7 s = **22 ms/Dok.** | 4× |
| `bulk_edit remove_tag`, 997 Dok. (mit Workflows) | 83,2 s = 83 ms/Dok. | 15,1 s = **15 ms/Dok.** | 5,5× |
| `bulk_edit add_tag`, 997 Dok., ohne Workflows „geändert“ | 5,6 s = 5,6 ms/Dok. | 2,1 s = **2,1 ms/Dok.** | 2,7× |
| Speicherpfad mit 5 000 Dok. umbenennen (mit Workflows) | ≈ 560 s = ≈ 112 ms/Dok. | ≈ 450 s = ≈ 90 ms/Dok. | 1,25× |
| Speicherpfad umbenennen, ohne Workflows „geändert“ | ≤ 60 s (≈ 11 ms/Dok.) | ≤ 60 s (≈ 9 ms/Dok.) | ≈ 1,2× |
| Einlesen 50 Textdateien ohne OCR | 223 ms/Dok. | 200 ms/Dok. | (nicht geändert, Streuung) |

Hochgerechnet: 10 000 Dokumente taggen dauerte im Nachlauf 15 min, jetzt ≈ 3,5 min (ohne
filterlose Workflows ≈ 20 s). Die Umbenenn-Zeiten sind aus den 30-s-Stichproben von `run.py`
abgelesen (grob); in allen Läufen bleiben dieselben 14 Dokumente am alten Ort (Namenskollision/
fehlende Datei im Datensatz, vorher wie nachher – nicht untersucht).

**Warum Umbenennen mit Workflows kaum schneller wird:** Die Vorlage enthält `{{ correspondent }}`, und
die filterlosen Workflows setzen den Korrespondenten nacheinander auf drei verschiedene Werte. Upstream
speichert nach jedem Workflow und verschiebt die Datei jedes Mal (bis zu 4 Verschiebungen je Dokument).
Dieses Verhalten ist vorgeschrieben (Migrationsgleichheit) und wird exakt nachgebildet; gebündelt
wird nur, solange sich kein Dateiname ändert.

**Was geändert wurde** (`crates/pl-consumer/src/bulk.rs`, Worker `pl-server/src/worker.rs`):
- (a) Workflows „Dokument geändert“ einmal je Aufgabe laden (vorher: alle Workflows mit Triggern
  und Aktionen je Dokument, ≈ 50 Abfragen). Dokumente, Namen, Tags, neueste Versionen und Custom
  Fields in Blöcken à 500 mit je **einer** Abfrage; Workflow-Prüfung und Dateiname im Speicher;
  unveränderte Dateinamen → ein `UPDATE … SET modified` je Block; Cache-Schlüssel je Block in einem
  Aufruf; WebSocket-Berechtigte je Block. Suchindex war schon gebündelt (ein Commit am Ende).
- (a) Workflow-Treffer mit „einfachen“ Aktionen (Tags, Korrespondent, Typ, Speicherpfad, Besitzer):
  die Dokumentzeile wird im Speicher fortgeschrieben und **einmal** geschrieben statt bis zu
  2 × Workflows + 1 Mal; Tags, Audit-Einträge und Workflow-Läufe entstehen sofort und in derselben
  Reihenfolge. Dateiverschiebung, andere Aktionen (Titel, Rechte, Custom Fields, Mail, Webhook,
  Papierkorb, KI) oder Versionen → ab dort exakter Einzelweg.
- (c) Tags/Custom Fields für den Dateinamen nur laden, wenn eine betroffene Vorlage `tag`,
  `custom_field` oder `document` enthält.
- Einzelweg bleibt: `PAPERRUST_BULK_UPDATE=single`.

**Gleichheit:** `crates/pl-consumer/tests/bulk_equal.rs` (`#[ignore]`, braucht die Referenz-DB):
zwei Kopien von `paperless_snapshot` + Referenzmedien, Aufbau mit filterlosem/abhängigem/nie
passendem Workflow, Workflow mit Titelvorlage (Rückfall), Namenskollisionen (`_01`, `_02`),
Speicherpfad mit `tag_list`, Dateinamenwechsel mitten in der Workflow-Kette; Einzelweg ↔ gebündelt.
Verglichen: alle Dokumentfelder außer `modified`-Zeitstempel (aber welche Dokumente berührt
wurden), Tags, Workflow-Läufe, Audit-Einträge (Inhalt ohne `modified`), eingereihte Aufgaben,
Dateipfade, Dateiinhalte (SHA-256), Verzeichnisse. **Grün** (204 Dokumente, 403 Dateien, 62
Audit-Einträge, 33 Workflow-Läufe). Debug-Build: Einzelweg 9,5 s, gebündelt 1,9 s.

**(b) Verteilung auf mehrere Worker – bewusst nicht umgesetzt:** Bei Namenskollisionen entscheidet
die Reihenfolge der Verschiebungen über die Endnamen (`_01`, `_02` …); parallele Teilaufgaben würden
das Ergebnis vom Zufall abhängig machen. Bei 2–22 ms/Dok. ist es auch nicht mehr nötig.

**(d) Sequenzer v2:** Nach allen Läufen war `paperrust_sync_pending` leer (der Sequenzer hält mit).
Die gebündelte Variante erzeugt deutlich weniger Zeilenänderungen je Dokument (1 statt bis zu 9) und
damit weniger Wartelisten-Einträge; der Sequenzer verarbeitet ohnehin nur berührte Objekte (also nur
die geänderten Dokumente). Gemessen auf `pl_bulk_100k`: 10 000 geänderte Dokumente → zwei Stapel à
5 000 in **0,28 s und 0,18 s** (Ziel < 1 s; am 1-Mio.-Bestand vorher > 25 min für einen Lauf).
Die bisher übersprungenen DB-Tests von pl-sync (`tests/db.rs`, 10 Tests inkl. Sequenzer v2,
späte Transaktionen, kein Warten auf Schreiber) laufen gegen eine leere lokale Test-DB
`pl_sync_test_bulkops` **alle grün**.

**Befund für eine andere Baustelle:** Die Trigramm-Indizes `paperrust_doc_content_trgm` und
`paperrust_doc_title_trgm` (Quick Win Q4) machen **jede** Zeilenänderung an `documents_document`
≈ 8–10× teurer (500 Zeilen `modified` setzen: 3,9 s statt 0,4 s; HOT-Updates sind unmöglich, weil
`modified` indexiert ist). Vorschlag (pl-store): Inhalt/Titel für die Suche in eine eigene Tabelle
auslagern, Index dort, per Trigger nur bei Inhalts-/Titeländerung nachführen.

**Einlesen:** Auf 100 000 Dokumenten ohne Fremdlast 200–220 ms je Textdatei (vorher wie nachher;
am 11.11. unter Last der Tempo-Messung 536 ms). Die 0,8 s aus Abschnitt 4.2 (1 Mio. Dokumente,
5 000 Korrespondenten) habe ich nicht nachgeprüft; das Einlesen wurde in diesem Auftrag nicht
profiliert und nicht geändert (offen).

## 11. Massenänderungen Runde 2 (2026-11-13): Hintergrund-Bulk-Edit, Umbenennen, Einlesen

Gleicher Messaufbau wie §10 (`pl_bulk_100k`, Port 58191, Profil `perf`). Vergleich **r2base** =
a7d94ac (Stand nach §10) gegen **v2** = dieser Stand, direkt nacheinander gemessen
(`results/bench/scale/2026-11-11-bulk/r2base/`, `v2/`). **Achtung Last:** beim r2base-Lauf liefen
Builds anderer Sitzungen (5 rustc/cargo-Prozesse beim Start), beim v2-Lauf noch einer; beide Läufe
sind langsamer als in §10 (Einlesen 334–343 ms statt 200 ms). Die Zahlen zeigen die Richtung, sind
aber kein sauberer Faktor.

| Szenario | r2base | v2 |
|---|---:|---:|
| `bulk_edit add_tag` 997 Dok., Antwortzeit | 664 ms | **209 ms** (im Hintergrund, `task_id`) |
| `bulk_edit remove_tag` 997 Dok., Antwortzeit | 623 ms | **103 ms** |
| … bis Warteschlange leer (Änderung + Nachlauf) | 37,7 s / 23,8 s | 33,3 s / 25,8 s |
| Speicherpfad mit 5 000 Dok. umbenennen (mit filterlosen Workflows) | 132 ms/Dok. (3 160 in 420 s) | **≈ 56 ms/Dok.** (alle 4 986 nach 282 s) |
| Einlesen 50 Textdateien, gesamt je Dokument | 343 ms | 334 ms |
| … davon Workflows laden (WorkflowTriggerPlugin) | ≈ 38 ms | **≈ 9 ms** |

**1. Bulk-Edit im Hintergrund (upstream `dev` 60a35edee, eigene Umsetzung):** reine
Metadaten-Methoden ab 500 Dokumenten (`PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS`) laufen als Aufgabe
`execute_bulk_operation`; die Antwort kommt sofort mit `task_id`. Kleinere Auswahlen bleiben
sofort (alte Oberflächen laden nach `OK` neu). Die Gesamtzeit ändert sich kaum – der Gewinn ist,
dass der Request nicht mehr hängt (bei 100 000 Dokumenten und Spaltenänderung Minuten).
Details: Vertrag pl-documents-write, UPSTREAM-SYNC §8.

**2. Umbenennen: nur noch eine Verschiebung je Dokument.** Ändert sich der Dateiname mitten in
der Workflow-Kette (filterlose Workflows setzen den Korrespondenten 3× um), verschiebt die
gebündelte Kette die Datei nicht mehr nach jedem Workflow, sondern einmal an den Endort. Der
Endname ist derselbe: die Kollisionsprüfung nimmt den ersten Kandidaten, der der eigene alte Name
ist oder auf der Platte fehlt – die eigene Datei zählt nie als Kollision, andere Dokumente ändern
sich in der Kette nicht. Workflow-Filter prüfen `original_filename`, nicht den Speicherort.
Gleichheitstest (`bulk_equal.rs`) grün, mit 7 eingesparten Verschiebungen im Testdatensatz.
Einziger Unterschied: upstream löscht beim Wegverschieben leere Zwischenordner, die *vorher schon
leer* waren; hier bleiben sie (Vertrag pl-consumer, Known differences).

**3. Einlesen: Workflows gebündelt laden.** `workflows_for_trigger` lud je Workflow Trigger und
Aktionen mit je 13 bzw. 16 Einzelabfragen (bei 50 Workflows ≈ 1 000 Abfragen je Dokument, zweimal
je Einlesen). Jetzt ≈ 30 Abfragen für alle Workflows; Inhalt und Reihenfolge gleich. Wirkt auch für
„Dokument geändert“ im Einzelweg (PUT/PATCH) und für geplante Workflows. Gemessen: 38 → 9 ms je
Ladevorgang. Der restliche große Block beim Einlesen (≈ 80 ms zwischen Speichern und den
„Hinzugefügt“-Workflows, vermutlich die automatische Zuordnung über 2 000 Korrespondenten) ist
**nicht** untersucht – nächster Kandidat.

**4. Verteilung auf mehrere Worker – weiter bewusst nicht umgesetzt.** Die Reihenfolge der
Verschiebungen bestimmt bei Namenskollisionen die Endnamen (`_01`, `_02`); parallele Teilaufgaben
würden das vom Zufall abhängig machen. Eine sichere Aufteilung müsste nach *Ziel*-Ordner
gruppieren, der aber erst nach den Workflows feststeht. Nach Punkt 2 liegt Umbenennen bei
≈ 56 ms/Dok. (10 000 Dokumente ≈ 9 min), Taggen bei 15–22 ms/Dok.; der Aufwand lohnt sich nicht.

## 12. Tempo-Reste (2026-11-13): Titel+Inhalt mit häufigem Wort, Auswahl „alle“, Einlesen, Upload-Route

**Messaufbau.** Eigene Kopie `pl_perfrest_1m` von `pl_scale_1m` (1 Mio. Dokumente, mit
Trigramm-Index, PaperRust-Migrationen dieses Stands eingespielt), Messinstanz Port 58201 ohne
Worker, leerer Arbeitsordner (kein Suchindex – die gemessenen Endpunkte brauchen ihn nicht),
`run.py --n1 5 --dur8 5`, Binärdateien Profil `perf` (release, thin LTO). **vorher** = Binärdatei
des Stands nach §11 (target/perf vom 13.11. 18:18), **nachher** = dieser Stand, einmal mit
`PAPERRUST_TEXT_HIT_CACHE=off` und einmal mit Standard. Die drei Läufe direkt nacheinander;
CPU-Last beim Start 7 % / 19 % / 25 % (andere Sitzungen bauten zeitweise). Rohdaten:
`results/bench/scale/2026-11-13-perf-rest/{before,after-off,after-on}/`.

| Endpunkt (Median c=1, ms) | admin vorher | admin nachher | eingeschr. vorher | eingeschr. nachher |
|---|---:|---:|---:|---:|
| `f_title_content_common` („rechnung“, 517 745 Treffer im Inhalt), wiederholt | 21 172 | **1 472** | 5 104 | **1 207** |
| … erster Aufruf nach dem Start (Treffermenge wird aufgebaut) | 63 185¹ | 16 457 | – | – |
| `f_title_content_rare` | 4,4 | 5,6 | 11 | 7,3 |
| `selection_data_all` (POST `all=true`) | 21 419 | **3 501**² | 5 360 | **1 535**² |
| `ui_docs_default` (Liste mit `include_selection_data`) | 5 228 | 3 774² | 3 564 | 1 761² |
| `selection_data_10k` / `_100` | 311 / 15 | 317 / 13² | 399 / 15 | 403 / 13² |
| `docs_default` (Kontrolle) | 116 | 107² | 175 | 152² |

¹ kalter Plattencache direkt nach dem Start, nicht vergleichbar (ohne Trefferspeicher, 19 % Last:
23,9 s). ² Lauf `after-off` (19 % Last); im Lauf `after-on` (25 % Last) 3 992 / 4 464 / 1 860 /
1 973 ms – der Trefferspeicher wirkt dort nicht, der Unterschied ist Fremdlast.

**Gleichheit.** Alle Antworten byte-gleich zwischen vorher, nachher-ohne und nachher-mit
(admin und eingeschränkt): `title_content=rechnung` Seite 2, `title_content=beitrag` nach Titel,
`selection_data` `all` ohne Filter / mit Korrespondent-Filter und `excluded_documents` / mit
`title_content`, Angular-Liste mit `selection_data`. L1-Differenztest (`docquery_diff`, alter Weg ↔
AST-Weg; dort greift der Trefferspeicher bei reinen Textsuchen, weil die Kopie keinen
Trigramm-Index hat) auf einer Kopie von `paperless_snapshot`: **5 255 Fälle, 0 verschieden**.
Neuer Test `crates/pl-store/tests/textcache_db.rs`: 208 Vergleiche mit/ohne Treffermenge gleich,
auch nach Änderungen **nach** dem Aufbau (Inhalt/Titel trifft neu bzw. nicht mehr, neue Zeile,
Papierkorb, neue Versionen; offene und bestätigte Transaktion).

### 12.1 Titel+Inhalt mit häufigem Wort: Trefferspeicher (`pl_store::textcache`)

Befund: Schon ein `LIKE` ohne jede Umwandlung braucht für die Gesamtzahl 20–42 s – die Zeit steckt
im Lesen von 2,2 GB Text (TOAST, pglz), nicht im Vergleich. Der Trigramm-Index hilft nicht
(> 50 % Treffer, jeder wird nachgeprüft). tantivy kann es nicht **gleich** beantworten: der Inhalt
ist dort gestemmt und ASCII-gefaltet, Wörter > 129 Zeichen fehlen, und die Bigramme haben keine
Positionen – ein Teilstring wie „rechnung“ in „Abrechnungsnummer“ lässt sich nicht exakt finden.
Eine Obergrenze für die Zählung hätte das Ergebnis geändert.

Lösung (exakt, Vertrag pl-docfilter „L3b“): Die Treffer-ids je Suchtext werden einmal mit dem
Schnappschuss der Datenbank ermittelt und als Bitmaske gehalten (125 kB je 1 Mio. ids). Danach
prüft die Abfrage nur noch neue oder geänderte Zeilen am Text; unveränderte erkennt sie an
`xmin` (jedes UPDATE erzeugt eine neue Zeilenfassung). Gilt nur, wenn die Abfrage **ausschließlich**
Titel/Inhalt filtert und ohnehin alle Treffer zählt (v1-Liste, Auswahl „alle“). 15 Minuten gültig,
dann neuer Aufbau. Folge: Blättern, Umsortieren, „alle auswählen“ nach einer Suche ≈ 1,2–1,5 s.
Der **erste** Aufruf je Suchtext bleibt so teuer wie der Scan (hier 16 s) – das Ziel < 2 s ist nur
für Folgeaufrufe erreicht.

Verworfen nach Messung: id-Liste als `IN (…)` (500 000 ids passen nicht in `work_mem`, dann
lineare Suche je Zeile – Abbruch nach 7 min); paralleler Scan (der Cluster erlaubt 2 Worker,
bringt höchstens 2–3×).

### 12.2 Auswahl „alle“ (`selection_data`)

Drei Änderungen, Ergebnis gleich (Vertrag pl-documents):
1. `all=true` setzt die Auswahl als Unterabfrage ein, statt 1 Mio. ids nach Rust zu holen und als
   Liste zurückzuschicken (die zwei Prüfungen danach sind für diese Menge per Bau erfüllt).
2. Teilindex `paperrust_doc_live_fk` (Migration `pl_store.0003`, 38 MB, 22 s Aufbau auf 1 Mio.):
   die Auswahlspalten kommen per Index-Only-Scan statt aus der 1-GB-Heap-Tabelle.
3. `SET LOCAL work_mem = '96MB'` nur für die Zählabfrage: vorher lagerten materialisierte
   Auswahl und Hash-Verknüpfung (8 Durchläufe) auf die Platte aus.

Rest (admin 3,5 s): die Zählung über die Zwischentabellen (5,1 Mio. Tag-Zuordnungen, 3 Mio.
Custom-Field-Werte) – jede Zeile muss einmal angefasst werden. Unter 2 s ginge es nur mit
vorberechneten Zählern je Objekt (neue Tabelle + Trigger, Sache des Datenmodells) – nicht umgesetzt.

### 12.3 Einlesen (profiliert, nicht geändert)

Profil (`sqlx::query=debug`, 50 Textdateien auf `pl_perfrest_1m`, 5 000 Korrespondenten, 50
Workflows): je Dokument ≈ 180 ms, davon ≈ 60–75 ms in **≈ 250 SQL-Abfragen**; keine einzelne
Abfrage > 2 ms (außer dem Sequenzer im Hintergrund, 13 ms), die einzige Lücke ohne SQL > 4 ms ist
die Vorschau (13 ms). Die in §11 vermuteten 80 ms „zwischen Speichern und Workflows“ sind also kein
Block, sondern viele kleine Rundreisen: Korrespondent-Name 31×, Dokument neu laden 16×,
Tag-Vorfahren 10×, ContentType 11×, Tags einzeln einfügen 11×, Audit 11×, mehrere `UPDATE` der
Dokumentzeile (je eins pro Zuordnung, wie upstream). Spürbar weniger ginge nur durch Bündeln
(Zuordnungen im Speicher sammeln, einmal schreiben – wie §10 für `bulk_edit`); das berührt
Audit-Einträge und Signale und braucht einen eigenen Gleichheitstest. Nicht umgesetzt (offen).
Das Rohlog liegt nicht im Repo (Scratchpad der Sitzung).

### 12.4 Upload-Route `documents.post_document`

`routes.toml` jetzt `rust` (war die letzte `django`-Route). Der Rust-Weg (`pl-server/src/upload.rs`)
war vollständig; der Eintrag stand nur wegen des Mischbetriebs mit Celery auf `django`. Damals hielt
`worker::adjust_plan` die Route mit Celery-Brücke automatisch bei Django (Warnung), mit
`PAPERRUST_QUEUE=postgres` bediente Rust; seit 2026-11-14 gibt es Brücke und Schalter nicht mehr,
Rust bedient die Route immer. Standalone mit der Repo-Datei scheiterte Hochladen vorher
mit 405 (docs/DESKTOP.md). Belege: Unit-Test `errors_like_reference` (Fehlertexte gegen die
Referenz), E2E-Upload über Rust (`results/e2e/2026-11-08`), Einlese-Parität pl-consumer, Lauf
oben (50 Uploads, alle `SUCCESS`). **Nicht** erneut gegen Django geprüft: die Referenz-Django lief
in dieser Sitzung nicht, und der Paritäts-Harness hat keine Multipart-Fälle für POST.

## 13. Tempo-Reste Runde 2 (2026-11-14): Auswahl „alle“ aus den Zählern, Trefferspeicher im eigenen Datenmodell

**Kurz.** Auswahl „alle“ ohne Filter: **3,97 s → 0,21 s** (Admin), Ziel < 1 s erreicht.
Titel+Inhalt mit sehr häufigem Wort: auf der umgebauten Datenbank lieferte `main` einen
**500-Fehler**; jetzt richtig, erster Aufruf **8,5 s**, weitere **≈ 2,3 s**. Das Ziel „auch der erste
Aufruf deutlich unter 2 s“ ist **nicht erreicht** (Begründung und Weg dorthin: 13.2).

**Messaufbau.** Eigene Kopie `pl_perflast_1m` von `pl_scale_1m`, mit `manage migrate` auf den
Stand `main` gebracht – also **mit den Umbaustufen 1–4 des eigenen Datenmodells** (7:42 min). Die
Werte sind darum nicht direkt mit §12 vergleichbar (anderer Tabellenaufbau). Messinstanz Port
58211 ohne Worker, `run.py --n1 5 --dur8 5`. **Debug-Binärdateien** vorher wie nachher (vorher =
`main` 0660b3c; ein Profil `perf` gibt es im Repo nicht mehr) – die gemessenen Endpunkte hängen an
der Datenbank, nicht am Rust-Teil. Der Rechner war nicht ruhig: die Sitzung zum Datenmodell fuhr
zeitweise Massenänderungen auf einer eigenen 1-Mio.-Kopie im selben Cluster. Die Kontrollzeilen
(unveränderter Code) zeigen, wie stark ein Lauf gestört war. Rohdaten:
`results/bench/scale/2026-11-14-perf-last/{before,after,after-2}/`.

| Endpunkt (Median c=1, ms) | admin vorher | admin nachher¹ | eingeschr. vorher | eingeschr. nachher² |
|---|---:|---:|---:|---:|
| `selection_data_all` (POST `all=true`, ohne Filter) | 3 968 | **208** | 1 925 | **885** |
| `ui_docs_default` (Liste mit `include_selection_data`) | 4 530 | **609** | 2 196 | **1 178** |
| `f_title_content_common` („rechnung“), wiederholt | 500-Fehler³ | **2 379** | 500-Fehler³ | **2 359** |
| … erster Aufruf nach dem Start | 500-Fehler³ | **8 545**⁴ | – | – |
| … ohne Trefferspeicher (`PAPERRUST_TEXT_HIT_CACHE=off`) | – | 37 000–41 000⁵ | – | 8 300–13 500⁵ |
| `f_title_content_rare` | – | 8,8 | – | 12,8 |
| Kontrolle `selection_data_10k` | 249 | 264 | 280 | 333 |
| Kontrolle `selection_data_100` | 70 | 77 | 71 | 73 |
| Kontrolle `docs_default` | 183 | 229 | 226 | 300 |

¹ Lauf `after-2` (Admin-Teil ruhig). ² Lauf `after` (im Lauf `after-2` war der Teil des
eingeschränkten Nutzers gestört: Kontrolle `docs_default` 594 ms, `selection_data_all` 1 427 ms).
Der Admin-Teil von `after` war gestört (Kontrolle `selection_data_10k` 799 ms,
`selection_data_all` 595 ms). ³ `column documents_document.xmin does not exist` nach 13 s, siehe
13.2. ⁴ zwei Neustarts bei 0 % und 21 % Fremdlast: 8,54 s und 8,58 s, davon Aufbau der
Treffermenge 6,4 s bzw. 5,8 s. ⁵ Einzelaufrufe unter Fremdlast, kein ruhiger Wert.

### 13.1 Auswahl „alle“ ohne Filter: aus den Label-Zählern

Der Rest aus §12.2 war die Zählung über 5,1 Mio. Tag-Zuordnungen und 3 Mio. Feldwerte. Seit den
Umbaustufen 2/3 führt das Datenmodell genau diese Zahlen mit (`paperrust_label_count`, je Label und
Sichtbarkeits-Besitzer, samt Versionen, nur nicht gelöschte Dokumente). Ohne wirksamen Filter
rechnet `selection_data` jetzt damit (`pl_documents::selection_data::for_all_unfiltered`,
Vertrag pl-documents):

    Zahl = Σ Eimer − lebende Versionen − ausgeschlossene Wurzeln + freigegebene fremde Wurzeln

Eimer = alle (Superuser) bzw. eigene und besitzerlose (eingeschränkt). Angefasst werden nur noch
die Korrekturmengen: bei 1 Mio. Dokumenten für den Admin nichts, für den eingeschränkten Nutzer
der Messung 14 978 freigegebene Dokumente (daher 0,9 s statt 0,2 s). Gilt für die Liste mit
`include_selection_data` und für POST `all=true`, je ohne Filter (mit `excluded_documents`). Mit
Filter bleibt der Weg aus §12.2 (z. B. Korrespondent-Filter 2,3 s wie vorher).

Drei Einzelheiten, die Zeit kosteten:
1. Die Sicht `paperrust_label_count_live` gruppiert erst je Eimer, dann wird je Label summiert
   (0,4 s). Direkt über Zähler + Differenzen in einer Gruppierung: 0,08 s.
2. Freigaben als eigene UND-Bedingung statt „eigen ODER besitzerlos ODER freigegeben“: sonst
   schätzt der Planer Hunderttausende Dokumente und hasht alle 5 Mio. Tag-Zuordnungen (3,9 s).
3. `SET LOCAL random_page_cost = 1.1` nur für diese Abfrage: mit dem Standard 4 liest der Planer
   für 15 000 Dokumente lieber alle 3 Mio. Feldwerte (1,6 s statt 0,4 s).

Neuer Teilindex `paperrust_field_value_deleted` (Migration `pl_store.0004`, läuft nach den
Umbaustufen): POST zählt wie upstream auch gelöschte Feldwerte, die Zähler nicht – die kommen
direkt dazu.

**Gleichheit.** Alte Binärdatei ↔ neue, Admin und eingeschränkt, byte-gleich: POST `all` ohne
Filter / mit `excluded_documents` (auch nicht vorhandene ids) / ohne `filters` / mit wirkungslosen
Filtern / mit Korrespondent-Filter; Liste mit `selection_data` ohne Filter, mit leerem Filterwert,
mit Filter. Danach in der Kopie fünf Dokumente zu Versionen gemacht (eigene, besitzerlose, fremde,
freigegebene Wurzel), 22 Feldwerte auf acht Dokumenten gelöscht (62 ungefaltete Zähler-Differenzen)
und alles wiederholt: `selection_data` wieder gleich (die Listen unterschieden sich nur im Port der
Blätter-Links). Nicht geprüft: gegen Django selbst (lief nicht); der Vergleich ist alt ↔ neu.

### 13.2 Titel+Inhalt mit häufigem Wort

**Fehler behoben.** Der Trefferspeicher aus §12.1 las `"documents_document"."xmin"`. Nach
Umbaustufe 1 ist `documents_document` eine Sicht ohne Systemspalten: jede Suche, die den
Trefferspeicher nutzte, endete mit 500. Jetzt gilt die Menge im eigenen Datenmodell für
`paperrust_document_content`; die Abfrage verknüpft die seit dem Schnappschuss geänderten
Textzeilen und prüft nur sie am Text (Vertrag pl-docfilter „L3b“). Titel bekommen dort keine
Menge (schmale Tabelle, 0,4 s direkt).

**Aufbau parallel.** Bis zu 8 eigene Verbindungen, je ein Blockbereich, alle mit demselben
exportierten Schnappschuss – dieselbe Menge wie eine Abfrage (Test: Bitmaske gleich). Gemessen
6 s statt ≈ 20 s für den Aufbau; auf 6 Kernen mit Fremdlast also Faktor 3, nicht 8.

**Warum nicht unter 2 s.** Gemessen auf der Kopie, eine Verbindung, ruhig:

| Schritt über 1 Mio. Dokumente (2,2 GB Text) | Zeit |
|---|---:|
| Textzeilen lesen, ohne zu entpacken (`octet_length`) | 0,8 s |
| `LIKE` ohne Faltung | 9,3 s |
| Faltung `UPPER(… COLLATE "C")` + `LIKE` | 12,6 s |
| dieselbe Faltung mit den zwei `replace()` der Datenbank-Kollation | ≈ 20 s |
| Trigramm-Index: Kandidaten (517 745 = alle Treffer) | 0,26 s |
| … mit der Pflicht-Nachprüfung jedes Kandidaten | 18,3 s |

Die Zeit ist Rechenzeit im Vergleich selbst, nicht Platte und nicht Planung. Ein exaktes Ergebnis
verlangt, den Text jedes Treffers einmal zu prüfen; der Trigramm-Index liefert die Kandidaten in
0,26 s, darf aber ohne Nachprüfung nicht gelten. LZ4 statt pglz brächte 20 % (gemessen an 200 000
Zeilen: 2,03 → 1,59 s). Mehr Verbindungen bringen auf 6 Kernen nichts mehr (4 → 8 → 12
Verbindungen: 4,4 / 3,3 / 3,3 s für die einfache Faltung).

Folgeaufrufe ≈ 2,3 s (alt: 1,5 s im alten Tabellenaufbau): 1,0 s kostet das Lesen der
Zeilenfassungen der Texttabelle (812 MB Heap, weil Text unter 2 kB in der Zeile bleibt), 0,4 s der
Titelvergleich, der Rest Zählung und Sortierung.

**Was es bräuchte (Sache des Datenmodells, nicht gebaut):**
- *Erster Aufruf:* ein exakter Wortindex – je Dokument die verschiedenen, durch Leerraum getrennten
  Wörter (in Großschreibung der Datenbank), Suche = Wörter mit dem Teilstring im Wörterbuch finden
  und ihre Dokumentlisten vereinigen. Exakt für Suchtexte ohne Leerraum, per Trigger gepflegt.
  Grob geschätzt 100–200 Mio. Einträge bei 1 Mio. Dokumenten (nicht gemessen).
- *Folgeaufrufe:* wenn jede Textänderung auch die schmale Zeile `paperrust_document` anfasst
  (heute nur über die Sicht garantiert, nicht für Schreiber direkt auf der Texttabelle bei
  gleicher Länge), reicht deren Zeilenfassung: aus den Einzelmessungen geschätzt ≈ 0,6 s statt 2,3 s (nicht gebaut, nicht gemessen).
- Klein, ohne Datenmodell, offen: die zwei `replace()` nur für Zeilen mit den seltenen
  Sonderzeichen ausführen (Aufbau ≈ −30 %).

**Gleichheit.** `crates/pl-store/tests/textcache_db.rs` auf zwei Kopien von `paperless_snapshot`
(alter Aufbau und mit Umbaustufen): je 208 Vergleiche mit/ohne Treffermenge gleich, dazu
Bitmaske mit 3 Verbindungen = mit einer, und im eigenen Datenmodell eine Änderung direkt in der
Texttabelle. Auf 1 Mio. Dokumenten mit ↔ ohne Trefferspeicher byte-gleich (Admin und
eingeschränkt): Zählung, alle Treffer-ids (v9 `all`), Seite 3 nach Titel, `content__icontains`,
`selection_data` mit `title_content` und Ausschluss. Seiten mit `ordering=-created` haben gleiche
Zählung, aber andere Reihenfolge innerhalb desselben Datums: die Sortierung hat dort wie upstream
keine Gleichstandsregel, die Reihenfolge hängt vom Plan ab (`apply_sort` ist unverändert).

## 14. Titel+Inhalt mit häufigem Wort, Runde 3 (2026-11-15): Ziel unter 2 s nicht erreichbar

**Kurz.** Ziel war: erster Aufruf und Folgeaufrufe je unter 2 s. **Nicht erreicht.** Erreicht:
Wörter mit vielen „Sonderzeichen-Partnern“ (z. B. „beitrag“, „steuer“) beim ersten Aufruf
**3× bzw. 1,5× schneller**, „rechnung“ ≈ 10 %; Folgeaufrufe ≈ 2,1–2,6 s (vorher 2,2–3,0 s).
Antworten gleich. Unter 2 s geht es nur mit einer Datenmodell-Änderung (14.3).

**Messaufbau.** Neue Kopie `pl_perflast_1m` von `pl_scale_1m`, mit `manage migrate` auf `main`
21ac8ec (Umbaustufen 1–5, 26 min), danach `VACUUM ANALYZE`. Debug-Binärdateien vorher (= `main`)
und nachher, je zweimal abwechselnd, jede Instanz frisch gestartet (erster Aufruf = Aufbau der
Treffermenge). Fremdlast während der Läufe 15–41 % CPU (andere Sitzungen), keine fremden
DB-Abfragen. Skript und Rohdaten: `results/bench/scale/2026-11-15-perf-last/`.

| Aufruf (Admin, s) | vorher Lauf 1 / 2 | nachher Lauf 1 / 2 |
|---|---:|---:|
| `title_content=rechnung`, erster Aufruf | 11,1 / 10,2 | **9,2 / 9,1** |
| … Folgeaufrufe (4×, Spanne) | 2,2–2,4 / 2,3–3,0 | 2,1–2,3 / 2,3–2,6 |
| … eingeschränkter Nutzer | 1,9–2,2 / 2,0–2,1 | 1,9–1,9 / 2,2–2,3 |
| `title_content=beitrag`, erster Aufruf | 41,5 / 44,6 | **12,8 / 15,6** |
| `title_content=steuer`, erster Aufruf | 15,4 / 16,0 | **9,8 / 11,3** |
| `title_content=steuer`, Folgeaufruf | 3,1 / 3,0 | 2,3 / 2,7 |

**Gleichheit.** v9-Liste (`all` = alle Treffer-ids, Zählung) für „rechnung“ (517 358),
„beitrag“ (70 412), „steuer“ (341 285): vorher ↔ nachher byte-gleich.
`crates/pl-store/tests/textcache_db.rs` auf altem und neuem Tabellenaufbau: je 272 Vergleiche
mit/ohne Treffermenge gleich (neu: „beitrag“, exakter Vergleich, und ein Dokument mit `ı`/`ſ`, das
nur über die gefaltete Form trifft).

### 14.1 Was geändert ist

1. **Aufbau: Text einmal großschreiben, Sonderzeichen nur wo vorhanden.** Die Faltung ist
   `replace(…(UPPER(x COLLATE "C"))…) LIKE P` mit je einem `replace()` für jedes Sonderzeichen der
   Datenbank-Kollation, dessen Großbild einen Buchstaben des Suchtexts enthält (`ß`, `ı`, `ſ`, `ŉ`,
   `ẖ`, `ẗ`, `ẚ`, `ﬀ`…`ﬆ`). Ab mehr als 4 solchen Zeichen nahm sie die Kollations-`UPPER()` – auf
   diesem Cluster 7× langsamer („beitrag“ hat 7 Partner: 28 s statt 8 s auf 200 000 Dokumenten).
   Jetzt im Aufbau: `u = UPPER(x COLLATE "C")` einmal je Zeile (Unterabfrage mit `OFFSET 0`), dann
   `u LIKE P ODER (u enthält eines der Zeichen UND replace-Kette(u) LIKE P)`. Beweis der Gleichheit
   im Code (`textcache::upper_once_cond`): ein ASCII-Treffer in `u` bleibt nach den `replace()`
   stehen, und ohne Sonderzeichen ändern die `replace()` nichts.
2. **ODER mit Treffermenge zuerst** (`docsql`, `title_content` = Titel ODER Inhalt): die billige
   Bitmasken-Prüfung steht vorn, der Titelvergleich läuft nur noch für Zeilen ohne Inhaltstreffer.

Verworfen nach Messung: Prüfung auf Sonderzeichen mit `strpos()` (mehrbytige Zeichen: 1,4 s je
Zeichen auf 200 000 Dokumenten, langsamer als `replace()`), regulärer Ausdruck (5,7 s).

### 14.2 Warum es nicht unter 2 s geht

*Erster Aufruf.* Exakt heißt: den Text jedes Dokuments einmal prüfen (2,2 GB). Die billigste
Prüfung, die noch gleich ist, kostet auf einer Verbindung ≈ 12 s (Entpacken + `UPPER … COLLATE
"C"` + `LIKE`); 8 Verbindungen auf 6 Kernen mit Fremdlast bringen Faktor 2–3 (§13.2). Dazu kommt
die Zählung (≈ 2 s). Kein Index kann das heute abkürzen: der Trigramm-Index liefert die Kandidaten
in 0,26 s, verlangt aber die Nachprüfung jedes Kandidaten am Text.

*Folgeaufrufe.* Gemessen ≈ 2,1–2,6 s, davon ≈ 1,0 s allein, um die Zeilenfassungen der
Texttabelle zu lesen (812 MB Heap, Text unter 2 kB bleibt in der Zeile) – nötig, damit Änderungen
direkt an der Texttabelle erkannt werden. Der Rest: schmale Tabelle, Bitmaske, Titelvergleich für
Nicht-Treffer, Zählung und Sortierung von 517 000 Treffern.

### 14.3 Was es bräuchte (Datenmodell, nicht gebaut)

- **Folgeaufrufe unter 2 s:** jede Textänderung fasst auch die Zeile in `paperrust_document` an
  (heute nur über die Sicht garantiert; der Längen-Trigger der Texttabelle tut es nur bei
  geänderter Länge). Dann reicht deren Zeilenfassung (127 MB statt 812 MB): aus den
  Einzelmessungen geschätzt ≈ 1,2 s statt 2,3 s – nicht gebaut, nicht gemessen.
- **Erster Aufruf unter 2 s:** ein exakter Index für Teilstrings. Prototyp auf 20 000 Dokumenten
  (GIN über alle Wort-Endstücke, Abfrage als Präfixsuche): exakt gleiche Treffer für „rechnung“
  und „echn“, Abfrage 85–330 ms. Aber hochgerechnet **≈ 5 GB Index und ≈ 30 min Aufbau** bei
  1 Mio. Dokumenten, jedes Einfügen deutlich teurer, und `tsvector` ist auf 1 MB je Dokument
  begrenzt (sehr lange Texte würden beim Einfügen scheitern). Für Suchtexte mit Leerzeichen ist er
  nicht exakt (Nachprüfung). Das ist eine Entscheidung über Speicher und Schreibkosten – Sache des
  Datenmodells, nicht dieses Lesepfads.
