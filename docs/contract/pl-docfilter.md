# Vertrag `pl-docfilter`

Filter und Sortierung der Dokumentliste `GET /api/documents/` (ohne Tantivy-Suche).
**Keine eigenen Routen** – Bibliothek für `pl-documents` (und ggf. `pl-search` für die ORM-Schnittmenge).

> **L1 (2026-11-10):** Neben dem bisherigen Weg (`apply`/`apply_ordering` setzen SQL direkt) ist
> `pl-docfilter` jetzt auch **Parser** in den typisierten Filter-AST `pl_domain::filter` (`ast::parse`);
> SQL baut `pl_store::docsql`. Umschalter `PAPERRUST_QUERY_ENGINE`. Design, Abbildungstabelle, *(seit Datenmodell-Stufe 7 entfernt: es gibt nur noch den AST-Weg, docs/DATAMODEL-V2.md §6)*
> erhaltene Django-Eigenheiten, Differenztest und Messung: Abschnitt **„L1“** am Ende.

Upstream (v3.4.2):

| Teil | Quelle |
|---|---|
| Filter | `src/documents/filters.py` → `DocumentFilterSet`, `ObjectFilter`, `InboxFilter`, `TitleContentFilter`, `EffectiveContentFilter`, `SharedByUser`, `CustomFieldsFilter`, `MimeTypeFilter`, `CustomFieldQueryFilter`, `CustomFieldQueryParser`, `DateQueryField`, `MonetaryAmountField`, `SelectField` |
| Sortierung | `src/documents/filters.py` → `DocumentsOrderingFilter`; DRF `OrderingFilter` |
| Suche `?search=` | DRF `SearchFilter` mit `DocumentViewSet.search_fields = ("title", "correspondent__name", "effective_content")` |
| Reihenfolge der Backends | `DocumentViewSet.filter_backends = (DjangoFilterBackend, SearchFilter, DocumentsOrderingFilter, PermittedObjectsFilter)` |
| Basis-Queryset | `DocumentViewSet.get_queryset`: `Document.objects.filter(root_document__isnull=True).order_by("-created", "-id")`, `num_notes` |
| Rechte in Filtern | `src/documents/permissions.py` → `permitted_document_ids` (über `pl_authz::objects`) |

## Schnittstelle

```rust
// crates/pl-docfilter/src/lib.rs
pub async fn apply<D: FilterDb>(q: &mut SelectStatement, params: &QueryParams, user: &User,
                                ctx: &FilterContext, db: &D) -> Result<(), FilterError>;
pub async fn apply_ordering<D: FilterDb>(q: &mut SelectStatement, ordering: Option<&str>,
                                         db: &D) -> Result<(), FilterError>;

pub use pl_core::QueryParams;            // QueryDict-Semantik (get = letzter Wert)
pub use pl_authz::User;
pub struct FilterContext { tz: chrono_tz::Tz, now: DateTime<Utc>, date_parser_languages: Option<Vec<String>> }
pub trait FilterDb { … }                 // implementiert für sqlx::PgPool; in Tests als Mock
pub enum FilterError { Validation(Value) /*400*/, Internal(String) /*500*/, Db(sqlx::Error) }
impl From<FilterError> for pl_core::ApiError   // `?` in Handlern
impl IntoResponse for FilterError
```

Abweichung von der Skizze in `ARCHITECTURE.md`: beide Funktionen sind `async` und erhalten zusätzlich
`ctx` (Zeitzone, Referenzzeit, `PAPERLESS_DATE_PARSER_LANGUAGES`) und `db`, weil upstream beim Filtern
Custom-Field-Definitionen und Dokumentlink-Rückverweise aus der DB liest. `FilterContext::new(tz)` setzt
`now = Utc::now()` und `date_parser_languages = None`.

### Verwendung durch `pl-documents`

```text
SELECT … FROM "documents_document"                       -- ohne Alias! (pl_docfilter::DOC)
WHERE root_document_id IS NULL AND deleted_at IS NULL    -- Basis (Document.objects + root filter)
pl_docfilter::apply(&mut q, &params, &user, &ctx, &pool).await?;            // 1. DjangoFilterBackend, 2. SearchFilter
pl_docfilter::apply_ordering(&mut q, params.get("ordering"), &pool).await?; // 3. DocumentsOrderingFilter
-- 4. PermittedObjectsFilter (pl_authz::objects::permitted_objects_filter) – Sichtbarkeit
-- 5. Paginierung (Count über dieselbe WHERE-Klausel ohne ORDER BY)
```

* `apply` hängt nur `AND`-verknüpfte `WHERE`-Bedingungen an. Keine JOINs, kein `DISTINCT`, keine
  `GROUP BY` – mehrwertige Relationen sind korrelierte `EXISTS`-Unterabfragen. Damit bleibt `COUNT(*)`
  korrekt und die Abfrage bleibt index-freundlich.
* `apply_ordering` erwartet eine Abfrage **ohne** `ORDER BY` und setzt die komplette Sortierung
  (bei fehlender/ungültiger Angabe den Standard `created DESC, id DESC`).
* Fehlerreihenfolge wie upstream: Formularfehler aller Filter → Laufzeitfehler einzelner Filter (in
  Filterreihenfolge) → Suche → Sortierung. Danach erst Sichtbarkeit/Paginierung.
* Nur für `?query=`/`?more_like_id=` (Tantivy) gilt die Sortierung über `pl-search`; die ORM-Filter
  (`apply`) gelten dort ebenfalls.

## Fehlerformat

| Fall | Status | Körper |
|---|---|---|
| Formularfehler (django-filter, `DjangoFilterBackend.raise_exception`) | 400 | `{"<param>": ["<msg>"], …}` – **alle** fehlerhaften Felder, in Formularreihenfolge |
| `custom_field_query` ungültig | 400 | `{"custom_field_query": …verschachtelt…}` (siehe DSL) |
| `?search=` mit NUL | 400 | `["Null characters are not allowed."]` |
| `ordering=custom_field_<id>` unbekannt | 400 | `{"custom_field_<str(int(id))>": ["Custom field not found"]}` |
| unbehandelte Ausnahme in upstream (`ValueError`, `FieldError`, `AttributeError` …) | 500 | Djangos `Server Error (500)` (`FilterError::Internal`) |

Formular-Fehlertexte (Django/django-filter, Englisch):

| Feldtyp | Text |
|---|---|
| NumberFilter (`forms.DecimalField`) | `Enter a number.` / `Ensure this value is less than or equal to 1e+50.` |
| DateFilter (`forms.DateField`, Eingabeformate Locale `en`) | `Enter a valid date.` |
| IsoDateTimeFilter | `Enter a valid date/time.` / `<YYYY-MM-DD HH:MM:SS> couldn’t be interpreted in time zone <tz>; it may be ambiguous or it may not exist.` |
| CharFilter (`forms.CharField`, `strip=True`) | `Null characters are not allowed.` |

## Filterparameter (Formularreihenfolge)

Leere Werte (`""`, bei Zeichenfiltern auch nur Leerraum) → Filter wird nicht angewendet. Unbekannte
Parameter werden ignoriert. Bei mehrfachem Parameter gilt der **letzte** Wert.

### Aus `Meta.fields`

