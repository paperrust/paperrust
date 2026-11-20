# Vertrag `pl-templating` (paperless-ngx v3.4.2)

Reine Bibliothek, **keine Routen, keine DB**. Nachbau der Dateinamen-, Speicherpfad- und
Workflow-Vorlagen von upstream; genutzt von `pl-objects` (`storage_paths` create/update/test),
`pl-documents-write` (Umbenennen nach Änderungen), `pl-workflows` (Validierung `assign_title`) und
später dem Rust-Worker (Konsum, Workflows, Mail-/Webhook-Texte).

Quellen (alle unter `upstream/src/documents/`):
`templating/environment.py` (`JinjaEnvironment`, `_template_environment`),
`templating/filters.py` (`localize_date`, `format_datetime`, `get_cf_value`),
`templating/filepath.py` (`FilePathTemplate`, `PlaceholderString`, `MatchingModelContext`,
`create_dummy_document`, `get_*_context`, `is_safe_relative_path`,
`validate_filepath_template_and_render`), `templating/utils.py`
(`convert_format_str_to_template_format`), `templating/workflows.py`
(`_known_placeholder_names`, `validate_workflow_template`, `parse_w_workflow_placeholders`),
`file_handling.py` (alle Funktionen), `views.py::StoragePathViewSet.test`,
`serialisers.py::StoragePathSerializer.validate_path`, `WorkflowActionSerializer.validate`,
`signals/handlers.py::update_filename_and_move_files` (Aufrufer-Logik, nicht nachgebaut),
`workflows/mutations.py`, `workflows/actions.py`, `consumer.py::_parse_title_placeholders` (Aufrufer).
Fremdbibliotheken (laut `reference/downloads/requirements-lock.txt`): Jinja2 3.1.6, pathvalidate 3.3.1,
Babel 2.18.0, Django 5.2 (`slugify`, `dateparse`).

## Öffentliche API (Rust)

```rust
// Einstellungen (django.conf.settings)
pub struct Options { filename_format: Option<String>, filename_format_remove_none: bool, time_zone: chrono_tz::Tz }
Options::from_env()       // PAPERLESS_FILENAME_FORMAT, PAPERLESS_FILENAME_FORMAT_REMOVE_NONE, PAPERLESS_TIME_ZONE

// Kontext: alles, was upstream aus Document/Tags/CustomFieldInstance in die Vorlage legt
pub struct DocumentContext { pk, title, content, page_count, created: NaiveDate, added, modified: DateTime<Utc>,
    archive_serial_number, mime_type, checksum, archive_checksum, filename, archive_filename, original_filename,
    owner: Option<OwnerRef>, correspondent/document_type: Option<NamedRef>, storage_path: Option<StoragePathRef>,
    tags: Vec<TagRef> /* order_by("name") */, custom_fields: Vec<CustomFieldInstance> /* global_objects! */,
    file_type: String /* Document.file_type */, root_document: Option<Box<DocumentContext>>, version_index }
pub struct CustomFieldInstance { field_name, data_type, select_options: Option<Vec<SelectOption>>, value: CustomFieldValue }
pub fn default_file_extension(mime) -> &'static str     // get_default_file_extension (Standard-Parser)

// file_handling.py
pub fn generate_filename(doc, GenerateOptions{counter, archive_filename, use_format}, &Options) -> Result<String, TemplateError>
pub fn generate_unique_filename(doc, archive: bool, &Options, exists: impl FnMut(&str)->bool) -> Result<String, TemplateError>
pub fn format_filename(doc, template, &Options) -> Result<String, TemplateError>      // = render_filename
pub fn validate_path_in_root(path, root) -> Result<(), UnsafeFilePathError>
pub fn create_source_path_directory(path) / delete_empty_directories(dir, root)
pub const MAX_STORED_FILENAME_LENGTH: usize = 1024
// views.py StoragePathViewSet.test
pub fn storage_path_test_result(doc, path, &Options) -> Result<Option<String>, TemplateError>
// templating/filepath.py
pub fn validate_filepath_template_and_render(template, Option<&DocumentContext>, &Options) -> Result<String, TemplateError>
pub fn is_safe_relative_path(&str) -> bool; pub fn clean_filepath(&str) -> String
// templating/utils.py
pub fn convert_format_str_to_template_format(&str) -> String
// templating/workflows.py
pub fn validate_workflow_template(&str) -> Result<(), WorkflowTemplateError>     // .message() = Feldfehlertext
pub fn parse_w_workflow_placeholders(&str, &WorkflowPlaceholders) -> Result<Option<String>, TemplateError>
pub fn parse_w_workflow_placeholders_with_id(&str, &WorkflowPlaceholders, &WorkflowDocId) -> Result<Option<String>, TemplateError>
pub enum WorkflowDocId { Unset, Empty, Id(i64) }   // doc_id: None / "" / pk
pub fn render_workflow_title(&str, &WorkflowPlaceholders) -> Option<String>     // mutations.py-Semantik
pub const KNOWN_PLACEHOLDER_NAMES: &[&str]
// Helfer
pub fn sanitize_filename(value, replacement) -> String   // pathvalidate 3.3.1, Plattform "universal"
pub fn django_slugify(&str) -> String
```

