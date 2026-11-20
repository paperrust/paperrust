# Vertrag: pl-classifier

Reine Bibliothek (keine Routen, keine DB): Ersatz für den scikit-learn-Klassifikator von upstream
(automatische Zuordnung `MATCH_AUTO` und `/api/documents/{id}/suggestions/`), Baustein des
Python-freien Workers. Quelle: paperless-ngx v3.4.2 (`upstream/`), scikit-learn 1.9.1, numpy 2.4.1,
scipy 1.17.0, tantivy (Python) 0.26.2 – Versionen aus `upstream/uv.lock`, CPython 3.12 (`reference/venv`).

**Wichtig:** upstream v3.4.2 benutzt **kein NLTK** mehr. Die Vorverarbeitung läuft über einen
tantivy-Analyzer (`FORMAT_VERSION 11`, „Tantivy text preprocessing“). Es werden keine NLTK-Daten gebraucht.

## Module

| Modul | Upstream-Quelle | Inhalt |
|---|---|---|
| `text` | `documents/classifier.py` (`RE_WORD`, `_text_analyzer`, `preprocess_content`), `paperless/settings/__init__.py` (`CLASSIFIER_LANGUAGES`, `_get_classifier_language_setting`), `documents/models.py` (`Document.suggestion_content`) | `preprocess_content`, `analyze`, `Language` (+ `from_ocr_language`), `suggestion_content` |
| `stopwords` | tantivy 0.26.0 `tokenizer/stop_word_filter/stopwords.rs`, `documents/_snowball_stopwords.py` | Snowball-Stoppwortlisten, unverändert (Lizenz siehe unten) |
| `vectorizer` | `classifier.py` `train()` + sklearn `feature_extraction/text.py` | `CountVectorizer` (fit_transform/transform_one), `Csr` |
| `mlp` | sklearn `neural_network/_multilayer_perceptron.py`, `_stochastic_optimizers.py`, `_base.py` | `Mlp::fit` (Adam, Mini-Batches, Abbruchregel wie sklearn), `MlpParams` |
| `nb` | – (Vergleichsmodell) | Multinomial Naive Bayes |
| `model` | `classifier.py` (`DocumentClassifier`, `_predict_with_threshold`, `load_classifier`), `tasks.py` (`train_classifier`) | `DocumentClassifier::{train, predict_*, predict_all, save, load}`, `training_hash`, `load_classifier`, `Predictor` |
| `metrics` | – | Qualitätsmaß (identisch zu `gen_reference.py`) |

## 1. Vorverarbeitung (`preprocess_content`) – muss exakt gleich sein

`settings.CLASSIFIER_LANGUAGE` = `CLASSIFIER_LANGUAGES[OCR_LANGUAGE.split("+")[0]]`:
`dan→danish, nld→dutch, eng→english, fin→finnish, fra→french, deu→german, ita→italian,
nor→norwegian, por→portuguese, rus→russian, spa→spanish, swe→swedish`, sonst `None`
(Standard `PAPERLESS_OCR_LANGUAGE=eng` → `english`). Rust: `Language::from_ocr_language`.

1. `unicodedata.normalize("NFC", content)` (Rust: `unicode-normalization`).
2. Sprache `None`: alle Treffer von Python-`re` `\b[\w]+\b`, je Treffer `str.lower()`, mit `" "`
   verbunden. Python-`\w` = `str.isalnum()` oder `_` = Kategorien L* und N* plus `_` (Rust
   `[\p{L}\p{N}_]+`; ein gieriger Klassen-Treffer ist genau ein maximaler Lauf = `\b…\b`).
3. Sprache gesetzt: tantivy-py `TextAnalyzerBuilder(Tokenizer.regex(r"\w+"))` →
   `Filter.lowercase()` → Stoppwörter → `Filter.stemmer(language)`; Ausgabe `.analyze()` mit `" "`
   verbunden. Details aus tantivy 0.26.0:
   - Regex-Tokenizer: Rust-`regex`-`\w` (Alphabetic + M + Nd + Pc + Join_Control – **anders** als Python-`\w`).
   - LowerCaser: ASCII → `make_ascii_lowercase`, sonst `char::to_lowercase` je Zeichen
     (**ohne** Final-Sigma-Regel: `Σ` → `σ`).
   - Stoppwörter: Englisch = Snowball-Liste aus `_snowball_stopwords.py`, jedes Wort mit `RE_WORD`
     zerlegt (`don't` → `don`, `t`); andere Sprachen tantivys Snowball-Listen.
     Vergleich nach dem Kleinschreiben, exakt.
   - Stemmer: Snowball über `rust-stemmers 1.2.0` (dieselbe Version steckt in tantivy-py 0.26.2).
   - Lange Tokens bleiben, Akzente werden nicht gefaltet.
