# Vertrag: pl-matching

Reine Bibliothek (keine Routen, keine DB). Grundlage für den späteren Python-freien Rust-Worker
und für Crates, die Zuordnung/Workflows/Datumserkennung brauchen. Quelle: paperless-ngx v3.4.2
(`upstream/`), dateparser 1.4.3, rapidfuzz 3.14.6, CPython 3.12 (`reference/venv`).

## Module

| Modul | Upstream-Quelle | Inhalt |
|---|---|---|
| `matching` | `documents/matching.py` (`matches`, `_split_match`, `log_reason`, `match_correspondents/_document_types/_tags/_storage_paths`), `documents/models.py` (`MatchingModel`, `Document.suggestion_content`) | `matches(&Matchable, &MatchDocument) -> Result<bool, UnsupportedMatchingAlgorithm>`, `split_match`, Konstanten `MATCH_NONE..MATCH_AUTO` (0..6), Trait `Classifier` (MATCH_AUTO, Umsetzung später), `match_*` über vom Aufrufer gelieferte (rechtegefilterte) Kandidaten |
| `workflow` | `documents/matching.py` (`consumable_document_matches_workflow`, `existing_document_matches_workflow`, `prefilter_documents_by_workflowtrigger`, `document_matches_workflow`), `documents/data_models.py` (`DocumentSource`) | eigene Typen `WorkflowTrigger`, `ConsumableDocument`, `ExistingDocument`, `ObjRef`; Trait `CustomFieldQueryEvaluator` (DSL gehört pl-docfilter); `prefilter_matches` (In-Memory-Prädikat mit Django-NULL-Semantik) + `filename_prefilter_regex` (Regex-Text für `original_filename__iregex`) |
| `fnmatch` | CPython 3.12 `Lib/fnmatch.py` | `translate` (zeichengleich, inkl. `(?>.*?…)`), `fnmatch` (POSIX: `normcase` = no-op), `iregex_search` |
| `fuzz` | rapidfuzz `fuzz_py.py` (`partial_ratio_alignment`, `_partial_ratio_impl`), `distance/Indel_py.py` | `partial_ratio(s1, s2, score_cutoff)` auf Codepoints, Float-Arithmetik Schritt für Schritt wie Python |
| `regex_compat` | `documents/regex.py` (`validate_regex_pattern`, `safe_regex_search/_match/_sub`) | Python-`regex`-Syntax → fancy-regex, Fehler/Timeout → `None` + Log |
| `pyre` | CPython `re` | `\w` = `[\p{L}\p{N}_]`, `\s` = White_Space + U+001C..1F, `\b`, `re.escape`, `findall`, `textwrap.shorten` |
| `dates` | `documents/plugins/date_parsing/{__init__,base,regex_parser}.py`, `paperless/settings/custom.py` (`parse_dateparser_languages`, `parse_ignore_dates`), `paperless/utils.py` (`ocr_to_dateparser_languages`) | `DateParserConfig`, `RegexDateParser` (`parse` = Iterator wie der Python-Generator, `parse_string`, `filter_date`), `date_regex_matches`, Einstellungsparser |
| `dates::dp*` | dateparser 1.4.3: `__init__.parse`, `date.py`, `languages/{loader,locale,dictionary}.py`, `parser.py`, `freshness_date_parser.py`, `timezone_parser.py`, `utils/*` | Port der tatsächlich genutzten Teilmenge von dateparser (siehe unten) |

## matches()

Reihenfolge exakt wie upstream: leerer/Whitespace-`match` → `false`; dann je Algorithmus:

- **ANY/ALL**: `_split_match` (Anführungszeichen-Gruppen `"…"`, Whitespace normalisiert, Teile
  `re.escape`, innere Leerzeichen → `\s+`), Suche `\b{wort}\b` mit Python-`re`-Wortgrenze.
  Implementierung: regex-Crate für den Innenteil, Wortgrenzen manuell geprüft, *jede* Startposition
  versucht (exakt, da Innenteil je Start genau ein Ende hat).
- **LITERAL**: `\b{re.escape(match)}\b`.
- **REGEX**: `safe_regex_search` (Python-`regex`-Modul, Flags nur IGNORECASE).
- **FUZZY**: `re.sub(r"[^\w\s]", "", …)` auf Muster und Inhalt, bei `is_insensitive` `lower()`,
  Treffer wenn `partial_ratio(match, text, score_cutoff=90) != 0`.
- **AUTO** → `false` (Klassifikator separat), **NONE** → `false`, unbekannt → `Err` (upstream
  `NotImplementedError`).
- Log `paperless.matching` DEBUG: `"{Klasse} {name} matched on document {doc} because {grund}"` (Texte 1:1).

## Workflow-Trigger

