# Vertrag: Automatik (Workflows neu) – `pl-automation` + Ausführung in `pl-consumer`

Stand 2026-11-18. Bauplan: `docs/WORKFLOWS-NEU.md`. **Kein upstream-Gegenstück** (PaperRust-Erweiterung);
das ngx-Speicherformat (`documents_workflow*`, `paperless_mail_mailrule*`) bleibt unverändert, Erweiterungen
liegen in eigenen `paperrust_*`-Tabellen.

| Teil | Ort |
|---|---|
| API | `crates/pl-automation` (Router unter `/rs/api/v2/automation`, von pl-server vor dem v2-Router genestet) |
| Ausführung: Verlauf, Sonst-Zweig, KI-Bedingung | `crates/pl-consumer/src/workflow_ext.rs` (eine Entscheidung `decide` für alle Wege) |
| Testlauf | `crates/pl-consumer/src/workflow_dryrun.rs` (ruft den Produktionskern `run_workflow_list_inner`) |
| Tabellen | `crates/pl-consumer/migrations/0001_paperrust_workflow.sql` (`pl_consumer.0001_paperrust_workflow`) |
| ngx-Teile | pl-workflows `serializers::validate_workflow/validate_actions`, `write::workflow_create/update/upsert_actions/delete_workflow`; pl-mail `rules::save_in/render/load_for` |

## 1 Tabellen (`paperrust_*`, expand-only)

| Tabelle | Spalten | Zweck |
|---|---|---|
| `paperrust_workflow_run` | `id bigserial`, `workflow_id` → `documents_workflow` (CASCADE), `trigger_type smallint`, `document_id int NULL` (kein FK: Verlauf überlebt endgültiges Löschen), `run_at timestamptz`, `result` (`ok`/`skipped`/`error`), `branch` (`main`/`else`/`none`), `code`, `message`, `changed_fields jsonb` (Liste), `source smallint NULL` (`DocumentSource`, 3 = Mail) | Verlauf |
| `paperrust_workflow_else_action` | `workflow_id` → `documents_workflow` (CASCADE), `workflowaction_id` → `documents_workflowaction` (CASCADE), PK beide | Sonst-Zweig (normale ngx-Aktionszeilen) |
| `paperrust_workflow_ext` | `workflow_id` PK → `documents_workflow` (CASCADE), `ai_condition jsonb NULL`, `updated_at` | KI-Bedingung |

Angelegt durch `paperrust manage migrate` (pl-cli) oder bei Bedarf: `workflow_ext::ensure_pool` (API, Start) bzw.
`ensure_conn` (Ausführung, auch in einer Transaktion; ein `DO`-Block mit `pg_advisory_xact_lock`, merkt sich
„bereit“ erst über `ensure_pool`, weil eine äußere Transaktion zurückgerollt werden kann).

Indizes: `(workflow_id, id DESC)` für die Liste, `(run_at)` fürs Aufräumen.

## 2 Ausführung (pl-consumer)

### 2.1 Entscheidung `workflow_ext::decide` (eine Funktion für alle Wege und den Testlauf)

Eingang: Workflow, Auslösertyp, Ergebnis des Filterabgleichs (`MatchState`), Dokument-id, `ExtOptions`.

1. Kein Auslöser dieses Typs (`NotFired`) → nichts (auch kein Sonst-Zweig).
2. Abgleichfehler (`Error`) → nichts, Verlauf „error“ (`match_error`); Sonst-Zweig läuft **nicht**.
3. Filter nicht erfüllt → Sonst-Zweig (`else`), falls vorhanden; sonst nichts (`conditions_not_met`, im Verlauf nur
   mit `log_skips`).
4. Filter erfüllt, ohne KI-Bedingung → Hauptzweig (`ok`).
5. Filter erfüllt, mit KI-Bedingung:
   * Einlese-Auslöser oder kein Dokument → Hauptzweig, `ai_ignored_consumption` (markiert);
   * keine KI-Anbindung → Hauptzweig, `ai_ignored_unavailable`; KI aus → `ai_ignored_disabled`;
   * `AiMode::Defer` → Lauf nach dem Commit als Aufgabe nachholen (`RunOutcome.deferred`);
   * `AiMode::NotChecked` (Testlauf ohne KI) → Hauptzweig, `ai_not_checked`;
   * `AiMode::Inline` → KI fragen: ja → Haupt (`ai_met`), nein → Sonst (`ai_not_met_else`) oder nichts
     (`ai_not_met`, Verlauf „skipped“), Fehler → nichts (`ai_error`, Verlauf „error“).