| Parameter | Typ / Validierung | SQL-Semantik |
|---|---|---|
| `id__in`, `id` | NumberFilter (CSV bei `__in`) | `id IN (…)` / `id = v` |
| `title__{istartswith,iendswith,icontains,iexact}` | CharFilter | `UPPER(title::text) LIKE UPPER('…')` (`iexact`: `=`), `\ % _` escaped |
| `archive_serial_number`, `__gt/__gte/__lt/__lte` | NumberFilter | Vergleich; Wert `int(Decimal)` (abgeschnitten) |
| `archive_serial_number__isnull` | BooleanFilter | `IS [NOT] NULL` |
| `created__year/__month/__day` | NumberFilter | `year`: `BETWEEN 'Y-01-01' AND 'Y-12-31'` (Jahr ∉ 1..9999 → **400** `{"created__year": ["Ensure this value is between 1 and 9999."]}`, upstream 500; ebenso `added__year`/`modified__year`); sonst `EXTRACT(MONTH/DAY FROM created) = v` |
| `created__gt/__gte/__lt/__lte` | DateFilter | `created > 'YYYY-MM-DD'` |
| `added__…`, `modified__…`: `year`, `month`, `day` | NumberFilter | `year`: `[lokaler Jahresanfang, nächster)`; `EXTRACT(MONTH FROM col AT TIME ZONE tz)` |
| `added__date__gt/gte/lt/lte`, `modified__date__…` | DateFilter | lokales Datum in `PAPERLESS_TIME_ZONE`, als Zeitpunktvergleich (s. u.) |
| `added__gt/gte/lt/lte`, `modified__…` | IsoDateTimeFilter | `col > timestamptz`; naive Zeiten in `PAPERLESS_TIME_ZONE` |
| `original_filename__…`, `checksum__…` | CharFilter | wie `title` |
| `correspondent__isnull`, `document_type__isnull`, `storage_path__isnull`, `owner__isnull` | BooleanFilter | `<fk>_id IS [NOT] NULL` |
| `correspondent__id__in`, `correspondent__id` (analog `document_type`, `storage_path`, `owner`) | NumberFilter | `<fk>_id IN (…)` / `= v` |
| `correspondent__name__…` (analog `document_type`, `storage_path`) | CharFilter | `<fk>_id IN (SELECT id FROM <tabelle> WHERE UPPER(name::text) LIKE …)` |
| `tags__id__in` | ObjectFilter (deklariert) | `EXISTS(document_tags WHERE tag_id IN (…))` |
| `tags__id` | NumberFilter | `EXISTS(document_tags WHERE tag_id = v)` |
| `tags__name__…` | CharFilter | `EXISTS(document_tags WHERE tag_id IN (SELECT id FROM documents_tag WHERE …))` |
| `custom_fields__icontains` | CustomFieldsFilter (deklariert) | s. u. |

BooleanFilter-Werte (`BooleanWidget`): `true`/`1` → wahr, `false`/`0` → falsch (Groß-/Kleinschreibung egal),
alles andere → Filter ignoriert.

NumberFilter: Python-`Decimal`-Syntax (Leerraum, `1e3`, `.5`, `1_000`), `NaN`/`Inf` → `Enter a number.`,
> 1e50 → Maximalfehler. Djangos `IntegerFieldOverflow` (Django 5): außerhalb des Spaltenbereichs (AutoField
`int4`, ASN `0..2147483647`) liefert `exact` keine Treffer, `gt/gte` bei Unterlauf **alle** (Bedingung
entfällt, auch ASN `NULL`), `lt/lte` bei Überlauf alle. CSV (`__in`): leere Elemente werden zu `None`
und verworfen (`id__in=,` → keine Treffer).

`…__date__<op>` (DateTimeField): upstream `("added" AT TIME ZONE tz)::date <op> D`. Hier
gleichwertig und index-freundlich: `gt` → `added >= start(D+1)`, `gte` → `added >= start(D)`,
`lt` → `added < start(D)`, `lte` → `added < start(D+1)`, mit `start(d)` = erster Zeitpunkt des lokalen
Tages `d` (bei Zeitumstellung um Mitternacht fold=0 wie zoneinfo). `lte 9999-12-31` → `added IS NOT NULL`.

### Deklarierte Filter (Reihenfolge nach `Meta.fields`)

| Parameter | Wert | SQL-Semantik |
|---|---|---|
| `has_duplicates` | Boolean | `[NOT] EXISTS(SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND dup.id IN permitted_document_ids(user, include_deleted=True) AND dup.id <> doc.id AND (dup.checksum = doc.checksum OR dup.checksum = NULLIF(doc.archive_checksum,'') OR dup.archive_checksum = doc.checksum OR dup.archive_checksum = NULLIF(doc.archive_checksum,'')))` – nur für den Nutzer sichtbare Duplikate (auch im Papierkorb). Bei `true` steht davor ein Kandidaten-Vorfilter `documents_document.id = ANY(ARRAY(<Dokumente, deren checksum/archive_checksum im Bestand ≥ 2× als Prüfsumme vorkommt>))` (Obermenge, Index-Only-Scans über die Prüfsummen-Indizes; Ergebnis unverändert, nur schneller – Messung docs/contract/pgvalidate.md Abschnitt 8). `false` bleibt `NOT EXISTS(...)` wie Django. |
| `is_tagged` | Boolean | `[NOT] EXISTS(document_tags)` |
| `tags__id__all` | IDs, Python `int()`, ungültig → ignoriert | je ID `EXISTS(tag_id = x)` |
| `tags__id__none` | dto. | je ID `NOT EXISTS(tag_id = x)` |
| `correspondent__id__none`, `document_type__id__none`, `storage_path__id__none`, `owner__id__none` | dto. | je ID `NOT (<fk>_id = x AND <fk>_id IS NOT NULL)` (NULL bleibt drin) |
| `is_in_inbox` | exakt `true`/`false` (case-sensitiv) | `[NOT] EXISTS(document_tags WHERE tag_id IN (inbox tags))` |
| `title_content` | Rohwert, `strip()` | `title ICONTAINS v OR effective_content ICONTAINS v` – bei ASCII-Suchtext und kalibrierter DB in der ergebnisgleichen Faltungsform (s. „Schnelle Textsuche“) |
| `content__{istartswith,iendswith,icontains,iexact}` | Rohwert, `strip()` | auf `effective_content` (Inhalt der neuesten nicht gelöschten Version, sonst eigener Inhalt) |
| `custom_fields__id__all/__none/__in` | IDs | `EXISTS(documents_customfieldinstance WHERE field_id …)` |
| `has_custom_fields` | Boolean | `[NOT] EXISTS(documents_customfieldinstance)` |
| `custom_field_query` | JSON-DSL | s. u. |
| `shared_by__id` | Rohwert → `int()` | `owner_id = v AND (EXISTS(guardian_userobjectpermission …) OR EXISTS(guardian_groupobjectpermission …))` (content_type Document, `object_pk = id::varchar`); `int()`-Fehler (auch leerer Wert) → 500 wie upstream |
| `mime_type` | Rohwert, `strip()` | `mime_type ICONTAINS v` |
| `created__date__gt/gte/lt/lte` | DateFilter | wie `created__gt` usw. |

`custom_fields__icontains` (veraltet): `CustomField.objects.filter(extra_data__icontains=v)` aus der DB,
Optionen mit `label.lower()` enthält `v.lower()` → deren IDs; Bedingung
`EXISTS(cfi WHERE field_id IN (Felder mit name ICONTAINS v) OR value_text/bool/int/float/date/url/monetary/document_ids ICONTAINS v OR value_select IN (<ids>) OR value_long_text ICONTAINS v)`.

Hinweis: Wie upstream (JOIN über `custom_fields`) berücksichtigen Filter über
`documents_customfieldinstance` auch soft-gelöschte Instanzen; `num_notes`, Custom-Field-Sortierung und
Dokumentlink-Rückverweise verwenden wie upstream nur nicht gelöschte Zeilen.

## `custom_field_query`-DSL

```
expr  := [feld, op, wert]            # Atom
       | ["AND"|"OR", [expr, …]]      # nichtleere Liste, Groß-/Kleinschreibung egal
       | ["NOT", expr]
feld  := int (ID; bool zählt als int) | string (Name, exakt) | float (→ Name str(float))
op    := [<datumsteil>"__"]<operator>
```

* JSON wie Pythons `json.loads` (inkl. `NaN`/`Infinity`, beliebig große Ganzzahlen).
* Grenzen: Tiefe 10 (`Maximum nesting depth exceeded.`), 20 Atome (`Maximum number of query conditions exceeded.`).
* Operatoren je Datentyp:

| Datentyp | Operatoren |
|---|---|
| `string`, `url`, `longtext` | `exact in isnull exists icontains istartswith iendswith` |
| `date` | `exact in isnull exists gt gte lt lte range`, Präfixe `year iso_year month day week week_day iso_week_day quarter` |
| `boolean`, `select` | `exact in isnull exists` |
| `integer`, `float` | `exact in isnull exists gt gte lt lte range` |
| `monetary` | `exact in isnull exists icontains istartswith iendswith gt gte lt lte range` (`exact/in/arithmetisch` auf `value_monetary_amount`) |
| `documentlink` | `exact in isnull exists contains` |

