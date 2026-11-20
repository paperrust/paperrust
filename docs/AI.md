# KI nativ in PaperRust (Ersatz für paperless-gpt / paperless-ai)

Ziel: Keine externen Werkzeuge mehr nötig. Benennen, Zuordnen und Abfragen von Dokumenten per LLM
laufen im System – mit derselben Rechteprüfung wie die API – und **ein Agent legt nie blind neue
Korrespondenten, Tags, Dokumenttypen, Speicherpfade oder Custom Fields an, wenn es schon gleiche oder
ähnliche gibt.**

## Was upstream v3.4.2 schon hat (Basis, 1:1 portieren)

`upstream/src/paperless_ai/`: KI-Vorschläge (`/api/documents/{id}/ai_suggestions/`; `/suggestions/` ist der klassische Klassifikator), Workflow-Aktion `APPLY_AI_SUGGESTIONS` (`documents/workflows/ai.py`), Chat mit
Dokumenten (RAG, `/api/documents/chat/`), Vektorindex (sqlite-vec), Taxonomie-Kandidaten aus ähnlichen
Dokumenten (`taxonomy.py`), Namensabgleich mit Schwelle 0,8 (`matching.py`), Anbieter Ollama /
OpenAI-kompatibel / HuggingFace-Embeddings (`PAPERLESS_AI_*`). Vorschläge werden nur angezeigt,
nicht automatisch übernommen.

## Was PaperRust zusätzlich bringt (eigene Erweiterung, API-kompatibel)

Eigene Funktionen nur unter eigenem Präfix (`/api/x-ai/…`, `/mcp`), nie Änderungen an upstream-Routen.

1. **Taxonomie-Abgleich als Dienst (Kern der Duplikatvermeidung)**
   - Normalisierung: Groß/klein, Umlaute/Akzente, Satzzeichen, Rechtsformen (GmbH, AG, SA, Sàrl,
     Ltd, Inc …), Füllwörter; Token-Sortierung.
   - Ähnlichkeit: Jaro-Winkler/Levenshtein + Token-Überlappung + optional Embedding-Ähnlichkeit;
     Treffer mit Score und Begründung.
   - Aliase: bestätigte Zuordnungen ("Swisscom (Schweiz) AG" = "Swisscom") werden gespeichert und
     beim nächsten Mal direkt erkannt (eigene Tabelle, expand-only).
   - **Serverseitig erzwungen:** Anlegen über KI-Pfade (MCP, Auto-Verarbeitung) prüft immer zuerst;
     gibt es einen Kandidaten über der Schwelle, wird **nicht** angelegt, sondern der Kandidat
     zurückgegeben. Neuanlage nur, wenn kein Kandidat passt, oder mit ausdrücklichem `force` +
     Begründung (im Audit-Log).
