# Upstream-Updates übernehmen (paperless-ngx → PaperRust)

PaperRust folgt **genau einer** paperless-ngx-Version (derzeit v3.4.2, Submodul `upstream/`). Eine
neue upstream-Version wird als eigener, geprüfter Schritt übernommen – nie nebenbei. Grundsatz wie
beim Port: erst Verhalten festnageln (Vertrag, Paritätstest), dann Code.

`upstream/` wird nie von Hand geändert; es wird nur der Submodul-Stand angehoben.

## 1. Vorbereiten: Was ändert sich?

```sh
git -C upstream fetch --tags
scripts/upstream-diff.sh v3.4.2 v3.5.0          # Beispiel-Versionen
UPSTREAM_DIFF_SCHEMA=1 scripts/upstream-diff.sh v3.4.2 v3.5.0   # zusätzlich OpenAPI-Schemas erzeugen
```

Bericht: `results/upstream-diff/<alt>..<neu>.md` (+ `.json`). Er enthält:

1. **OpenAPI-Diff** (Pfade, Methoden, Parameter, Felder),
2. **neue/geänderte/entfernte Migrationen**,
3. **geänderte Python-Dateien → betroffene Crates** über `UPSTREAM_MAP.md` (nicht zugeordnete
   Dateien gesondert gelistet – dort `UPSTREAM_MAP.md` ergänzen),
4. **geänderte Frontend-Dienste/-Modelle** (`src-ui`: was die Oberfläche sendet/erwartet),
5. **rote Ampel** (Exit 1): Celery-Einstellungen (upstream; in PaperRust nachgebaut über die
   PostgreSQL-Warteschlange und den Zeitplaner), tantivy-Version, Suchschema
   (`documents/search/_schema.py`, `_tokenizer.py`), API-Versionen (`ALLOWED_VERSIONS`,
   `DEFAULT_VERSION`). Jeder rote Punkt braucht eine ausdrückliche Entscheidung.

Daraus eine Arbeitsliste je Crate machen (Route/Funktion, upstream-Quelle, erwartete Änderung).

## 2. Submodul anheben (auf einem eigenen Zweig)

```sh
git switch -c upstream-v3.5.0
git -C upstream checkout v3.5.0
git add upstream
```

Danach `README.md`, `docs/ARCHITECTURE.md`, `AGENTS.md`, `docs/user/*` und die Kopfkommentare
`@v3.4.2` erst ändern, wenn der Schritt abgeschlossen ist (sie beschreiben den geprüften Stand).

## 3. Referenzumgebung neu aufsetzen

Die Paritätstests vergleichen gegen ein laufendes paperless-ngx der **neuen** Version
(`reference/README.md`): Python-Abhängigkeiten der neuen Version, Frontend aus dem offiziellen
Release-Archiv (SHA-256 prüfen und in `reference/README.md` vermerken), Testdaten neu einspielen
(`reference/seed.sh`, `reference/snapshot.sh`). Schema-Abzug für `pl-dbmigrate`/`pl-cli`:

- `crates/pl-dbmigrate/schema/` (pre-/post-data aus `pg_dump` einer frisch migrierten DB),
- `crates/pl-cli/baseline/` (`tests/parity/pl-cli/gen_baseline.sh`: ContentTypes, Permissions,
  angewandte Migrationen, Sequenzen),
- `reference/schema/<tag>.json` (OpenAPI).

## 4. Rust anpassen – Crate für Crate

Für jede betroffene Route bzw. Funktion:

1. `docs/contract/<crate>.md` an die neue upstream-Quelle anpassen (Felder in Reihenfolge,
   Statuscodes, Fehlertexte, Nebenwirkungen; Quelle mit neuem Tag).
2. Code ändern, Kopfkommentar `// upstream: …@v<neu>`.
3. Neue Migrationen: Schema-Abzug (Schritt 3) und `migrate`-Erkennung (`pl-cli`, `pl-dbmigrate`
   `classify`) um die neue Migrationsliste erweitern; die alte Version bleibt als Quelle für
   `paperrust-migrate-db` zulässig, solange unterstützt.