### Abbildung upstream `None` / Ausnahmen

| upstream | Rust |
|---|---|
| `validate_filepath_template_and_render` → `None` | `Err(Undefined/Syntax/Security/Other/UnsafePath)` |
| Ausnahme beim Kontextaufbau (z. B. `StopIteration` bei unbekannter Select-Option, `ValueError` bei `with_suffix` auf leerem Namen) | `Err(TemplateError::Context)` – wird von `generate_filename`/`storage_path_test_result` durchgereicht (upstream: 500/Abbruch) |
| `format_filename` → `None` | `Err(..)` |
| `generate_filename` (Vorlage ungültig → Standardname `{pk:07}`) | `Ok(name)`; `Err` nur bei `Context` |
| `parse_w_workflow_placeholders`: Syntax-/SecurityError → `None`; UndefinedError/sonstige → raise | `Ok(None)` / `Err(Undefined/Other)` |
| `validate_workflow_template`: `ValueError("Template syntax error: …")`, `ValueError("Template assertion error: …")`, `KeyError("Template references unknown placeholders: …")` | `WorkflowTemplateError::{ValueError, KeyError}`; `message()` = `e.args[0]` |
| `StoragePathSerializer.validate_path`: `None` → `ValidationError("Invalid variable detected.")` | Aufrufer (pl-objects): `validate_filepath_template_and_render(convert(path), None, opts).is_err()` |

## Verhalten (wie upstream)

### Umgebung (`environment.py`)
`SandboxedEnvironment(trim_blocks=True, lstrip_blocks=True, keep_trailing_newline=False,
autoescape=False, extensions=["jinja2.ext.loopcontrols"])`, `undefined = StrictUndefined` (mit Logging),
Filter `get_cf_value`, `datetime` (= `format_datetime`), `slugify` (Django), `localize_date`.
`is_safe_callable` sperrt Methoden namens `save`, `delete`, `update`. Eine gemeinsame Umgebung für
Dateinamen und Workflows (upstream registriert alle Filter auf demselben Objekt).

### Dateipfad-Kontext (`validate_filepath_template_and_render`)
Zusammengesetzt in dieser Reihenfolge (spätere Schlüssel überschreiben):
1. `document`: `id`, `pk`, `title` (roh), `content`, `page_count`, `created` (date), `added`, `modified`
   (aware datetime, UTC), `archive_serial_number`, `mime_type`, `checksum`, `archive_checksum`,
   `filename`, `archive_filename`, `original_filename`, `owner` (`{username,id}`|None),
   `tags` (`[{name,id}]`), `correspondent`/`document_type` (MatchingModelContext: str() = Name, `.id`,
   `.name`, `.path`=None), `storage_path` (MatchingModelContext mit `.path`).