6. Manuell „auf alle anwenden“ (`Bypassed`) → Hauptzweig ohne jede Bedingung (`filters_bypassed`).

### 2.2 Wege

| Weg | Funktion | KI-Modus | nicht zutreffend im Verlauf |
|---|---|---|---|
| Einlesen gestartet (inkl. Mail) | `workflows::run_consumption` | – (ignoriert, markiert) | nein |
| Dokument hinzugefügt (Consumer-Transaktion) | `run_added` → `run_workflow_list_inner` | Defer | nein |
| Dokument geändert (Worker, Massenänderung) | `run_workflows`/`run_workflow_list` → Kern | Inline | nein |
| Dokument geändert (API PATCH/Version) | pl-documents-write `run_updated` (eigene ngx-Umsetzung, gleiche `decide`) | Defer | nein |
| Zeitplan | `scheduled.rs` → `run_workflows(…, Some(wf))` | Inline | nein |
| Manuell / nachgeholt / rückwirkend | `manual.rs::run_workflows_for_documents_as` → `run_workflow_as` | Inline | ja |
| Gebündelter Nachlauf (`bulk.rs`) | nur Workflows ohne Erweiterung (`is_simple_workflow`), sonst Einzelweg; `first_matching_workflow` zählt nicht passende Workflows mit Sonst-Zweig als Treffer | – | nein |

Nachgeholte Läufe: Aufgabe `documents.tasks.run_workflows_for_documents` mit kwargs
`{"document_ids": [id], "workflow_id", "filter_documents": true, "match_trigger": <2–5>}` (PaperRust-Erweiterung
`match_trigger`: Filter dieses Auslösertyps statt „manuell“; Lauf zählt im ngx-`WorkflowRun` und im Verlauf als
dieser Typ). Eingereiht nach dem Commit über `workflows::enqueue_ai_suggestions` (gleicher Haken wie
`apply_ai_suggestions`).

### 2.3 Verlauf

Je ausgeführtem Workflow (Haupt- oder Sonst-Zweig) neben dem ngx-`WorkflowRun` ein Eintrag `ok` mit
`changed_fields` = geänderte Zeilenfelder (`title`, `correspondent`, `document_type`, `storage_path`, `owner`,
vorher/nachher verglichen) ∪ berührte Bereiche der Aktionen (`tags`, `custom_fields`, `permissions`, `email`,
`webhook`, `password_removal`, `trash`, `remote_ocr`, `ai_suggestions`). Einlesen: `document_id = NULL`,
`source` gesetzt, nur berührte Bereiche. Fehler: bestmöglich `error` mit gekürzter Meldung (ohne Interna).
Aufräumen: höchstens einmal je Stunde und Prozess beim Schreiben, Frist `PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS`
(Standard 90, `0` = unbegrenzt; Aliase `PAPERLESS_*` über `pl_core::config::var`).

### 2.4 Testlauf (`workflow_dryrun::dry_run`)

Je Dokument in der Transaktion des Aufrufers: `SAVEPOINT` → Zustand vorher (`DocState`) → **derselbe Kern**
`run_workflow_list_inner` (Abgleich, `decide`, Aktionen, Speichern, Verlauf) mit aufzeichnenden Wirkungen
(`Recorder`: E-Mail mit Empfänger, Webhook mit URL, Passwort entfernen, Papierkorb), ohne `settings` (keine
Dateibewegung), KI-Vorschläge nur gemeldet → Zustand nachher → `ROLLBACK TO SAVEPOINT`. Einlese-Auslöser:
Abgleich über ein nachgebildetes Einlese-Dokument (`/consume/<original_filename>`, Quelle/Mailregel aus Anfrage
oder Auslöser), Aktionen auf das Dokument → `approximate: true`. Wächter-Test
`workflow_dryrun::tests::dry_run_uses_production_core`.

## 3 API `/rs/api/v2/automation`

Anmeldung wie v1/v2 (Session+CSRF, `Token`, `Bearer prt_…`); inaktive Nutzer 403. Fehlerform wie v2:
`{"error": {"code": "...", "message": "...", "fields": {...}?}}` – Codes `unauthenticated` (401), `forbidden`
(403), `not_found` (404), `invalid` (400; bei Workflows enthält `fields` die DRF-Fehler von `/api/workflows/`
unverändert, Sonst-Aktionen unter `else_actions`, KI-Bedingung unter `ai_condition`, Mail-Regel unter
`mail_rule`, Workflows des Mail-Ablaufs unter `workflows.<index>`), `not_installed` (503), `not_implemented`
(501), `throttled` (429, `Retry-After`), `internal` (500, ohne Interna), `method_not_allowed` (405).
Antworten `application/json`, `Cache-Control: private, no-cache`. Zeitpunkte RFC 3339 UTC.