* Wertvalidierung wie die DRF-Felder in upstream (`BooleanField`, `IntegerField(min/max int4)`, `FloatField`,
  `CharField(max_length=128, allow_blank=True)`, `SelectField` (Label → ID, max 16), `DateQueryField`
  (ISO-Datum oder relativ: `today`, `in 30 days`, `3 weeks ago`, …), `MonetaryAmountField`
  (`"USD100.00"` → 100.00), `ListField` für `in` (nicht leer) und `range` (genau 2)).
* Fehlerpfade (`handle_validation_prefix`): `{"custom_field_query": …}` → bei logischen Ausdrücken `"1"`
  (Argumentliste) und dann der Listenindex; im Atom `"0"` (Feld), `"1"` (Operator), `"2"` (Wert);
  ListField-Kindfehler unter dem Index. Texte:
  * `Value must be valid JSON.` · `Invalid custom field query expression` · `Invalid expression list. Must be nonempty.`
  * `{"0": ["Invalid logical operator 'xyz'"]}` (Python-`repr`)
  * `'name' is not a valid custom field.` · `<typ> does not support query expr 'op'.`
  * DRF: `This field may not be null.`, `This field may not be blank.`, `Not a valid string.`,
    `Ensure this field has no more than 128 characters.`, `A valid integer is required.`,
    `A valid number is required.`, `Must be a valid boolean.`, `Ensure this value is less than or equal to 2147483647.`,
    `Expected a list of items but got type "str".`, `This list may not be empty.`,
    `Ensure this field has at least 2 elements.`, `Date has wrong format. Use one of these formats instead: YYYY-MM-DD.`,
    `Expected a date or a relative date such as "today" or "in 30 days".`
* SQL je Atom: `EXISTS(SELECT 1 FROM documents_customfieldinstance cq WHERE cq.document_id = doc.id AND cq.field_id = F AND <bedingung>)`;
  `exists false` → `NOT EXISTS(…field…)`; `NOT` negiert das ganze Atom (Dokumente ohne Feld passen).
  `year/iso_year` mit `exact/gt/gte/lt/lte` → Datumsgrenzen (`BETWEEN`), andere Teile `EXTRACT(…)`
  (`week_day` = `DOW + 1`). Dokumentlink `contains [ids]`: Rückverweise der genannten Dokumente
  (`FilterDb::doc_link_instances`), Schnittmenge → `doc.id IN (…)`; fehlt ein Rückverweis → keine Treffer;
  `contains []` → Feld vorhanden und `value_document_ids IS NOT NULL`.
* 500 wie upstream bei: Operator eines logischen Ausdrucks kein String, Feldschlüssel Liste/Dict,
  `<teil>__exists`, nicht endlicher Geldbetrag (`-inf`), `select` ohne `extra_data`.
  Jahr außerhalb 1..9999: **400** `{"custom_field_query": ["Ensure this value is between 1 and 9999."]}`
  (upstream 500, Known differences).

## Sortierung (`ordering`)

* `ordering` enthält `custom_field_` → `int(teil nach dem Präfix)` (Fehler → **400**
  `{"ordering": ["Invalid custom field ordering. Use custom_field_<id> or -custom_field_<id> on its own."]}`, upstream 500); Feld fehlt → 400
  `{"custom_field_<id>": ["Custom field not found"]}`; Sortierung `has_field DESC, wert ASC|DESC`
  (`has_field` = nicht gelöschte Instanz vorhanden; Wert = erste Instanz nach `created`).
  Wertspalte: `string`/`longtext` → `value_text` (upstream sortiert auch Langtext über `value_text`),
  `integer` `value_int`, `float` `value_float`, `date` `value_date`, `monetary` `value_monetary_amount`,
  `documentlink` `value_document_ids`, `url` `value_url`, `boolean` `value_bool`,
  `select` → Index der Option in nach Label sortierter Liste (Summe über alle Instanzen, kein Wert → Anzahl Optionen).
  Alles außer genau `custom_field_<id>`/`-custom_field_<id>` (z. B. `custom_field_1,title`) → **400**
  `{"ordering": ["Invalid custom field ordering. Use custom_field_<id> or -custom_field_<id> on its own."]}` (upstream 500, Djangos `FieldError`).
* Sonst DRF: kommagetrennte Terme (`strip`), gültig sind `id title correspondent__name document_type__name
  storage_path__name created modified added archive_serial_number num_notes owner page_count` mit
  optionalem `-`; ungültige werden still verworfen. Keine gültigen → `created DESC, id DESC`.
  `owner` → `owner_id`; `*__name` → skalare Unterabfrage auf den Namen (NULL zuletzt bei ASC wie
  PostgreSQL); `num_notes` → `COALESCE((SELECT COUNT(*) FROM documents_note WHERE … AND deleted_at IS NULL), 0)`.
* **Kein Tiebreaker** bei expliziter Sortierung – exakt wie upstream (`order_by(*ordering)` ersetzt
  `-created, -id`). Gleichstände sind in PostgreSQL nicht deterministisch (gilt für Django genauso).

## Suche (`search`)

DRF `SearchFilter`: Wert wird mit `search_smart_split` zerlegt (Anführungszeichen halten Phrasen
zusammen, Kommas trennen); jeder Term muss in `title`, Korrespondentenname oder `effective_content`
(`ICONTAINS`) vorkommen (UND über Terme, ODER über Felder).

### Schnelle Textsuche (`src/textfold.rs`, O2 2026-11-08)

`title_content`, `content__{icontains,istartswith,iendswith,iexact}` und die Titel-/Inhaltsteile von
`?search=` setzen Django `UPPER(x::text) LIKE UPPER(p)` ab. Ist der (gestrippte) Suchtext reines ASCII
und hat `FilterDb::upper_fold()` eine Faltung geliefert, wird stattdessen
`replace(…replace(UPPER((x)::text COLLATE "C"), s1, U(s1))…, sk, U(sk)) LIKE 'P'` (bzw. `= 'P'`)
erzeugt: `P` = ASCII-Großbild des Musters (gleiches Escaping wie Django), `s_i` = die Nicht-ASCII-Zeichen,
deren Großbild in der DB-Kollation ASCII enthält **und** einen Buchstaben von `P` teilt (ICU `en-US`:
`ß ı ŉ ſ ǰ ẖ ẗ ẘ ẙ ẚ ﬀ ﬁ ﬂ ﬃ ﬄ ﬅ ﬆ`), `U(s_i)` = deren Großbild laut DB. Der Ausdruck `x`
(inkl. `effective_content`-Unterabfrage) kommt genau einmal vor (eine Dekomprimierung je Zeile).
Begründung der Ergebnisgleichheit und Abschaltbedingungen im Modulkopf; kalibriert wird einmal je
Prozess und Datenbank (`upper(chr(i))` der Ebenen 0–1, ~0,2 s), nur wenn ein solcher Filter aktiv ist.
Mehr als 4 relevante Sonderzeichen (`MAX_REPLACES`, z. B. `strasse`; `rent` mit 4 bleibt gefaltet) →
Django-Form, weil jedes `replace()` ein weiterer Durchlauf ist (gemessen: 2 Ersetzungen −30 %,
6 Ersetzungen +10 % gegenüber `UPPER()` der ICU-Kollation). Nicht-ASCII-Suchtexte (`für`, `ß` …), Nicht-UTF-8-DBs, türkische/aserbaidschanische/litauische
Kollationen und eigene Spaltenkollationen behalten die Django-Form. Prüfung gegen die echte
Kollation: `tests/textfold_db.rs` (12 168 Vergleiche Texte × Muster × 4 Lookups, alle gleich).
`MockDb` liefert `None` → Erwartungs-SQL der übrigen Tests unverändert.

`has_content_text_filter(params)` meldet pl-documents, dass eine solche Bedingung aktiv ist (dann
Seite + Zählung in einem Scan, `pl_core::pagination::fetch_page_windowed`).

## Rechte

* Alle Filter schränken nur ein; die Sichtbarkeit setzt `pl-documents` danach (`PermittedObjectsFilter`).
* `has_duplicates` zählt nur Duplikate, die der Nutzer mit `view_document` sieht (`pl_authz::objects::permitted_document_ids(user, include_deleted=true)`).
* `owner__id`, `owner__id__none`, `shared_by__id` filtern nur sichtbare Dokumente weiter.
* Hinweis (upstream-treu, nicht verändert): `custom_field_query` mit Dokumentlink `contains [x]` liest die
  Verweise von Dokument `x` unabhängig davon, ob der Nutzer `x` sehen darf. Das Ergebnis enthält nur
  sichtbare Dokumente, verrät aber, welche sichtbaren Dokumente `x` verlinkt. Für das Audit vermerkt.