2. `title`, `correspondent`, `document_type` (jeweils NFC + `sanitize_filename(…, "-")`), `asn`
   (str, wenn ≠ 0/None), `owner_username` (nicht bereinigt), `original_name` (NFC, ohne letzte Endung),
   `doc_pk` (`{pk:07}`); fehlende Werte → `PlaceholderString("-none-")` (rendert "-none-", ist falsy,
   `== "none"` und `== "-none-"` sind wahr).
3. `created`, `created_year`, `created_year_short`, `created_month`, `created_month_name`,
   `created_month_name_short`, `created_day` (strftime C-Locale, englische Monatsnamen).
4. `added…` wie 3., aus `timezone.localdate(document.added)` (→ `Options.time_zone`).
5. `tag_list` (sortierte Namen mit "," verbunden, bereinigt), `tag_name_list` (Reihenfolge der Tags).
6. `custom_fields`: `{bereinigter Feldname: {"type": data_type, "value": wert}}`; string/url/longtext/
   monetary bereinigt, select → bereinigtes Label der Option (fehlt sie: Ausnahme), date → `date`,
   documentlink → Liste, sonst Rohwert, NULL → None.

Validierung ohne Dokument nutzt `create_dummy_document()` (pk 1, Titel "Sample Title", …,
`created` als datetime `now()`, Tags "Test Tag 1"/"Another Test Tag", Feld "Text Custom Field").

Nach dem Rendern `clean_filepath`: NFC, `\n`/`\r` entfernen, `\s*/\s*` → `/`, `strip()`, führende/
folgende `/` entfernen. Danach `is_safe_relative_path` (kein absoluter Pfad, kein `..`), sonst Fehler.

### `format_filename` / `generate_filename` / `generate_unique_filename`
* Vorlage: `storage_path.path` des (Wurzel-)Dokuments, sonst `PAPERLESS_FILENAME_FORMAT` nach
  `convert_format_str_to_template_format` (`{x}` → `{{ x }}`, `{{…}}` bleibt), sonst keine.
* `FILENAME_FORMAT_REMOVE_NONE`: `"/-none-/"`→`"/"`, `" -none-"`→`""`, `"-none-"`→`""`, `/` abschneiden;
  danach immer `"-none-"`→`"none"`; erneute Pfadprüfung (Schutz gegen `.-none-.` → `..`).
* Ergebnis `<verz>/<name>{_v<version_index>}{_<counter:02>}<endung>`; Endung `.pdf` für Archiv,
  sonst `file_type`; ohne (gültige) Vorlage `{pk:07}…`. Versionen nutzen das Wurzel-Dokument.
* `generate_unique_filename`: für Archiv zuerst `<verz>/<stem(filename)>.pdf`; dann Zähler `_01`, `_02` …
  bis frei oder gleich dem bisherigen Namen. Längenprüfung (`> 1024` → nicht verschieben) macht der
  Aufrufer (`update_filename_and_move_files`) mit `MAX_STORED_FILENAME_LENGTH`.
* `storage_path_test_result`: `format_filename(doc, path)` (ohne `{}`-Umwandlung) + Endung aus
  `document.filename` (Suffix) bzw. `file_type`; `None` → JSON `null`.