Gemeinsame Objektformen:

* **Kombinierter Workflow** (`CombinedWorkflow`), Felder in Reihenfolge: `workflow` (genau wie
  `GET /api/workflows/{id}/`), `else_actions` (Liste wie `WorkflowAction` in `/api/workflows/`), `ai_condition`
  (`{"field","object_id","min_confidence"}` oder `null`), `ai` (`{"available","enabled"}`), `summary` (Kurzform).
* **Kurzform** (`summary`): `trigger_types` (sortiert), `triggers[]` (`id`, `type`, `type_key`
  `consumption|document_added|document_updated|scheduled|manual`, `conditions[]`, `schedule`), `ai_condition`
  (`field`, `object{id,name}`, `min_confidence`, `active`, `inactive_reason` `ai_unavailable|ai_disabled|consumption_only|null`
  oder `null`), `actions[]`, `else_actions[]` (je `id`, `type`, `type_key`
  `assignment|removal|email|webhook|password_removal|move_to_trash|remote_ocr|ai_suggestions`, `items[]`).
  Bedingungsarten: `sources{values}` (nur Einlesen, wenn nicht alle), `filename{value}`, `path{value}`,
  `content_match{algorithm,value,insensitive}` (nicht beim Einlesen), `tags_any|tags_all|tags_none{objects}`,
  `correspondent_is{object}`, `correspondent_any|correspondent_none{objects}`, dito `document_type_*`,
  `storage_path_*`, `mail_rule{object}`, `custom_field_query{value}`. Aktions-Items: `assign_title{value}`,
  `assign_<feld>{object|objects}` (Custom Fields zusätzlich `values`), `remove_all_*`, `remove_<feld>{objects}`,
  `email{to,subject,include_document}`, `webhook{url,as_json,include_document}`,
  `ai_suggestions{fields,create_missing,overwrite_existing}`. Namen nur für Objekte, die der Nutzer sehen darf
  (sonst `name: null`); Passwörter, E-Mail-Texte, Webhook-Köpfe/-Körper erscheinen nie.

### 3.1 `GET /workflows` – Übersicht

Recht `documents.view_workflow`. Antwort `{"items": [...], "week_start"}`; je Eintrag in Reihenfolge `id`,
`name`, `order`, `enabled`, `runs_this_week` (Läufe `ok`+`error` seit Montag 00:00 in `PAPERLESS_TIME_ZONE`),
`last_run` (`{id, at, result, code}` oder `null`), `has_else`, `summary`. Sortiert nach `order`, `id`.

### 3.2 `GET /workflows/{id}` – lesen

Recht `view_workflow`. 200 `CombinedWorkflow`; 404 unbekannt/kein int.

### 3.3 `POST /workflows` / `PUT|PATCH /workflows/{id}` – gemeinsam speichern

Rechte `documents.add_workflow` bzw. `documents.change_workflow`. Körper = ngx-Workflow wie `/api/workflows/`
plus optional `else_actions` (Liste ngx-Aktionen) und `ai_condition` (Objekt oder `null`). **Eine Transaktion**:
`validate_workflow` + `workflow_create/update` (inkl. `prune`), dann `validate_actions` + `upsert_actions` für den
Sonst-Zweig (mitgeschickte Aktions-ids nur, wenn sie schon zu diesem Sonst-Zweig gehören; entfernte Sonst-Aktionen
werden gelöscht), dann `paperrust_workflow_ext`. POST/PUT: fehlende Erweiterungsfelder = leeren; PATCH: fehlende
unverändert. KI-Bedingung: `field` ∈ `document_type|correspondent|storage_path|tags`, `object_id` muss existieren,
`min_confidence` 0…1 (Standard 0.9). Antwort 201/200 `CombinedWorkflow`. `prune` von pl-workflows lässt
Sonst-Aktionen stehen; `delete_workflow` (auch `/api/workflows/{id}/` DELETE) löscht sie mit.

### 3.4 `POST /dry-run` – Testlauf