4. Caching: upstream cached `_text_analyzer` je Sprache (`functools.cache`) und das Ergebnis von
   `_vectorize` 5 min im `read-cache` (Schlüssel sha256(content)|FORMAT_VERSION|Sprache|Vektorisierer-Hash,
   signiertes Pickle). Rust: Analyzer je Sprache in `OnceLock`; **kein** Ergebnis-Cache (reine
   Funktion, Ergebnis identisch; Cache ist Sache des Aufrufers). `predict_all` vektorisiert einmal für alle vier Ziele.
5. Vorhersage-Eingabe ist `document.suggestion_content`: Inhalt (`get_effective_content`, bei
   Wurzeldokumenten der Inhalt der neuesten Version) – über 1 200 000 Zeichen die ersten 800 000 + `" "`
   + die letzten 200 000 (Codepoints). Rust: `suggestion_content()`; die Auswahl der Version macht der Aufrufer.

## 2. Merkmale (CountVectorizer) – exakt gleich

`CountVectorizer(analyzer="word", ngram_range=(1, 2), min_df=0.01)`, übrige Defaults von sklearn 1.9.1:
`lowercase=True` (ganzes Dokument `str.lower()`), `token_pattern=r"(?u)\b\w\w+\b"` (Python-`\w`,
mind. 2 Zeichen), `max_df=1.0`, `max_features=None`, `binary=False`, `dtype=int64`.
- Merkmale je Dokument: erst alle Unigramme, dann Bigramme `"a b"` aufeinanderfolgender Tokens.
- Behalten, wenn `df >= min_df * n_docs` (Float-Vergleich, z. B. 747 Dokumente → ≥ 7,47 → 8)
  und `df <= n_docs`; Vokabular nach Codepoints sortiert (Spaltenreihenfolge).
- Fehler wie sklearn: leeres Vokabular → `"empty vocabulary; perhaps the documents only contain stop words"`,
  nichts übrig → `"After pruning, no terms remain. Try a lower min_df or a higher max_df."`.
- `stop_words_` wird upstream nach dem Fit verworfen (nur Speicher).
- Transformation bei Vorhersage: unbekannte Terme werden ignoriert, Werte = rohe Zählungen (keine Normierung).

## 3. Training (`DocumentClassifier.train`, Task `train_classifier`)

**Auslöser:** Celery-Beat „Train the classifier“, `PAPERLESS_TRAIN_TASK_CRON` (Standard `5 */1 * * *`,
Ablauf 59 min), manuell `document_create_classifier`, `POST /api/tasks/run/` (TaskType `train_classifier`).
`train_classifier`: gibt es **kein** Tag/Typ/Korrespondent/Speicherpfad mit `matching_algorithm=6 (AUTO)`
→ Ergebnis `"No automatic matching items, not training"`, vorhandene Modelldatei wird gelöscht.
Sonst `load_classifier()` (oder neu), `train()`; `True` → `save()`, Ergebnis
`"Training completed successfully"`, `False` → `"Training data unchanged"`. **Diese Task-Hülle und
die DB-Abfrage gehören dem Worker**, die Bibliothek bekommt fertige `TrainingDoc`s.

**Dokumente (Aufrufer):** `Document.objects.exclude(tags__is_inbox_tag=True).order_by("pk")` –
Standard-Manager ohne Papierkorb (soft-deleted), **Versionen (root_document gesetzt) zählen mit**,
Dokumente mit irgendeinem Posteingangs-Tag fallen raus. Keine Dokumente → `ValueError("No training data available.")`
(`ClassifierError::NoTrainingData`).

**Labels je Dokument:** Typ/Korrespondent/Speicherpfad = pk, wenn gesetzt **und** dessen
`matching_algorithm == MATCH_AUTO`, sonst `-1`; Tags = pks der Tags des Dokuments mit `MATCH_AUTO`,
nach pk sortiert (Nicht-AUTO-Tags zählen nicht).