- `consumable_document_matches_workflow`: Quelle, Mailregel, Dateiname (`fnmatch`, beides
  `lower()`), Pfad (`original_path` sonst `original_file`, case-sensitiv). Alle Bedingungen werden
  geprüft, der *letzte* Fehlgrund gewinnt (wie upstream). Gründe zeichengleich, z. B.
  `Document source ApiUpload not in ['ConsumeFolder', 'MailFetch']`.
- `existing_document_matches_workflow`: Inhalt (`matching_algorithm > 0`), Tags any/all/none,
  Korrespondent/Typ/Speicherpfad (any, exakt, none), `filter_custom_field_query`
  (Evaluator-Trait; Fehler → `"Invalid custom field query configuration"`), Dateiname. Erster
  Fehlgrund wird zurückgegeben; Listen als Python-`repr` (`[<Tag: name>]`), Reihenfolge = vom
  Aufrufer gelieferte Reihenfolge (soll Modell-Ordering sein).
- `prefilter_matches`: Semantik des Querysets: `exclude(fk__in=…)` behält NULL; Dateiname als
  unverankerte, case-insensitive Suche von `fnmatch.translate(...)` (upstream `__iregex`),
  `original_filename` NULL → raus; ungültige Custom-Field-Query → kein Dokument.
- `document_matches_workflow`: Trigger nach Typ filtern, erster Treffer gewinnt; Logs
  `Document matched WorkflowTrigger {id} from Workflow: {name}` / `Document did not match Workflow: {name}`.

## Datumserkennung (RegexDateParser)

1. `DATE_REGEX` (7 Alternativen, IGNORECASE). Umgesetzt mit dem regex-Crate: die führende Gruppe
   `(\b|(?!=([_-])))` ist immer leer erfüllbar (entfällt), die abschließende
   `(\b|(?=([_-])))` wird als konsumiertes `(?:[\W_]|\z)` nach der Capture-Gruppe ausgedrückt;
   Prioritäten (leftmost-first) bleiben gleich → identische Treffer (Parität geprüft).
2. Je Treffer `dateparser.parse(s, settings={DATE_ORDER, PREFER_DAY_OF_MONTH: first,
   RETURN_AS_TIMEZONE_AWARE: True, TIMEZONE}, locales=languages)`; Fehler → Log
   `Error while parsing date string '…': …` (Logger `documents.plugins.date_parsing.base`) und `None`.
3. `_filter_date`: `year > 1900`, `date <= reference_time` (Zeitpunktvergleich), `date.date()` nicht in IGNORE_DATES.
4. Reihenfolge: zuerst Dateiname (nur wenn `FILENAME_DATE_ORDER` gesetzt), dann Inhalt, jeweils Textreihenfolge.
   Consumer nimmt `next(...)`, `suggestions` nimmt `NUMBER_OF_SUGGESTED_DATES` erste, dedupliziert, sortiert.

Einstellungen: `languages = PAPERLESS_DATE_PARSER_LANGUAGES or ocr_to_dateparser_languages(OCR_LANGUAGE)`
(`resolve_languages`), `parse_dateparser_languages` (zh-Fallback, Validierung, sortiert nach
dateparser-Sprachreihenfolge), `parse_ignore_dates` (dateparser-Defaults: alle Sprachen, TIMEZONE local,
Tag „current“).

### Genutzte Teilmenge von dateparser (nachgebaut)

- Ablauf `parse` → `sanitize_date` → anwendbare Locales (gegebene Locales nach `language_order`
  sortiert; leer = alle Basissprachen) → je Locale `is_applicable` (Original- und tz-bereinigter
  String) → Parser `relative-time` (Freshness) und `absolute-time` (`_parser`); `timestamp` entfällt
  (DATE_REGEX liefert nie 10 Ziffern am Anfang), `custom-formats` ungenutzt, `no-spaces-time` nicht
  in den Default-PARSERS.
- Locale: Ziffern-Übersetzung (alle Unicode-Nd), NFKD + Mn entfernen (NORMALIZE), `simplifications`,
  Russisch-Sonderfall, Split an relativen Ausdrücken und bekannten Wörtern, `are_tokens_valid`,
  Übersetzung inkl. `relative-type-regex`, `_clear_future_words`, Leerzeichenlogik, `_join`.
- `_parser`: Tokenizer, Zeiterkennung (`13.20`, Mikrosekunden, am/pm), Zahlen-Direktiven nach
  DATE_ORDER (`%d`, `%m`, `%y`/`%Y`, inkl. „vorheriges Token neu parsen“/`unset_tokens`,
  `year_position_already_passed`), Monats-/Wochentagsnamen (englisch, nach Übersetzung),
  Fehlerbehandlung „day is out of range“, Wochentagskorrektur, `_correct_for_month/day`
  (PREFER_MONTH_OF_YEAR current, PREFER_DAY_OF_MONTH first/current/last).
