# Lücken gegenüber paperless-ngx 3.4.2 (Funktionen, nicht API-Routen)

Erste Fassung 2026-11-18 (Stand Commit `c1ddc97`, 51 Lücken). **Neu geprüft 2026-11-18** auf dem Stand
Commit `cc5697c` plus uncommittete Arbeit (KI-Ausbau, Integration). **Restlücken bearbeitet 2026-11-18**
(Stand `5f5de7c` plus uncommittete Arbeit): E3, E6, E13, E18, W8, W9, K1, S2 geschlossen (begründeter
Rest unter §9), C4 als bewusste Abweichung. Quelle der Wahrheit: `upstream/`
(Code + docs). Jede Zeile wurde im Rust-Code gelesen (nicht nur Commit-Texte); Fundstellen gelten für
diesen Stand. Ausgeführt wurden die Unit-/Integrationstests der Crates (siehe Abschlussbericht), keine
Ende-zu-Ende-Läufe je Lücke.

## Zusammenfassung (einfach gesagt)

Von den 51 Lücken sind **50 geschlossen** und **1 bewusste Abweichung** (C4); keine ist mehr teilweise
oder offen. Wo ein Rest bleibt, steht er mit Begründung in §9 („Bewusste Abweichungen“). Geschlossen sind alle zehn
wichtigsten Punkte der ersten Fassung: Geraderichten und Säubern, Mail-PDF mit Layout, alte Office-Formate,
PDF-Fassung für Tabellen/Präsentationen, HEIC, „Passwort entfernen“, Suchindex nach Sprachwechsel, Azure-
Remote-OCR, E-Mail-Bestätigung `mandatory` und fremde Anmelde-Anbieter (die Anmeldeseite fällt nicht mehr aus).

Zuletzt geschlossen: fehlende Schriften im PDF/A-Archiv werden durch metrisch gleiche Ersatzschriften
eingebettet (E3), `unpaper_args` und wertabhängige OCRmyPDF-Optionen wirken (E6), Office-Text und
-Metadaten wie Tika inkl. Excel-Zellformaten (E13), TIFF mit G4/JPEG wird unverändert übernommen (E18),
Relativdaten in 21 Sprachen (W8), `list.append`/`%*d`/`{0.attr}` (W9), unscharfe Treffer, Rekursion und
Lookbehinds mit Alternativen (K1), Discord und Gitea als Anmelde-Anbieter (S2). Bewusst nicht: ein lokales
HuggingFace-Modell im Prozess (C4) – es bräuchte ein großes Fremdsystem und viel Arbeitsspeicher.

| Größe | Bedeutung | gesamt | geschlossen | teilweise | offen |
|---|---|---|---|---|---|
| S | klein (Stunden bis 1 Tag) | 28 | 28 | 0 | 0 |
| M | mittel (einige Tage) | 17 | 17 | 0 | 0 |
| L | groß (Wochen, neuer Renderer o. Ä.) | 6 | 5 | 0 | 0 (C4: bewusste Abweichung) |
| | **gesamt** | **51** | **50** | **0** | **0** (+1 bewusst) |

Hinweis zu Hilfsprogrammen: E8 (HEIC), E9–E12 (Office/Mail-PDF) brauchen `paperrust-pdftool` (im
Docker-Image enthalten); M1 (GPG) braucht `gpg`, E17 (Barcodes ohne PDFium) braucht `pdftoppm` – beide
**nicht** im Docker-Image. Sie zählen als geschlossen, weil sie ohne das Programm mit klarer Meldung
scheitern statt still.

Neu seit der ersten Fassung (keine ngx-Lücke, zur Einordnung): KI-Texterkennung (Vision-OCR) ist jetzt in
Einlesen und Neuverarbeiten eingebunden (pl-core `vision`, pl-consumer `Consumer::apply_vision`,
pl-server `worker::AiVisionOcr`; Feld `ai_ocr` beim Hochladen und in `reprocess`/`bulk_edit reprocess`).
Seit 2026-11-18 mit Schalter „KI-Texterkennung“ in beiden Oberflächen (Hochladen, Neu verarbeiten im
Dokument-Menü und in der Mehrfachauswahl; nur sichtbar, wenn KI und Bildmodell an sind).