**Neu-Training nötig?** sha256 über alle Labels als 4-Byte-Little-Endian (signed) je Dokument in
pk-Reihenfolge: Typ, Korrespondent, jeder Tag, Speicherpfad. `latest = max(modified)`. Kein
Training, wenn `last_doc_change_time >= latest` **und** Hash gleich (dann setzt upstream nur die
Cache-Schlüssel `classifier_modified`/`classifier_hash`/`classifier_version` für 50 min) → `Ok(false)`.
Labels über `i32` → upstream `OverflowError` → `ClassifierError::LabelOutOfRange`.
Inhalte werden erst danach in Blöcken zu 1000 pks geholt (`content`-Rückruf); fehlt ein Dokument
inzwischen → `""` (Rückruf liefert `None`).

**Anzahlen:** `num_tags` = verschiedene Tags; `num_x = |set(labels) ∪ {-1}| - 1`.

**Modelle (upstream):** alle `MLPClassifier(tol=0.01, random_state=0)` mit sklearn-Defaults
(1 versteckte Schicht 100 ReLU, Adam lr 0.001, β 0.9/0.999, ε 1e-8, alpha 1e-4, Batch min(200, n),
Mischen je Epoche, max 200 Epochen, Abbruch wenn der Trainingsverlust 11 Epochen nicht um ≥ tol
gegenüber dem besten sinkt; Glorot-Uniform-Init). Trainingsreste (`_optimizer`, `_best_coefs`,
`_best_intercepts`) werden vor dem Speichern entfernt.
- **Tags** (`num_tags > 0`): `num_tags == 1` → `LabelBinarizer` über `[tag oder -1]` (binär; haben
  alle Dokumente den Tag: eine Klasse → Tag wird immer vorhergesagt); sonst `MultiLabelBinarizer`
  (ein logistischer Ausgang je Tag). **Ohne** Stichprobengewichte.
- **Korrespondent / Typ / Speicherpfad** (`num_x > 0`): einlabelig über die Klassen inkl. `-1`,
  `sample_weight = compute_sample_weight("balanced", y)` = `n / (n_klassen · count(klasse))`.
  1 Klasse → ein Nullspalten-Ausgang (Wahrscheinlichkeit der Klasse = 1 − p), 2 Klassen → ein
  logistischer Ausgang, > 2 → Softmax.
- `num_x == 0` → Klassifikator `None` (Vorhersage `None` bzw. `[]`).

**Statusmeldungen** (`status_callback`) zeichengleich: `Gathering data from {n} document(s)...`,
`Vectorizing document content...`, `Training tags classifier ({n} tag(s))...`,
`Training correspondent classifier ({n} correspondent(s))...`,
`Training document type classifier ({n} type(s))...`, `Training storage path classifier ({n} path(s))...`.

Training ist immer **vollständig** (kein inkrementelles Lernen), wie upstream.

## 4. Vorhersage

- `predict_correspondent/document_type/storage_path` = `_predict_with_threshold(clf, X, CLASSIFIER_MATCH_THRESHOLD)`:
  `predict_proba`, bei 1 Klasse nur Spalte 0; beste Klasse (erste bei Gleichstand); `-1` → `None`;
  `threshold > 0` und Wahrscheinlichkeit `< threshold` → `None` (genau gleich = angenommen).
  `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD`, Standard **0.3**; 0.0 schaltet ab. Rust: Parameter `threshold`
  bzw. `Predictor { classifier, threshold }` (Signaturen wie `pl_matching::matching::Classifier`).
- `predict_tags`: `predict()` (logistisch `> 0.5`), zurück über den Binarizer; binär → `[tag]` oder `[]`.
  **Kein** Schwellwert für Tags.
- Verwendung: `matching.match_correspondents/_document_types/_tags/_storage_paths` (AUTO-Objekt
  passt, wenn `pk == Vorhersage` bzw. `pk in Vorhersage`, Kandidaten rechtegefiltert), Consumer
  (`load_classifier()` einmal je Einlesen, dann die Zuordnungs-Signale), `document_retagger`,
  `/api/documents/{id}/suggestions/` (Antwort `correspondents/tags/document_types/storage_paths/dates`,
  Cache mit `classifier_version`/`classifier_hash`).

## 5. Modelldatei

Upstream: `PAPERLESS_MODEL_FILE` (Standard `DATA_DIR/classification_model.pickle`), Inhalt
`HMAC-SHA256(SECRET_KEY, pickle) || pickle` des Tupels `(FORMAT_VERSION=11, last_doc_change_time,
last_auto_type_hash, data_vectorizer, tags_binarizer, tags_classifier, correspondent_classifier,
document_type_classifier, storage_path_classifier)`; atomar über `<datei>.part` + Umbenennen.
`load_classifier`: Datei fehlt → `None`; falsche Version oder sklearn-Versionswarnung → Datei löschen,
`None` (neu trainieren); HMAC/Pickle kaputt → Datei löschen, `None`; IO-Fehler → `None`, Datei bleibt;
`raise_exception=True` reicht den Fehler weiter.