### Workflows
`parse_w_workflow_placeholders`: Kontext `correspondent`, `document_type`, `added` (isoformat der lokalen
Zeit), `added_year…added_day`, `added_time` (`%H:%M`), `owner_username`, `original_filename`/`filename`
(Path.stem), `custom_fields`, bei `created`: `created…created_day`, `created_time` (immer `00:00`),
optional `doc_title`, `doc_url`, `doc_id` (als str; `if doc_id is not None`). `doc_id` hat drei
Zustände (`WorkflowDocId`): `Unset` (= `None`, undefiniert → StrictUndefined-Fehler), `Empty` (= `""`,
E-Mail-/Webhook-Kontext beim Einlesen aus `build_workflow_action_context(..., overrides)`), `Id(pk)`.
`parse_w_workflow_placeholders(text, p)` nimmt `p.doc_id: Option<i64>` (Unset/Id),
`parse_w_workflow_placeholders_with_id(text, p, &WorkflowDocId)` erlaubt `Empty`. Keine `{}`-Umwandlung. Plain `Template` (kein
`clean_filepath`). `validate_workflow_template`: Parsen → unbekannte Filter/Tests
(`"No filter named 'foo'.\n  File \"<introspection>\", line 1"`) → undeklarierte Variablen außerhalb von
`_known_placeholder_names` (Jinja2-Globals `range`, `dict`, `lipsum`, `cycler`, `joiner`, `namespace`
zählen nicht).

### Filter
* `datetime(value, format)`: str → `parse_datetime` oder `parse_date` (Django 5.2), sonst
  `ValueError("Invalid date or datetime string: 'x'")`; date/datetime → `strftime` (glibc C-Locale,
  Flags `-`/`_`/`0`/`^`/`#`, unbekannte Direktiven bleiben stehen).
* `localize_date(value, format, locale)`: str → `parse_datetime`; Locale-Prüfung (`Invalid locale
  identifier: x`); datetime → `babel.dates.format_datetime`, date → `format_date`, sonst
  `TypeError("Unsupported type <class '…'> for localize_date")`. CLDR-Daten aus Babel 2.18.0
  (1133 Kennungen inkl. Kurzformen ohne Schrift, `tools/gen_cldr.py`), Muster-Tokenizer und Felder wie
  `babel.dates.DateTimeFormat`, Presets `full/long/medium/short`.
* `get_cf_value(data, name, default=None)` wie upstream.
* `slugify`: Django (`NFKD` → ASCII, `[^\w\s-]` entfernen, `[-\s]+` → `-`, `strip("-_")`).

## Sicherheit
* Keine Dateizugriffe aus Vorlagen; nur Werte aus dem Kontext, keine Modellobjekte.
* `save`/`delete`/`update` und alle Namen mit `_` am Anfang sind gesperrt (SecurityError).
* `range()` höchstens 100 000 Elemente (wie Jinja2 `MAX_RANGE`).
* Zusätzlich (Rust bricht bei Speichermangel ab, Python wirft `MemoryError`): Fuel 20 Mio.
  VM-Schritte, Ausgabe ≤ 16 MiB, erzeugte Zeichenketten ≤ 16 MiB (`*`, `~`, `+`, `replace`, `join`,
  `center`, `format`, …), Listen ≤ 1 Mio. Elemente, Vorlage ≤ 1 MiB, Rekursion ≤ 200. Überschreitung →
  `TemplateError::Security` (upstream-Pfad „SecurityError“: Dateiname fällt auf Standard zurück).
* Die Originalvorlage wird nur geparst, nie kompiliert (minijinja faltet beim Kompilieren Konstanten,
  z. B. `'a' * 10**11`, ohne Grenze); kompiliert wird nur die umgeschriebene Fassung.
* Pfad-Traversal: `is_safe_relative_path` vor und nach REMOVE_NONE; `validate_path_in_root` (resolve
  mit Symlinks des existierenden Präfixes).

## Umsetzung: Jinja2 ↔ minijinja

minijinja 2.19.0 (Sandbox ohne Python-Objekte) mit diesen Angleichungen:

| Thema | Jinja2 | minijinja (roh) | Angleichung |
|---|---|---|---|
| Whitespace | trim/lstrip_blocks, kein Schluss-Newline | gleich einstellbar | gesetzt |
| Autoescape | aus | Callback | `AutoEscape::None` |
| Undefined | `StrictUndefined` | `UndefinedBehavior::Strict` | gesetzt; Formatter prüft selbst |
| Ausgabe `None/True/1.0/[..]` | `None`, `True`, `1.0`, `['a', 1]`, `{'a': 1}` | `none`, `true`, `1.0`, `["a", 1]` | eigener Formatter mit Python-`str()` |
| `==`/`!=` | Python, `PlaceholderString.__eq__` | Wertgleichheit | Quelltext-Umschreibung → `__pl_eq` |
| `~` | `str(a)+str(b)` | Display (`none`) | → `__pl_concat` |
| `+`, `*` | Python (str/list/Zahl) | ähnlich, ohne Grenzen | → `__pl_add`/`__pl_mul` mit Größengrenzen |
| Python-Methoden (`.upper()`, `.split()`, `.items()`, `.get()`, `.strftime()`, `"{}".format()` …) | erlaubt | nicht vorhanden | `unknown_method_callback` (methods.rs) |
| Filter `string, join, lower, upper, capitalize, title, replace, trim, center, truncate, wordcount, int, float, round, length/count, format, filesizeformat, striptags, forceescape` | Python-Semantik | abweichend/fehlend | eigene Implementierung |
| Tests `eq/equalto/==/ne/!=`, `callable` | Python | Wertgleichheit / fehlt | eigene |
| minijinja-Extras (`lines, bool, chain, zip, split, json`, Tests `startingwith, endingwith, safe`, Global `debug`) | gibt es nicht | vorhanden | entfernt (→ Fehler wie Jinja2) |
| Unbekannter Filter/Test | Kompilierfehler `TemplateAssertionError` | Laufzeitfehler | Vorprüfung per Tokenizer mit Jinja2-Namensliste |
| Datum/Zeit-Objekte | `date`/`datetime` mit Attributen/Methoden | – | eigene Objekte (`year`, `month`, …, `isoformat`, `strftime`, `weekday`, `date`) |

Die Umschreibung nutzt den minijinja-Tokenizer (`unstable_machinery`, Version fest gepinnt) und
bestimmt Operanden über die Bindungsstärken von Jinja2 (Vergleich < `+ -` < `~` < `* / // %` < `**`
< Filter/Postfix); Kommentare, `raw` und Text bleiben unberührt.

## Known differences

1. **Fehlertexte von Jinja2**: Syntaxfehler-Details stammen von minijinja (Präfix
   `"Template syntax error: "` gleich, Rest abweichend, z. B. bei `{{created_year}`). Logtexte
   (`Template variable warning: 'x' is undefined` usw.) sind sinngemäß, nicht wörtlich.
2. **Reihenfolge unbekannter Platzhalter**: upstream gibt eine Python-`set`-Reihenfolge aus (zufällig je
   Prozess); Rust sortiert alphabetisch.
3. **Jinja2-Filter/-Globals** (W9, 2026-11-18, `src/extras.rs`): `random`, `urlize` (Policy
   `urlize.rel = noopener`, `extra_schemes`, `trim_url_limit`, `nofollow`, `target`), `wordwrap`
   (Python-textwrap, `break_long_words`, `break_on_hyphens`, `wrapstring`), `xmlattr` (Schlüsselprüfung
   wie 3.1.6) sowie die Globals `lipsum`, `cycler` (`next`, `reset`, `current`) und `joiner` sind
   umgesetzt. Abweichungen: `random` nutzt einen anderen Zufallsgenerator (Ergebnis ohnehin zufällig);
   `lipsum` ist auf 1 Mio. Wörter begrenzt (Sandbox); textwrap trennt an Bindestrichen nach einer
   vereinfachten Regel (Buchstabe/Ziffer `-` Buchstabe) statt der vollen `wordsep_re`.
