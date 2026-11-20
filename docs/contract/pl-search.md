# Vertrag pl-search (Volltextsuche, tantivy, nur lesend)

Quellen (alle `upstream/src/`, v3.4.2): `documents/search/_backend.py` (`TantivyBackend`,
`build_permission_filter`, `SearchHit`, `TantivyRelevanceList`), `_schema.py`, `_fields.py`,
`_registry.py`, `_tokenizer.py`, `_query.py`, `_errors.py`; `documents/views.py`
(`UnifiedSearchViewSet.list`, `_validate_tantivy_search_params`, `_get_tantivy_query_and_mode`,
`_get_more_like_id`, `DocumentSelectionMixin._get_search_document_ids`, `SearchAutoCompleteView`,
`GlobalSearchView`); `documents/serialisers.py` (`SearchResultSerializer`); Bibliothek
**whoosh-compat 0.3.0** (Parser + tantivy-Emitter der Query-Syntax) und **tantivy-py 0.26.0**
(`src/searcher.rs`, `src/query.rs`, `src/index.rs`, `src/schemabuilder.rs`).
Tests: `documents/tests/search/*`, `documents/tests/test_api_search*.py`.

## Index-Format und Version

- tantivy-py 0.26.0 bettet laut seinem `Cargo.lock` **tantivy 0.26.0** ein (tantivy-common 0.11.0,
  tantivy-columnar/sstable/stacker 0.7.0, tantivy-fst 0.5.0, rust-stemmers 1.2.0). pl-search
  verwendet `tantivy = "=0.26.0"` mit Standard-Features (wie tantivy-py) → gleiches On-Disk-Format,
  gleiche Scoring-, Snippet- und Sortier-Implementierung.
- Index liegt in `DATA_DIR/index` (`PAPERLESS_DATA_DIR`), geschrieben **nur** vom Python-Worker.
  Rust öffnet `Index::open_in_dir`, registriert die Tokenizer (sonst sind Abfragen nicht
  analysierbar) und liest mit `ReloadPolicy::OnCommitWithDelay` (wie tantivy-py `index.reader()`):
  Commits des Workers werden ohne Neustart sichtbar. Kein `IndexWriter`, keine Lock-Datei, kein
  Löschen/Neuaufbau.
- `.index_settings.json` wird geprüft (`schema_version == 4`, `language == SEARCH_LANGUAGE`),
  zusätzlich die geordnete Feldliste gegen `field_descriptors()`. Passt etwas nicht oder fehlt das
  Verzeichnis, sucht Rust in einem leeren RAM-Index mit gleichem Schema (= was upstream nach
  Wipe/Neuanlage sieht) und versucht alle 5 s erneut zu öffnen. `.rebuilding` wird wie upstream
  ignoriert (halb gebauter Index wird gelesen).

### Schema (33 Felder, Reihenfolge = On-Disk-Vertrag)

`id`(u64 stored/indexed/fast), `title`, `content`, `correspondent`, `document_type`,
`storage_path`, `original_filename`, `tag` (text, stored, `paperless_text`), `checksum`
(text, stored, `raw`), `asn`, `page_count`, `num_notes` (u64 fast), `created`, `modified`, `added`
(date fast), `notes` (json, `paperless_text`), `notes_text` (text), `custom_fields`, `barcodes`
(json), `title_sort`, `correspondent_sort`, `type_sort` (text fast `simple_analyzer`, nicht
stored), `bigram_content`, `bigram_title`, `bigram_correspondent`, `bigram_document_type`,
`bigram_tag` (`bigram_analyzer`), `simple_title`, `simple_content` (`simple_search_analyzer`),
`autocomplete_word` (`raw`), `owner_id`, `viewer_id`, `viewer_group_id` (u64 fast, nicht stored).

### Tokenizer (Namen und Pipelines exakt wie `_tokenizer.py`)

| Name | Pipeline |
|---|---|
| `paperless_text` | simple → remove_long(129) → lowercase → ascii_fold [→ Snowball-Stemmer `SEARCH_LANGUAGE`] |
| `simple_analyzer` (auch Fast-Field-Tokenizer) | simple → lowercase → ascii_fold |
| `bigram_analyzer` | ngram(2,2, prefix_only=false) → lowercase |
| `simple_search_analyzer` | regex `\S+` → remove_long(129) → lowercase → ascii_fold |