**Rust liest das Pickle nicht** und trainiert ein eigenes Modell. Eigenes Format (`FORMAT_VERSION = 1`):
`HMAC-SHA256(key, payload) || payload`, `payload = "PAPERRUST-CLASSIFIER\n" || JSON` (serde; Gewichte
als Base64 von f32-LE). Gleiche Fehlerlogik (`load_classifier(path, key, raise_exception)`), gleicher
atomarer Schreibweg. Vorschlag Dateiname für den Worker: eigene Datei neben dem Pickle, z. B.
`DATA_DIR/paperrust_classifier.model` (nie dieselbe Datei wie upstream, damit Django sie nicht löscht).

## 6. Qualitätsmessung und Modellentscheidung

Bit-Gleichheit mit sklearn ist nicht das Ziel (anderer Zufallsgenerator, f32). Gemessen wird die
**Vorhersagequalität** auf einem synthetischen, deterministischen Korpus
(`tests/parity/pl-classifier/gen_reference.py`, Seed 20261108): 1000 Dokumente DE/EN/FR, 16
Korrespondenten (2 nicht AUTO), 9 Typen (1 nicht AUTO), 6 Speicherpfade (1 nicht AUTO), 10 AUTO-Tags +
1 Nicht-AUTO-Tag + Posteingangs-Tag (7 %), OCR-Rauschen, 4–5 % falsche Labels. Training auf
pk % 5 ≠ 0 (747 Dokumente nach Posteingangs-Ausschluss), Test auf pk % 5 = 0 (200 Dokumente).
Upstream-Code läuft **unverändert** (Django/ORM/Cache durch In-Memory-Stubs ersetzt).

Maße (in `metrics.rs` und im Skript identisch): je Einzelziel Trefferquote (None zählt als Wert) und
Makro-F1 über echte Klassen; Tags Mikro-F1, Makro-F1, exakte Übereinstimmung der Tag-Menge.

Ergebnisse (Testmenge, Schwellwert 0.3):

| Modell | Sprache | Korr. acc / F1 | Typ acc / F1 | Pfad acc / F1 | Tags mikro / makro / exakt | Modell |
|---|---|---|---|---|---|---|
| sklearn MLP seed 0 (upstream) | english | 0.920 / 0.935 | 0.795 / 0.770 | 0.720 / 0.765 | 0.874 / 0.853 / 0.625 | – |
| sklearn MLP seeds 1–4 (Spanne) | english | 0.910–0.915 / 0.922–0.932 | 0.795–0.815 / 0.770–0.782 | 0.680–0.710 / 0.737–0.757 | 0.871–0.880 / 0.846–0.861 / 0.605–0.630 | – |
| **Rust MLP** (Standard) | english | 0.915 / 0.924 | 0.790 / 0.759 | 0.695 / 0.748 | 0.872 / 0.847 / 0.615 | 2.1 MiB |
| Rust linear (MLP ohne versteckte Schicht) | english | 0.810 / 0.839 | 0.710 / 0.721 | 0.650 / 0.703 | 0.752 / 0.705 / 0.385 | 0.2 MiB |
| Rust Naive Bayes | english | 0.640 / 0.662 | 0.610 / 0.592 | 0.325 / 0.377 | 0.617 / 0.599 / 0.080 | 0.3 MiB |
| sklearn MLP seed 0 | None | 0.905 / 0.915 | 0.785 / 0.757 | 0.705 / 0.763 | 0.869 / 0.846 / 0.610 | – |
| **Rust MLP** | None | 0.905 / 0.919 | 0.775 / 0.747 | 0.685 / 0.735 | 0.862 / 0.840 / 0.595 | 2.3 MiB |
| Rust linear | None | 0.810 / 0.839 | 0.710 / 0.711 | 0.675 / 0.721 | 0.737 / 0.701 / 0.365 | 0.2 MiB |
| Rust Naive Bayes | None | 0.630 / 0.635 | 0.605 / 0.582 | 0.305 / 0.366 | 0.572 / 0.560 / 0.040 | 0.3 MiB |

Zusatzläufe (`cargo test -p pl-classifier --test parity model_comparison -- --ignored --nocapture`, ~12 min Debug):