Rechte `view_workflow` + `view_document`; mit Entwurf zusätzlich `add_workflow` **oder** `change_workflow`.
Körper (Reihenfolge der Auswertung): `workflow` (Entwurf wie 3.3, optional mit `id` eines bestehenden Workflows,
dessen Bearbeitung getestet wird – nur mit `change_workflow`) **oder** `workflow_id`; `limit` (1–200, Standard 50;
die neuesten sichtbaren Wurzeldokumente nach `added DESC, id DESC`); `document_ids` (statt `limit`, höchstens 200,
unsichtbare werden stillschweigend übergangen); `trigger_type` (muss am Workflow sein; Standard: erster Auslöser,
der kein Einlesen ist, sonst Einlesen); `with_ai` (KI-Bedingung wirklich prüfen, höchstens 10 Dokumente);
`source` (1–4) und `mail_rule` (nur Einlese-Auslöser).
Ablauf: Transaktion mit `lock_timeout 3s`, `statement_timeout 30s`; Entwurf mit Platzhalter-Namen speichern (gleiche
Prüfung → gleiche Fehler wie beim Speichern), laden mit `workflow_by_id`, `dry_run`, **Rollback**.
Antwort 200, Felder in Reihenfolge: `workflow_id` (null bei Entwurf), `trigger_type`, `trigger_key`,
`approximate`, `ai` (`available`, `enabled`, `checked`), `notes` (`consumption_approximate`,
`schedule_window_not_checked`, `ai_condition_ignored`, `ai_condition_not_checked`), `counts` (`main`, `else`,
`skipped`, `error`, `checked`), `documents[]` je `id`, `title`, `added`, `result` (`main|else|skipped|error`),
`code`, `reason` (z. B. upstream-Grundtext „Document tags … do not include …“), `changes[]`
(`{field, before, after}` für Titel/Felder, `{field, added[], removed[]}` für Tags/Rechte,
`{field:"custom_field", custom_field, before, after, removed}`, `in_trash`), `effects[]`
(`{action_id, kind: email|webhook|password_removal|trash|ai_suggestions, target}`), `before`, `after`
(voller Zustand mit Namen; `after` nur bei `main`/`else`).
Nebenwirkungen: keine (alles zurückgerollt; keine Mails/Webhooks, keine Dateien, keine Aufgaben). Mit `with_ai`:
ein Modellaufruf je Dokument (pl-ai).

### 3.5 `POST /workflows/{id}/apply` – rückwirkend anwenden

Rechte `view_workflow` + `documents.change_document`, und **jedes** Dokument muss für den Nutzer änderbar sein
(sonst 403 „Insufficient permissions.“, wie bulk_edit). Zusätzlich (Sicherheitsprüfung 2026-11-19 F1, wie
bulk_edit `run_workflow`): Nutzer **besitzt** alle Dokumente (oder sie haben keinen Besitzer; Superuser
ausgenommen) – sonst 403; Workflow **aktiv** – sonst 400 `{"workflow": ["Workflow is not enabled"]}`; anderer
Auslöser als „Manuell“ (5) nur mit `documents.change_workflow` – sonst 403. Test `dryrun::tests::apply_gate_rules`.
Körper `document_ids` (1–200), `trigger_type` (wie 3.4).
Reiht `run_workflows_for_documents` ein (PaperlessTask-Zeile, trigger_source `manual`): Einlese-Auslöser →
`filter_documents: false` (Auswahl aus dem Testlauf), sonst `filter_documents: true` + `match_trigger`. Echte
Ausführung im Worker (Verlauf, Sonst-Zweig, KI inline). Antwort 202 `{task_id, documents, trigger_type,
filter_documents}`; ohne Warteschlange 503.

### 3.6 `GET /workflows/{id}/runs` – Verlauf

Recht `view_workflow`. Query `cursor` (id aus `next_cursor`), `limit` (1–200, Standard 50), `result`
(`ok|skipped|error`), `document` (id). Antwort `items[]` (neueste zuerst) je `id`, `trigger_type`, `trigger_key`,
`document` (`{id,title,in_trash}`, für nicht sichtbare `{id:null, hidden:true}`), `run_at`, `result`, `branch`,
`code`, `message`, `changed_fields`, `source`; `next_cursor` (oder `null`), `counts` (`this_week`,
`errors_this_week`, `total`), `retention_days`. 404 für unbekannte Workflows.

### 3.7 `POST /runs/cleanup`

Nur Superuser. Körper `older_than_days` (≥ 1, Standard = eingestellte Frist). Antwort `{deleted, older_than_days}`.

### 3.8 `GET /suggestions` – Vorschläge aus dem Archiv (ohne KI)