Sprachen: `_LANGUAGE_MAP` (ar, da, nl, en, fi, fr, de, el, hu, it, no, pt, ro, ru, es, sv, ta, tr
und englische Namen). `SEARCH_LANGUAGE`: `PAPERLESS_SEARCH_LANGUAGE`, sonst aus erster
`PAPERLESS_OCR_LANGUAGE` (ISO 639-2 → 639-1), sonst `None` (kein Stemming).

## Abfragelogik (Engine, `pl_search::backend::SearchBackend`)

- Jede Abfrage wird NFC-normalisiert (`normalize_search_text`).
- **TEXT** (`text=`): Tokens wie `simple_search_analyzer`; je Token ein `RegexQuery` auf
  `simple_title` (Boost 2.0) und `simple_content`, alle Tokens `Must`. Muster `.*tok.*`; bei
  mehreren Tokens werden rein numerische Tokens an Wortgrenzen/Satzzeichen verankert
  (`(tok.*|.*[\x20-\x2f\x3a-\x40\x5b-\x60\x7b-\x7e]tok.*)`). Escape wie Pythons `regex.escape`.
  Enthält die Eingabe CJK, zusätzlich `Should` über `bigram_content` (QueryParser der CJK-Läufe).
- **TITLE** (`title_search=`): dasselbe nur über `simple_title` / `bigram_title`.
- **QUERY** (`query=`): Whoosh-Syntax über whoosh-compat (Port in `src/wc/`, `src/query_user.rs`):
  Standardfelder `title`(Boost 2), `content`, `correspondent`, `document_type`, `tag`; Präfixe
  `title: content: correspondent: document_type:/type: storage_path:/path: original_filename:
  tag:(Komma-Werte) checksum: asn: page_count: num_notes: created:(Datum) modified: added:
  notes:/notes.note: notes.user: custom_fields:/custom_fields.value: custom_fields.name:
  barcodes:/barcodes.value: barcodes.format:`; Datumsbereiche und relative Daten in
  `PAPERLESS_TIME_ZONE`; CJK-Blätter zusätzlich über `bigram_*`; bei gesetztem
  `PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD` Unscharf-Verbreiterung (Fuzzy, Distanz 1, Präfix,
  Boost 0.1, nicht unter Negation) und Mischung `Must const_score(widened,0.1)` +
  `Should scored` + `Should boost(widened,0.01)`.
- Filter-Query: `text`/`title_search` zusammen mit `query` → `query` als
  `Must const_score(…, 0.0)`.
- **Rechtefilter im Index** (nicht für Superuser): `Must DisjunctionMax(no_owner, owner_id=user,
  viewer_id=user, viewer_group_id∈aktuelle Gruppen)`, `no_owner = All AND NOT exists(owner_id)`.
  Zusätzlich **DB-seitig**: Liste schneidet mit `filter_queryset(get_queryset())` (pl-documents),
  `more_like_id` prüft `has_perms_owner_aware`, globale Suche schneidet mit
  `permitted_document_ids`.
- Relevanz: `TopDocs(limit=num_docs).order_by_score()`, Scores durch Maximum geteilt
  (Max 0 → 1), bei Schwelle `score >= threshold`. Sortierung nach `created/added/modified`
  (date), `archive_serial_number→asn`, `page_count`, `num_notes` (u64) per
  `order_by_fast_field`, `Asc` bzw. `Desc` bei `-`.
- Hervorhebungen (`highlight_hits`): eine Suche `Must user_query AND Should(id=…)`, je Treffer
  `SnippetGenerator(content)` (150 Zeichen, `<b>…</b>`, HTML-escaped) mit der Nutzer-Query bzw.
  im TEXT-Modus `"tok1" "tok2"` über `content`; Notizen nur im QUERY-Modus über `notes_text` mit
  der Query ohne `feld:`-Präfixe (`re.sub(r"\w[\w.]*:", "", q)`). Rang = absolute Position (1-based).