| Modell | Sprache | Korr. acc / F1 | Typ acc / F1 | Pfad acc / F1 | Tags mikro / makro / exakt |
|---|---|---|---|---|---|
| Rust MLP seed 1 | english | 0.900 / 0.916 | 0.795 / 0.769 | 0.700 / 0.752 | 0.872 / 0.845 / 0.605 |
| Rust MLP seed 2 | english | 0.895 / 0.917 | 0.805 / 0.785 | 0.685 / 0.736 | 0.866 / 0.843 / 0.590 |
| Rust linear lr 0.01, tol 1e-4 | english | 0.875 / 0.889 | 0.760 / 0.726 | 0.630 / 0.692 | 0.845 / 0.832 / 0.540 |
| Rust MLP seed 1 | None | 0.895 / 0.909 | 0.785 / 0.753 | 0.725 / 0.760 | 0.863 / 0.842 / 0.595 |
| Rust MLP seed 2 | None | 0.915 / 0.924 | 0.805 / 0.771 | 0.705 / 0.750 | 0.866 / 0.849 / 0.595 |
| Rust linear lr 0.01, tol 1e-4 | None | 0.870 / 0.885 | 0.760 / 0.726 | 0.620 / 0.669 | 0.841 / 0.833 / 0.540 |

Übereinstimmung der Korrespondent-Vorhersagen Rust-MLP ↔ sklearn je Dokument: 95.5–98 %.

**Toleranz (Test `quality_against_sklearn`):** jedes der 8 Maße der Rust-MLP darf höchstens
**0.03 absolut** unter dem Mittel der sklearn-Läufe (Seed 0–4) liegen. Begründung: sklearn selbst
schwankt allein durch `random_state` um bis zu 0.04 (Pfad-Trefferquote 0.680–0.720).

**Entscheidung: Standard bleibt das MLP wie upstream** (`ModelKind::Mlp`). Die einfacheren Modelle
sind auf diesem Korpus deutlich schlechter (linear −0.11 Korrespondent, −0.12 Tags-Mikro-F1, auch
mit angepasster Lernrate noch −0.04/−0.03 und Pfad −0.07; Naive Bayes −0.28/−0.26) – „gleiche Qualität“ ist nicht gegeben, also kein Wechsel. Determinismus ist kein
Argument: auch das Rust-MLP ist deterministisch (eigener Seed-RNG, kein Thread-Parallelismus). RAM ist
klein genug (siehe 7); die Alternativen bleiben als `ModelKind::Linear`/`NaiveBayes` wählbar (z. B. für
sehr große Bestände), sind aber nicht Standard.

## 7. Laufzeit und Speicher (grob)

Debug-Build (nur `cargo test`, kein Release erlaubt), Laptop unter Last durch andere Agenten:

| | sklearn (Python, nativ) | Rust MLP (debug) |
|---|---|---|
| Training 747 Dok., 4 Modelle, ~1000 Merkmale | 11–25 s | 75–95 s |
| Vorhersage je Dokument (alle 4 Ziele) | 2.0–2.2 ms (inkl. 4× Vektorisieren) | 1.8–3.1 ms |
| Speicher Training | tracemalloc-Spitze 21 MB (nur Python-/numpy-Heap; Prozess-RSS nicht gemessen, allein die Importe von numpy/scipy/sklearn liegen deutlich darüber) | **17.4 MB Spitze Working Set des ganzen Testprozesses** (Korpus-JSON laden + Training + 200 Vorhersagen), gemessen per PowerShell-Abtastung alle 200 ms |
| Modellgröße | – | 2.1–2.3 MiB (JSON+Base64) |

Rechenaufwand Rust-MLP: je Batch dünnbesetzte Matrix × (F×100) vorwärts/rückwärts plus Adam über
alle F×100 Gewichte (wie sklearn). Speicher beim Training: Gewichte, Adam-Momente m/v und Gradientenpuffer
= 4 × F × 100 × 4 Byte (F = 1000 → 1.6 MB; F = 50 000 → 80 MB) je Modell, Modelle nacheinander;
dazu die CSR-Matrix (nnz × 8 Byte) und während des Fits das volle n-Gramm-Vokabular wie sklearn.
Release dürfte nach Erfahrung 10–30× schneller sein (nicht gemessen, `--release` verboten).

## Known differences

1. **Modell nicht bitgleich** (eigener RNG xoshiro256**, f32 statt f64): andere Gewichte, andere
   Einzelvorhersagen (2–4.5 % der Korrespondenten), gleiche Qualität im Rahmen der sklearn-Seed-Streuung.