4. Neue Routen erst einhängen (in `all_routes()`), wenn ihr Paritäts-Diff 0 ist; bis dahin antwortet
   der Pfad 404. Eine Weiterleitung an Django gibt es seit 2026-11-14 nicht mehr; `routes.toml` kann
   eine fertige Route nur noch abschalten (`"off"`).
5. Neue `PAPERLESS_*`-Variablen in `pl-core::config` und in `docs/user/configuration.md`
   (Deutsch und Englisch, Anker in beiden Schreibweisen) aufnehmen; die englische Referenz lässt
   sich aus `upstream/docs/configuration.md` neu erzeugen (Skripte des Doku-Agenten R3:
   mkdocs → GitHub-Markdown, Anker `<a id>`, PaperRust-Statuszeile je Variable).

## 5. Paritätstests

```sh
python tests/parity/harness/run_parity.py                 # alle Kataloge, PaperRust gegen Django
python tests/parity/harness/run_parity.py --crate pl-objects
tests/parity/w2-run.sh --crate <crate>                    # Schreibfälle auf privater DB-Kopie
TARGET=rust python tests/security/run_security.py         # Sicherheits-Suite
tests/upstream-adapter/run.sh compare                     # upstream-Tests über HTTP gegen PaperRust
```

Ausstieg: `different = 0` für alle aktivierten Routen; neue Abweichungen nur als „Known
differences“ im Vertrag bzw. freigegeben in `tests/parity/accepted.json`. Danach Browser-E2E
(`tests/e2e/`) mit und ohne Django und – bei Änderungen an Listen/Filtern – die
Postgres-Validierung (`tests/pgvalidate/`, docs/contract/pgvalidate.md).

## 6. Frontend-Patches neu anwenden

> **Entfällt seit 2026-11-13:** PaperRust liefert die Angular-Oberfläche nicht mehr aus, `frontend/` ist
> gelöscht. Oberflächen-Fixes von upstream werden – wenn gewünscht – in der eigenen Oberfläche
> `frontend-light/` nachgebaut (die Paritätsliste dazu wird intern geführt).
> Der Rest dieses Abschnitts beschreibt den früheren Ablauf.

Die Oberfläche bleibt upstream; PaperRust ändert sie nur über `frontend/patches/*.patch` und
`frontend/rebrand.mjs` (Agent R2, `frontend/README.md`).

```sh
bash frontend/build.sh            # kopiert upstream/src-ui, wendet Patches an, baut nach frontend/dist
```

- Passt ein Patch nicht mehr, bricht `build.sh` mit dem Namen des Patches ab → Patch gegen die
  neue `src-ui` neu erstellen (gleicher Inhalt, neue Kontextzeilen).
- **Doku-Links:** neue `docs.paperless-ngx.com`-Verweise suchen
  (`grep -rn "docs.paperless-ngx.com\|infoLink" upstream/src-ui/src/app`), in
  `frontend/doc-links.json` eintragen und in [docs/user/LINKMAP.md](user/LINKMAP.md) ergänzen;
  fehlende Seiten/Anker in `docs/user/` (und `docs/user/en/`) anlegen.
- Neue Konfigurationsschlüssel in `src-ui/src/app/data/paperless-config.ts` brauchen einen Anker
  in beiden `configuration.md`.
- Neue Texte mit „Paperless-ngx“ prüfen (`rebrand.mjs`), Lizenz-/Herkunftshinweis bleibt.

## 7. Abschluss

- `docs/PLAN.md` (Stand), `README.md` (Version, Messwerte nur nach neuer Messung),
  `docs/user/*` (Versionsangaben, neue Funktionen), `NOTICE` (Version), `AGENTS.md`.
- Kopfkommentare `@v3.4.2` → `@v<neu>` nur dort, wo die Quelle geprüft wurde.
- Hinweis für Nutzer in den Versionshinweisen: zuerst paperless-ngx (bzw. die Datenbank) auf die
  neue Version bringen; `paperrust manage migrate` lehnt Datenbanken anderer Stände ab.

<a id="uebernommene-einzel-fixes"></a>
## 8. Übernommene Einzel-Fixes (paperless-ngx `dev` nach v3.4.2)