## Known differences

1. **Keine Duplikatzeilen:** upstream liefert bei `tags__name__*` (Join ohne `distinct`) ein Dokument
   mehrfach, wenn mehrere seiner Tags passen (Bug: doppelte Einträge/falscher `count`). Hier per `EXISTS`
   je Dokument genau einmal.
2. **Formularsprache/-formate:** Fehlertexte nur Englisch; `DateFilter` akzeptiert die Eingabeformate des
   Locales `en` (ISO und z. B. `10/25/2006`, `Oct 25 2006`). Bei aktiver anderer Sprache (Accept-Language)
   würde Django übersetzen und deren Formate akzeptieren. (Folgearbeit: `pl_core::datetime::parse_date_input(lang)` nutzen.)
3. **Relative Datumsangaben** (`dateparser`, W8, nachgeprüft 2026-11-18): Nachbau des
   dateparser-Ablaufs (`src/relative_dp.rs`: `sanitize_date`, Unicode-Normalisierung, `simplifications`,
   `Dictionary.split`/`are_tokens_valid`, `Locale.translate`, `FreshnessDateDataParser`) mit den
   **originalen Sprachdaten** von dateparser 1.4.3 (`src/relative_dp_data.rs`, erzeugt von
   `tools/gen_relative_dateparser.py`) für en, ru, es, fr, de, tr, pt, it, nl, pl, cs, uk, el, sv, ro, hu,
   da, fi, sk, nb, ca (Reihenfolge wie dateparser; en immer, die übrigen wenn `DATE_PARSER_LANGUAGES`
   leer ist oder sie enthält). Damit gelten auch dateparsers Eigenheiten (freie Wortstellung wie
   `ago 3 days`, „in“ irgendwo = Zukunft, `letzte Woche`, `3 j`, Bruchzahlen bei Tagen, Fehler bei
   Bruchzahlen von Monaten/Jahren). Prüfung: `tests/w8_dateparser.rs` gegen von dateparser selbst
   erzeugte Erwartungen (`tools/gen_w8_corpus.py`; 3 360 Zeilen im Repo, beim Erstellen auch 16 429 bzw.
   26 667 Zeilen ohne Abweichung). Die frühere, von Hand geschriebene Wortliste wich in rund 19 000 von
   26 667 Fällen ab (fehlende Wörter und Formen, aber auch falsche Treffer wie `vor 3 jahrzehnten`,
   `tra un anno`, `peste 2 săptămâni`) und ist ersetzt.
   Bleibende Abweichungen: ohne `DATE_PARSER_LANGUAGES` probiert upstream alle ~200 dateparser-Sprachen
   (hier nur die 21 oben; Sprachen ohne Wortzwischenräume wie ja/zh/ko/th fehlen bewusst);
   Zeitzonen-Kürzel im Text (`pop_tz_offset_from_string`) und Ziffern anderer Schriften
   (`_translate_numerals`) werden nicht ausgewertet; `no`/`nn` gelten als Alias für `nb` (upstream
   startet mit `no` gar nicht, dateparser kennt das Kürzel nicht); Regionalvarianten (`de-AT`) nutzen die
   Daten der Grundsprache. Erster Gebrauch einer Sprache übersetzt deren Muster (einmal je Prozess).
4. **Ziffern:** Python akzeptiert in `int()/Decimal()/float()` auch Nicht-ASCII-Ziffern; hier nur ASCII.
5. **Datum `__date`-Vergleiche** als Zeitpunktbereich statt `AT TIME ZONE … ::date` (gleiche Treffer,
   chrono-tz statt PostgreSQL-tzdata).
6. **Deprecation-Logs** (`title_content`, `custom_fields__icontains`) werden nicht geschrieben.
7. **Gruppierungs-Artefakte** von upstream (`Count`/`Sum` + `GROUP BY` mit zusätzlichen Joins, z. B.
   Select-Sortierung kombiniert mit Tag-Name-Filtern) werden nicht nachgebildet.
8. `custom_field_query` wird direkt auf die äußere Abfrage angewendet statt über
   `pk__in=Document.objects…` (gleiches Ergebnis, da die Basis bereits nicht gelöschte Dokumente liest).
9. **400 statt 500 bei feindlichen Parametern** (B-305/B-313/D-039, 2026-11-16, bewusst – weg von ngx):
   * `ordering=custom_field_<kein int>` (z. B. `custom_field_abc`, `custom_field_`) → 400
     `{"ordering": ["Invalid custom field ordering. Use custom_field_<id> or -custom_field_<id> on its own."]}`;
   * `custom_field_<id>` kombiniert mit anderen Termen (`custom_field_1,title`, `xcustom_field_1`) → 400
     `{"ordering": ["Invalid custom field ordering. Use custom_field_<id> or -custom_field_<id> on its own."]}`;
   * `created__year`/`added__year`/`modified__year` außerhalb 1..9999 (auch `-1`, `99999999999`) → 400
     `{"<param>": ["Ensure this value is between 1 and 9999."]}`;
   * `custom_field_query` mit `year__*`/`iso_year__*` außerhalb 1..9999 → 400
     `{"custom_field_query": ["Ensure this value is between 1 and 9999."]}`.
   upstream: Django `ValueError`/`FieldError` → 500. Der Differenztest gegen Django zählt diese Fälle
   jetzt als „verschieden“ (500 vs. 400). Andere 500-Fälle oben (Operator kein String usw.) bleiben.
10. **Ordner-Filter (nur PaperRust, 2026-11-22):** `folder__id`, `folder__id__in`, `folder__id__tree`,
    `folder__isnull` (Ordneransicht, docs/contract/pl-api-v2.md „Ordner“). Kennt paperless-ngx nicht – dort
    werden die Parameter ignoriert (ganze Liste), hier filtern sie. Sie stehen **nach** allen upstream-Filtern,
    damit Formularfehler der upstream-Filter in gleicher Reihenfolge bleiben; Prüfung wie `NumberFilter`
    („Enter a number.“) bzw. `BaseInFilter`/`BooleanWidget`. AST `Pred::Folder(FolderCond::{In,Tree,Any})`,
    SQL `sql::folder_exists` (`EXISTS` auf `paperrust_document_folder`, `Tree` mit rekursivem CTE über
    `paperrust_folder`). Ohne PaperRust-Migrationen fehlt die Tabelle → 500, nur wenn der Parameter benutzt wird.

## Tests

`cargo test -p pl-docfilter` (ohne DB, Mock für `FilterDb`):

* `tests/filterset.rs` – Formularfehler (Texte, Reihenfolge), alle Filterarten, Überlaufsemantik, Zeitzonen,
  Rechte-Unterabfragen, Suche.
* `tests/custom_field_query.rs` – alle Fehlerfälle aus `test_api_filter_by_custom_fields.py`
  (Pfade/Texte), Tiefe/Atome, relative Daten/Sprachen, Geldbeträge, Dokumentlinks, SQL-Form.
* `tests/ordering.rs` – Standard, DRF-Terme, `num_notes`, Custom-Field-Sortierung aller Typen inkl.
  Select, Fehler (`test_document_custom_fields_sorting_invalid*`).
* Unit-Tests in `src/pyval.rs`, `src/dates.rs`, `src/search.rs` (Python-/Django-Parser).

* `tests/parity.rs` (`#[ignore]`, nur mit laufender Referenzumgebung):
  `cargo test -p pl-docfilter -j 2 --test parity -- --ignored --nocapture`. Vergleicht für admin
  (Superuser) und alice (Gruppenrechte) je 181 Anfragen (alle Filterarten, alle Sortierungen inkl.
  `custom_field_1..11` aller Datentypen, 30+ `custom_field_query`-Fälle, Formular-/DSL-Fehler) die
  Django-Antwort (`fields=id`, `page_size=100000`) mit der hier erzeugten SQL auf derselben DB
  (Sichtbarkeit über `pl_authz::objects::permitted_objects_filter`). `FilterDb for PgPool` wird dabei
  mitgetestet.

### Ergebnis Paritätslauf (2026-11-08, Referenz v3.4.2, Europe/Zurich)