2. **Eigenes Modellformat** statt Pickle; Rust liest/schreibt keine `classification_model.pickle`.
   Solange Django `suggestions` bedient, nutzt es weiter sein Pickle – die Route bleibt auf Django,
   bis der Rust-Worker trainiert und `pl-documents` die Rust-Vorhersage nutzt.
3. **Sprache im Modell gespeichert:** Rust verarbeitet bei der Vorhersage mit der Trainingssprache vor.
   Upstream nimmt die *aktuelle* Einstellung (Wechsel von `PAPERLESS_OCR_LANGUAGE` ohne Neu-Training
   ergäbe dort ein unpassendes Vokabular). Die Neu-Training-Prüfung ist wie upstream (ignoriert die Sprache).
4. **Kein Vektor-Cache** (`read-cache`, 5 min): Ergebnis identisch, nur Rechenzeit.
5. **Einklassen-Fall Naive Bayes** (nur wenn `ModelKind::NaiveBayes` gewählt): konstant mit
   Wahrscheinlichkeit 1.0. Das Standard-MLP trainiert den Fall wie sklearn (1 − p, Schwellwert greift).
6. **Unicode-Version:** Python 3.12 (Unicode 15.0) vs. Rust `regex`/`std`/`unicode-normalization`
   (Unicode 16): Abweichungen nur bei in Unicode 16 neu vergebenen Zeichen. Python-`isnumeric()` zählt
   zusätzlich Unihan-Zahlzeichen – die sind Kategorie Lo und damit ohnehin in `\p{L}`.
7. Cache-Schlüssel `classifier_modified/hash/version` (Django-Cache) setzt die Bibliothek nicht;
   `last_doc_change_us`/`last_auto_type_hash` stehen im Modell. K2 (2026-11-18,
   `pl-documents/src/suggestions.rs`): ETag `"<last_auto_type_hash>:<NUMBER_OF_SUGGESTED_DATES>"` und
   Last-Modified (`last_doc_change_time`) kommen direkt aus dem geladenen Modell (upstream aus dem
   Django-Cache, nur wenn der Klassifikator in den letzten 50 min geprüft/trainiert wurde – hier immer,
   solange die Modelldatei existiert und die Formatversion passt). Vorschlags-Cache im Speicher des
   Web-Prozesses, 50 min, Treffer verlängert; gültig nur bei gleicher Version/Hash, gleichem
   `modified` des Dokuments, gleichem Nutzer und gleicher Datumsanzahl (upstream: ein Eintrag je
   Dokument für alle Nutzer, gelöscht über `clear_document_caches`).

## Lizenzen

- Stoppwortlisten: Snowball-Projekt, BSD-3-Clause (Kopf in `src/stopwords.rs` erhalten; englische
  Liste aus upstream `_snowball_stopwords.py`, ebenfalls Snowball BSD-3-Clause). Übernommen über
  tantivy 0.26.0 (MIT).
- `rust-stemmers 1.2.0`: MIT / BSD-3-Clause, reines Rust (Snowball-generierter Code).
- Kein NLTK, keine NLTK-Daten.

## Tests

- Unit-Tests (`cargo test -p pl-classifier`): Analyzer-Fälle aus upstream `test_classifier.py`
  (Stoppwörter, Unterstrich, alle Sprachen, leerer Inhalt), Sprachzuordnung, `suggestion_content`,
  Vektorisierer (Sortierung, 1-Zeichen-Tokens, min_df-Grenze, Fehler), MLP/linear/NB lernen,
  Einklassen- und Ein-Tag-Fälle, Schwellwert-Semantik, Hash-Layout, Speichern/Laden/HMAC/Version/Löschen.
- `tests/parity.rs`: upstream-Beispieldateien (`samples/content.txt` → `preprocessed_content*.txt`),
  27 Randfälle × 13 Sprach-Einstellungen exakt, alle 1000 Korpus-Dokumente × 13 Einstellungen exakt
  (SHA-256-Präfix), Vokabular + Zeilen-nnz/-Summen + Hash + letzte Änderung exakt (english, None),
  Qualität Rust-MLP (english) innerhalb der Toleranz; `model_comparison` (`#[ignore]`, langsam im Debug-Build).
- Referenz neu erzeugen: `reference/venv/Scripts/python.exe -I -X utf8 tests/parity/pl-classifier/gen_reference.py`
  (~3 min, < 300 MB).