Legende Status: **geschlossen** = Verhalten wie upstream (kleine, benannte Abweichungen möglich; ein
bewusst nicht umgesetzter Rest steht in §9) · **bewusste Abweichung** = absichtlich nicht umgesetzt,
Begründung in der Zeile und in §9 · **teilweise**/**offen** kommen nicht mehr vor.

---

## 1. Einlesen, Parser, OCR, Archiv-PDF, Vorschaubilder

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| E1 | Geraderichten `OCR_DESKEW` (+ AC `deskew`) | `paperless/parsers/tesseract.py` → ocrmypdf `deskew` | **geschlossen** | `crates/pl-ocr/src/preprocess.rs:359`, verdrahtet `pl-ocr/src/native.rs:143-145,708-719`, AC `pl-ocr/src/config.rs:369` | – | M |
| E2 | Säubern `OCR_CLEAN` clean/clean-final (+ AC `unpaper_clean`) | `tesseract.py` → ocrmypdf (unpaper) | **geschlossen** | `pl-ocr/src/preprocess.rs:419,695-708`; `native.rs:969-1000` (clean-final ersetzt das Seitenbild); AC `config.rs:365` | – | M |
| E3 | Echtes PDF/A-Archiv | ocrmypdf + Ghostscript | **geschlossen** | `pl-ocr/src/pdfwrite.rs` (`pdfa::normalize`, Farben, Selbstprüfung); `pl-ocr/src/pdfa.rs:157` → `pl-ocr/src/pdfa_fonts/mod.rs:221` (fehlende Standardschrift → metrisch gleiche Liberation-Schrift einbetten, wie Ghostscript) | Ersatzschriften aus `PAPERRUST_PDFA_FONT_DIR` bzw. `/usr/share/fonts/liberation` (im Docker-Image); ohne sie (Windows-Desktop), bei Symbol-/CID-Schriften und unbekannten Zeichennamen weiter ohne PDF/A-Kennzeichnung mit Warnung (§9); veraPDF nicht gelaufen (§11) | L |
| E4 | `OCR_COLOR_CONVERSION_STRATEGY` (+ AC) | ocrmypdf/Ghostscript | **geschlossen** | `pl-ocr/src/color.rs:42-56`, angewendet `pdfwrite.rs:1038`, gelesen `native.rs:282-285` | wirkt wie upstream nur bei PDF/A; CMYK → RGB mit Warnung | M |
| E5 | Archiv-Verkleinerung (`optimize`, JBIG2) | ocrmypdf-Standard | **geschlossen** | `pl-ocr/src/optimize.rs:220`, `pl-ocr/src/jbig2.rs`, `native.rs:318-331` | verlustbehaftetes JBIG2 nicht (auch in OCRmyPDF abgekündigt) | M |
| E6 | `OCR_USER_ARGS` mit beliebigen ocrmypdf-Optionen | `tesseract.py` | **geschlossen** | `pl-ocr/src/native.rs:51,70-95` (wertabhängige Prüfung), `:335` + `pl-ocr/src/preprocess.rs` (`CleanFilters::from_unpaper_args`) | `unpaper_args` schaltet die Filter des eingebauten Säuberns; `pdf_renderer`/`rasterizer`/`sidecar_format` still, wenn der Wert dem eingebauten Verhalten entspricht, sonst Warnung; bewusst nicht: `fast_web_view`, `remove_background` (nur Warnung, upstream bricht ab), Plugins (§9) | M |
| E7 | Remote-OCR Azure Document Intelligence | `paperless/parsers/remote.py` | **geschlossen** | `pl-ocr/src/remote.rs:160-194,229-310`; Registrierung `pl-consumer/src/remote.rs:81`; AC-Spalten `pl-consumer/src/consumer.rs` (`load_app_config`); Workflow-Aktion `pl-consumer/src/workflows.rs:621`; Neuverarbeiten `reprocess.rs` | – | M |
| E8 | HEIC-Bilder | `tesseract.py` (pillow_heif) | **geschlossen** | `pl-consumer/src/native.rs:116-126,230-235` (HEIC → PNG im Kindprozess), `pl-consumer/src/office.rs:99` | braucht `paperrust-pdftool`; 4:2:2/4:4:4 und Bildfolgen nicht (troubleshooting.md) | M |
| E9 | Alte Office-Formate .doc/.xls/.ppt | `paperless/parsers/tika.py` | **geschlossen** | `pl-parsers/src/office/mod.rs:61` (`UNSUPPORTED_LEGACY` leer), neuer OLE2-Leser `pl-parsers/src/office/ole.rs`; Archiv-PDF `pl-consumer/src/office.rs` | – | L |
| E10 | Archiv-PDF, Seitenzahl, Vorschau für xlsx/pptx/ods/odp/odg | `tika.py` (Gotenberg) | **geschlossen** | `pl-consumer/src/office.rs:63-65,81-91` (`Op::Slides`); Vorschau/Seitenzahl aus dem Archiv `pl-consumer/src/native.rs:328-341,355` | braucht `paperrust-pdftool` (Standard an, im Image) | L |
| E11 | Archiv-PDF für docx/odt/rtf immer | `tika.py` (Gotenberg) | **geschlossen** | `pl-consumer/src/office.rs:41-43` (`PAPERRUST_OFFICE_TO_PDF` Standard true), `docker/Dockerfile` liefert das Werkzeug | – | S |
| E12 | E-Mail → PDF inkl. HTML, Layouts | `paperless/parsers/mail.py` | **geschlossen** | `pl-parsers/src/mail/mod.rs:468-471` (`PdfLayout`), `pl-parsers/src/mail/pdf.rs`; Regel-Layout bzw. `EMAIL_PARSE_DEFAULT_LAYOUT` `pl-consumer/src/native.rs:79-85,263-265` | – | L |
| E13 | Office-Text und -Metadaten wie Tika | `tika.py` | **geschlossen** | `pl-parsers/src/office/numfmt.rs:491` (`format_cell`: Datum, Uhrzeit, Prozent, Tausender, eigene Formate), genutzt in `office/ooxml.rs` und `office/ole.rs`; Metadaten (u. a. `dc:publisher`, Schreibschutz, `custom:*`) in `office/ooxml.rs` | Zeilenumbrüche auch in Zellen per Test; wissenschaftliche Formate und Brüche → „General“; Tikas Laufangaben (`X-TIKA:*`, `Content-Length`, `resourceName`) bewusst nicht (§9); Schlüsselnamen nicht gegen echten Tika geprüft (§11) | S |
| E14 | Text digitaler PDFs in Lesereihenfolge | `paperless/parsers/utils.py::extract_pdf_text` | **geschlossen** | `pl-parsers/src/pdftext.rs:1-16,330-389`, genutzt in `pl-parsers/src/pdf.rs:125-145` | Positionssortierung je Seite, auch bei redo/mixed | M |
| E15 | Vorschaubild-Rückfall über Reparatur | `make_thumbnail_from_pdf_qpdf_fallback` | **geschlossen** | `pl-consumer/src/native.rs:128-167` (lopdf-Reparatur, dann PdfCraft, dann Standardbild) | – | S |
| E16 | Ordnerüberwachung mit Dateiereignissen (`POLLING_INTERVAL=0`) | `document_consumer.py` (watchfiles) | **geschlossen** | `pl-consumer/src/watcher.rs:348-366,502-560` (`notify`, Abfrage nur als Rückfall) | – | S |
| E17 | Barcodes ohne PDFium | `documents/barcodes.py` (poppler) | **geschlossen** | `pl-consumer/src/consumer.rs` (`Consumer::from_env`, Rückfall) + `pl-consumer/src/consumer/barcode_raster.rs` (`pdftoppm`) | `pdftoppm` nicht im Docker-Image (nur ohne PDFium relevant) | S |
| E18 | TIFF→PDF wie img2pdf | `documents/converters.py` | **geschlossen** | `pl-pdfops/src/convert.rs:188,245,257,418` (CCITT-G4 und JPEG unverändert übernommen; DPI je Bild, EXIF-Drehung) | mehrere Streifen, Kacheln, gespiegelte Seiten → verlustfrei Flate; JPEG-Durchreichung zusätzlich zu img2pdf (A4-Farbscan geschätzt 4,7 → 0,95 MB); G4-Darstellung nicht im Viewer geprüft (§11) | S |
| E19 | `CONSUMER_DISABLE` | `docker/rootfs/.../svc-consumer/run` | **geschlossen** | `pl-consumer/src/watcher.rs:333-337,486-489` | Befehl `document_consumer` bleibt nutzbar | S |

## 2. Workflows

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| W1 | Aktion „Passwort entfernen“ (Typ 5) | `workflows/actions.py::execute_password_removal_action` | **geschlossen** | `pl-server/src/worker.rs` (`install_password_removal`), `pl-documents-write/src/workflow_password.rs`, Consumer-Pfad `pl-consumer/src/workflows.rs:1046-1048` | – | M |
| W2 | „Dokument geändert“ für das Stammdokument bei neuer Version | `documents/consumer.py` | **geschlossen** | `pl-consumer/src/consumer.rs` (`document_updated` für die Wurzel nach dem Speichern) | – | S |
| W3 | WebSocket `document_updated` nach Bearbeiten | `signals/handlers.py` | **geschlossen** | `pl-documents-write/src/hooks.rs:140-155`, `update.rs:249`, `versions.rs:115` | – | S |
| W4 | „KI-Vorschläge anwenden“ bei „Geändert“ über die API | `signals/handlers.py` | **geschlossen** | `pl-documents-write/src/workflows.rs:91-97,178`; eingereiht `update.rs:248`, `versions.rs:114` | – | S |
| W5 | Datei umbenennen/verschieben nach KI-Vorschlägen | `workflows/ai.py` | **geschlossen** | `pl-server/src/worker.rs` (`ConsumerDocWriter`: `update_filename_and_move_files` nach jedem Schreiben; Benutzerfelder über pl-documents-write `cfi`) | nur mit Feature `ai` | S |
| W6 | E-Mail/Webhook bei „Geändert“ mit Zustand in der Transaktion | `signals/handlers.py` | **geschlossen** | `pl-documents-write/src/workflows.rs:167-168` (`email_in`/`webhook_in` auf derselben Verbindung), `pl-mail/src/effects/mod.rs:204-220` | 120-s-Wartezeit nur noch auf dem anderen, asynchronen Weg | S |
| W7 | Audit bei Feld-Zuweisung per Workflow | `workflows/mutations.py` | **geschlossen** | `pl-consumer/src/workflows.rs:1116-1145` (alter Wert + `custom_field_instance_updated`) | – | S |
| W8 | Relativdaten in Feld-Abfragen (alle Sprachen) | dateparser | **geschlossen** | `pl-docfilter/src/relative_dp.rs` (Nachbau des dateparser-Ablaufs mit den Originaldaten von dateparser 1.4.3 für 21 Sprachen, `relative_dp_data.rs`; geprüft gegen dateparser selbst, `tests/w8_dateparser.rs`) | Sprachen ohne Leerzeichen (ja/zh/ko/th) und die übrigen dateparser-Sprachen bewusst nicht (§9) | M |
| W9 | Volle Jinja2-Vorlagen | `templating/environment.py` | **geschlossen** | `pl-templating/src/pylist.rs` (`list.append`/`extend`/`insert`/`pop`/`remove`/`clear`/`reverse`/`sort`, eingebunden in `env.rs` `prepare`), `filters.rs` (`percent_format`: `%*d`, `%.*f`), `methods.rs` (`resolve_field`: `{0.attr}`, `{0[key]}`, `{:>{w}}`) | Listen aus dem Kontext bleiben unveränderlich, `sort(key=…)` Fehler, keine Dict-Mutation (pl-templating.md KD 4, §9) | M |

## 3. Mail-Abruf

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| M1 | GPG-Entschlüsselung (`EMAIL_ENABLE_GPG_DECRYPTOR`, `EMAIL_GNUPG_HOME`) | `paperless_mail/preprocessor.py` | **geschlossen** | `pl-mail/src/fetch/gpg.rs:26-56,188-234`, verdrahtet `pl-mail/src/fetch/handler.rs:372`, Einstellungen `pl-mail/src/fetch/mod.rs:44-62` | braucht `gpg` (nicht im Image); ohne → Fehler im Log, nicht still | M |

## 4. Klassifikator und Zuordnung

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| K1 | Regex mit Python-`regex`-Syntax und `MATCH_REGEX_TIMEOUT_SECONDS` | `documents/matching.py` | **geschlossen** | `pl-matching/src/pyext.rs` (unscharf `(?:text){e<=k}` mit k ≤ 3, Rekursion `(?R)`/`(?N)`/`(?&name)` bis Tiefe 10, Lookbehinds mit Alternativen fester Länge), eingebunden in `regex_compat.rs:201` (`prepare_pattern`); echtes Zeitlimit | unscharf auf Klassen/Quantoren, Fehlerkosten, Lookbehinds mit `+`/`*`/`{m,n}`, `(?-1)`, `(?r)` bewusst weiter ungültig (Fehler-Log, kein Treffer; §9) | M |
| K2 | ETag/Cache für Vorschläge | `documents/conditionals.py`, `caching.py` | **geschlossen** | `pl-documents/src/suggestions.rs:17-26,219-275,289-362` | – | S |

## 5. Aufgaben und Zeitpläne

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| A1 | Abgelaufene Freigabe-Bündel aufräumen | `tasks.py::cleanup_expired_share_link_bundles` | **geschlossen** | `pl-server/src/worker.rs` (Handler) → `pl-queue/src/maintenance.rs:38` | – | S |
| A2 | Suchindex neu bauen, wenn Sprache/Schema nicht passt | `init-search-index/run` | **geschlossen** | `pl-server/src/worker.rs` (`reindex_if_needed` beim Start), `pl-indexer/src/writer.rs:208` | nur im Worker-Betrieb | S |

Ebenfalls im Worker (neu, keine ngx-Aufgabe): `paperrust.ai.backfill` (KI-Nachbearbeitung des Archivs,
`pl_ai::server::jobs::run_job_reporting`), vorher im Web-Prozess.

## 6. Verwaltungsbefehle und Export/Import

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| B1 | `document_archiver [-f] [-d ID]` | `document_archiver.py` | **geschlossen** | `pl-cli/src/archiver.rs`, `pl-cli/src/lib.rs:95` | – | S |
| B2 | `document_fuzzy_match [--ratio] [--delete]` | `document_fuzzy_match.py` | **geschlossen** | `pl-cli/src/fuzzy.rs`, `lib.rs:97` | – | M |
| B3 | `document_llmindex {rebuild,update,compact,migrate}` | `document_llmindex.py` | **geschlossen** | `pl-cli/src/llmindex.rs:34-79`, `lib.rs:98` | ohne Feature `ai` klare Fehlermeldung | S |
| B4 | `document_consumer [ordner] [--oneshot]` | `document_consumer.py` | **geschlossen** | `pl-cli/src/consumer.rs:44-63`, `lib.rs:96` | – | S |
| B5 | `manage.py check` | `paperless/checks.py`, `documents/checks.py` | **geschlossen** | `pl-cli/src/check.rs`, `lib.rs:99` | – | M |
| B6 | Export-ZIP mit bzip2/lzma/zstd | `documents/export/compression.py` | **geschlossen** | `pl-cli/src/export/zip.rs:6-7,29-57,94,233` | – | S |
| B7 | `--processes` bei archiver/thumbnails | diverse | **geschlossen** | `pl-cli/src/par.rs:1-25`, `thumbnails.rs:25-28`, `archiver.rs:42,58` | – | S |

## 7. Anmeldung und Sicherheit

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| S1 | `ACCOUNT_EMAIL_VERIFICATION=mandatory` | allauth `EmailVerificationStage` | **geschlossen** | `pl-accounts/src/flow.rs:214-235,250-271`, `pl-accounts/src/emailaddr.rs:211,249` | Konto ohne Mail-Adresse wartet auf Bestätigung (Warnung `flow.rs:230`) | M |
| S2 | Social-Login außer OIDC | allauth `SOCIALACCOUNT_PROVIDERS` | **geschlossen** | `pl-accounts/src/social.rs` (`PROVIDER_CLASSES`: GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea), `pl-accounts/src/oauth2.rs:33,76`; andere nur Warnung | Keycloak, Authentik, Okta, Slack, LinkedIn u. a. über OIDC; Facebook (appsecret_proof), Apple, Twitter (OAuth1) bewusst nicht (§9); Discord/Gitea nur mit Unit-Tests (§11) | L |
| S3 | Seite „Registrierung geschlossen“ | allauth `CloseableSignupMixin` | **geschlossen** | `pl-accounts/src/signup.rs:50` → `pl-accounts/src/socialaccount.rs:515-516` | – | S |
| S4 | Login-Drossel/TOTP-Sperre über alle Prozesse | Django-Cache (Redis) | **geschlossen** | `pl-core/src/cache.rs:11-18,505-550` (Schutz-Schlüssel in der gemeinsamen Tabelle auch beim lokalen Cache) | – | S |
| S5 | Hasher Argon2, BCryptSHA256, Scrypt | Django `PASSWORD_HASHERS` | **geschlossen** | `pl-authz/src/hashers.rs:166-168,215-240` | – | M |
| S6 | `csrfmiddlewaretoken`-Formularfeld | Django CSRF | **geschlossen** | `pl-authz/src/csrf.rs:524-537,650` | – | S |
| S7 | Client-IP im Log fehlgeschlagener Anmeldungen | `paperless/signals.py` (ipware) | **geschlossen** | `pl-authz/src/clientip.rs`, genutzt `pl-accounts/src/auth.rs:111`, `pl-authz/src/authn.rs:441` | – | S |

## 8. Einstellungen und Betrieb

| # | Funktion | upstream-Quelle | Status | Fundstelle (heute) | noch offen / Hinweis | Größe |
|---|---|---|---|---|---|---|
| C1 | `PAPERLESS_*_FILE` (Docker-Secrets) | `init-env-file/run` | **geschlossen** | `pl-core/src/config.rs` (`plan_file_env`, `apply_env_aliases`), aufgerufen `pl-server/src/bin/paperrust.rs:8` | – | S |
| C2 | `FORCE_SCRIPT_NAME` mit Präfix-Abschneiden | settings + granian | **geschlossen** | `pl-server/src/serve.rs:34-35,52-56,80-109` (`strip_script_name`) | nur ganze Pfadsegmente; live hinter Proxy ungeprüft | S |
| C3 | `MODEL_FILE` | settings | **geschlossen** | `pl-system/src/status.rs:499,610-612,777-788` (Status prüft das tatsächlich geladene Modell) | `PAPERLESS_MODEL_FILE` bewusst ohne Wirkung (Pickle-Format) | S |
| C4 | KI-Einbettungen HuggingFace | `paperless_ai/embedding.py` | **bewusste Abweichung** | `pl-ai/src/embedding.rs` (über TEI bzw. HF Inference API; die Meldung ohne Endpunkt nennt den Ausweg Ollama mit `all-minilm`) | kein lokales Modell im Prozess: bräuchte ONNX/candle + tokenizers, ~90 MB Modell und 150–300 MiB mehr Arbeitsspeicher – gegen die Vorgabe „1. Geschwindigkeit, 2. Speicher“ (§9) | L |

Einstellungen: alle 44 Felder von ApplicationConfiguration werden gelesen und wirken; Reste ohne Wirkung
sind nur noch die bewussten aus Abschnitt 9 sowie einzelne `OCR_USER_ARGS`-Schlüssel (E6).

---

## 9. Bewusste Abweichungen (keine Lücken)

Entscheidung „weg von paperless-ngx, Umstieg leicht halten“ – diese Punkte sind gewollt:

- **Kein Celery, Redis, Django, granian, s6, Flower:** Warteschlange, Zeitplan und Cache in PostgreSQL.
  Variablen `REDIS`, `REDIS_PREFIX`, `READ_CACHE_*`, `DB_READ_CACHE_ENABLED`, `CHANNELS_BACKEND`,
  `CACHE_BACKEND` (abgebildet), `APPS` (nur für Provider gelesen), `ENABLE_FLOWER`, `SUPERVISORD_WORKING_DIR`,
  `WEBSERVER_WORKERS`, `STATIC_URL` sind wirkungslos. Kein `/admin/`, kein allauth-headless, kein
  `invalidate_cachalot`.
- **Kein Tika/Gotenberg, kein OCRmyPDF/Ghostscript/poppler/zxing/pikepdf:** ersetzt durch eigene
  Office-Leser, WordCraft (`paperrust-pdftool`), Tesseract, PDFium, lopdf, rxing. `TIKA_*` wirkungslos.
- **Nur PostgreSQL:** kein SQLite/MariaDB, daher kein `convert_mariadb_uuid`.
- **Keine Python-Plugins** (Datums-Parser- und Parser-Einstiegspunkte).
- **Eigenes Klassifikator-Format** statt Pickle (nach Umstieg neu trainieren; daher `MODEL_FILE` ohne Wirkung).
- **Regex mit Backtrack-Limit plus Zeitlimit** statt Python-`regex`; die Sondersyntax aus K1 ist teilweise
  nachgebaut, der Rest (unscharf auf Klassen/Quantoren, Fehlerkosten, Lookbehinds mit `+`/`*`/`{m,n}`,
  `(?-1)`, `(?r)`) bräuchte einen Nachbau des `regex`-Moduls – ungültig mit Fehler-Log.
- **PDF/A-Ersatzschriften (E3)** nur für die Standardschriften (Helvetica/Arial, Times, Courier über
  Liberation); Symbol-/CID-Schriften und Systeme ohne Liberation-Dateien behalten das Archiv ohne
  PDF/A-Kennzeichnung (Warnung) statt einer falschen Kennzeichnung.
- **OCRmyPDF-Optionen (E6):** `fast_web_view` (lopdf kann nicht linearisieren), `remove_background`
  (nur Warnung statt Abbruch) und Plugins bleiben ohne Wirkung.
- **Tika-Laufangaben (E13)** `X-TIKA:*`, `Content-Length`, `resourceName` beschreiben den Tika-Lauf, nicht
  das Dokument – nicht nachgebildet.
- **Relativdaten (W8)** in 21 Sprachen; Sprachen ohne Leerzeichen zwischen Wörtern (ja/zh/ko/th) und die
  übrigen dateparser-Sprachen nicht.
- **Jinja (W9):** Listen aus dem Kontext unveränderlich, `sort(key=…)` und Dict-Mutation nicht (Sandbox).
- **Anmelde-Anbieter (S2):** Facebook, Apple, Twitter (OAuth1) nicht; OIDC-fähige Anbieter über OIDC.
- **Lokale HuggingFace-Einbettungen (C4):** kein Modell im Prozess (großes Fremdsystem, viel Speicher);
  Einbettungen über einen Dienst (Ollama, TEI, HF Inference API).
- **Größere Seiten** werden mit niedrigerer Auflösung erkannt statt abgelehnt.
- **GPG-verschlüsselte Dokumente:** upstream in v3 entfernt; PaperRust lehnt solche Alt-DBs beim Umstieg ab.
- **Prozessmodell:** Standard nur HTTP; Zeitplan, Mail und Ordnerüberwachung nur mit `--worker`.
- **Zusätzliches** (nicht in ngx): manueller Workflow-Auslöser, Geräte-Tokens, Scanner-Anbindung,
  PDF-Werkzeuge, Ransomware-Schutz, `clear_expired_sessions`, KI-Texterkennung, KI-Nachbearbeitung.

## 10. Widersprüche in der eigenen Doku – abgearbeitet 2026-11-18

Aus der ersten Fassung:

- docs/SETTINGS.md §1.9 (`REMOTE_OCR_*`, `OCR_COLOR_CONVERSION_STRATEGY` als wirksam): **kein Widerspruch
  mehr**, beides wirkt (E4, E7).
- docs/SETTINGS.md (`EMAIL_PARSE_DEFAULT_LAYOUT`, `ENABLE_GPG_DECRYPTOR`, `EMAIL_GNUPG_HOME`,
  `CONSUMER_DISABLE` als Klasse X): **schon korrigiert** (Klassen A/B/D).
- docs/SETTINGS.md §1.7 „0 = Ereignisse“: **stimmt jetzt** (E16).
- docs/user/configuration.md (`mandatory`): wirkt jetzt; **ergänzt** um den Hinweis auf Mailserver und
  Mail-Adresse.
- docs/INTEGRATION-TODO.md: „Nicht portierte Tasks“, „OIDC portieren“, `PdfOps`, APPLY_AI_SUGGESTIONS
  **abgehakt**; „OCR offen“ auf veraPDF und Trait-Textextraktion gekürzt; FORCE_SCRIPT_NAME als umgesetzt,
  nur live ungeprüft vermerkt.
- docs/contract/pl-cli.md `mail_fetcher` „Stub“ → **fertig**; docs/contract/pl-authz.md KD 4
  (Remote-User) → **erledigt** vermerkt.

Beim Neuprüfen zusätzlich gefunden und **korrigiert**:

- docs/user/troubleshooting.md: Polling-Aussage, `OCR_CLEAN`/`OCR_DESKEW` „nur Warnung“, „Office ohne
  Vorschau-PDF“.
- docs/user/configuration.md: „keine PDF-Fassung von Office-Dateien“ (zweimal), „bei PDF/A nur
  Metadaten“, „HEIC nicht lesbar“, `CONSUMER_POLLING_INTERVAL=0`, „Verschlüsselte Mails“,
  `MATCH_REGEX_TIMEOUT_SECONDS`/`ENABLE_GPG_DECRYPTOR`/`EMAIL_GNUPG_HOME`/`CONSUMER_DISABLE` in der
  Liste „ignoriert“, Anbieterliste bei `SOCIALACCOUNT_PROVIDERS`.
- docs/user/administration.md und docs/user/en/administration.md: `document_archiver`,
  `document_fuzzy_match`, `document_consumer`, `document_llmindex`, `check` als fertig; „Befehl
  `document_llmindex` gibt es nicht“ entfernt.
- docs/user/advanced.md: „GPG-Entschlüsselung noch nicht umgesetzt“.
- docs/SETTINGS.md `PAPERRUST_OFFICE_TO_PDF`: Formatliste vervollständigt.

Noch nicht angepasst (bekannt): docs/contract/pl-ocr.md:371-376 beschreibt die Lesereihenfolge noch als
Abweichung (`-layout` fehlt), seit `pl-parsers/src/pdftext.rs` weitgehend überholt; englische
Nutzerdoku außer administration.md nicht durchgesehen.

## 11. Nicht geprüft

Nicht live ausprobiert: echte PDF/A-Konformität (kein veraPDF, auch nicht mit Ersatzschriften), G4-Seiten
aus TIFF im Viewer, Tika-Schlüsselnamen gegen echten Tika, Discord/Gitea gegen die echten Dienste (Quelltext gegen allauth 65.20.1 geprüft), Qualität rxing vs. zxing und PDFium vs.
poppler, Azure-Remote-OCR gegen den echten Dienst, OAuth-Refresh gegen Google/Microsoft, TLS bei
IMAP/SMTP, `FORCE_SCRIPT_NAME` hinter einem echten Proxy, GPG mit echtem Schlüsselbund, KI-Texterkennung
gegen ein echtes Bildmodell (nur Unit-Tests der Bausteine), Verhalten ohne Feature `ai`.