`same=318 tie_only=44 different=0` – alle 44 Abweichungen sind nur die Reihenfolge bei Gleichstand
unter `ordering=[-]custom_field_<id>` (viele Dokumente ohne Feld, upstream ohne Tiebreaker; gleiche
Menge; die Sortierschlüssel selbst wurden nicht einzeln verglichen). Fehlerantworten (400) sind im JSON gleich.

## L1: Filter-AST und eigener SQL-Bauer (ARCHITECTURE-V2 §5, 2026-11-10)

Ziel: die django-filter-Emulation (`DocumentFilterSet`, Lookups, `custom_field_query`-DSL) wird
durch einen **typisierten, aufgelösten AST** ersetzt; v1-Parameter werden nur noch **geparst**,
SQL entsteht an einer Stelle. Gleiches Ergebnis wie upstream (Parität bleibt 0 different), eine
Abfrage je Liste/Zählung/Auswahl, Sichtbarkeit mengenbasiert und für L5 austauschbar.

### Schichten

```text
v1  GET /api/documents/?…          ┐
v1  all=true-Auswahl (filters={…})  ├─ pl_docfilter::ast (Parser) ──┐
v2  POST /rs/api/v2/documents/query ┘   (v2: Blätter über denselben   │   pl_domain::filter::DocFilter
                                          Parser, all/any/not als       ├─► { pred: Pred, sort: Sort }
                                          UND/ODER/Komplement)          │
                                                                        ▼
                         pl_store::docsql::Builder (Visibility, UpperFold) → eine SelectStatement
                         → pl_core::pagination (OFFSET-Seite + COUNT als InitPlan) bzw. v2-Keyset
```

| Teil | Ort |
|---|---|
| AST (`DocFilter`, `Pred`, `Sort`, `CfCond`, …), ohne Tabellennamen | `crates/pl-domain/src/filter.rs` |
| Parser v1 → AST: `ast::parse` (Liste: Filter + `search` + `ordering`), `ast::parse_filters` (strikt/nachsichtig) | `crates/pl-docfilter/src/ast.rs`, `filterset.rs::parse_filterset`, `cfquery.rs::custom_field_query_pred`, `search.rs::search_pred`, `ordering.rs::parse_ordering` |
| SQL-Bauer, Sichtbarkeit, Umschalter, v1-Einstiege `v1_list`/`v1_selection` | `crates/pl-store/src/docsql.rs` |
| v2: `Cond` → `Pred` (`cond_pred`), Keyset auf der AST-Abfrage | `crates/pl-store/src/documents.rs` |
| Aufrufer | `pl-documents/src/listing.rs::filtered_documents_ext` (Liste, Detail-`get_object`, PATCH-Liste, Suche ∩ ORM), `selection_filter.rs::resolve_all` (bulk_edit/delete/… `all=true`, selection_data), `pl-store` `query`/`sync::resolve`/`visible_ids_sql` |

Der Parser benutzt **dieselbe** Formularprüfung (`clean`), dieselben DB-Nachschläge
(`FilterDb`: Custom-Field-Definitionen, Dokumentlink-Rückverweise, Select-Optionen für
`custom_fields__icontains`, Custom Field der Sortierung) und wirft dieselben Fehler in derselben
Reihenfolge wie der bisherige Weg (Formularfehler aller Felder → Laufzeitfehler je Filter in
Formularreihenfolge, auch hinter einem konstanten FALSE → `search` → `ordering`). Konstante
Teilergebnisse (Djangos `EmptyResultSet`/`FullResultSet`) stehen als `Pred::False`/`Pred::True` im
Baum und werden wie Django vereinfacht.

### Umschalter

`PAPERRUST_QUERY_ENGINE=legacy|ast` (Klasse A, live; `pl-settings`, docs/SETTINGS.md 1.10). Standard
siehe „Stand“ unten. Gilt für v1-Liste/Detail/Suche-Schnittmenge, `all=true`-Auswahl, v2-Abfrage ohne
`any`/`not`, `sync/resolve`, `visible_ids_sql` (Zeitstrahl, `/d2`). v2 mit `any`/`not` nimmt immer
den AST-Weg (der bisherige kann es nicht).

### AST → SQL (Abbildungstabelle)

`D` = äußere Zeile `"documents_document"` (ohne Alias), Basis immer
`D.root_document_id IS NULL AND D.deleted_at IS NULL`.

| v1-Parameter | Knoten | SQL |
|---|---|---|
| `id`, `id__in`, `archive_serial_number[__gt/gte/lt/lte]`, `<fk>__id`, `<fk>__id__in` (`fk` = correspondent, document_type, storage_path, owner) | `Num{col,op,v}` / `NumIn{col,vs}`; Überlauf (int4, ASN `0..2³¹-1`) schon im Parser zu `True`/`False` | `D.col <op> v` / `D.col IN (…)` |
| `<x>__isnull` | `IsNull{col,is_null}` | `D.col IS [NOT] NULL` |
| `<fk>__id__none` | je ID `FkNot{col,id}` | `NOT (D.fk = x AND D.fk IS NOT NULL)` (NULL bleibt drin) |
| `title__i*`, `original_filename__i*`, `checksum__i*`, `mime_type` | `Text{col,lookup,v}` | `UPPER(D.col::text) LIKE UPPER('%v%')` (`iexact`: `=`); Titel in Faltungsform, wenn kalibriert |
| `content__i*`, `title_content` | `Text{Content,…}` (`title_content` = `Or(Title, Content)`) | auf `effective_content` = `COALESCE((neueste nicht gelöschte Version).content, D.content)`; Faltungsform |
| `<fk>__name__i*` | `RelName{rel,lookup,v}` | `D.fk IN (SELECT id FROM <rel> WHERE UPPER(name::text) LIKE …)` |
| `tags__id`, `tags__id__in`, `tags__id__all` (je ID), `tags__id__none` (je ID, `Not`), `tags__name__i*`, `is_tagged`, `is_in_inbox` | `Tag(Is/In/Name/Any/Inbox)` | `EXISTS (SELECT 1 FROM documents_document_tags dt WHERE dt.document_id = D.id AND …)` |
| `custom_fields__id__all/none/in`, `has_custom_fields`, `custom_fields__icontains` | `Instance(Field/FieldIn/Any/IContains)` | `EXISTS (… documents_customfieldinstance ci …)` inkl. soft-gelöschter Instanzen (JOIN-Semantik upstream) |
| `custom_field_query` Atom | `CustomField{field_id, test: Option<CfCond{column,test}>}` | `EXISTS (SELECT 1 FROM documents_customfieldinstance cq WHERE cq.document_id = D.id AND cq.field_id = F [AND test(cq.column)])` |
| `custom_field_query` `AND/OR/NOT` | `And/Or/Not` | SQL `AND/OR/NOT` |
| Dokumentlink `contains [ids]` | im Parser aufgelöst (Rückverweise, Schnittmenge) → `NumIn{Id}` oder `False`; `contains []` → `CfCond{DocumentIds, IsNull(false)}` | `D.id IN (…)` |
| `created__gt/gte/lt/lte`, `created__date__*` | `Created{op,date}` | `D.created <op> 'YYYY-MM-DD'` (DateField, keine Zeitzone) |
| `created__year` | `CreatedBetween{from,to}` | `D.created BETWEEN 'Y-01-01' AND 'Y-12-31'` |
| `created__month/__day` | `CreatedPart{part,Eq,v}` | `EXTRACT(MONTH FROM D.created) = v` |
| `added/modified__gt/gte/lt/lte` (IsoDateTime) | `Ts{col,op,at UTC}` | `D.col <op> timestamptz` (naive Eingabe in `TIME_ZONE`, mehrdeutig → 400) |
| `added/modified__date__*` | `Ts{…}` mit lokalem Tagesbeginn (`TsNotNull` bei `lte 9999-12-31`) | `D.col >= start(d+1)` usw. (= `(col AT TIME ZONE tz)::date <op> d`) |
| `added/modified__year` | `Ts ≥ start AND Ts < end` (lokales Jahr) | Zeitpunktbereich |
| `added/modified__month/__day` | `TsPart{col,part,tz,Eq,v}` | `EXTRACT(MONTH FROM D.col AT TIME ZONE 'tz') = v` |
| `has_duplicates` | `HasDuplicates(b)` | `true`: Kandidaten-Vorfilter (`id = ANY(ARRAY(...))`) `AND EXISTS(dup …)`; `false`: `NOT EXISTS(dup …)`; Sichtbarkeit des Duplikats direkt auf `dup` (s. u.), auch im Papierkorb |
| `shared_by__id` | `SharedBy(uid)` | `D.owner_id = uid AND (EXISTS guardian_user… OR EXISTS guardian_group…)` |
| `?search=` | je Term `Or(Text Title, RelName Correspondent, Text Content)`, UND über Terme | wie oben |
| `ordering` | `Sort::Default/Terms/CustomField` | wie Abschnitt „Sortierung“: `*__name` per LEFT JOIN, `num_notes` per gruppierter Notizzählung, `custom_field_<id>` per `EXISTS DESC, Wert-Unterabfrage`, Select per Optionsindex |