2. **MCP-Server eingebaut** (`/mcp`, Streamable HTTP; Anmeldung per API-Token; Agent handelt mit den
   Rechten dieses Nutzers). Werkzeuge u. a.:
   `search_documents`, `get_document`, `list_taxonomy` (Korrespondenten, Tags, Typen, Pfade, Felder),
   `find_similar(kind, name)`, `suggest_metadata(document_id)`, `update_document(...)`,
   `create_taxonomy(kind, name, force?, reason?)` (mit erzwungenem Abgleich); dazu Ingest (`add_document`),
   Wissen (`ask_documents` über den bestehenden LLM-Index, `get_document_content`, gespeicherte Ansichten),
   Notizen, Custom Fields, Ressourcen/Prompts und Token-Scopes (docs/user/ai.md#mcp).
   Kein Löschen über MCP. Externe Agenten (Claude, eigene Agenten) können so arbeiten.
3. **Automatische Verarbeitung** (ersetzt paperless-gpt/-ai): nach dem Einlesen oder per Tag/Workflow
   (z. B. Tag `ai-process`): Titel, Datum, Korrespondent, Typ, Tags, Speicherpfad, Custom Fields
   vorschlagen. Modi: **nur vorschlagen** (Prüfliste in der UI) oder **automatisch übernehmen ab
   Konfidenz X**. Jede KI-Änderung im Audit-Log, rückgängig machbar.
4. **Später:** Vision-OCR per LLM für schlechte Scans (optional), Zusammenfassungen.

## Sicherheit

- **Dokumentinhalt ist nicht vertrauenswürdig** (Prompt-Injection in PDFs): Werkzeuge der
  automatischen Verarbeitung sind auf Vorschlagen/Zuordnen begrenzt, kein Löschen, keine Rechteänderung,
  keine Mails/Webhooks; Grenzen pro Lauf (max. neue Objekte, max. Änderungen).
- Rechte immer serverseitig (pl-authz), nie Daten anderer Nutzer im Prompt (wie upstream `taxonomy.py`).
- Datenschutz: lokales LLM (Ollama) als Standardempfehlung; Cloud-Anbieter nur bei ausdrücklicher Konfiguration.
- Rate-Limits pro Token für `/mcp` (auch **vor** der Token-Prüfung), Begrenzung fehlgeschlagener Anmeldungen,
  höchstens 20 Sitzungen je Nutzer.
- Automatik: manueller Lauf `/api/x-ai/run/` nur im Rahmen von `PAPERRUST_AI_AUTO_MODE` (off/suggest/apply),
  höchstens 10 Dokumente je Aufruf und `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` (100) je Nutzer; neue
  Tags/Korrespondenten/Typen nur mit dem `add_*`-Recht des Auslösers bzw. Dokument-Eigentümers.
- `PAPERRUST_AI_API_BASE` erhält die API-Tokens der MCP-Nutzer: nur auf PaperRust/Django zeigen lassen.

## Reihenfolge

1. Vertrag + 1:1-Port von `paperless_ai` (Vorschläge, Chat, Index) – Teil der Parität.
2. Taxonomie-Abgleich (Bibliothek, gut getestet, ohne LLM nutzbar).
3. MCP-Server.
4. Automatische Verarbeitung + UI-Prüfliste (UI-Erweiterung erst nach Abschluss der Parität).

## Stand (W8, 2026-11-08)

KI läuft ohne Python (Details und Belege: `docs/contract/pl-ai.md`, Abschnitte 3.6, 4, 5.3, 7, 8):

- **Upstream 1:1 in Rust:** `GET /api/documents/{id}/ai_suggestions/` (RAG-Kandidaten, Prompt mit
  tiktoken-Kürzung, Lokalisierung, Abgleich 0,8, gemeinsamer Django-Cache) und `POST /api/documents/chat/`
  (Streaming, compact/refine wie llama_index). Parität gegen eine eigene Django-Instanz mit Mock-LLM:
  60/60 bzw. 75/75 Antworten gleich, LLM-Anfragen byte-gleich. Aktivierbar: `documents.ai_suggestions`,
  `documents.chat`.
- **Vektorindex im upstream-Format** (SQLite + sqlite-vec 0.1.9, gleiche Knoten/Metadaten/Sperren):
  Django und PaperRust lesen und schreiben dieselbe Datei; Index-Aufgaben `llmindex_index`,
  `update_document_in_llm_index`, `remove_document_from_llm_index` als Rust-Funktionen für den Worker.
  Embeddings Ollama/OpenAI-kompatibel per HTTP; HuggingFace lokal nur dokumentiert (nicht in Rust).
- **Workflow-Aktion `APPLY_AI_SUGGESTIONS`** in Rust (Parität 4/4 gegen die Celery-Aufgabe).
- **Automatische Verarbeitung (Erweiterung):** Modi vorschlagen/übernehmen ab Konfidenz, Duplikat-Sperre
  über den Taxonomie-Abgleich (auch direkt vor jedem Anlegen), Grenzen pro Lauf, Audit vorher/nachher,
  Rückgängig mit Konfliktprüfung; Prüfliste als API `/api/x-ai/review/`, manueller Lauf `/api/x-ai/run/`,
  Tag-Auslöser für den Zeitplaner. Eigene Tabellen `paperrust_*` (Migration `0001_paperrust_ai.sql`,
  noch nicht ausgerollt).
- **Offen:** UI für die Prüfliste, Custom-Field-Extraktion, Vision-OCR, anpassbare Prompts,
  django-auditlog-Einträge der KI-Schreibpfade (Schreiber von pl-consumer einhängen).