Rechte `view_workflow` + `view_document`. Query `min_documents` (2–100000, Standard 5), `min_share` (0.5–1,
Standard 0.9), `limit` (1–100, Standard 30), `refresh` (Zwischenspeicher umgehen). Nur über Wurzeldokumente,
die der Nutzer sieht. Muster: `correspondent_tag`, `correspondent_document_type`, `correspondent_storage_path`,
`document_type_tag` (ohne Posteingangs-Tags), `filename_document_type` (Dateiname beginnt mit einem Wort aus
≥ 3 Buchstaben; allgemeine Anfänge wie „scan“, „img“ ausgenommen). Antwort `items[]` je `key`, `kind`, `from`
(`{field, id, name}` bzw. `{field:"filename_prefix", value}`), `to` (`{field, id, name}`), `matches`, `total`,
`share` (3 Stellen), `covered_by_workflow` (ein aktiver Workflow weist das schon zu); `generated_at`,
`min_documents`, `min_share`, `cached`. Sortierung: `matches` absteigend, dann `share`, dann `key`.
Zwischenspeicher je Nutzer und Parametersatz 10 min (Prozess), Abfragen mit `statement_timeout 20s`.

### 3.9 `POST /suggestions/draft`

Recht `add_workflow`. Körper `key`. Antwort `{"workflow": <ngx-Entwurf>}` (`enabled: false`, nichts gespeichert):
Korrespondent/Typ-Muster → Auslöser „hinzugefügt“ **und** „geändert“ mit `filter_has_correspondent` bzw.
`filter_has_document_type`; Dateinamen-Muster → Einlese-Auslöser mit `filter_filename "<wort>*"`. Unbekannte oder
für den Nutzer unsichtbare Objekte → 400 `key`.

### 3.10 `POST /draft-from-description` – Workflow aus Beschreibung (KI)

Recht `add_workflow`; `description` (1–2000 Zeichen). Durchreichung an `pl_ai::server::assist::workflow_draft`
(gleiche Antwort wie `POST /api/x-ai/assist/workflow-draft/`: `workflow`, `missing`, `explanation`, `dropped`),
30/h je Nutzer (eigenes Budget). KI aus → 503; ohne Feature `ai` → 501.

### 3.11 Mail als Auslöser `/mail-flows`

* `GET /mail-flows`: Rechte `view_workflow` + `paperless_mail.view_mailrule`; sichtbare Regeln (pl-mail
  Objektrechte) je `mail_rule{id,name,enabled,account}`, `workflows[{id,name,enabled}]` (Workflows mit einem
  Auslöser `filter_mailrule = Regel`).
* `GET /mail-flows/{rule}`: `{mail_rule: <wie GET /api/mail_rules/{id}/>, workflows: [CombinedWorkflow]}`.
* `POST /mail-flows` (`add_mailrule`; mit Workflows `add_workflow`) und `PUT|PATCH /mail-flows/{rule}`
  (`change_mailrule` + Objektrecht; bestehende Workflows `change_workflow`, neue `add_workflow`, Löschen
  `delete_workflow`): Körper `mail_rule` (PATCH optional), `workflows[]` (kombinierte Workflows, mit `id` =
  ändern), `delete_workflow_ids[]` (nur Workflows dieses Ablaufs). **Eine Transaktion**: `rules::save_in`
  (gleiche Prüfung/Schritte wie `/api/mail_rules/`), dann je Workflow Einlese-Auslöser an die Regel binden
  (fehlt einer → `{type:1, sources:[3], filter_mailrule}` ergänzt; vorhandene bekommen `filter_mailrule` und,
  bei begrenzten Quellen, die Quelle 3) und speichern wie 3.3, dann löschen. Antwort 201/200 wie GET.

### 3.12 `GET /export-ngx` – Rückweg

Recht `view_workflow`. Antwort `workflows[]` (`{source_workflow_id, workflow: <ngx-Workflow ohne ids>}`) und
`warnings[]` (`{workflow_id, code, message}`; Codes `ai_condition_dropped`, `else_not_expressible`,
`else_never_runs`, `else_ai_part_dropped`). Sonst-Zweig → eigener Workflow „<Name> (else)“ mit umgekehrter
Bedingung, wenn je Auslösertyp genau ein Auslöser mit genau einer umkehrbaren Bedingung besteht (Tags beliebig ↔
keiner, „alle“ mit einem Tag → keiner, Korrespondent/Typ/Speicherpfad ist/beliebig ↔ keiner, Quellen beim Einlesen
→ Rest). Nicht umkehrbar: Dateiname, Pfad, Mailregel, Inhaltsabgleich, Benutzerfeld-Abfrage, mehrere Bedingungen.