`CfCond`-Tests (`cq.<spalte>`; Spalte nach Datentyp, Geldbetrag bei `exact/in/gt/gte/lt/lte/range` →
`value_monetary_amount`, sonst Rohtext `value_monetary`): `Cmp(op, v)`, `In`, `IsNull`, `Between`,
`Text(lookup)`, `YearBounds(op, Jan 1, Dez 31)` (`year/iso_year` mit `exact/gt/gte/lt/lte` →
`BETWEEN` bzw. Grenze), `Part(part, IsNull/Cmp/In/Between)` → `EXTRACT(part FROM cq.value_date)`
(`week_day` = `DOW + 1`, `iso_week_day` = `ISODOW`). Werte: `Int`, `HugeInt` (`CAST('±1e400' AS
numeric)`), `Float`, `Text`, `Date`, `Numeric` (`CAST('…' AS numeric)`), `Json` (Dokumentlink-Liste).

v2-Blätter → v1-Parameter → derselbe Parser (z. B. `created {from,to}` → `created__date__gte/lte`,
`tags {any,all,none,is_null}` → `tags__id__in/all/none`, `is_tagged`); `all` → `And`, `any` → `Or`
(leer → nichts), `not` → `Complement` = `(p) IS NOT TRUE` (Mengenkomplement inkl. NULL, z. B.
`not asn.gt 5` trifft Dokumente ohne ASN).

### Sichtbarkeit (`Visibility`, Austauschpunkt L5)

`PermittedObjectsFilter` (`view_document`) bzw. `permitted_document_ids` als Bedingung **auf der
Wurzelzeile**: `All` (aktiver Superuser), `Nothing` (inaktiv → `FALSE`), `Grants(user)` →
`D.owner_id = uid OR D.owner_id IS NULL OR CAST(D.id AS varchar) IN (guardian user ∪ group keys)`.

Gleichwertigkeit: upstream prüft `D.id IN permitted_object_ids(..., parent_field=root_document)`, also
eine Zeile `T` mit `T.deleted_at IS NULL` und `authorizing_owner(T) ∈ {uid, NULL}` oder
`str(authorizing_id(T)) ∈ grants`, mit `authorizing_owner = CASE WHEN T.root IS NULL THEN T.owner
ELSE parent.owner`, `authorizing_id = COALESCE(T.root, T.id)`. Für die äußere Zeile gilt `root IS
NULL` und `deleted_at IS NULL` (Basis) ⇒ `T = D`, `authorizing_owner = D.owner_id`,
`authorizing_id = D.id`. Für Duplikate (`include_deleted=True`, `dup.root IS NULL`) ebenso ohne
Papierkorbfilter. Die Rechte-Unterabfrage ist unkorreliert (PostgreSQL: gehashter SubPlan, einmal je
Abfrage); der bisherige Selbst-JOIN über `root_document` und der zweite Durchlauf über
`documents_document` entfallen. `Visibility::visible_ids_sql()` liefert dieselbe Menge als
Unterabfrage für Seitentabellen (`pl_store::visible_ids_sql`). **L5** ersetzt nur
`Visibility::root_row` (z. B. `EXISTS paperrust_grant …`); AST und Bauer bleiben.

### Django-Eigenheiten, die erhalten bleiben (Prüfliste)

1. **Groß-/Kleinschreibung:** `UPPER(x::text) LIKE UPPER(p)` / `= UPPER(p)` wie Djangos
   PostgreSQL-Backend, **nicht** `ILIKE` (das vergleicht per `lower()`/Faltung und unterscheidet sich
   bei `ß`→`SS`, `ı`, `ſ`, `ﬁ` …, und bei ICU-Kollationen). `%`, `_`, `\` werden wie
   `prep_for_like_query` maskiert. Die Faltungsform (`textfold`) ist ergebnisgleich und nur bei
   kalibrierter DB (UTF-8, Standardkollation der Spalten, kein tr/az/lt) aktiv.
2. **Zeitzonen (`USE_TZ=True`, `TIME_ZONE`):** `created` ist ein `DateField` (ohne Zeitzone);
   `added`/`modified` sind `timestamptz`: `__year` = lokale Jahresgrenzen, `__month/__day` =
   `EXTRACT(… AT TIME ZONE tz)`, `__date__<op>` = lokaler Tagesbeginn (fold=0 bei Zeitumstellung,
   wie zoneinfo), naive IsoDateTime-Eingaben in `TIME_ZONE`, nicht existente/mehrdeutige → 400.
   `custom_field_query`-Daten sind `DateField` (`value_date`).
3. **Ganzzahlüberlauf** (Django 5 `IntegerFieldOverflow`): `exact` außerhalb → nichts, `gt/gte` bei
   Unterlauf / `lt/lte` bei Überlauf → Bedingung entfällt (auch NULL-Zeilen), sonst nichts.
4. **`exclude()` auf nullable FK** behält NULL (`__id__none`); `tags__id__none`/`NOT EXISTS`.
5. **Mehrwertige Relationen ohne Zeilenvervielfachung** (`EXISTS`); upstream-Duplikatzeilen bei
   `tags__name__*` werden wie bisher nicht nachgebildet (Known difference 1).
6. **Sortierung:** kein Tiebreaker bei expliziter `ordering` (wie `order_by(*ordering)`),
   PostgreSQL-NULL-Reihenfolge (ASC = NULLS LAST, DESC = NULLS FIRST) wie Django ohne
   `nulls_first/last`; Standard `-created, -id`. Custom-Field-Sortierung: `has_field DESC` (nicht
   gelöschte Instanz), Wert der ersten Instanz nach `created`, Select über den Optionsindex der nach
   Label sortierten Liste (Python-`sorted`-Fehler → 500).
7. **Verschachtelte Tags:** v3.4.2 filtert nur über die Zuordnungstabelle (beim Speichern stehen
   die Eltern schon darin); keine Auflösung von Nachkommen im Filter.
8. **Soft-gelöschte Custom-Field-Instanzen** zählen in Filtern mit (JOIN upstream), nicht in
   Sortierung/`num_notes`/Rückverweisen.
9. **Dokumentlink `contains`:** Rückverweise werden ohne Rechteprüfung gelesen (upstream-treu,
   siehe „Rechte“).
10. **`is_in_inbox=1|0`** (2026-11-17): upstream wertet nur `true`/`false` aus und liefert bei `1`
    stillschweigend alle Dokumente; die Oberflächen senden Wahrheitswerte aber als `1`/`0`
    (`query-params.ts`). Hier gelten `1`/`0` wie `true`/`false`.

### Indizes (expand-only, `pl_store::migrations`, parity-neutral)

`crates/pl-store/migrations/0001_paperrust_docquery_indexes.sql` (eingespielt über
`paperrust manage migrate`, Name `pl_store.0001_paperrust_docquery_indexes`):

```sql
CREATE INDEX IF NOT EXISTS paperrust_doc_live_created
    ON documents_document (created DESC, id DESC) INCLUDE (owner_id)
    WHERE root_document_id IS NULL AND deleted_at IS NULL;