- Autocomplete: `ascii_fold(term.lower())`, Präfixscan im Term-Dictionary von
  `autocomplete_word`, Zählung je Term = Dokumentfrequenz (ohne Filter, inkl. gelöschter Dokumente
  wie tantivy-py) bzw. Anzahl Postings im Rechtefilter (gelöschte ausgeschlossen); Sortierung
  Anzahl absteigend, dann Term aufsteigend; `limit`.
- More like this: Dokument per `id`-Term, `MoreLikeThisQuery(min_doc_frequency=1,
  min_term_frequency=1, max_query_terms=12, stop_words=[])`, Rechtefilter, `limit+1`, Original
  entfernt. Panik in tantivy (BM25 idf mit gelöschten Docs) → leere Liste (wie upstream).

## Routen

### `search.list` – GET `/api/documents/?query=|text=|title_search=|more_like_id=`

Teil von `UnifiedSearchViewSet.list` → **Route gehört pl-documents** (gleicher Pfad wie die
Dokumentliste). pl-search liefert die Funktion; Vorschlag für Coder 3 (pl-documents):

```rust
// pl_search::views
pub fn is_search_request(params: &[(String, String)]) -> bool;
pub trait FilteredDocuments: Send + Sync {          // implementiert pl-documents
    /// filter_queryset(get_queryset()).filter(pk__in=ids) – ids in ORM-Reihenfolge (ordering!)
    fn ids_in<'a>(&'a self, ids: &'a [i64]) -> BoxFut<'a, Result<Vec<i64>, ViewError>>;
    /// filter_queryset(get_queryset()) – alle ids (Reihenfolge egal)
    fn all_ids(&self) -> BoxFut<'_, Result<Vec<i64>, ViewError>>;
}
pub async fn document_search(db: &PgPool, backend: Arc<SearchBackend>, user: &RequestUser,
    params: &[(String, String)], page_size: usize, filtered: &dyn FilteredDocuments)
    -> Result<DocumentSearchOutcome, ViewError>;
pub enum DocumentSearchOutcome { Page(SearchResultPage), Response(ViewResponse /* 400/403 fertig */) }
impl SearchResultPage { fn count(&self) -> usize; fn slice(&self, start, stop) -> Vec<SearchHit>; }
impl SearchHit { fn to_json(&self) -> Value /* {"score","highlights","note_highlights","rank"} */ }
```

Ablauf in pl-documents: wenn `is_search_request` → Filter des FilterSets wie gewohnt (Fehler 400
vorher wie upstream), `document_search(...)`; bei `Page(p)`: StandardPagination über
`p.count()` (ungültige Seite → 404 `{"detail":"Invalid page."}` wie DRF), Seite
`p.slice(start, stop)`, `serialize_documents(ids der Hits)` und je Dokument als **letzten** Schlüssel
`"__search_hit__": hit.to_json()`; Antwort `{count,next,previous,all,results}` (Version wie
Liste) + `"corrected_query": null` + bei `include_selection_data=true` `"selection_data"` über
`filtered_qs.filter(pk__in=p.ordered_ids)`.

- Rechte: wie Dokumentliste (`IsAuthenticated`, `PaperlessObjectPermissions`→`view_document`).
- Query-Parameter: `query`, `text`, `title_search`, `more_like_id`, `ordering` (`score`, `-score`,
  sortierbar per Index: `created added modified archive_serial_number page_count num_notes`; andere
  Felder → ORM-Reihenfolge), `page`, `page_size`, `include_selection_data`, alle Filter der Liste.
- `__search_hit__`: `score` (float, Relevanz; bei Index-Sortierung der Score der Highlight-Suche,
  bei more_like_id 0.0), `highlights` (str, `""` wenn keiner), `note_highlights` (str|null), `rank`
  (int, 1-based absolut). Seiten, die nicht bei `page_offset` beginnen (z. B. `page=last`),
  liefern Stub-Treffer (score 0.0, keine Highlights) – wie upstream.