## 4 Known differences / Grenzen

1. **Fehler im Verlauf**: ein Fehler in einer Transaktion, die danach zurückgerollt wird (Einlesen, API-PATCH),
   erscheint nicht im Verlauf (steht im Log und in der Aufgabe). Im Worker (Autocommit) erscheint er.
2. **Nicht zutreffende Bedingungen** werden bei automatischen Auslösern nicht vermerkt (sonst ein Eintrag je
   Änderung und Workflow); bei manuellen/nachgeholten Läufen und im Testlauf schon.
3. **KI-Bedingung**: beim Einlese-Auslöser ignoriert (es gibt noch kein Dokument), markiert. Bei „hinzugefügt“
   (Consumer) und „geändert“ über die API wird der Lauf nach dem Commit im Worker nachgeholt (das LLM sieht nur
   festgeschriebene Dokumente; die API wartet nicht auf das Modell). Die nachgeholte Ausführung prüft die Filter
   erneut. Ohne Warteschlange (Tests) geht ein nachgeholter Lauf verloren (Warnung im Log). KI-Fehler → nichts
   ausgeführt (auch kein Sonst-Zweig), Verlauf „error“.
4. **Testlauf „geändert“** nutzt den Worker-Kern (`run_workflow_list_inner`), nicht pl-documents-write
   `run_updated` (API-Weg); beide setzen dieselben ngx-Aktionen um und nutzen dasselbe `decide`.
5. **Testlauf Zeitplan**: das Zeitfenster (`schedule_offset_days` …) wird nicht geprüft, nur die Filter (`notes`).
6. **Testlauf Einlesen**: nachgebildet (Hinweis `approximate`), Aktionen wirken aufs Dokument statt auf die Vorgaben.
7. **Vorschläge**: „Mailregel → Typ“ ist nicht möglich (ngx speichert die Mailregel eines Dokuments nicht).
8. **Export**: nur die Umwandlung als API; der Exporter (`document_exporter`, pl-cli) übernimmt sie noch nicht
   (CORE-REQUEST). Ohne Umwandlung landen Sonst-Aktionen als verwaiste Aktionen im Export; ngx räumt sie beim
   nächsten Speichern eines Workflows weg (`prune`).
9. **pl-mail**: `POST/PUT/PATCH /api/mail_rules/` schreiben Regel, Rechte und Tags jetzt in einer Transaktion
   (vorher drei getrennte Schritte; upstream ebenfalls getrennt). Bei einem Fehler nach dem Setzen der Rechte
   bleiben diese nicht mehr stehen.
10. **Reihenfolge bei nachgeholten Läufen**: ein wegen KI-Bedingung nachgeholter Workflow läuft nach den
    übrigen Workflows desselben Auslösers (nicht an seiner `order`-Stelle).
11. **Zeitplan + KI „nein“** ohne Sonst-Zweig schreibt einen ngx-`WorkflowRun` (gilt als erledigt), damit nicht
    jede Prüfrunde das Modell erneut fragt. Zeitplan mit Sonst-Zweig: der Vorfilter der fälligen Dokumente
    entfällt (nicht passende Dokumente bekommen den Sonst-Zweig, einmal bzw. je Intervall).
12. Die klassische Oberfläche (`/api/workflow_actions/`) listet Sonst-Aktionen wie andere nicht verknüpfte Aktionen.

## 5 Behobene Fehler

* **N4 (2026-11-18, tests/stress/BEFUNDE.md)**: `GET /workflows/{id}`, Testlauf und `apply` endeten zufällig mit 500
  (`incorrect binary data format in bind parameter 1` / `insufficient data left in message`). Ursache: `Names::resolve`
  (`common.rs`) baut z. B. `SELECT "id", "name" FROM "documents_correspondent" WHERE "id" IN ($1)` mit `i32`, andere
  Crates denselben Text mit `i64`; sqlx' Statement-Cache je Verbindung kennt nur den Text. Behebung projektweit:
  sea-query-Abfragen binden über `pl_core::sqlbind::SqlxBinder` (Typ-Signatur im Text). Außerdem setzt ein
  Workflow-Lauf Tags jetzt mit `ON CONFLICT DO NOTHING` (wie Django `tags.add`): zwei gleichzeitige `apply` auf
  dasselbe Dokument ergaben vorher einen Verlaufseintrag „error“ (duplicate key, N4b).