- Zweistellige Jahre: CPython `%y` (00–68 → 20xx, 69–99 → 19xx); PREFER_DATES_FROM = current_period
  → keine weitere Verschiebung.
- Zeitzonen: Tabelle `_tz_offsets` (773 Einträge), `pop_tz_offset_from_string`, `localize` wie
  pytz `is_dst=False`, `apply_timezone` (Abkürzungen zuerst, dann tz-Datenbank). Gesucht wird erst mit
  einem Vorfilter nur aus den Zonennamen (`dp_tz::tz_name_pattern`), dann je Kandidat in
  Tabellenreihenfolge mit dem vollen Muster – gleiches Ergebnis wie „erstes passendes volles Muster“
  (Test `name_prefilter_equals_full_patterns`), aber 1,3 statt 47 MiB und deutlich schneller
  (Grundspeicher, docs/contract/pl-server.md).
- Monatsnamen/Abkürzungen/Skip-Wörter/relative Ausdrücke **aller** dateparser-Locales (505 Locales,
  228 Datensätze) liegen als `crates/pl-matching/data/dateparser.json` vor, erzeugt von
  `crates/pl-matching/tools/gen_dateparser_data.py` direkt aus den Laufzeitstrukturen von dateparser
  (keine Handpflege). Kompilierung je Locale lazy.

## Tests

- Unit-Tests in den Modulen (u. a. alle 228 Locale-Datensätze kompilieren).
- `tests/matchables.rs`: alle Fälle aus `documents/tests/test_matchables.py` (+ Klassifikator-/AUTO-Fälle).
- `tests/workflows.rs`: DB-freie Teile aus `documents/tests/test_workflows.py` (Consumption-Filter,
  Document-Added-Filter mit Gründen, any/all/none-Filter, Custom-Field-Query, Prefilter).
- `tests/date_parsing.rs`: `documents/tests/date_parsing/test_date_parsing.py` (ohne Mocks, echte
  Werte gleich), `TestParseIgnoreDates`, `test_parse_dateparser_languages`, `test_ocr_to_dateparser_languages`.
- `tests/parity.rs` gegen `tests/parity/pl-matching/dates_reference.json` (erzeugt von
  `tests/parity/pl-matching/dates.py` mit `reference/venv`, unverändertem upstream-Code und
  eingefrorener Uhr): 304 `parse`-Fälle (8 Konfigurationen, 26 Texte in en/de/fr/es/it/pt/nl/ru/cs/pl/tr/sv,
  4 Dateinamen), 528 `_parse_string`-Fälle, DATE_REGEX-Trefferlisten, 10 rapidfuzz-, 77 fnmatch-Fälle.
  **Ergebnis: 0 Abweichungen.**

Neu erzeugen: `reference/venv/Scripts/python.exe -X utf8 tests/parity/pl-matching/dates.py`, dann
`cargo test -p pl-matching -j 1 --test parity`.

## Known differences

1. **Regex-Timeout** (MATCH_REGEX, DATE_REGEX): upstream bricht nach `MATCH_REGEX_TIMEOUT_SECONDS`
   (Standard 0,1 s) Wanduhrzeit ab. Rust (K1, 2026-11-18): Nutzer-Muster (`safe_regex_search/_match`)
   mit Backtracking-Konstrukten (Lookaround, Backrefs, atomare Gruppen) laufen in einem Hilfsthread;
   nach `PAPERRUST_/PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS` wartet der Aufrufer nicht weiter („timed out“-
   Log, kein Treffer). Der Hilfsthread endet spätestens am Backtrack-Budget (≈ 50 Mio. Schritte je
   Sekunde Zeitlimit, mindestens 1 Mio.). Wert ≤ 0 → nur das Budget. Muster ohne solche Konstrukte
   laufen im linearen regex-Crate ohne Zeitlimit und brechen nie ab (`(a+)+$` → kein Treffer, ohne
   „timed out“-Log). `safe_regex_sub`, `fnmatch` und die Datums-Muster nutzen weiter nur das Budget.
   Bei `safe_regex_finditer` zählt upstream auch die Zeit, die dateparser pro Treffer braucht – bei
   langsamem Parsen (kalte Caches, viele Sprachen) werden **upstream** spätere Daten
   nichtdeterministisch abgeschnitten (im Paritätslauf 3 von 304 Fällen). Rust liefert immer alle;
   verglichen wird gegen `dates_full`.