4. **Python-Teilmengen**: `str.format`/`%`-Formatierung decken die gängigen Formatangaben ab (`s r d i f
   e g x o c %`, Füllung/Ausrichtung/Breite/Präzision/Tausender), dazu `%*d`/`%.*f` (Breite/Genauigkeit aus
   dem Argument), Feldzugriffe `{0.attr}`/`{0[key]}` (wie `SandboxedFormatter`: `_`-Attribute gesperrt) und
   verschachtelte Angaben `{:>{w}}` (eine Ebene) (W9, `src/filters.rs` `percent_format`, `src/methods.rs`
   `resolve_field`). `str % args` als Operator ist umgesetzt (Folge = Tupel, Mapping = `%(name)s`,
   Zahlen-Modulo mit Vorzeichen des Divisors); eine Liste rechts wird wie ein Tupel behandelt (Python: als
   ein Wert). **Veränderbare Listen** (W9, `src/pylist.rs`): ruft die Vorlage `append`, `extend`, `insert`,
   `pop`, `remove`, `clear`, `reverse` oder `sort` auf, werden ihre Listen-Literale zu veränderbaren Listen
   (`__pl_list([...])`); ohne solchen Aufruf bleibt alles unverändert. Abweichungen: Listen aus dem
   Kontext (z. B. `document.tags`) und Ergebnisse von `+`/Filtern bleiben unveränderlich; eine Liste, die
   sich selbst enthält, ist ein Fehler (Python: `[[...]]`); `sort(key=...)` und gemischte Typen sind Fehler;
   Dict-Mutation (`setdefault`, `pop`) fehlt. Tupel-Literale rendern als Liste
   (`[1, 2]` statt `(1, 2)`); `dictsort`/`items` liefern Listen statt Tupel.
5. **Vergleichsketten** `a < b == c`: Jinja2 wertet als `a < b and b == c`, hier `a < (b == c)`.
   `in`/`not in` nutzen minijinja-Gleichheit (`PlaceholderString in ['none']` ist hier falsch).
6. **localize_date**: Zeitzonennamen (`z`, `v`, `V`) nur für UTC wie Babel ("UTC"), sonst GMT-Format;
   Tagesperioden `b`/`B` nicht umgesetzt (Fehler). Babel-Likely-Subtag-Auflösung nur für die in den
   Daten vorhandenen Kennungen und Kurzformen ohne Schrift (z. B. `sr_RS`); Aliase wie `iw`/`zh_TW`
   ohne Datensatz → `Invalid locale identifier`. Rundung von `S` per `round_ties_even` statt Python-Dezimalrundung.
7. **strftime**: glibc-Verhalten für Jahre < 1000 (`%Y` ohne Auffüllen, wie Python 3.12); Python ≥ 3.14
   füllt auf 4 Stellen.
8. **Sandbox-Grenzen** (Fuel, 16 MiB, 1 Mio. Elemente, 1 MiB Vorlage, Rekursion 200) gibt es upstream
   nicht; dort führen solche Vorlagen zu langer Laufzeit/`MemoryError`. Bewusst strenger.
9. **`generate_unique_filename`**: höchstens `MAX_UNIQUE_ATTEMPTS` (1 Mio.) Zählerversuche (upstream
   unbegrenzt).
10. **Dateisystem/DB** (Signal-Handler `update_filename_and_move_files`, Verschieben, Papierkorb,
    `CustomFieldInstance.global_objects`, Tag-Sortierung per DB-Collation) bleiben bei den Aufrufern;
    `generate_unique_filename` prüft Existenz über eine Closure.
11. `Options.time_zone`: `timezone.localdate` mit `chrono-tz` statt `zoneinfo` (gleiche IANA-Daten,
    Versionsstand kann abweichen).