- `ordering=score` (aufsteigend) kehrt die Relevanzreihenfolge um; `-score` = beste zuerst.
- Fehler:
  - mehr als ein Suchparameter (außer `{text,query}`, `{title_search,query}`) → 400
    `{"detail":"Specify only one of text, title_search, query, or more_like_id."}`
  - Query länger als 4096 Zeichen → 400 `{"query":["The search query is too long (N characters).
    The maximum allowed length is 4096 characters."]}`
  - Syntaxfehler (Datum/Zahl/…) → 400 `{"query":[…alle Meldungen…]}`, z. B.
    `Invalid date value 'foo' for field 'created'.`, `Invalid numeric value 'x' for field 'asn'.`,
    `The search query is nested too deeply.`, `Wildcard patterns are not supported for field 'asn' (numeric).`,
    `Range searches are not supported for field 'title'.`,
    `Existence searches (field:*) are not supported for field 'notes'.`
  - `more_like_id` keine Zahl / Dokument fehlt / gelöscht → 403 Klartext `Invalid more_like_id`
    (text/html); ohne Recht → 403 Klartext `Insufficient permissions.`

### `search.global` – GET `/api/search/` (`GlobalSearchView`)

- Rechte: `IsAuthenticated`; je Abschnitt Modellrecht.
- Parameter: `query` (Pflicht), `db_only` (jeder nicht-leere Wert = wahr, auch `false`!).
- Fehler (400 Klartext text/html): `Query required`, `Query must be at least 3 characters`,
  `Query must be at most 4096 characters` (Länge in Codepoints).
- Antwort (Reihenfolge): `total` (Summe aller Listen), `documents`, `saved_views`, `tags`,
  `correspondents`, `document_types`, `storage_paths`, `users`, `groups`, `mail_rules`,
  `mail_accounts`, `workflows`, `custom_fields`; je höchstens 3.
- `documents` (nur mit `documents.view_document`): `db_only` → `title__icontains` über
  permitted (nicht gelöscht) Dokumente, `ORDER BY created DESC LIMIT 3`; sonst TEXT-Suche im Index
  (`limit=9`, Rechtefilter), Schnitt mit permitted, erste 3 in Index-Reihenfolge. Felder
  `{"id","title","created":"YYYY-MM-DD"}`.