2. **Regex-Dialekt** für Nutzer-Muster: Python-`regex`-Syntax wird übersetzt (`\Z`, `$` vor
   abschließendem `\n`, `{,n}`, literales `{`, `[`/`&`/`~` in Klassen). Zusätzlich (K1, `src/pyext.rs`):
   **unscharfe Treffer** `(?:text){e<=k}` (auch `s/i/d`, `<`, Kombinationen; k ≤ 3) auf reinem Text als
   Alternation aller Varianten (ob ein Treffer existiert, ist gleich; bei mehreren Möglichkeiten kann ein
   anderer Ausschnitt geliefert werden, wie ohne `BESTMATCH` der linkeste); `(?b)`/`(?e)` werden entfernt;
   **Rekursion** `(?R)`, `(?0)`, `(?N)`, `(?&name)`, `(?P>name)` bis Tiefe 10 ausgerollt (tiefer
   verschachtelter Text trifft nicht; bei großen Mustern geringere Tiefe); **Lookbehinds mit Alternativen**
   unterschiedlicher fester Länge (`(?<=Nr\.|Nummer)`). Bewusst weiter ungültig (→ kein Treffer +
   Fehler-Log): Fuzzy auf Klassen/Quantoren (`(?:\d+){e<=1}`), Fehlerkosten (`{1i+1d<5}`), Untergrenzen,
   Lookbehinds mit `+`/`*`/`{m,n}`, relative Rekursion `(?-1)`, `(?r)`, `\p{…}`-Varianten des
   regex-Moduls. Grund: diese bräuchten eine eigene Backtracking-Engine (Nachbau des regex-Moduls),
   der Nutzen für Zuordnungsregeln ist gering.
   Fehlertexte von `validate_regex_pattern` sind die von fancy-regex, nicht die des Python-Moduls.
3. **`re`-Zeichenklassen**: `\w` für ANY/ALL/LITERAL/FUZZY = `[\p{L}\p{N}_]` (Unicode-Version des
   Rust-Crates statt Unicode 15.0 von CPython 3.12 → Abweichung nur bei neu kodierten Zeichen).
4. **„now“ in dateparser**: upstream nutzt intern die Wanduhr (`datetime.now()` für fehlende
   Tage/Monate, relative Wörter wie „gestern“, Wochentage); Rust nutzt `reference_time` des Configs
   (bei upstream `timezone.now()` zum Erzeugen des Parsers, praktisch identisch, aber deterministisch).
5. **Lookbehinds in dateparser-Daten** (ru, cs): `(?<=(через|спустя|в течение)\s+)` u. ä. sind in
   fancy-regex nur mit fester Länge möglich und werden als `\s` (ein Leerzeichen) umgeschrieben;
   exakt, weil der String vorher durch `sanitize_spaces` auf Einzelleerzeichen normalisiert ist.
6. **`re.split` mit Leertreffern**: nach einem leeren Treffer an Position p wird ab p+1 gesucht
   (CPython sucht erst noch einen nicht-leeren Treffer an p); betrifft nur Locales ohne relative
   Ausdrücke, deren Split-Regex dort nur leer treffen kann → gleiches Ergebnis.
7. **relativedelta**: Bruchteile von Tagen/Stunden werden als eine Gesamtdauer (µs, Banker's
   Rounding) addiert statt über `relativedelta._fix`; Jahr/Monat mit Bruchteil → kein Datum (upstream ValueError → None).
8. **pytz `localize(is_dst=False)`** in nicht existierenden lokalen Zeiten (DST-Lücke um Mitternacht,
   z. B. America/Havana): Offset vor der Umstellung; bei ungewöhnlichen Basisoffset-Wechseln evtl. abweichend.
9. **Plugin-Discovery** (`paperless_ngx.date_parsers` Entry-Points) nicht portiert: immer RegexDateParser
   (= upstream-Default ohne installierte Plugins).
10. **Fehlerarten in dateparser**, die keine `ValueError` sind (z. B. IndexError bei leerem String nach
    tz-Entfernung), lässt upstream bis `_parse_string` durchlaufen (Log ERROR); Rust liefert still `None`.
11. **fnmatch** nur POSIX-Semantik (paperless läuft unter Linux); unter Windows würde CPython `normcase`
    (Kleinschreibung, `/`→`\`) anwenden.
12. **Performance-Hinweis** (kein Verhaltensunterschied): ohne Sprachkonfiguration (alle Sprachen) werden
    bei Wörtern, die keine frühe Locale kennt, alle Locale-Regexe kompiliert (Debug-Build ~90 s, Release
    deutlich schneller, RAM-Bedarf einige 100 MB möglich). Mit konfigurierten Sprachen nur diese.

## Für Aufrufer

- Rechte (`permitted_object_ids`) und `get_effective_content()` (neueste Version) löst der Aufrufer auf.
- `RegexDateParser::parse` ist lazy (wie der Generator); `parse_all` sammelt alles.
- Keine Routen → nichts für `routes.toml`.