12. **Sicherheit (Review H1, wave3):** `is_safe_relative_path` lehnt zusätzlich jeden Backslash und NUL
    ab (upstream unter Linux: `\` ist ein normales Zeichen). Grund: unter Windows ist `\` ein Trenner
    (`..\archive\x` verließe die Wurzel), und pl-documents-write liest gespeicherte Dateinamen mit `\`
    als Trenner. Folge: ein Vorlagenergebnis mit `\` (auch über Titel/Feldwerte) gilt als unsicher →
    Standardname (`format_filename`) bzw. Speicherpfad-Validierung schlägt fehl.

13. **Sicherheitsprüfung 2026-11-18 (Fuzzing, results/release/security/BERICHT.md):**
  (a) `strftime` (Filter `datetime`/`localize_date` u. a.): Breite über 256 × Länge des Formats → `""`
  wie CPython `time.strftime` (Puffergrenze); vorher Überlauf bzw. ~1 GB Speicher bei `%999999999d` (S-8).
  (b) Gerenderter Pfad über 32 KiB (`MAX_RENDERED_PATH_BYTES`) → `TemplateError::Security("rendered path
  too long")`; upstream hat keine Grenze (S-8b). (c) Panik in minijinja beim Rendern →
  `TemplateError::Other("template rendering failed")` statt Abbruch (S-9).

14. **Windows: Pfadteile bereinigt** (Sicherheitsprüfung 2026-11-19 F12): unter Windows bereinigt
  `format_filename` nach der letzten Sicherheitsprüfung jeden Teil des Ergebnisses
  (`filepath::windows_safe_parts`): `:` (Laufwerk/Strom `name:strom`) → `-`, Gerätename → `_` hinter den
  Namen (`CON_`, `nul_.tar`, `lpt1_ .txt`), Ordnerteile ohne Punkte/Leerzeichen am Ende (leer → `-`).
  Der letzte Teil behält Punkte am Ende (die Endung folgt). Betrifft nur Literaltext der Vorlage (Werte
  bereinigt upstream schon mit pathvalidate). Kein Fehler, damit Speicherpfade mit z. B. `a:b` unter
  Windows weiter funktionieren; `..` bleibt ein Fehler. Andere verbotene Zeichen (`<>"|?*`) bleiben wie
  upstream. `validate_filepath_template_and_render` selbst und Linux: unverändert wie upstream.

## Tests

`cargo test -p pl-templating` (Unit-Tests ohne DB, Stand: 93 grün, 0 ignoriert; clippy `-D warnings` sauber): siehe `src/tests.rs` (je upstream-Test eine
Funktion) und Modultests (`pathvalidate`, `localize`, `strftime`, `pystr`, `methods`, `convert`,
`env`, `filepath`, `file_handling`). Erwartete Werte für pathvalidate/Jinja2/Babel wurden zusätzlich
mit den Original-Bibliotheken (Wheels aus `reference/downloads/wheels`) gegengeprüft.

Nicht portiert (brauchen DB/Dateisystem/Signale; Aufgabe der Aufrufer): `test_file_renaming_missing_permissions`,
`test_file_renaming_database_error`, `test_root_storage_path_change_updates_version_files`,
`test_stale_save_recovers_already_moved_files`, `test_serializer_stale_update_does_not_clobber_filename`,
`test_document_delete*`, `test_directory_not_empty`, `test_no_move_only_save`, `TestFileHandlingWithArchive.test_move_*`,
`test_archive_deleted*`, `test_database_error`, `test_custom_field_not_in_template_skips_filename_work`,
`test_refresh_failure_surfaces_database_error`. Vom Rest wird jeweils der Namensteil geprüft.

## CORE-REQUEST

`[workspace.dependencies]` ergänzen (bis dahin im Crate fest gepinnt):
* `minijinja = { version = "=2.19.0", default-features = false, features = ["builtins", "macros",
  "multi_template", "adjacent_loop_items", "std_collections", "serde", "loop_controls", "fuel",
  "preserve_order", "json", "urlencode", "unstable_machinery"] }` – Pin wegen `unstable_machinery`.
* `unicode-normalization = "0.1.26"` (NFC/NFKD; schon im Lockfile).