- Andere Abschnitte: `name__icontains` (users: `username__icontains`), Modellrecht +
  `restrict_queryset_to_visible` (saved views, tags, correspondents, document types, storage
  paths, mail rules, mail accounts; nicht users/groups/workflows/custom fields), Serializer des
  jeweiligen Crates. Umsetzung `src/global.rs` (eingebaute Abschnitte; per
  `set_global_search_providers` registrierte Provider haben Vorrang):
  - Filter `UPPER(<feld>::text) LIKE UPPER('%…%')` mit `\`/`%`/`_` maskiert (Django `icontains`),
    Modellrecht `request.user.has_perm(...)` (inaktiv → nichts), `restrict_queryset_to_visible`
    über `pl_authz::objects::restrict_to_visible` (aktiver Superuser unbeschränkt).
  - Reihenfolge = `Meta.ordering`: Tags/Korrespondenten/Dokumenttypen/Speicherpfade/Saved Views
    `name`, Custom Fields `created`; Users, Groups, Mail-Regeln, Mail-Konten, Workflows ohne
    `ORDER BY` (wie Django – PostgreSQL liefert für dieselbe Abfrage dieselbe Reihenfolge). `LIMIT 3`.
  - Serializer-Kontext upstream nur `{"request": request}` (kein `user=`, kein `full_perms`, keine
    View): `user_can_change` = `owner is None` (für alle, auch Superuser), nie `permissions`;
    `document_count` fehlt auf oberster Ebene (nicht annotiert → `SkipField`), Tag-`children`
    mit `document_count` und Sortierung `Lower("name")` (`get_children` ohne View, `ordering`
    der Anfrage wird ignoriert); Saved Views mit `show_on_dashboard`/`show_in_sidebar` bei
    API-Version < 10.
  - Serializer: Tags/Korrespondenten/Dokumenttypen/Speicherpfade `pl_objects::serialize`, Users/
    Groups `pl_auth_api::{users,groups}::*_repr`, Workflows `pl_workflows::load::render_workflows`
    (ohne Viewset-Prefetch). Saved Views, Custom Fields, Mail-Regeln, Mail-Konten lokal
    nachgebaut (Felder in Reihenfolge wie pl-fields-views bzw. pl-mail; pl-mail → pl-consumer →
    pl-search wäre ein Zyklus, pl-fields-views hält die Serializer privat).

### `search.autocomplete` – GET `/api/search/autocomplete/` (`SearchAutoCompleteView`)

- Rechte: `IsAuthenticated` + `ViewDocumentsPermissions` (`documents.view_document`).
- Parameter: `term` (Pflicht, `strip()`), `limit` (Standard 10).
- Fehler: ohne `term` → 400 Klartext `Term required`; `limit <= 0` → 400 Klartext
  `Invalid limit`; `limit` keine Ganzzahl → **500** (unbehandelter `ValueError` upstream).
- Antwort: JSON-Liste von Wörtern (siehe Autocomplete oben), Nicht-Superuser mit Index-Rechtefilter.

### Auswahl-Filter (bulk_edit u. a., `DocumentSelectionMixin._get_search_document_ids`)

`pl_search::views::selection_search_ids(db, backend, user, filters)` → `Ok(None)` ohne
Suchfilter, sonst ids. Fehler: 400 `{"detail": "Specify only one …"}` bzw. `{"query":[…]}`;
`more_like_id` ungültig/ohne Recht → **403 JSON** `{"detail":"Invalid more_like_id"}` /
`{"detail":"Insufficient permissions."}` (hier nicht in Klartext umgewandelt). Werte werden wie
Pythons `str()` behandelt (`True`, Zahlen, …). Keine Sortierung, Rechtefilter wie Liste.

Nebenwirkungen: keine (nur lesend). Keine Celery-Aufgaben.

## Status

| id | Status |
|---|---|
| Engine (Schema, Tokenizer, TEXT/TITLE, Rechtefilter, Sortierung, Schwelle, Highlights, Autocomplete, MLT, Reload) | fertig, Unit-Tests gegen selbst erzeugten Index |
| QUERY-Modus (whoosh-compat-0.3.0-Port, `src/wc/`, `src/query_user.rs`) | fertig: Parser (Tagger linear statt O(n²), gleiche Ergebnisse), Datumsgrammatik, Emitter, CJK-/Fuzzy-Verbreiterung, Fehlertexte; 16 Unit-Tests. Nicht getestet: Zeitzonen ≠ UTC |
| `search.list` | Funktion fertig, Route gehört pl-documents |
| `search.autocomplete` | fertig, gemountet (`src/routes.rs`, AuthUser + `VIEW_DOCUMENTS_PERMISSIONS`) – aktivierbar nach Paritätslauf |
| `search.global` | fertig: Dokument-Teil + alle 11 Objekt-Abschnitte (`src/global.rs`), Parität `search.global.*` ohne Abweichung |

Backend: prozessweiter Singleton (`routes::backend`), Index-Verzeichnis und Zeitzone aus
`pl_core::Config`, Sprache/Fuzzy-Schwelle aus der Umgebung. Paritätsprüfung gegen den
Referenz-Index: **offen** (Referenzumgebung/Index lag noch nicht vor); Fälle in
`tests/parity/pl-search/cases.json` (+ `cases.base.json`).

## L3: Suche ohne „alle Treffer ranken + alle IDs schneiden“ (ARCHITECTURE-V2 §5, 2026-11-10)

Ziel: Volltext-Dokumentliste (`query=`, `text=`, `title_search=`, `more_like_id=`) skaliert mit der
Seite statt mit der Trefferzahl; v1-Antworten bleiben identisch (Ids, Reihenfolge, `count`,
`__search_hit__` der Seite). **Indexformat unverändert** (upstream tantivy-py 0.26, Schema v4): ein
von paperless-ngx geschriebener Index wird weiter gelesen, `document_index reindex` baut ihn
unverändert neu, ein Rückweg zu paperless-ngx braucht keinen Neuaufbau.

### Umschalter

`PAPERRUST_SEARCH_ENGINE=legacy|native` (Klasse A, live; `pl-settings`, docs/SETTINGS.md 1.10),
Standard `native`. `legacy` = upstream-Ablauf 1:1 (`search_ids` mit `limit=num_docs`, dann
`filtered.ids_in` bzw. `filtered.all_ids()` ab 5 000 Treffern).

### Schneller Weg (`views::native_window`)

Upstream: Liste = `tantivy(q ∧ Rechte_Index)` geschnitten mit `filter_queryset(get_queryset())`
(Basis = Wurzeldokumente außerhalb des Papierkorbs, DocumentFilterSet, `PermittedObjectsFilter`).
Der Rechtefilter im Index (`owner_id`, `viewer_id`, `viewer_group_id`, „ohne Eigentümer“) ist
dieselbe Regel wie `PermittedObjectsFilter` (Eigentümer, ohne Eigentümer, direkte
`view_document`-Freigabe an Nutzer/aktuelle Gruppen; Gruppenmitgliedschaft wird bei beiden zur
Abfragezeit gelesen). Ist der Index für Rechte und Lebendigkeit aktuell, liegt jeder Index-Treffer
in der ORM-Menge, und die Schnittmenge ist der Index-Treffer selbst. Bedingungen (sonst exakter
Weg = `legacy`):

1. keine Filterparameter außer Suche/Sortierung/Paginierung/Darstellung (`NON_FILTER_PARAMS`;
   jeder andere Schlüssel, auch unbekannte → exakt),
2. aktiver Nutzer,
3. Sortierung `-score`/ohne oder ein Index-Feld (`created added modified archive_serial_number
   page_count num_notes`); `ordering=score` (aufsteigend) und ORM-Sortierungen → exakt,
4. `paperrust_search_dirty` existiert und ist leer (siehe unten).

Dann: Trefferzahl = `Count`-Collector über dieselbe Query, Seite = Top-(Offset+Größe)
(`backend::search_window`; `more_like_this_window` analog ohne das Ausgangsdokument). Gleichheit:
tantivys `TopNComputer` ordnet total (Schlüssel, dann `DocAddress` aufsteigend), Top-k ist also
das Präfix der vollständigen Rangfolge. Ausnahmen mit vollständiger Rangfolge (`limit =
Trefferzahl`, wegen der nie voll werdenden Puffer exakt wie `limit = num_docs`, aber ohne
`num_docs`-große Puffer): Relevanz-Sortierung **ohne** Rechtefilter (Superuser – eine reine
Term-Vereinigung wird per Block-WAND ausgewertet, deren Summationsreihenfolge von der Schwelle
abhängt), gesetzte `ADVANCED_FUZZY_SEARCH_THRESHOLD` (Normierung über das Maximum), und wenn der
Aufrufer alle IDs braucht (`SearchOptions::need_all_ids`: API v9 `all`, `include_selection_data`,
`page=last`). Die IDs der Seite werden trotzdem per SQL gegen die ORM-Menge geprüft
(`filtered.ids_in(seite)`); fehlt eines (Wettlauf mit einer Änderung), gilt der exakte Weg.
Hervorhebungen nur für die Seite (`highlight_hits`, unverändert).

Nicht umgestellt (exakter upstream-Ablauf): `selection_search_ids` (bulk_edit u. a. mit Suche),
globale Suche (`limit=9`), Autocomplete.

### Index-Konsistenz für Rechte/Papierkorb (`pl_indexer.0001_paperrust_search_dirty`)

Expand-only Migration (pl-indexer): Tabelle `paperrust_search_dirty(doc_id, seq, marked)`,
Zeilentrigger auf `documents_document` (`UPDATE OF owner_id, deleted_at, root_document_id` bei
Änderung, `DELETE`) und auf `guardian_userobjectpermission`/`guardian_groupobjectpermission`
(`UPDATE`, `DELETE`, `BEFORE TRUNCATE` merkt alle betroffenen Objekte). Nur Änderungen, die
Sichtbarkeit/Lebendigkeit **wegnehmen** können, müssen gemerkt werden: neue Freigaben/Dokumente
lässt auch upstream erst nach dem Neuindexieren erscheinen (Rechtefilter des Index gilt dort
ebenso). Nachführen: `pl-server` `search_sync` (im HTTP-Prozess, ruht bei `legacy`,
`PAPERRUST_INDEX_WRITER=off`, ohne Migration/Index) → `pl_indexer::dirty::drain_once`: bis 1 000
Einträge entfernen + neu schreiben (lebende Wurzel bzw. Wurzel einer Version), Commit (Sperre wie
der Python-Writer), Leser neu laden, erst dann genau die gesehenen `(doc_id, seq)` löschen.
`document_index reindex` (Rust) löscht danach alles bis zur `seq` vom Start. Rückbau: siehe Kopf
der Migration.

Grenze: Einträge, die am Index vorbei entstehen (Index aus einer Sicherung zurückgespielt, Datenbank
ohne Trigger geändert), erkennt der Weg nicht – dann `document_index reindex` (wie upstream nach
solchen Eingriffen).

### Tests und Ergebnisse

Siehe docs/PERF-SCALE.md §9 (L3) und unten „Status“. Differenztest
`crates/pl-server/tests/search_l3_diff.rs` (`#[ignore]`, DB- + Index-Kopie): `native_equals_legacy`
vergleicht je Nutzer × Anfrage (häufige Index-Begriffe, UND/ODER/NICHT/Phrase/Feld/Präfix/Datum,
TEXT/TITLE, Filter-Query, ungültige Syntax, `more_like_id`, zufällige Sortierung, Seiten 1/2/last/
zufällig/zu groß, Größen 25/10) Trefferzahl, Seiten-Ids, `__search_hit__` und mit `need_all_ids`
die ganze Liste; `dirty_tracking_and_drain` (`PL_L3_MUTATE=1`) entzieht Freigaben, verschiebt in den
Papierkorb, wechselt Eigentümer und prüft vor und nach dem Nachführen gegen `legacy`.