Zwischen zwei Versionssprüngen übernimmt PaperRust einzelne upstream-Fixes aus `dev`, wenn sie
Sicherheit oder klare Fehler betreffen und **kein Schemawechsel** nötig ist (Ausnahme: Schritte des
eigenen Datenmodells L6 als PaperRust-Migration, z. B. #14612 unten). Referenz für Parität
bleibt v3.4.2; jede Abweichung steht hier, im Vertrag des Crates und – falls der Paritäts-Harness sie
sieht – als Regel in `tests/parity/accepted.json` (Grund „upstream fix <hash> after v3.4.2“).
Beim nächsten Versionssprung (Schritt 1–7) entfallen die Regeln, weil die Referenz die Fixes dann
selbst enthält.

| upstream | Thema | PaperRust (Dateien) | Test | Parität |
|---|---|---|---|---|
| fa83a4c5d (#14687) | Tag löschen prüft Rechte aller Nachfahren | `pl-objects`: `objects.rs` (`tag_ids_with_descendants`, `check_tag_descendants_deletable`), `handlers.rs` (destroy), `bulk.rs` (`delete` + Nachfahren) | `crates/pl-objects/tests/tag_delete_descendants.rs` (DB-Kopie, spiegelt `test_delete_checks_descendant_permissions`, 15 Fälle) | Referenzdaten ohne betroffene Konstellation – keine Regel |
| 22746aa20 (#14688) | Erst-Registrierung: Zeilensperre, nur ein Superuser | `pl-accounts`: `page.rs` (`first_install_locked`), `signup.rs`, `socialaccount.rs` (`save_social_user`) | `crates/pl-accounts/tests/first_user_lock.rs` (eigene Wegwerf-DB, spiegelt `test_concurrent_bootstrap_signups_create_only_one_superuser`) | keine sichtbare Abweichung |
| cb3513c6f (#14693) | `PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR` Standard `["mfa"]`, Trenner `,` | `pl-accounts`: `settings.rs` | `settings::tests::mfa_trusted_amr_default_and_list`; `tests/social_parity.rs` Variante a: `mfa.amr_trusted` nur noch Rust-Seite | W12-Social-Parität angepasst (kein Harness-Fall) |
| 0736c07d8 (#14710) | Statistik: `current_asn` nur über sichtbare Dokumente | `pl-system`: `statistics.rs` (bereits 2026-11-08 umgesetzt, `ASN_MAX_USES_FILTERED_DOCUMENTS`) | `crates/pl-system/tests/upstream_fixes.rs::statistics_current_asn_respects_document_permissions` | Regel `statistics-asn-filtered` (bestand) |
| 53d9aa34a (#14709) | `/api/config/` lesen braucht `view_applicationconfiguration` | `pl-system`: `config.rs` (`require_view` in list/retrieve); Schreiben/OPTIONS bleiben Django | `crates/pl-system/tests/upstream_fixes.rs::read_config_requires_view_permission` | Regeln `config-read-needs-view-perm-user1`/`-noperm` |
| e88303cc1 (#14698) | PDF/A-Umwandlung bei Mail-Archiven nur einmal | **nicht übernommen – entfällt**: der Rust-Mailparser erzeugt kein Archiv-PDF (kein Gotenberg-Weg, `can_produce_archive = false`); Mail-PDFs entstehen weiter in Django | – | – |
| c5239f9f7 (#14699) | Saved-View-Widget: Vor/Zurück-Warteschlange | `frontend/patches/10-upstream-saved-view-widget-queue.patch` (unverändert, inkl. Specs) | Patchprüfung gegen gepatchte `src-ui` (01–09) | – · **frontend-light**: `SavedViewWidget.svelte` hat dieselbe Navigation zu prüfen (Hinweis an den frontend-light-Besitzer) |
| 2e0d92d8f (#14717) + b7b0de100 (#14682) | Dropdown-Fixes, ID-Badge mobil | `frontend/patches/11-upstream-dropdown-badge-fixes.patch` | Patchprüfung | – |
| 5357be2d4 (#14597) | `selection_data` ohne jede Dokument-ID als SQL-Literal | **nicht nötig**: `pl-documents::views::selection_data_list` übergibt die IDs bereits als **einen** Text-Parameter (`unnest(string_to_array($1, ',')::bigint[])`) und wertet die gefilterte Menge nur einmal aus (pgvalidate Phase 6). Ausgabe/Reihenfolge bleibt v3.4.2 (upstream `dev` ordnet FK-Listen jetzt nach `Meta.ordering`) | bestehende Paritäts-/pgvalidate-Fälle | – |
| 45a2ac487 (#14673) | Dokument-Chat: nur ein Abruf je Frage | `pl-ai`: `server/chat.rs` (Synthese aus `top`, zweite Einbettung/Abfrage entfernt) | `tests/parity_w8.rs` (Django v3.4.2: zweite identische Embedding-Anfrage wird beim Vergleich entfernt, `chat_reqs_v342`) | W8-Parität angepasst |
| a4543c03d (#14674) | Lesesperre des LLM-Index vor dem Streamen freigeben | `pl-ai`: `server/chat.rs` (Sperre + Store nur im Abruf-Block) | wie oben (Verhalten), Sperre per Code-Review | – |
| e0c33b3d3 (#14723) | Chat: bis zu 5 Referenzen, kompakte Quellenliste | `pl-ai`: `chat.rs` (`MAX_CHAT_REFERENCES = CHAT_RETRIEVER_TOP_K`); `frontend/patches/12-upstream-chat-references.patch` + `frontend/i18n-new.json` („+N more“) | `chat::tests::refs_up_to_top_k`; `parity_w8.rs` vergleicht Trailer auf 3 gekürzt (`chat_same`) | W8-Parität angepasst |
| d14cfc93f (#14678) | LLM-Index: vec0 `chunk_size=32`, Seitengröße 64 KiB | `pl-ai`: `vector_store.rs` (`VEC0_CHUNK_SIZE`, `DB_PAGE_SIZE`; neue Datei/Tabelle bzw. bei Kompaktierung) | `vector_store::tests::new_index_uses_chunk_size_and_page_size`, `compact_moves_legacy_table_and_page_size` | Datei bleibt mit Django v3.4.2 (sqlite-vec 0.1.9) austauschbar |
| 3dbeca603 (#14704) | **Funktion** (keine Fehlerbehebung): Workflow von Hand auf ausgewählte Dokumente anwenden, Auslösertyp „Manual“ (5) | eigene Umsetzung (kein Code übernommen): `pl-matching` (`TRIGGER_MANUAL`), `pl-workflows` (`schema::TRIGGER_TYPES`, `filters.rs`: `id`/`enabled`/`trigger_type` auf `/api/workflows/`), `pl-documents-write` (`bulk_edit` Methode `run_workflow`, Rechte wie `set_permissions` + `view_workflow`), `pl-consumer` (`manual.rs`, `workflows::run_workflow_manual`/`workflow_by_id`), `pl-server` (`worker.rs`: Aufgabe `documents.tasks.run_workflows_for_documents`), `pl-queue` (`registry.rs`); frontend-light (klassisch: `BulkEditor`, `DocumentDetail`, `RunWorkflowConfirmDialog`; PaperRust-Design: `BulkBar`, `DocDetail`, `RunWorkflowSheet`; Workflow-Editor), `frontend/patches/14-paperrust-manual-workflow.patch` + `frontend/i18n-new.json`. Kein Schemawechsel (Typ ist nur ein neuer Wert in `smallint`) | `pl_workflows::filters::tests`, `pl_documents_write::bulk_edit::tests::run_workflow_method`, `pl_consumer::manual::tests`, `src/api/v1.test.ts` (frontend-light) | bewusste Erweiterung gegenüber v3.4.2, Vertrag pl-workflows Known differences 12–14; keine Harness-Regel (Referenzfälle enthalten weder Typ 5 noch `run_workflow`) |
| 8aa99a381 (#14612) | Tag-Baum als Adjazenzliste statt django-treenode | **eigene Migration** `pl_objects.0001_paperrust_tag_adjacency` + Code in `pl-objects`, `pl-consumer`, `pl-documents-write`, `pl-ai`, `pl-cli` (Abschnitt unten) | `crates/pl-objects/tests/tag_adjacency.rs`, `tagtree::tests`, `pl-cli export::tagtree::tests` | Regel `tag-adjacency-no-treenode-fields` |
| a139bdff4 (#14722) | Abgelaufene Sitzungen täglich löschen (Beat „Clear expired sessions“, `PAPERLESS_SESSION_CLEANUP_TASK_CRON`, Standard `0 3 * * *`, Durchgänge zu 500) | `pl-accounts`: `session.rs` (`clear_expired_sessions`); `pl-queue`: `beat.rs`, `registry.rs`; `pl-server`: `worker.rs` (Task); `pl-settings`: `schema.rs`, `help_en.rs`; Doku `docs/user/(en/)configuration.md` | `crates/pl-accounts/tests/clear_sessions.rs` (Wegwerf-DB, spiegelt `test_clears_only_expired_sessions_in_batches`: 501 + 1, zwei DELETEs; `#[ignore]`, Referenz-PG); `beat::tests::default_schedule_like_upstream_test` | keine (Beat/Worker nicht im Harness) |
| 00096e025 (#14735) | `ALLOWED_HOSTS`: `*` nur ohne `PAPERLESS_ALLOWED_HOSTS` **und** ohne `PAPERLESS_URL`; URL-Host normalisiert (Punkt am Ende, IPv6 in `[]`, `*`/`.…` ungültig) | `pl-core`: `config.rs` (`initial_allowed_hosts`, `allowed_host_from_url_hostname`); `pl-authz`: `config.rs`; `pl-server`: `config.rs`; Doku `docs/user/(en/)configuration.md` | `pl-core` `config::tests::allowed_hosts_at_startup_upstream_00096e025` (alle 9 Fälle + 5 ungültige URLs); `pl-authz` `config::tests::url_host_normalised_like_upstream_00096e025`; `pl-server` `config::tests::hosts_like_upstream` | Referenzumgebung setzt beides nicht bzw. `ALLOWED_HOSTS` explizit – keine Regel |
| d05e96cda (#14734) | OCR-`user_args` dürfen `input_file_or_options`, `output_file`, `sidecar` nicht setzen (API 400, beim OCR ignoriert) | `pl-ocr`: `args.rs` (`OCR_RESERVED_USER_ARGS`); `pl-system`: `config_write.rs` (`validate_user_args`) | `pl-ocr` `args::tests::ocr_user_args_cannot_override_reserved`; `pl-system` `config_write::tests` (reservierte Schlüssel) | kein Harness-Fall mit diesen Schlüsseln – keine Regel |
| 759f9cd22 (#14741) | Bild ohne Archiv-PDF: Vorschaubild direkt aus dem Bild statt Standardbild (auch `document_thumbnails`) | `pl-ocr`: `thumbnail.rs` (`make_thumbnail_from_image`); `pl-consumer`: `native.rs` (`thumbnail_with_text`) | `pl-ocr` `thumbnail::tests::image_without_archive_has_image_thumbnail` (JPEG/PNG/TIFF/GIF/BMP/WebP/Alpha) | Vorschaubilder nicht byte-gleich verglichen; HEIC → weiter Standardbild (image-Crate, Known difference pl-consumer) |
| a1d76f3a1 (#14745) | `.eml`-Datei aus Mails mit CRLF statt LF (RFC 5322, Outlook/quoted-printable) | `pl-mail`: `fetch/generator.rs` (`eml_bytes` → `as_bytes_linesep(.., CRLF)`, `header_encode_linesep`) | `generator::tests::eml_bytes_use_crlf`; `tests/worker_messages.rs` (LF-Fassung gegen v3.4.2-Referenz, CRLF-Fassung = dieselbe Nachricht mit CRLF) | W6-Mail-Parität angepasst (Vergleich über `eml_bytes_linesep(.., b"\n")`) |
| 62d729e30 (#14754) | KI-Chat über mehr als 32 700 erlaubte Dokumente: kürzere Liste (IN/NOT IN), über 32 700 IDs ein JSON-Parameter (`json_each`), Nachfilter auf erlaubte IDs | `pl-ai`: `vector_store.rs` (`build_where`, `WhereParam`, `indexed_document_ids`), `chat.rs` (`permitted_documents_filter`, `retain_permitted`), `server/chat.rs`, `server/mcp_index.rs` (gleiche Filter) | `vector_store::tests::large_in_and_not_in_use_json_each` (echte vec0-Abfrage, 45 000 IDs); `chat::tests::permitted_filter_uses_shorter_list` | PostgreSQL-Grenze (65 535 Bind-Parameter) nicht berührt: erlaubte IDs kommen als Abfrageergebnis, nie gebunden. W8-Chat-Parität ohne Fall > 32 700 – keine Regel |
| 7cdcd71a6 (#14728) | Hugging-Face-Embedding-Modell offline aus dem Cache laden | **nicht übernommen – entfällt**: PaperRust hat kein lokales Hugging-Face/sentence-transformers-Backend (`pl-ai` `embedding.rs`: „Unsupported embedding backend: huggingface“, Vertrag pl-ai Known differences 10); Ollama/OpenAI-kompatibel fragen keinen Hub | – | – |
| fefce7be1 (#14736) | Aufgaben-Liste: Textfilter nur serverseitig (zweiter Filter im Browser verglich übersetzte Anzeigetexte und verwarf Treffer) | `frontend/patches/15-upstream-tasks-text-filter.patch` (unverändert, inkl. Specs); `frontend-light`: `components/admin/tasks/tasks.ts` (`taskMatchesFilters` ohne Textvergleich) | Patchprüfung gegen v3.4.2 `src-ui` (`patch --dry-run`, kein anderer Patch berührt die Datei); `frontend-light` `tasks.test.ts` (vitest) | – (Server filtert `name`/`result` schon wie v3.4.2: `pl-system` `tasks/query.rs`) |
| 60a35edee (#14711) | Metadaten-Massenänderungen im Hintergrund | **eigene Umsetzung der Idee** (Richtung seit 2026-11-10): `pl-documents-write`: `bulk_task.rs` (Schwelle, Aufgabe), `views.rs` (`bulk_edit` reiht ab `PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS`=500 Dokumenten ein, Antwort `{"result":"OK","task_id":…}`; kleinere Auswahlen sofort wie v3.4.2); `pl-queue`: Aufgabe getrackt (`bulk_update`, `input_data`/`owner_id`); `pl-core`: `TRACKED_TASKS`; `pl-server`: Worker-Handler. Keine WebSocket-Meldung `bulk_operation_status`, Nachlauf als eigene Aufgabe (Vertrag pl-documents-write, Known differences) | `bulk_task::tests` (Schwelle, Entscheidung, kwargs), `paperless_task::tests` (input_data/owner) | nur Auswahlen ≥ 500 Dokumente betroffen; Paritätsfälle nutzen kleinere – keine Regel |
| (#14762) | Workflow von Hand nur für Besitzer: Knopf/Menüpunkt in der Massenbearbeitung nur, wenn dem Nutzer **jedes** gewählte Dokument gehört (oder Superuser), Bearbeitungsrecht reicht nicht | **umgesetzt 2026-11-15**, eigene Umsetzung: Server prüfte schon (`pl-documents-write` `bulk_edit.rs`, `ownership` für `run_workflow`, unverändert); frontend-light klassisch `BulkEditor.svelte` (`canRunWorkflow` = Änderungsrecht + `userOwnsAll`), PaperRust-Design `BulkBar.svelte` über `lib/manual-workflows.ts::canRunManualWorkflow` (nicht geladene gewählte Dokumente gelten als nicht sicher im Besitz) | `src/lib/manual-workflows.test.ts` (vitest) | – (nur Oberfläche) |
| (#14760) | Workflow für **ein** Dokument: Schalter „Filter der Auslöser ignorieren“ steht anfangs auf an; Massenaufruf bleibt aus | **umgesetzt 2026-11-15**, eigene Umsetzung: Props `forceDefault`/`singleDocument` in `RunWorkflowConfirmDialog.svelte` und `design2/workspace/RunWorkflowSheet.svelte`, gesetzt in `DocumentDetail.svelte` bzw. `DocDetail.svelte`; eigene Texte für ein Dokument (`common.ignore_trigger_filters`, `common.ignore_trigger_filters_document`, 10 Sprachen) | `src/lib/manual-workflows.test.ts` (Quelltextprüfung, keine Komponententests vorhanden); `tools/i18n-check.mjs` | – |
| (#14763) | Workflow-Start: Oberfläche erwartet keine `task_id` in der Antwort | **nichts zu tun**: PaperRust wertet beim Workflow-Start keine `task_id` aus (Oberfläche wartet nur auf Erfolg/Fehler, Lauf im Hintergrund) – schon anders gelöst | – | – |
| (#14769) | Dialog „Ansicht speichern“: Nutzerliste für Rechte beim Öffnen sicher geladen | **nichts zu ändern** (geprüft 2026-11-15): frontend-light lädt `data.users` beim Start (`main.ts` → `data.loadAll()`, vor dem Einhängen der App); `SaveViewConfigDialog.svelte` liest die Liste reaktiv (`$derived`), kommt sie nach dem Öffnen an, erscheint sie ohne Neuladen | – | – |
| django-allauth 65.21.1 | Ratenlimit: Zeitfenster gehört in den Cache-Schlüssel, sonst teilen sich zwei Limits mit verschiedenen Fenstern für denselben Bereich einen Zähler | **umgesetzt 2026-11-15**, eigene Umsetzung: `pl-accounts` `ratelimit.rs::cache_key` (`allauth:rl:allauth.account:<aktion>:<fenster>:<quelle>`, z. B. `60s`); Schlüssel nicht mehr gleich zu allauth 65.20.1 – egal, Django läuft nicht mehr mit (Vertrag pl-accounts) | `ratelimit::tests::keys_like_allauth`, `ratelimit::tests::windows_count_separately` | `tests/parity.rs` (historisch, `#[ignore]`): Rust-Vorbelegung auf neues Format |
| 64bf7f845 (#14784) | Passwort-Felder als konstante Maske, „unverändert“-Prüfung zusammengefasst | `pl-mail` `accounts.rs`, `pl-system` `config.rs`, `pl-auth-api` `profile.rs` (`obfuscate` → immer 10 Sterne); „nur Sterne/leer = unverändert“ war schon an allen drei Stellen gleich | `accounts::tests`, `config::tests::obfuscation`, `profile::tests::obfuscation` | Referenzdaten ohne Passwort > 10 Zeichen – keine Regel; Verträge pl-mail/pl-system/pl-auth-api Known differences |
| c177c13ac (#14783) | Dateinamen-Filter zeitgesteuerter Workflows: portable, verankerte Regex statt `fnmatch.translate` (PostgreSQL lehnte diese ab) | `pl-matching` `fnmatch.rs` (`glob_to_portable_regex`), `workflow.rs` (`filename_prefilter_regex`). Bei uns trat der PostgreSQL-Fehler nie auf (Vorfilter im Speicher); Endergebnis gleich, weil danach `fnmatch` entscheidet. **Nebenbei behoben:** Groß-/Kleinschreibung bei Mustern mit Vorausblick/Rückblick/atomaren Gruppen (auch benutzerdefinierte Zuordnungs-Regex) | `fnmatch::tests::glob_to_portable_regex_like_upstream_14783` (alle Fälle aus `test_glob_to_portable_regex.py`), `regex_compat::tests::ignore_case_also_with_lookaround_and_atomic_groups`, `tests/workflows.rs::prefilter_null_semantics_and_filename_regex` | Vertrag pl-matching Known differences |
| 237521ec4 (#14808) | Massenänderung: `set_actor` nur bei eingeschaltetem Änderungsprotokoll | **nichts zu tun**: kein `set_actor`/auditlog-Kontext; `pl-documents-write` prüft `audit_log_enabled` an jeder Schreibstelle (`actions.rs`, `update.rs`, `views.rs`, `tasks.rs`) | – | – |
| de75249e5 (#14773) | Alle Massenoperationen im Hintergrund | **bewusst anders** (schon seit #14711): ab `PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS` (500) im Hintergrund, kleinere sofort – schneller für den Alltag | – | – |
| b5d6009e1 (#14797) | Auswahl bleibt nach eingereihter Massenänderung | **nichts zu ändern**: frontend-light `BulkEditor.svelte` leert die Auswahl nur, wenn Originale gelöscht/zusammengeführt wurden (`clearSelection`), sonst bleibt sie; den Fall „Hintergrundaufgabe fertig → Auswahl verkleinern“ (app-frame) gibt es nicht | – | – |
| 14252054a (#14821) | Eingabefelder melden den Wert beim Tippen, nicht erst beim Verlassen | frontend-light `InputText`, `InputTextarea`, `InputUrl`, `InputNumber` (`oninput` meldet an `onchange`; Zahl roh beim Tippen, normalisiert beim Verlassen) – betrifft v. a. ASN und benutzerdefinierte Felder in der Dokumentansicht | `svelte-check`, vitest (keine Komponententests vorhanden) | – |
| 199d1a88e (#14803), 4973be849 (#14824) | Chat-Streaming unter ASGI; Socket-Zeitlimits für Broker/Celery im Status | **entfällt**: kein ASGI/Python-Server, kein Celery/Redis | – | – |
| (#14626) | Notizen als Markdown | **geplant, nach Datenmodell Stufe 7** – braucht Schemaänderung, noch nicht umgesetzt | – | – |

### Übernommen als eigene Migration (2026-11-13): 8aa99a381 (#14612, django-treenode → Adjazenzliste)

Früher hier „zurückgestellt bis zum upstream-Release“, weil es das Schema ändert. Seit der Produktrichtung
vom 2026-11-10 (paperless-ngx läuft nicht mehr auf derselben DB, docs/DATAMODEL-V2.md §0) ist es der **erste
Schritt des eigenen Datenmodells (L6)** – Idee nachgebaut, kein Code übernommen. Vertrag:
docs/contract/pl-objects.md „Tag-Baum (Adjazenzliste, L6 Schritt 1)“.

- **Schema:** PaperRust-Migration `pl_objects.0001_paperrust_tag_adjacency` (nicht upstream `0028`): die 13
  `tn_*`-Spalten außer `tn_parent_id` fallen weg. `django_migrations` bleibt bei v3.4.2 – Umstiegs-Erkennung
  (`pl_cli::migrate::detect_state`, `pl-dbmigrate`) und Nachholen ab 3.0 unverändert; die Spalten werden nach
  dem v3.4.2-Stand von PaperRust selbst entfernt. `pl-system::migrations_generated` unverändert.
- **pl-objects:** `tagtree.rs` (neu, Momentaufnahme der Elternkanten), `update_tree`/treenode-Cache entfernt,
  `treenode.rs` nur noch Export-Projektion; `listing.rs`, `objects.rs` (`children_of`), `handlers.rs`,
  `bulk.rs`, `write.rs` (`check_parent`, `update_document_parent_tags`), `audit.rs` (Feldliste wie #14612).
- **Dokument-Tags:** `pl-consumer::db` (`tag_ancestors`, `tags_descendants`, `with_ancestors`: rekursives SQL),
  `pl-documents-write` (`docs.rs`, `bulk_edit.rs`, `update.rs` Inbox), `pl-ai::server::workflow`.
- **Import/Export:** `pl-cli::export::tagtree` – Import überspringt die 13 Felder (wie upstream), Export
  **berechnet** sie für das v3.4.2-Manifest (anders als upstream, das sie nicht mehr schreibt: der Rückweg
  zielt auf paperless-ngx 3.4.2).
- **Tests/Daten:** `crates/pl-objects/tests/tag_adjacency.rs` (Referenzkopie: Migration, API gegen die
  treenode-Werte), `tag_delete_descendants.rs`/`pl-consumer/tests/bulk_equal.rs`/`tests/parity/pl-consumer/w15/
  reset_db.sh` spielen die Migration nach dem v3.4.2-Schnappschuss ein; `tests/bench/scale/gen.py`/`run.py`
  ohne `tn_*`-Spalten. Paritätsregel `tag-adjacency-no-treenode-fields` (`tests/parity/pl-objects/accepted.json`);
  die globale Regel `tag-tree-audit-django-process-artifact` wird damit überflüssig (Harness-Besitzer).
- **Beim nächsten Versionssprung** (upstream-Release mit `0028`): Quell-DBs mit angewandter `0028` haben die
  Spalten nicht mehr – `pl-dbmigrate` (`schema/`, Erkennung) und `pl-cli` (`baseline/`, Nachholen) müssen
  diesen Stand als „gleich, Tag-Spalten schon weg“ annehmen; die PaperRust-Migration ist idempotent.