```

Teilindex genau über die Basismenge der Liste in Standardsortierung, `owner_id` als INCLUDE: COUNT
und Seite laufen als Index-Only-Scan, für Nicht-Superuser wird die Sichtbarkeit (Eigentümer +
gehashte Freigabemenge auf `id`) ohne Heap-Zugriff geprüft (vorher kein Gewinn für Nicht-Superuser,
pgvalidate.md P3, weil die Rechte-Unterabfrage den Heap brauchte). Ergebnisse unverändert (nur
Pläne); Rückbau `DROP INDEX IF EXISTS paperrust_doc_live_created;`. Weitere Indizes waren nicht
nötig: Tags/Custom-Field-Instanzen/guardian haben passende Django-Indizes (`(document_id, tag_id)`,
`(document_id, field_id)`, `(user_id, permission_id, object_pk)` …).

### Tests

* `cargo test -p pl-domain`, `-p pl-docfilter`, `-p pl-store` (ohne DB): AST-Vereinfachung,
  Umschalter, Sichtbarkeitsform, Komplement.
* **Differenztest** `crates/pl-store/tests/docquery_diff.rs` (`#[ignore]`, DB-Kopie):
  bisheriger Weg vs. AST-Weg – gleiche IDs in gleicher Reihenfolge, gleiche Zählung, gleiche
  Fehler – für alle Nutzer der Kopie (Superuser, Gruppenrechte, Einzelnutzer, ohne Rechte,
  inaktiv) über (1) die 181 Anfragen des Django-Paritätstests, (2) alle `GET /api/documents/`-
  Fälle der Paritätskataloge, (3) zufällige Kombinationen (fester Startwert; 1–4 Parameter aus
  allen Filterarten inkl. ungültiger Werte, Zufalls-`custom_field_query`-Bäume bis Tiefe 3,
  Zufalls-`ordering`), (4) `all=true`-Auswahl mit nachsichtigen Filtern. `PL_L1_TIMING=1` misst
  zusätzlich beide SQL-Formen.

### Stand und Ergebnisse (2026-11-10)

**Standard `PAPERRUST_QUERY_ENGINE=ast`** (seit diesem Stand; `legacy` bleibt als Rückfall). *(seit Datenmodell-Stufe 7 entfernt: es gibt nur noch den AST-Weg, docs/DATAMODEL-V2.md §6)*

| Prüfung | Ergebnis |
|---|---|
| Differenztest `pl_l1` (Kopie `paperless_snapshot`, 5 Nutzer, `PL_L1_RANDOM=20000`) | **25 655 Fälle**: 1 655 fest (Paritätsanfragen + Kataloge × Nutzer), 20 000 zufällig, 4 000 Auswahl; 22 397 gleiche Zeilen, 3 239 gleiche Fehler (400 im Wortlaut, 500 als Klasse), 19 nur Gleichstand*, **0 verschieden** (8 083 Fälle mit Treffern) |
| Differenztest `pl_l1_100k` (63 Messanfragen × admin/alice/bob, ohne/mit Index) | 0 verschieden (5 bzw. 6 nur Gleichstand*) |
| Paritätslauf lesend (A Django :8000, B `ast` Standard, Mischbetrieb) | **2 024 verglichen, 2 002 identisch, 0 verschieden**, 22 akzeptiert, 580 übersprungen, 0 Lecks (`results/parity/l1/2026-11-10_*.md`, letzter Lauf); pl-docfilter 188/0, pl-documents 199/0, pl-search 76/0 |
| Paritätslauf schreibend (W2-Kopie `wl1`, `--write-only`, `ast`) | 567 verglichen, 555 identisch, 4 verschieden, 8 akzeptiert, 3 übersprungen, 0 Lecks (`results/parity/l1-write/2026-11-10_154209.md`); die 4 sind ausschließlich `gaps.config.*` (`externally_configured_variables`, Artefakt (a) unten); Nachlauf ohne Variable `gaps.config.*`: **11/11 identisch** (`…_164225.md`) → effektiv 0 verschieden |
| v2 `documents/query` (`pl_l1`) | `not`/`any` mengengleich (195 = 96 + 99; `any[asn>1050, asn null]` = 161), Keyset-Durchlauf je Sortierung vollständig ohne Dubletten |

\* Nur Gleichstand: bei expliziter `ordering` ohne Tiebreaker (upstream) dürfen gleichrangige Zeilen
in beliebiger Reihenfolge kommen; geprüft, indem beide Abfragen mit zusätzlichem `ORDER BY id`
identisch sind. Schon der bisherige Weg ist hier zwischen zwei Läufen nicht stabil (Messung unten:
`-num_notes`, `correspondent__name`, `ordering=-created` auf `DateField`).

Hinweis Paritätslauf: zwei Zwischenläufe zeigten nur Testaufbau-Artefakte – (a) gesetzte Variable
`PAPERRUST_QUERY_ENGINE` erscheint wie jede `PAPERLESS_*`/`PAPERRUST_*`-Umgebung in
`/api/config/ externally_configured_variables` (dazu `PAPERLESS_THREADS_PER_WORKER`, das
`pl_core::resources` bei knappem RAM selbst setzt), (b) ohne `PAPERRUST_BRANDING=off` andere
Login-/Index-HTML. Mit Standard `ast` (keine Variable) und Branding aus: 0 verschieden.

### Messung (100k-Kopie `pl_l1_100k` von `pl_pgvalidate`, Laptop, warm)

**SQL** (`docquery_diff` mit `PL_L1_TIMING=1`, Minimum aus 3; `tests/l1/queries_scale.txt`;
Rohdaten `results/l1/sql_100k_*.csv`, Tabelle `results/l1/sql_100k_table.md`). Vorher = bisheriger
Weg ohne Index, nachher = AST-Weg mit Index; Summe über 21 Anfragen × admin/alice/bob:

| | Seite (`LIMIT 50`) | `COUNT` |
|---|---:|---:|
| vorher (`legacy`, ohne Index) | 6 698 ms | 14 530 ms |
| nur AST (ohne Index) | 5 827 ms | 13 568 ms |
| nur Index (`legacy`) | 5 465 ms | 8 229 ms |
| **nachher (`ast` + Index)** | **4 629 ms (−31 %)** | **6 082 ms (−58 %)** |

Einzelwerte (ms, vorher → nachher): Standardliste COUNT admin 230 → 8, alice 255 → 36, bob 276 → 29;
`-num_notes` Seite alice 389 → 88; `tags__id__none` alice Seite 98 → 11, COUNT 344 → 99;
`custom_field_query` alice COUNT 431 → 66; `owner__id__none` COUNT ~250 → 11–27. Unverändert (Scan
über Inhalt/Prüfsummen bzw. Fremdtabellen): `title__icontains`, `has_duplicates`,
`correspondent__name`, `shared_by__id`, `mime_type`.

**Server** (Debug-Binary, dieselbe Binary mit `legacy`/`ast`, verschränkt 2 Runden, Median, 32
Anfragen der Dokumentliste aus `tests/pgvalidate/pgv_requests.py` inkl. `selection_data`, Detail,
„Alle auswählen“; `tests/l1/l1_bench.sh`, Tabelle `results/l1/bench_table.md`; parallel lief der
Schreib-Paritätslauf, Werte daher verrauscht). Summe je Nutzer:

| Nutzer | legacy ohne Index | ast ohne Index | legacy mit Index | **ast mit Index** |
|---|---:|---:|---:|---:|
| admin (Superuser) | 33,4 s | 29,4 s | 19,4 s | **19,2 s** (−43 %) |
| alice (Gruppe) | 35,1 s | 26,1 s | 19,3 s | **17,3 s** (−51 %) |
| bob | 29,5 s | 24,3 s | 14,8 s | **13,9 s** (−53 %) |

Antwortkörper legacy = ast für alle Anfragen außer den Gleichstands-Sortierungen (s. o.; dort schwankt
schon `legacy` zwischen den Runden).

**1M-Bestand** (`pl_scale_1m`, nur lesend, ohne den neuen Index, weil die DB nicht verändert werden
darf; Messung lief neben einer Lastmessung eines anderen Agenten; `tests/l1/time1m.py`,
`results/l1/sql_1m_table.md`): Standardliste COUNT admin 4,8 → 3,8 s, user003 4,2 → 3,5 s;
`-num_notes` COUNT user003 4,7 → 2,2 s; `tags__id__all=6` Seite user003 163 → 55 ms. Ohne den Index
bleibt COUNT ein Seq Scan; mit Index wäre er (wie bei 100k) ein Index-Only-Scan – auf 1M nicht
gemessen, weil der Index dort nicht angelegt werden darf.

## L3: Trigramm-Indizes (2026-11-13)

Migration `pl_store.0002_paperrust_doc_trgm`: GIN-Indizes `paperrust_doc_title_trgm` /
`paperrust_doc_content_trgm` über `UPPER(title::text)` / `UPPER(content::text)` (genau Djangos
`icontains`-Ausdruck). Der SQL-Bauer (`pl_store::docsql::Builder::with_trigram`) ersetzt
Titel-/Inhaltsblätter dann durch `id IN (<Kandidaten über den Index> UNION ALL …)`. Das Ergebnis
bleibt gleich, weil pg_trgm jede Kandidatenzeile mit dem `LIKE` nachprüft.