Korrektur 2026-11-11 (Paritätslauf mit eingespielten Migrationen): `more_like_this_window` zählte
mit `Count` allein; für Superuser (MLT-Query ohne umgebenden Rechtefilter) schaltet tantivy dabei
das Scoring ab → „MoreLikeThisQuery requires to enable scoring“ → 500. Jetzt `count_scored`
(Tupel `Count` + Top-1 nach Score, gleiche Menge). Fall `search.documents.more_like@admin/v10`.

Korrektur 2026-11-13 (Nachmessung 1 Mio.): Nach Aktualisierungen ohne Segment-Merge zählt tantivy
0.26 bei `doc_freq` gelöschte Dokumente mit, `MoreLikeThis` rechnet die idf aber mit den lebenden
(`num_docs`). Ein überall vorkommender Begriff (`doc_freq > num_docs`) löste die Assertion in
`bm25::idf` aus; die Panik wurde abgefangen und lieferte eine **leere** Liste („Ähnliche Dokumente“
leer, bei 1 Mio. mit ~10 000 gelöschten Altfassungen immer). Jetzt beide MLT-Pfade mit
`with_max_doc_frequency(num_docs)`: solche Begriffe (idf ≈ 0) fallen weg, die Rangfolge der übrigen
bleibt. Test `more_like_this_survives_deleted_docs`. Abweichung von upstream (gibt in diesem Fall
`[]` zurück) bewusst – Ergebnis für Nutzer besser, im Paritätsdatensatz nicht auslösbar.

## Known differences

1. `schema_fingerprint` (blake2b über die Deskriptoren) wird nicht berechnet; stattdessen wird die
   geordnete Feldliste (Name, Typ, fast, stored) des geöffneten Index geprüft. Gleiches Ziel.
2. Upstream baut bei Settings-Abweichung/Korruption den Index neu (löscht ihn). Rust löscht nie,
   sondern sucht leer, bis der Worker neu indexiert hat.
3. `str.isdecimal()` für die Ziffern-Verankerung wird mit `char::is_numeric` angenähert (auch
   Unicode-No/Nl). Tokens sind bereits ascii-gefaltet, praktisch identisch.
4. Python `int()` (für `page`, `limit`, `more_like_id`) akzeptiert auch Nicht-ASCII-Ziffern; Rust nur
   ASCII (mit `_`, Vorzeichen, Leerraum).
5. `str.lower()` vs. `str::to_lowercase()` (Autocomplete-Präfix): Sonderfälle der Unicode-Kleinschreibung
   können abweichen.
6. `db_only`-Dokumente: Gleichstand bei `created` ist wie upstream nicht deterministisch.
7. MLT-Panik wird mit `catch_unwind` abgefangen (nur wirksam mit `panic = "unwind"`).
8. Pythons `\w`/`\s` im Feldnamen-Tagger werden mit `is_alphanumeric`/`is_whitespace` angenähert; `int()` für u64 nur ASCII-Ziffern; Phrase-Slop > u32 wird gekappt.
   `\p{Han}` u. a. im CJK-Regex: Rust nutzt für nackte Skriptnamen Script_Extensions – gegen Pythons `regex`-Modul noch zu prüfen.