Wann die Trigramm-Form gilt (`docsql::trigram_for`): beide Indizes gültig, jedes Titel-/Inhaltsblatt
hat einen Wortteil aus ≥ 3 Buchstaben/Ziffern **und** trifft höchstens 1000 Zeilen
(`TRGM_MAX_CANDIDATES`). Die Zahl wird wirklich gezählt: `count(*)` über `LIMIT 1001` mit dem
Indexausdruck, seltene Wörter ~20–60 ms, häufige 0,1–0,6 s, 5 Minuten je Datenbank/Spalte/Muster
zwischengespeichert. Sonst die bisherige Scan-Form mit Faltung. Die Planer-Schätzung (erster
Versuch) taugt dafür nicht: ANALYZE lässt Werte > 1 kB (fast jeden Inhalt) aus der Statistik weg,
„Beitrag“ wurde auf 100 Zeilen geschätzt, tatsächlich sind es 70 310. Das Dashboard-Widget mit
Korrespondent + „Beitrag“ wurde dadurch 0,5 s → 12 s langsamer. Grund (Nachmessung 1 Mio., PERF-SCALE §9): Bei häufigen Wörtern muss jeder Kandidat mit
dem Kollations-`UPPER()` nachgeprüft werden, und die Kandidatenmenge verhindert den frühen Abbruch
bei `LIMIT`. Das war langsamer als der Scan: `title_content=rechnung` 39 s → 80 s, v2-Text ohne
Zählung 59 ms → 96 s, Dashboard-Widgets mit Textfilter 0,06–0,5 s → 20 s. Seltene Wörter bleiben
schnell (42 s → 8 ms).

## L3b: Trefferspeicher für häufige Suchwörter (2026-11-13)

`pl_store::textcache`, eingebunden in `docsql::v1_list` und `docsql::v1_selection` (nicht in die
v2-Liste ohne Zählung). Gilt, wenn das Prädikat **nur** aus Titel-/Inhaltsblättern besteht
(`docsql::text_only`, z. B. `title_content=…` allein) und die Trigramm-Form nicht gewählt wurde
(häufiges Wort oder Index fehlt). Abschalten: `PAPERRUST_TEXT_HIT_CACHE=off`.

- Aufbau je Datenbank/Spalte/Lookup/Suchtext: `SELECT id FROM documents_document WHERE
  <Faltungsform auf der eigenen Spalte>` über **alle** Zeilen in einer `REPEATABLE READ`-
  Transaktion, deren erste Abfrage `pg_current_snapshot()` liest. Gespeichert: Bitmaske der ids
  (Bit i = id i), `xmax` und `xip` des Schnappschusses. Höchstens 32 Einträge, je 15 Minuten,
  nur ids < 2^26; immer nur ein Aufbau gleichzeitig.
- Abfrage: je Blatt auf der äußeren Zeile `CASE WHEN <Zeile unverändert> THEN get_bit(<Maske>, id)
  ELSE <Textbedingung> END`; unverändert = `age(xmin) > age(xmax)` und `xmin` nicht in `xip`.
  Inhalt: Wurzeln mit lebender Version prüfen wie bisher den effektiven Inhalt direkt.
- Gleichheit: Eine Zeilenfassung, deren `xmin` im Schnappschuss bestätigt war und die noch lebt,
  hatte im Schnappschuss denselben Text (jedes UPDATE schreibt eine neue Fassung mit neuer `xmin`).
  Neue/geänderte Zeilen werden am Text geprüft, gelöschte fehlen. `xmin` zeigt auch bei
  eingefrorenen Zeilen die ursprüngliche Transaktion (PostgreSQL ≥ 9.4, mit PostgreSQL 18 und
  `VACUUM FREEZE` geprüft). Altersüberlauf nach 2^31 Transaktionen kann nur „geändert“ vortäuschen
  (dann Nachprüfung). Test: `crates/pl-store/tests/textcache_db.rs` (Änderungen nach dem Aufbau in
  offener und in bestätigter Transaktion, Papierkorb, neue Zeile, neue Versionen; `#[ignore]`, DB-Kopie).
- Mit Treffermenge meldet `v1_list` keine Inhaltssuche mehr (`fetch_page` statt
  `fetch_page_windowed`): Seite über den Sortierindex mit frühem Abbruch, Zählung getrennt.
- Kosten: der erste Aufruf je Suchtext liest den Text wie bisher einmal (plus ≈ 1 s); danach liest
  die Zählung nur noch die Hauptzeilen ohne Text. Messung: docs/PERF-SCALE.md §12.
- **Eigenes Datenmodell (Stufe 1, 2026-11-14, PERF-SCALE §13):** `documents_document` ist dort eine
  Sicht ohne Systemspalten – die bisherige Form (`"documents_document"."xmin"`) brach mit
  „column documents_document.xmin does not exist“ (500 bei `title_content=<häufiges Wort>`). Jetzt
  (`textcache::layout`): Menge über `paperrust_document_content` (id = `document_id`). Die Abfrage
  verknüpft je Menge die seit dem Schnappschuss neuen/geänderten Textzeilen
  (`LEFT JOIN (SELECT unnest(ARRAY(SELECT document_id FROM paperrust_document_content WHERE NOT
  <xmin bestätigt>)) AS document_id) AS "hit_ch<i>"`, einmal je Abfrage, liest die Zeilenfassungen
  ohne Text) und prüft `CASE WHEN "hit_ch<i>".document_id IS NOT NULL THEN <Textbedingung, Text per
  Unterabfrage aus der Texttabelle> ELSE get_bit(<Maske>, id) END`. Damit sind auch Änderungen
  erfasst, die nur die Texttabelle schreiben. Die Textspalte der Sicht wird in der äußeren Zeile
  nicht berührt (ihr Join entfällt). Voraussetzung: eine Textzeile verschwindet nur zusammen mit
  ihrem Dokument (`ON DELETE CASCADE`; kein Schreiber löscht sie einzeln) – ein Dokument ohne
  Textzeile zählt wie im Schnappschuss. Titel bekommen dort keine Menge (schmale Tabelle, direkte
  Prüfung billig). `v1_list` meldet mit einer solchen Menge wieder „Inhaltssuche“
  (`fetch_page_windowed`: Seite und Zählung in einem Durchlauf, sonst läse jede der beiden
  Unterabfragen die Zeilenfassungen). Der Schlüssel des Zwischenspeichers enthält den Aufbau
  (keine Menge des alten Aufbaus nach einer Umbaustufe).
- **Paralleler Aufbau (2026-11-14):** bis zu `PAPERRUST_TEXT_HIT_CACHE_WORKERS` (Standard:
  Prozessorkerne, höchstens 8) eigene Verbindungen, je ein Blockbereich (`ctid`-Bereich, mindestens
  4 096 Blöcke je Verbindung), alle mit dem Schnappschuss der führenden Transaktion
  (`pg_export_snapshot()` / `SET TRANSACTION SNAPSHOT`) – dieselbe Menge wie eine einzige
  Abfrage. Scheitert ein Teil, ermittelt die führende Transaktion alles allein. Test: Bitmaske mit
  3 Verbindungen == mit einer (`textcache_db.rs`, beide Aufbauten).
- **Aufbau-Bedingung (2026-11-15, PERF-SCALE §14):** mit Faltung und ASCII-Suchtext prüft der
  Aufbau `u = UPPER(x COLLATE "C")` (einmal je Zeile, Unterabfrage mit `OFFSET 0`):
  `u op P ODER (u enthält ein relevantes Sonderzeichen UND replace-Kette(u) op P)` – mit **allen**
  relevanten Sonderzeichen, also ohne den Wechsel auf die Kollations-`UPPER()` ab mehr als
  `MAX_REPLACES` (`textcache::upper_once_cond`, Beweis im Code). Ergebnis gleich der Faltung;
  die Nachprüfung geänderter Zeilen in der Abfrage bleibt `folded_lookup`.
- **ODER-Reihenfolge (2026-11-15):** mit Treffermengen stellt `Builder::pred` die Blätter mit
  Menge im ODER nach vorn (kommutativ; der Titelvergleich läuft nur für Zeilen ohne Inhaltstreffer).