9. Django-URL `^search/autocomplete/` ist ein Präfix-Regex (auch `/api/search/autocomplete/x`
   trifft); Rust mountet nur den exakten Pfad, der Rest geht per Proxy an Django.
10. `more_like_id`-Rechteprüfung und `permitted_document_ids` laufen über lokales SQL (`src/db.rs`)
   statt über `pl_authz::perms::has_perms_owner_aware` / `pl_authz::objects::permitted_object_ids`
   (gleiche Semantik; Umstellung geplant, sobald die Engine-Funktionen pl-authz nutzen dürfen).
11. Globale Suche, Abschnitte Saved Views/Custom Fields/Mail-Regeln/Mail-Konten: Serializer sind
   in pl-search nachgebaut (Abhängigkeitszyklus bzw. private Serializer) – Änderungen an den
   Serializern in pl-fields-views/pl-mail müssen hier nachgezogen werden (CORE-REQUEST 4).

## CORE-REQUEST

1. `[workspace.dependencies]`: `tantivy = "=0.26.0"` (exakt, Index-Format), `unicode-normalization = "0.1"`;
   dev: `tempfile = "3"` (derzeit im Crate-Cargo.toml).
2. pl-core `AppState`: Platz für `Arc<pl_search::SearchBackend>` (ein Reader je Prozess; z. B.
   `extensions: http::Extensions` oder ein `search: OnceLock<…>`), Konfiguration `DATA_DIR`,
   `SEARCH_LANGUAGE`, `ADVANCED_FUZZY_SEARCH_THRESHOLD`, `TIME_ZONE` (bis dahin
   `SearchSettings::from_env()`).
3. pl-authz: Request-Nutzer mit `id`, `is_superuser`, `is_active`, aktuelle `group_ids`, Modellrechte
   (`has_perm`) → derzeit lokales `pl_search::db::RequestUser`; außerdem
   `permitted_document_ids(user)` und `has_perms_owner_aware(user, "view_document", doc)`
   (derzeit lokales SQL in `src/db.rs`).
4. Globale Suche: gemeinsamer Provider-Trait in pl-core (z. B. `pl_core::global_search::ObjectSearchProvider`),
   den pl-objects, pl-auth-api, pl-mail, pl-workflows, pl-fields-views implementieren (Serializer
   + `restrict_queryset_to_visible`), sonst kann `/api/search/` nicht in Rust laufen.
5. pl-core Antworttyp für Django-Klartext-Antworten (`HttpResponseBadRequest`/`Forbidden`,
   `text/html; charset=utf-8`) – derzeit `views::ViewResponse::Text`.

## Ergänzung 2026-11-18: Neuaufbau beim Start (A2)

Der Worker (pl-server `register_document_handlers`) ruft bei jedem Start im Hintergrund
`pl_indexer::Indexer::rebuild(if_needed = true)` auf – wie upstream `init-search-index`
(`document_index reindex --if-needed`): Neuaufbau, wenn der Index fehlt, der Marker eines
abgebrochenen Neuaufbaus liegt oder Schemaversion/Sprache (`SEARCH_LANGUAGE`) nicht passen. Der Leser
(`SearchBackend`) wartet bis dahin („waiting for reindex“) und öffnet den neuen Index beim nächsten
Versuch (alle 5 s). Unterschied: upstream baut synchron vor dem Start des Webservers; hier läuft der
Worker schon, Suchanfragen liefern bis zum Ende des Neuaufbaus nichts.
