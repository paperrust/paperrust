# Vertrag pl-ai (paperless_ai v3.4.2 + Erweiterungen)

Quelle: `upstream/src/paperless_ai/*` (ai_classifier.py, taxonomy.py, matching.py, client.py,
chat.py, embedding.py, indexing.py, vector_store.py, tables.py, migrations/, base_model.py, db.py,
exceptions.py, prompts/*.j2, prompts/render.py, prompts/context.py), `documents/views.py`
(`DocumentViewSet.ai_suggestions`, `ChatStreamingView`, `UiSettingsView`, `SystemStatusView`),
`documents/workflows/ai.py`, `documents/tasks.py`, `documents/signals/handlers.py`,
`documents/caching.py`, `paperless/settings/__init__.py`, `paperless/config.py::AIConfig`,
`paperless/models.py::ApplicationConfiguration`, `paperless/network.py`, `paperless/middleware.py`.
Tests: `paperless_ai/tests/*`, `documents/tests/test_views.py` (ai_suggestions, chat),
`documents/tests/test_api_chat.py`, `paperless/tests/test_compression_middleware.py`.

Golden-Daten (echte upstream-Ausgabe, ohne Django): `tests/parity/pl-ai/gen_golden.py` und
`gen_golden_w8.py` (→ `golden_w8.json`, `llmindex_py.db`) →
`crates/pl-ai/testdata/golden.json` (Prompts, JSON-Schema, Validierung, `_normalize`, difflib,
Chat-Trailer, **mitgeschnittene LLM-HTTP-Anfragen** von llama_index gegen einen lokalen Mock).

> Hinweis zu `docs/AI.md`: In v3.4.2 heißt die KI-Route **`/api/documents/{id}/ai_suggestions/`**;
> `/api/documents/{id}/suggestions/` ist der klassische Klassifikator (pl-documents, Django).

---

## 1. Routen (upstream)

### 1.1 `documents.ai_suggestions` – `GET /api/documents/{id}/ai_suggestions/`

Quelle: `documents/views.py::DocumentViewSet.ai_suggestions` (`@action(detail=True, filter_backends=[])`,
`cache_control(no_cache=True)`). Rechte: `IsAuthenticated` + `PaperlessObjectPermissions` (ViewSet), dann:

1. `get_object_or_404(Document…, pk=pk)` → 404 `{"detail": "No Document matches the given query."}`.
2. `has_perms_owner_aware(user, "change_document", doc)` nein → **403 `HttpResponseForbidden("Insufficient permissions")`**
   (text/html, kein JSON).
3. `AIConfig().ai_enabled` falsch → **400 `HttpResponseBadRequest("AI is required for this feature")`** (text/html).
4. `output_language = get_llm_output_language(ai_config, user)`: `PAPERLESS_AI_LLM_OUTPUT_LANGUAGE`/DB, sonst
   `user.ui_settings.settings["language"]`.
5. Cache: Schlüssel `llm_cache_backend = ":".join(p for p in (backend, model, endpoint, output_language, f"user={pk}") if p)`;
   `documents/caching.py`: Generations-Token pro Dokument (`…_llm_generation`, TTL 100 min), Variante
   `…_llm_{generation}_{sha256(backend)}`, `LLM_CACHE_CLASSIFIER_VERSION = 1002`. Gecacht werden nur die
   **rohen** `ClassificationSuggestions`, die Rechteauflösung läuft bei jedem Aufruf neu.
6. Kein Cache: `get_ai_document_classification(doc, user, output_language)`:
   * `ValueError` (Konfiguration, URL-Prüfung, pydantic-/JSON-Fehler, "kein Tool-Call") → **400 `{"ai": ["Invalid AI configuration."]}`**
   * `LLMTimeoutError` → **503 `{"ai": ["AI backend request timed out."]}`**
   * `LLMProviderError` (HTTP-Fehlerstatus des Anbieters) → **502 `{"ai": ["AI backend rejected the request. Check logs for details."]}`**
7. Auflösung je Kategorie: `resolve_*_ids(existing_ids, user)` (nur sichtbare, `restrict_queryset_to_owner_visible`)
   + `match_*_by_name(new_names, user)` (siehe 3.1), dedupliziert nach pk (erste Reihenfolge).

Antwort 200 (Schlüsselreihenfolge):

```json
{"title": "…", "tags": [1], "suggested_tags": ["…"], "correspondents": [2], "suggested_correspondents": [],
 "document_types": [], "suggested_document_types": [], "storage_paths": [], "suggested_storage_paths": [],
 "dates": ["2024-05-31"]}
```

`suggested_*` = `extract_unmatched_names(new_names, matched)`. Datum: Strings wie vom LLM (max. 3, nicht geprüft).
Nebenwirkungen: Cache-Schreiben; ggf. Celery `llmindex_index` (wenn der Index fehlt, s. 4.4).

### 1.2 `documents.chat` – `POST /api/documents/chat/`

Quelle: `ChatStreamingView` (`ensure_csrf_cookie`, `cache_control(no_cache=True)`), Rechte `IsAuthenticated`,
`ViewDocumentsPermissions`. Body `ChatStreamingSerializer`: `q` (CharField, Pflicht, `max_length=4000`),
`document_id` (int, optional, null erlaubt).

* `ai_enabled` falsch → 400 `HttpResponseBadRequest("AI is required for this feature")`.
* Validierung → 400 DRF-Feldfehler (`{"q": ["This field is required."]}`, `{"q": ["Ensure this field has no more than 4000 characters."]}`).
* `document_id` gesetzt: nicht vorhanden → **400 `HttpResponseBadRequest("Document not found")`**; ohne
  `view_document` (owner-aware, Root-Dokument) → 403 `HttpResponseForbidden("Insufficient permissions")`.
  Sonst: alle `permitted_document_ids(user)`; `unrestricted = user_is_unrestricted(user)` (aktiver Superuser).
* Antwort **200 `StreamingHttpResponse`, `Content-Type: text/event-stream`, aber ohne SSE-Framing**:
  der Körper sind die rohen Text-Deltas des LLM nacheinander, danach optional der Trailer
  `"\n\n__PAPERLESS_CHAT_METADATA__" + json.dumps({"references": [{"id": …, "title": …}, …]}, separators=(",", ":"))`
  (ensure_ascii=True, max. 3 Referenzen, Reihenfolge der Treffer, nur erlaubte Dokumente,
  Titel aus Knoten-Metadaten → `document.title` → `document.filename`).
  Keine Dokumente → einziges Stück `"Sorry, I couldn't find any content to answer your question."`;
  jede Ausnahme im Stream → `"Sorry, something went wrong while generating a response."` (Status bleibt 200).
* `paperless/middleware.py`: Antworten mit `text/event-stream` werden **nicht komprimiert**.
* Abruf: `VectorIndexRetriever(similarity_top_k=5, filters=…)`; Filter: unrestricted → `NIN` Papierkorb-IDs,
  sonst `IN` erlaubte IDs; Synthese llama_index "compact/refine" mit `chat_qa.j2`/`chat_refine.j2`,
  `PromptHelper(context_window=context_size, num_output=512, chunk_overlap_ratio=0.1, chunk_size_limit=chunk_size)`.
  Lesesperre (`read_store`) während des ganzen Streams.

### 1.3 Weitere upstream-Berührungspunkte

* `GET /api/ui_settings/` → `settings.ai_enabled = AIConfig().ai_enabled` (pl-auth-api).
* `GET /api/status/` → `tasks.llmindex_status` `DISABLED`/`OK`/`WARNING` ("No LLM index update tasks found")/`ERROR`,
  `llmindex_error`, `llmindex_last_modified` (pl-system).
* `/api/config/` → AI-Felder von `ApplicationConfiguration` (pl-system).
* Workflow-Aktion `APPLY_AI_SUGGESTIONS = 8` (`ai_suggestion_fields`, `ai_create_missing`, `ai_overwrite_existing`),
  Celery `documents.tasks.apply_ai_suggestions(action_id, document_id)` (autoretry bei `LLMTimeoutError`, max 3).

---

## 2. Konfiguration

`paperless/settings/__init__.py` ("AI Settings") – Umgebung, beim Start geprüft:

| Variable | Typ / Standard | Prüfung |
|---|---|---|
| `PAPERLESS_AI_ENABLED` | bool, `NO` | `str_to_bool` (true/1/t/y/yes, false/0/f/n/no, sonst Fehler) |
| `PAPERLESS_AI_LLM_EMBEDDING_BACKEND` | `huggingface` \| `openai-like` \| `ollama` \| unset | Auswahl |
| `PAPERLESS_AI_LLM_EMBEDDING_MODEL` / `_API_KEY` / `_ENDPOINT` | str | – |
| `PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE` | int, 1024 | ≥ 1 |
| `PAPERLESS_AI_LLM_CONTEXT_SIZE` | int, 8192 | ≥ 1 |
| `PAPERLESS_AI_LLM_REQUEST_TIMEOUT` | int s, 120 | ≥ 1 |
| `PAPERLESS_AI_LLM_BACKEND` | `ollama` \| `openai-like` \| unset | Auswahl |
| `PAPERLESS_AI_LLM_MODEL` / `_API_KEY` / `_ENDPOINT` | str | – |
| `PAPERLESS_AI_LLM_OUTPUT_LANGUAGE` | str | – |
| `PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS` | bool, `true` | – |
| `PAPERLESS_AI_LLM_EXTRA_PARAMS` | JSON-Objekt, `{}` | muss dict sein |
| `PAPERLESS_LLM_INDEX_TASK_CRON` | cron, `10 2 * * *` | Beat: `documents.tasks.llmindex_index`, expires 23 h |

`AIConfig` (`paperless/config.py`): jedes Feld = `ApplicationConfiguration.<feld> or settings.<FELD>`
(`ai_enabled` nur bei `NULL` aus der Umgebung); `llm_allow_internal_endpoints`, `llm_extra_params` nur Umgebung.
`llm_index_enabled = ai_enabled and llm_embedding_backend`. Standardmodelle: Ollama `llama3.1`, OpenAI-like
`gpt-3.5-turbo`; Embedding `text-embedding-3-small` / `sentence-transformers/all-MiniLM-L6-v2` (HF-Cache
`DATA_DIR/hf_cache`) / `embeddinggemma`; Ollama-Endpunkt `http://localhost:11434`.
Rust: `pl_ai::config::AiConfig::from_env_map` + `with_app_config` (getestet).

---

## 3. Logik

### 3.1 Namensabgleich (`matching.py`) – exakt portiert (`pl_ai::compat`)

* `MATCH_THRESHOLD = 0.8`.
* `_normalize(s) = re.sub(r"[^\w\s]", "", s.lower()).strip()` – Python-Unicode-Semantik: `\w` = `isalnum()` oder `_`
  (kombinierende Akzente **fallen weg**: `"école"` → `"ecole"`, `"İ"` → `"i"`), Emojis/Zero-Width fallen weg,
  `½`, `Ⅰ`, `ﬁ` bleiben; `strip()` inkl. U+001C..U+001F. 24 Golden-Fälle grün.
* `_match_names_to_queryset`: Pool = `model.objects.filter(pk__in=permitted_object_ids(user, model, "view_<m>"))`
  in `ORDER BY name`; pro Name (leer/None übersprungen) exakt (normalisiert, erster Treffer), sonst
  `difflib.get_close_matches(target, pool_names, n=1, cutoff=0.8)` (`autojunk` ab 200 Zeichen, Gleichstand →
  lexikographisch größerer String); Treffer verlässt den Pool. `user=None` (Workflow ohne Eigentümer) → nur
  Objekte ohne Eigentümer.
* `extract_unmatched_names(names, matched)` vergleicht normalisiert.
* Bekannte upstream-Schwächen (Golden `match_pool`): `"Swisscom AG"` trifft `"Swisscom (Schweiz) AG"` **nicht**
  (Ratio 0.73); `"Steuern 2025"` trifft fälschlich `"Steuern 2024"`. → Erweiterung 5.1.

### 3.2 Klassifikation (`ai_classifier.py`, `taxonomy.py`)

1. `get_taxonomy_context(document, user)`: mit Embedding-Backend `retrieve_similar_nodes(top_k=15,
   document_ids=None bei aktivem Superuser, sonst permitted_document_ids)`, Abfragetext
   `truncate_embedding_query(title + "\n" + content)`; ohne Backend Tantivy `more_like_this_ids(limit=15)`,
   Gewicht `15 - rang`, nachgeprüft mit `permitted_document_ids`. Fehler → leere Kandidaten, leerer Kontext.
2. `build_taxonomy_candidates`: Gewichte je Objekt aufsummieren (Posteingangs-Tags ausgenommen), nur sichtbare
   (`restrict_queryset_to_owner_visible`), absteigend, max. 10 Tags / 5 je Einzelwert.
3. Kontextblock: max. 5 ähnliche Dokumente, je `TITLE: <title|filename|Untitled>\n<content[:1000]>`, `\n\n`.
4. Prompt: `classification.j2` (Inhalt `content[:4000]`, dann `truncate_content` per tiktoken-Tokenizer),
   eingebettet in `classification_rag_context.j2`. Kandidatenblock `taxonomy_block.j2` mit
   `json.dumps({kategorie: [{"id","name"}]}, ensure_ascii=False)` (nur nichtleere Kategorien, Reihenfolge
   tags, document_types, correspondents, storage_paths).
5. `client.run_llm_query(prompt, allowed_candidate_ids=…)` → `model_to_classification_suggestions`: eine Zuordnung
   `matched_*[i]`/`*_ids[i]` gilt nur, wenn der Name in der (nicht-leeren) Namensliste steht, die ID für diese
   Kategorie gezeigt wurde und noch nicht verwendet ist; der Name fällt dann aus `new_names`.
6. Lokalisierung (wenn `output_language`): zweiter Aufruf mit `localization.j2`
   (`language_name = get_language_name(code)` aus `settings.LANGUAGES`, sonst Code) und
   `classification_suggestions_to_model(…).model_dump_json()`; `existing_ids` immer aus dem Original,
   Korrespondenten und Daten nie lokalisiert, leere lokalisierte Listen → Original.
7. Während des LLM-Aufrufs gibt upstream die DB-Verbindung frei (`db_connection_released`, #12976).

### 3.3 Prompts (1:1, `pl_ai::prompts`)

Jinja2 `trim_blocks=True, lstrip_blocks=True, keep_trailing_newline=False, autoescape=False,
undefined=StrictUndefined`, Ausgabe `.strip()`. Vorlagen: `classification`, `classification_rag_context`,
`localization`, `taxonomy_block`, `chat_qa`, `chat_refine`. In `chat_*` sind `{context_str}`, `{query_str}`,
`{existing_answer}`, `{context_msg}` llama_index-Platzhalter (Python `.format()`), `output_language` wird mit
`replace("{","{{")|replace("}","}}")` maskiert. Rust: feste Textbausteine, **byte-genau gegen Jinja2 getestet**
(Golden `classification_prompts`, `rag_prompt`, `localization_prompt`, `chat_qa`, `chat_refine`).

### 3.4 JSON-Schema der LLM-Antwort (`base_model.py::DocumentClassifierSchema`)

Felder (alle außer `title` mit Standard `[]`): `title` (str, maxLength 128, **Pflicht**), `tags`/`matched_tags`
(str, max 8), `tag_ids` (int, max 10), `correspondents`/`matched_correspondents`/`correspondent_ids`,
`document_types`/…, `storage_paths`/… (je max 4), `dates` (str, max 3). `mode="before"`-Validator kürzt Listen
und Strings auf die Maximallänge, statt abzulehnen. Schema = `model_json_schema()` byte-genau in
`crates/pl-ai/testdata/classifier_schema.json` (Docstring/Beschreibungen sind Anweisungen an das Modell).

### 3.5 LLM-Aufrufe auf dem Draht (mitgeschnitten, `pl_ai::llm`)

* **Ollama** `POST {endpoint}/api/chat`:
  `{"model", "stream": false, "options": {"temperature": null, "num_ctx": <context_size>, …EXTRA_PARAMS}, "format": <schema>, "messages": [{"role": "user", "content": <prompt>}], "tools": []}`.
  Antwort `message.content` = JSON. `think=False` wird **nicht** gesendet.
* **OpenAI-kompatibel** `POST {endpoint|https://api.openai.com/v1}/chat/completions`, `Authorization: Bearer <key|"fake">`:
  `{"messages": [{"role": "user", "content": <prompt> + "\n\nAnswer by calling the DocumentClassifierSchema tool. Do not write the answer as text."}], "model", "parallel_tool_calls": true, …EXTRA_PARAMS, "stream": false, "temperature": 0.1, "tool_choice": "required", "tools": [{"type": "function", "function": {"name": "DocumentClassifierSchema", "description": "Classification suggestions for a single document.", "parameters": {properties, required, type, "additionalProperties": false}, "strict": false}}]}`.
  Antwort `choices[0].message.tool_calls[0].function.arguments`.
* **`LLM_SYSTEM_PROMPT` wird von llama_index bei `chat()`/`chat_with_tools()` nicht gesendet** (upstream-Eigenheit, belegt im Mitschnitt).
* Ausgehende URL: `validate_outbound_http_url` (Schema http/https, Host Pflicht; ohne
  `ALLOW_INTERNAL_ENDPOINTS` keine privaten/Loopback-Adressen), upstream pinnt die aufgelöste IP.

### 3.6 Vektorindex (`vector_store.py`, `tables.py`, `indexing.py`)

* Ort `LLM_INDEX_DIR = DATA_DIR/llm_index`, Datei `llmindex.db` (SQLite, WAL, `synchronous=NORMAL`,
  Erweiterung **sqlite-vec**), Schema-Version 2.
* `documents` = `CREATE VIRTUAL TABLE documents USING vec0(id TEXT PRIMARY KEY, document_id INTEGER, +node_content TEXT, embedding float[<dim>] distance_metric=cosine)`;
  `document_chunks(chunk_id TEXT PK, document_id INTEGER NOT NULL)` + Index; `document_meta(document_id INTEGER PK, modified TEXT NOT NULL)`;
  `index_meta(key TEXT PK, value TEXT)` mit `dim`, `embed_model`, `schema_version`, `total_inserts`.
  `node_content` = llama_index `node_to_metadata_dict` (JSON), Embedding = float32 little-endian.
* Knoten: `build_llm_index_text` = `Notes: <n1,n2>` + `Custom Field - <name>: <value>` + `\nContent:\n` + Inhalt,
  dann `[._\-·]{4,}` → `" "` und `[ \t ]+` → `" "`; Metadaten (`document_id`, `title`, `tags`, `correspondent`,
  `document_type`, `filename`, `storage_path`, `archive_serial_number`, `created`, `added`, `modified`) nicht
  eingebettet; `SimpleNodeParser(chunk_size, chunk_overlap=min(200, chunk_size-1))`. Nur Root-Dokumente.
* Abfrage: `WHERE embedding MATCH ? AND k = ?` + Filter auf `document_id` (IN/NIN/NE, max 32 700 Werte, sonst
  "keine Treffer"), Ähnlichkeit `1 - distance`.
* Sperren: `LLM_INDEX_LOCK = llm_index/index.lock` (filelock, exklusiv für Schreiber),
  `LLM_INDEX_RWLOCK = llm_index/llmindex.rwlock.db` (filelock `ReadWriteLock` auf SQLite; Leser geteilt,
  Kompaktierung/Migration exklusiv, Timeout `LLM_INDEX_COMPACTION_LOCK_TIMEOUT = 30 s`, dann überspringen).
* Kompaktierung bei `total_inserts > 2 × live` per Datei-Tausch (`llmindex.db.compact`, `os.replace`).
* Aufgaben: `llmindex_index(rebuild)` (Beat), `update_document_in_llm_index(document)` /
  `remove_document_from_llm_index(document)` (Signal `document_consumption_finished`/`document_updated`
  bzw. `post_delete`; **Argument ist ein Python-`Document`-Objekt** → nicht aus Rust auslösbar).
  `queue_llm_index_update_if_needed`: kein laufender/kürzlicher (5 min) `LLM_INDEX`-Task → `llmindex_index.apply_async(kwargs={"rebuild": …}, headers={"trigger_source": "system"})`.
  Modellwechsel (`embed_model` ≠ konfiguriert) → Neuaufbau.

---

**Rust (`pl_ai::{vector_store, locks, node, embedding, tokenizer, punkt, server::index}`) – dasselbe Format:**

* `rusqlite` 0.32 (SQLite gebündelt) + `sqlite-vec` 0.1.9 (dieselbe Version wie das Python-Wheel der
  Referenz, als C-Quelle statisch gebaut, `sqlite3_auto_extension`). Gleiche Tabellen, gleiche SQL-Texte
  (`_INSERT`, `_build_where`, `query`, `upsert_document`, `delete`, `compact` per Datei-Tausch),
  `total_inserts`, `schema_version=2`, `embed_model`.
* `node_content` = byte-gleich `json.dumps(node_to_metadata_dict(node))` von llama_index 0.14.25:
  TextNode-Schlüsselreihenfolge, `relationships` (SOURCE "1" mit Document-Hash
  `sha256(metadata_str(ALL) + "-" + sha256(text))`, PREVIOUS "2" mit vollen Metadaten, NEXT "3" mit `{}`;
  `TextNode.hash = sha256(text + repr(metadata))`), `start_char_idx`/`end_char_idx` (`str.find`).
  Knoten-IDs `uuid4` (zufällig wie upstream).
* Zerlegung: `SentenceSplitter.split_text_metadata_aware` (Chunkgröße minus Tokens des längeren
  Metadaten-Strings EMBED/LLM), Absatz `"\n\n\n"` → **nltk-Punkt untrainiert** (`pl_ai::punkt`,
  von Hand nachgebaut) → `CHUNKING_REGEX` → `" "` → Zeichen; `_merge` mit Überlappung.
  Tokenizer **tiktoken-rs 0.12.1 `cl100k_base`** (`encode_with_special_tokens` = `allowed_special="all"`).
* Embedding-Text `get_content(EMBED)` = nur Text (alle Metadaten ausgeschlossen); LLM-Text
  `get_content(LLM)` = `"key: str(value)"`-Zeilen (ohne `document_id`) + `"\n\n"` + Text.
* Sperren wie `filelock` 4.2.6: `index.lock` exklusiv (Windows `LockFileEx`, Unix `flock`; Rust
  `File::lock`), `llmindex.rwlock.db` (SQLite `journal_mode=MEMORY`, Leser `BEGIN; SELECT … sqlite_master`,
  Kompaktierung `BEGIN EXCLUSIVE`, 30 s, sonst überspringen). Django-Worker und PaperRust können
  gleichzeitig arbeiten.
* Aufgaben für den Worker (W1): `server::index::llmindex_index(state, rebuild)` (=`documents.tasks.llmindex_index`,
  prüft `llm_index_enabled`), `update_document_in_llm_index(state, document_id)`,
  `remove_document_from_llm_index(state, document_id)` (**Dokument-ID statt Python-Objekt**),
  `llm_index_compact(state)`; `queue_llm_index_update_if_needed` reiht über `server::index::AiTasks`
  (eingehängt in `AppState::services`) oder – ohne Senke – über `pl_core::tasks::send_task` ein (bis
  2026-11-14: Celery).
* **Eingehängt (W10, pl-server `worker.rs`):** alle vier upstream-Aufgaben + `apply_ai_suggestions` unter den
  upstream-Namen registriert, `llm_index_compact` als `paperrust.ai.llm_index_compact`; `AiTasks` = PgQueue
  (immer; seit 2026-11-14 gibt es nur noch die PaperRust-Warteschlange); `document_consumption_finished` → `update_document_in_llm_index`
  (`{"document_id"}`) nach jedem erfolgreichen `consume_file`; `bulk_update_documents` →
  `update_llm_index(rebuild=False, document_ids)`; Beat „Rebuild LLM index“ aktiv. Belegt E2E
  (`crates/pl-server/tests/e2e_w10/`, siehe 7).
* Belegt: 195/195 Dokumente der Referenz-Kopie erzeugen in Rust dieselben Knoten (Text, Metadaten,
  `modified`) wie Django; ein von Rust neu gebauter Index wird von Django gelesen (Chat/ai_suggestions
  gleich); inkrementelles Update nach Rust-Neuaufbau meldet „No changes detected in LLM index.“


## 4. Entscheidungen: was 1:1 nach Rust (Stand W8)

| Teil | Stand | Beleg |
|---|---|---|
| Prompts, Schema, Antwortvalidierung, Kandidaten-Gewichtung, Abgleich (`matching.py`), Chat-Format, Konfiguration | portiert, golden | `tests/golden.rs` |
| `truncate_content` / `truncate_embedding_query` (`TokenTextSplitter` + `PromptHelper.truncate`, tiktoken) | **portiert** (`pl_ai::tokenizer`) | golden_w8: 40 Fälle inkl. Fehlertext „Calculated available context size … was not non-negative.“ |
| LLM Ollama / OpenAI-kompatibel (auch Streaming; System-Prompt bei `stream`) | portiert | golden_w8 `wire`, Parität (Mock-Mitschnitt gleich) |
| Embeddings Ollama / OpenAI-kompatibel (`/api/embed`, `/embeddings` base64, Stapel 10, `strip` bzw. `"\n"→" "`) | portiert (`pl_ai::embedding`) | golden_w8 `wire` |
| Embeddings **HuggingFace** (`huggingface`, Lücke C4) | **über HTTP**: text-embeddings-inference (lokal) oder HuggingFace-Inference-API, `AI_LLM_EMBEDDING_ENDPOINT` Pflicht | `embedding::tests::huggingface_*`, Known differences 10 |
| Vektorindex lesen/schreiben (sqlite-vec, gleiches Format, gleiche Sperren) | **portiert** | 3.6 |
| `GET …/ai_suggestions/` | **portiert** (`server::suggest`) | Parität 60/60 (5 Nutzer × 12 Dokumente), LLM-Anfragen byte-gleich, Cache geteilt 3/3 |
| `POST /api/documents/chat/` | **portiert** (`server::chat`, compact/refine-Streaming `pl_ai::synth`) | Parität 75/75 (5 Nutzer × 15 Körper), LLM-Anfragen gleich |
| Workflow-Aktion `apply_ai_suggestions` | **portiert** (`server::workflow`) | Parität 4/4 (Felder, Tags, neu angelegte Objekte, LLM-Anfragen) gegen die Celery-Aufgabe |

**Aktivierbare ids** (`routes.toml`, Parität belegt): `documents.ai_suggestions`, `documents.chat`.
Erweiterungen (kein upstream-Gegenstück, Freigabe durch den Koordinator): `ai.review.list`,
`ai.review.retrieve`, `ai.review.apply`, `ai.review.reject`, `ai.review.undo`, `ai.run`
(brauchen Migration `0001_paperrust_ai.sql`), `ai.mcp.post|get|delete`, `ai.mcp_scopes.list|create|delete`
(Migration `0002_paperrust_ai_mcp_scope.sql`).
`documents.chat` (seit 2026-11-20, `chat::request_data`): DRF-Standardparser – JSON wie bisher,
JSON-Parse-Fehler → 400 `JSON parse error - …` (CPython-Text über `pl_core::pyjson`), Formular/Multipart
→ `q` = letzter Wert, `document_id` = letzter Wert (`""` → `null`, `allow_null`), andere Content-Types →
415 `Unsupported media type "…" in request.` (vorher alles außer JSON „nicht unterstützt“ → 404).

### 4.1 `ai_suggestions` in Rust (Ablauf wie upstream)

`require_model_perms(view_document)` → pk wie Django (`int()`, sonst 500) → `Document.objects` (ohne
Papierkorb) → 404 → `has_perms_owner_aware(change_document, Root)` → 403 Text → `AIConfig()` aus Umgebung +
`paperless_applicationconfiguration` → 400 Text → Ausgabesprache → Cache-Schlüssel → Cache (Djangos `default`-Cache über `pl_core::cache`,
prozesslokal oder `PAPERRUST_CACHE=postgres`; Schlüssel `:1:doc_<id>_suggest_llm_<generation>_<sha256[:16]>`,
Pickle 5 von `documents.caching.SuggestionCacheData`, byte-gleich mit upstream; Generations-Token `get_or_set` + `touch(6000)`,
Eintrag 3000 s, Treffer → `touch`) → sonst Klassifikation: RAG-Abruf (Index, `top_k=15`, Superuser ohne
Filter, sonst `IN permitted_document_ids`, `NE` Dokument; fehlender Index → `llmindex_index` einreihen)
bzw. Tantivy-MLT (pl-search) → Kandidaten → Kontext (5 × 1000 Zeichen) → Prompt (tiktoken-Kürzung) →
LLM → ggf. Lokalisierung → Cache → Auflösung (sichtbare `existing_ids` nach Namen + `matching.py`,
dedupliziert) → 200. Fehler: 400 `{"ai": ["Invalid AI configuration."]}`, 503 timed out, 502 rejected
(503/502 mit `Cache-Control: no-cache`, 400 ohne – wie upstream, da per Exception).

### 4.2 `chat` in Rust

Jede Antwort (auch 401/403/400) mit `Cache-Control: no-cache`, CSRF-Cookie (`ensure_csrf_cookie`) und
`Vary: Cookie`. Prüfungen in upstream-Reihenfolge (DRF-Rechte → `ai_enabled` → Serializer → Dokument →
Recht). Stream: Lesesperre, Abfrage-Embedding, `top_k=5` mit `NIN Papierkorb` (unrestricted) bzw.
`IN erlaubte`, Referenzen (bis zu `MAX_CHAT_REFERENCES = CHAT_RETRIEVER_TOP_K` = 5, upstream-Fix e0c33b3d3;
v3.4.2: 3), **kein** zweiter Abruf mehr (upstream-Fix 45a2ac487: Synthese direkt aus den Treffern; v3.4.2
rief über `RetrieverQueryEngine.query` erneut ab), Lesesperre nur bis nach dem Abruf (upstream-Fix a4543c03d),
compact/refine mit
`PromptHelper(context, 512, 0.1, chunk)` inkl. System-Prompt-Tokens, Padding 500/5, `extendleft`-Umkehr,
`SafeFormatter` (kein `{{`-Entmaskieren); nur der letzte LLM-Aufruf wird gestreamt; Trailer; jede
Ausnahme → `CHAT_ERROR_MESSAGE` (Status 200).

## 5. Erweiterungen (paperless-rs, kein upstream-Pfad)

### 5.1 Taxonomie-Abgleich (`pl_ai::{normalize, similarity, taxonomy, alias}`)

* Normalisierung: NFC→NFKD, Akzente weg, ß→ss, æ/œ/ø/ł/đ/þ; **zwei Varianten** (ä→a und ä→ae);
  Satzzeichen trennen, `&`/`+` = "und"; Abkürzungspunkte zusammenziehen (S.A.→sa, e.V.→ev, S.p.A.→spa);
  mehrteilige Rechtsformen (GmbH & Co. KG, sp. z o.o.); Rechtsformen DE/FR/IT/EN/NL/Nordics und
  Länderzusätze (Schweiz/Suisse/Svizzera/…) **nur bei Korrespondenten**, Füllwörter DE/FR/IT/EN überall; Kern nie leer.
* Score (`score_names`): exakt 1.0 · nur Groß/klein 1.0 · upstream-normalisiert gleich 0.99 · gleicher Kern 0.97 ·
  gleich ohne Leerzeichen 0.95 · gleiche Wörter andere Reihenfolge 0.94 · sonst max(½JW+½Levenshtein, difflib,
  0.95·Token-Set), Teilmenge (≥4 Zeichen) mind. 0.9 · **Zahlen verschieden → höchstens 0.75** ·
  bestätigter Alias 1.0. Jeder Kandidat mit `reasons[{code, detail}]` und `upstream_match`.
* Schwelle Standard 0.8 (`PAPERLESS_RS_AI_MATCH_THRESHOLD`); `check_duplicate` blockiert zusätzlich alles, was
  upstream `matching.py` zuordnen würde.
* Modus `Upstream` = exakt `matching.py`; Modus `Extended` = obige Erweiterung.
* Rechte: nur sichtbare Objekte (`Principal::can` = `permitted_object_ids`; REST-Backend nutzt die API des Nutzers).
* Speicherpfade werden zusätzlich gegen die Pfadvorlage verglichen. Custom Fields: nur Name.
* **Aliase**: Tabelle `plrs_taxonomy_alias` (Migration `crates/pl-ai/migrations/0001_plrs_ai.sql`, expand-only,
  **nicht ausgerollt**): `(kind, target_id, alias, alias_key, source, created_by_id, created, note)`, eindeutig
  `(kind, alias_key, target_id)`. Regeln (`alias::validate_alias`): nicht leer, nicht gleich dem Zielnamen,
  kollidiert nicht mit dem Kern eines anderen sichtbaren Objekts; nur mit `change`-Recht am Ziel; sichtbar nur,
  wenn das Ziel sichtbar ist. Ohne Tabelle: lesen leer, schreiben → Fehler "alias storage not installed".

### 5.2 MCP-Server `/mcp` (`pl_ai::mcp`)

Transport **Streamable HTTP**, MCP 2025-06-18 (ausgehandelt auch 2025-03-26, 2025-11-25):

| Fall | Antwort |
|---|---|
| `POST` JSON-RPC-Request | 200 `application/json` |
| Notification/Response | 202, leer |
| Batch (Array) | 400, `-32600` |
| kein JSON | 400, `-32700` |
| ohne/ungültiges Token, inaktiver Nutzer | 401, `WWW-Authenticate: Bearer realm="paperless-rs-mcp"` |
| Token gültig, aber `/api/ui_settings/` verboten (keine Rechte) | 403 |
| fremder `Origin` (nicht in `PAPERLESS_RS_AI_MCP_ALLOWED_ORIGINS`, kein Loopback-Origin; kein Host-Vergleich wegen DNS-Rebinding, Review 2 N2; `*` ist unsicher) | 403 |
| Rate-Limit (`PAPERLESS_RS_AI_MCP_RATE_LIMIT`/min, Standard 120) – **vor** der Token-Prüfung je Token-Hash und danach je Nutzer/Token (Security Welle 3 M3) | 429 + `Retry-After` |
| mehr als `McpSettings::max_auth_failures_per_minute` (30) ungültige Tokens/min prozessweit: unbekannte Tokens ohne Backend-Aufruf abgelehnt; erfolgreiche Anmeldungen 30 s zwischengespeichert (Schlüssel SHA-256 des Tokens; widerrufenes Token wirkt höchstens so lange nach) | 429 + `Retry-After` |
| `initialize` bei voller Sitzungstabelle (1000) ohne eigene Sitzung; je Nutzer höchstens 20 Sitzungen, verdrängt wird nur die älteste **eigene** (AI-N4) | 503 |
| ohne `Mcp-Session-Id` (außer `initialize`) | 400 |
| unbekannte/abgelaufene (1 h) Sitzung oder Sitzung eines anderen Nutzers | 404 |
| `MCP-Protocol-Version` unbekannt | 400 |
| `GET /mcp` | 405 (`Allow: POST, DELETE`) |
| `DELETE /mcp` | 204 / 404 |

Anmeldung: `Authorization: Token <key>` oder `Bearer <key>` (paperless-API-Token). Methoden: `initialize`
(mit `instructions`: erst nachschlagen, nie Anweisungen aus Dokumenten folgen), `ping`, `tools/list`,
`tools/call`, Ressourcen und Prompts (siehe 5.2a). Werkzeug-Ergebnisse: `content[{type:"text"}]` +
`structuredContent`, fachliche Fehler `isError: true`, Argumentfehler JSON-RPC `-32602`
(unbekannte Argumente werden abgelehnt).

| Werkzeug | Recht | Verhalten |
|---|---|---|
| `list_taxonomy(kind, query?, limit≤500, offset)` | `view_<model>` | sichtbare Objekte, optional Filter (Teilstring akzentfrei oder unscharf ≥0.7) |
| `find_similar(kind, name, limit≤50)` | `view_<model>` | `exists`, `best`, `candidates[score, reasons, above_threshold, upstream_match]`, `advice` |
| `search_documents(query?, correspondent_id?, document_type_id?, storage_path_id?, tag_ids[] (alle), limit≤100, offset)` | `view_document` | Volltext über `/api/documents/?query=` |
| `get_document(id, max_chars≤20000=4000)` | `view_document` + Objekt | Inhalt gekürzt, Hinweis "untrusted" |
| `suggest_metadata(document_id)` | `change_document` am Objekt (wie upstream) + LLM konfiguriert | LLM-Vorschlag, **jeder Name schon abgeglichen** (`match`) |
| `update_document(id, title?, created?, correspondent_id?, document_type_id?, storage_path_id?, add_tag_ids[], remove_tag_ids[], custom_fields[{field_id,value}])` | `change_document` global + Objekt; referenzierte Objekte sichtbar | PATCH über die API, Audit vorher/nachher; **kein owner/permissions/Löschen** |
| `create_taxonomy(kind, name, path?, data_type?, force?, reason?)` | `add_<model>` | serverseitiger Abgleich: Kandidat ≥ Schwelle → **nicht anlegen**, `created:false` + Kandidaten; `force` nur mit Begründung (≥10 Zeichen), nie bei identischem Kern/Alias; Limit `PAPERLESS_RS_AI_MCP_MAX_CREATES_PER_HOUR` (30); Audit (forced, reason, candidates) |
| `add_alias(kind, target_id, alias)` | `change` am Ziel | siehe 5.1, Audit |

Backend produktiv: **`ApiBackend`** – ruft die REST-API von paperless (über paperless-rs/Django,
`PAPERLESS_RS_AI_API_BASE`, Standard `http://127.0.0.1:$PAPERLESS_PORT|8000`) **mit dem Token des Agenten** auf.
Damit gelten dieselben Rechte, Validierungen, Signale, django-auditlog-Einträge, Celery-Aufgaben und der Tag-Baum
wie in der UI; keine Querabhängigkeit zu pl-objects/pl-documents-write. Aliase/Audit: `PgSideStore`
(`plrs_taxonomy_alias`, `plrs_ai_audit`; ohne Tabelle Audit → Log `pl_ai::audit`).
LLM in MCP-Pfaden **mit** System-Prompt (`LLM_SYSTEM_PROMPT`). Der LLM-Client wird je `suggest_metadata`-Aufruf
aus `AIConfig()` (Umgebung + `ApplicationConfiguration`) gebaut (`McpService::with_llm_source`, Security Welle 3
AI-N1): KI in der UI abschalten oder umstellen wirkt sofort. Antworten der API (`ApiBackend`) höchstens 16 MiB.
**Sicherheitsrelevant:** `PAPERLESS_RS_AI_API_BASE` bekommt die Nutzer-Tokens – nur auf PaperRust/Django zeigen
lassen (Standard Loopback).

#### 5.2a Erweiterung „zweites Gehirn“ (Ingest, Wissen, Ressourcen, Prompts, Scopes)

Methoden zusätzlich: `resources/list` (25 zuletzt hinzugefügte sichtbare Dokumente),
`resources/templates/list` (`paperrust://document/{id}`, `…/{id}/metadata`), `resources/read`
(Fehler `-32002` „Resource not found“ für fehlend **und** nicht sichtbar), `prompts/list`, `prompts/get`
(`summarize_document`, `file_document`, `ask_archive`; die ersten beiden betten das Dokument als
Ressource ein, max. 20 000 Zeichen). `initialize.capabilities`: `tools`, `resources`, `prompts`.

**Werkzeuggruppen** (`pl_ai::mcp::scopes`): `read` | `write` | `ingest`. Wirksam = Admin-Schalter
`PAPERRUST_AI_MCP_TOOL_GROUPS` (Standard `read,write`) ∩ Token-Scopes (`paperrust_ai_mcp_scope`,
Migration `0002_paperrust_ai_mcp_scope.sql`; Schlüssel SHA-256 des Tokens, sonst Eintrag je Nutzer, sonst
alles Freigeschaltete). `tools/list` zeigt nur wirksame Werkzeuge; Aufruf eines nicht freigegebenen
Werkzeugs → `isError` („not available …“). Ressourcen/Prompts brauchen `read`. Scope-Fehler der DB →
JSON-RPC `-32603` (fail-closed); fehlende Tabelle → kein Eintrag. Scopes erweitern nie die Nutzerrechte.
Verwaltung `GET|POST /api/x-ai/mcp-scopes/`, `DELETE /api/x-ai/mcp-scopes/{id}/` (aktive Superuser;
POST `{"token"|"user_id", "scopes":[…], "label"?}` → 201, Antwort ohne Token, nur 8 Zeichen des Hashes;
Audit `channel='ui'`, `mcp_scope_set`/`mcp_scope_delete`). Route-ids `ai.mcp_scopes.list|create|delete`.

| Werkzeug | Gruppe | Recht | Verhalten |
|---|---|---|---|
| `add_document(file_base64+filename \| text(+text_format) \| url, title?, created?, correspondent_id\|correspondent?, document_type_id\|document_type?, storage_path_id?, tag_ids[], tags[], custom_fields[{field_id\|field, value}], create_missing?)` | ingest | `add_document` (+ `view_*` der Objekte; `add_*` bei `create_missing`) | genau eine Quelle; Größe ≤ `PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` (base64 vorab gemessen); Typ am Inhalt erkannt (PDF, PNG, JPEG, TIFF, GIF, WebP, Text/CSV/Markdown, OOXML/ODF/OLE nach Endung); Dateiname bereinigt; Namen → vorhandene Objekte per `check_duplicate` (≥ Schwelle), sonst Fehler, mit `create_missing` (braucht Gruppe `write`) duplikatgeschützt anlegen (Audit `create_taxonomy`, reason `add_document create_missing`, Create-Limit); Custom Fields nie anlegen; `POST /api/documents/post_document/` (multipart, `custom_fields` als JSON-Objekt id→Wert) → `task_id`; Limit `PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR`; Audit `add_document` (task_id, Dateiname, MIME, Größe, SHA-256, Quelle ohne Query, Zuordnungen) |
| `get_task_status(task_id)` | read | wie `GET /api/tasks/?task_id=` | kompakt: status, file_name, result, related_document, duplicate_documents |
| `ask_documents(question≤4000, document_ids[≤100]?, max_passages 1..20=8, answer=true)` | read | `view_document`; Treffer nur aus sichtbaren Dokumenten | Volltext (`search_documents`) + LLM-Index (`KnowledgeIndex`, Produktiv `server::mcp_index`: gleicher Index/Filter wie Chat) → RRF (k=60); jede Index-Passage zusätzlich per `get_document` des Nutzers geprüft; Antwort über `synth::compact_and_refine` mit `chat_qa`/`chat_refine` und `[n]`-nummerierten Chunks; Ausgabe `index_state`, `retrieval` (`hybrid`/`fulltext`), `answer`, `answer_error`, `citations[{n, document_id, title, page, snippet, source, score}]`, `notes` |
| `get_document_content(id, page? \| chunk?, chunk_chars 500..20000=8000)` | read | `view_document` + Objekt | Seiten über `\f` im Inhalt, sonst Abschnitte; `next` |
| `list_recent_documents(limit≤50=10)` | read | `view_document` | `ordering=-added` |
| `list_saved_views()` / `run_saved_view(id, limit≤100, offset)` | read | `view_savedview` + `view_document` | Regeln → Query wie upstream `queryParamsFromFilterRules` (`mcp::saved_view`), `ordering` aus sort_field/sort_reverse; unbekannter Regeltyp → Fehler |
| `get_custom_fields(document_id?)` | read | `view_customfield` / `view_document` | Definitionen oder Werte mit Namen |
| `set_custom_field(document_id, field_id\|field, value)` | write | wie `update_document` | Feld nur vorhanden (Abgleich), `null` entfernt; Audit `set_custom_field` |
| `list_notes(document_id)` / `add_note(document_id, note≤5000)` | read / write | wie `GET/POST /api/documents/{id}/notes/` + `change` am Dokument | Audit `add_note` |

Ingest-Pfad: die Consumer-Aufgabe (PgQueue) übernimmt Workflows, OCR, Zuordnung und danach
`update_document_in_llm_index` (bestehender Hook) – MCP baut keinen eigenen Index.
Körpergrenze `POST /mcp`: Route-Layer `DefaultBodyLimit` = 4/3 × max. Upload + 256 KiB; in pl-server
gilt zusätzlich das Body-Gate (2.5 MiB, größere Körper → Djangos 400-Seite), daher Standard 1.5 MiB (CORE-REQUEST:
`ai.mcp.post` in `upload::STREAMING_ROUTES`, dann größere Werte möglich).

### 5.3 Automatische Verarbeitung (`pl_ai::auto`, `server::review`, `server::workflow`)

* **Upstream-Aktion** `APPLY_AI_SUGGESTIONS` (8): `server::workflow::apply_ai_suggestions(state, action_id,
  document_id)` = `documents.tasks.apply_ai_suggestions` (Eigentümer als Nutzer, `ai_suggestion_fields`,
  `ai_create_missing` = `get_or_create(name[:128], owner)`, nie Speicherpfade, `ai_overwrite_existing`,
  `resolve_date`, `save(update_fields=[…, "modified"])` auch wenn nur Tags, `add_nested_tags`, danach
  Cache-Löschung + `index_document`). Fehler: `ValueError` → nichts tun; `LLMTimeoutError` →
  `AiTaskError{autoretry: true}` (W1: `autoretry_for`, max 3, Backoff 60…600); `LLMProviderError` → Fehler.
  Schreiben über `server::workflow::DocWriter` (Standard `SqlDocWriter`, siehe Known differences 11).
* **Erweiterung – Modi**: `suggest` (nur Prüfliste) oder `apply` (ab `PAPERLESS_RS_AI_AUTO_MIN_CONFIDENCE`,
  Standard 0.9). Konfidenz: vom Modell gewählte gezeigte Kandidaten 0.95, Abgleich-Score, Titel/Datum 0.6,
  neue Objekte 0.5 (also nie automatisch). Mehr als 8 neue Tag-Namen → nichts automatisch.
* **Duplikat-Sperre**: Planung mit `taxonomy::check_duplicate` (erweiterter Abgleich + Aliase; blockiert auch
  alles, was upstream `matching.py` zuordnen würde); **unmittelbar vor dem Anlegen erneut geprüft**.
  Neue Objekte nur mit `PAPERRUST_AI_AUTO_CREATE_MISSING=yes`, höchstens `…_MAX_NEW_OBJECTS` (3) je Lauf,
  bei Übernahme aus der Prüfliste nur mit `add_<model>`-Recht.
* **Grenzen**: `…_MAX_DOCUMENTS_PER_RUN` (50), 8 Änderungen je Dokument. Manueller Lauf (Security Welle 3 M1):
  höchstens 10 Dokumente je Aufruf, `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` (100) je Nutzer (im Prozess, 429).
* **Rechte bei Neuanlagen** (Security Welle 3 M2): nur, wenn der Auslöser – bei systemausgelöster Verarbeitung
  (Tag, Einlesen) der Dokument-Eigentümer – `documents.add_<model>` hat (wie DRF `POST /api/<model>/`); sonst
  bleibt die Änderung offen (nur Vorschlag). Dokument ohne Eigentümer und ohne Auslöser → nie anlegen.
* **Übernahme aus der Prüfliste** (AI-N2): zugewiesene bestehende Objekte müssen für den Übernehmenden sichtbar
  sein (`view_<model>` owner-aware, sonst 403); neue Objekte gehören dem Übernehmenden.
* **Audit**: jede Übernahme `paperrust_ai_audit(action='update_document', before, after, run_id,
  confidence=min)`; Rückgängig `action='undo'`, `undone_by_id`. Ohne Tabelle → Log `pl_ai::audit`.
* **Rückgängig**: nur wenn die betroffenen Felder noch dem `after`-Stand entsprechen, sonst
  **409** `{"detail": "...", "fields": [...]}`; Tags: nur die hinzugefügten werden entfernt.
* **Auslöser**: manuell `POST /api/x-ai/run/`, Tag (`server::review::process_trigger_tag(state)`, im Worker
  als `paperrust.ai.process_trigger_tag` mit Beat-Eintrag „PaperRust: process AI trigger tag“, nur mit
  `PAPERLESS_RS_AI_AUTO_TRIGGER_TAG` und Modus ≠ off, Zeitplan `PAPERRUST_AI_TRIGGER_TAG_CRON`, Standard
  `*/5 * * * *`; entfernt den Tag), Workflow (upstream-Aktion), nach dem Einlesen (Worker reiht
  `paperrust.ai.process_document{document_id, trigger: "consumed"}` ein →
  `server::review::run(state, &[id], mode, "consumed", None)`).
* **Prompt-Injection**: kein Werkzeug-Calling in der Automatik; nur festes Schema; IDs nur aus gezeigten
  Kandidaten (upstream-Allowlist); keine Löschung/Rechte/Eigentümer/Mails/Webhooks.

Routen (alle `IsAuthenticated` + `view_document` global; Einträge nur für sichtbare Dokumente;
Schreiben `change_document` global **und** owner-aware am Dokument):

| id | Route | Antwort |
|---|---|---|
| `ai.review.list` | `GET /api/x-ai/review/?status=&document=&page=&page_size=` | `{"count", "results": [Eintrag…]}`; `page_size` 1…100 (Standard 25), Seite außerhalb des Zahlenbereichs → 404 `Invalid page.` |
| `ai.review.retrieve` | `GET /api/x-ai/review/{id}/` | Eintrag; fremd → 404 |
| `ai.review.apply` | `POST …/{id}/apply/` `{"changes": [Index…]}` (optional; Standard alle offenen) | Eintrag (`applied`/`partial`); entschieden → 400 |
| `ai.review.reject` | `POST …/{id}/reject/` | Eintrag (`rejected`, bei `partial` → `applied`) |
| `ai.review.undo` | `POST …/{id}/undo/` | Eintrag (`undone`); Konflikt 409; nichts übernommen → 400 |
| `ai.run` | `POST /api/x-ai/run/` `{"documents": [ids], "mode": "suggest" oder "apply"}` | `{"run_id", "mode", "results": [{document, review, status, applied, pending, error}], "skipped"}`; fehlendes Recht an einem Dokument → 403; `PAPERLESS_RS_AI_AUTO_MODE=off` → 403, `=suggest` und `mode=apply` → 403; mehr als 10 Dokumente → 400; Kontingent je Nutzer/Stunde erschöpft → 429 |

Eintrag: `id, document, created, run_id, trigger, mode, status, suggestions, changes[{field, existing_id,
new_name, value, confidence, apply, why, applied}], audit, decided_by, decided_at`. Ohne Migration → 503.
Tabellen: Migration `crates/pl-ai/migrations/0001_paperrust_ai.sql` (expand-only, idempotent; umbenannt
von `0001_plrs_ai.sql`, Präfix `paperrust_`; ab W10 von pl-server beim Start eingespielt
(`pl_ai::server::migrations::apply`, bei Feature `ai` und PgQueue oder aktiver `ai.*`-Route; getestet in der
Kopie `pl_w10_test`):
`paperrust_taxonomy_alias`, `paperrust_ai_audit`, `paperrust_ai_review`.

### 5.4 paperless-ai/paperless-gpt eingebaut (Stand 2026-11-18)

Alles optional: ohne `PAPERLESS_AI_ENABLED` wirkt nichts davon; Optionen standardmäßig aus.

**Prüfliste – Erweiterungen von `ai.review.apply`** (`server::review::apply`): Körper
`{"changes": [i…], "overrides": {"<i>": {"value": …} | {"existing_id": n}}, "dismiss": [i…]}`.
* `dismiss` markiert Einzelvorschläge `dismissed: true` (Verwerfen je Feld); nur `dismiss` ohne `changes` → nichts
  wird übernommen. `overrides` („Ändern“) nur für Indizes aus `changes`: Titel (≤ 128, nicht leer), Datum
  (`YYYY-MM-DD`, `DD.MM.YYYY` …), Benutzerfeld (Normalisierung je Typ, s. u.), Objekte (`existing_id`, `new_name`
  wird gelöscht; Sichtbarkeitsprüfung AI-N2 gilt auch für geänderte IDs). Ungültig → 400 `{"overrides": {"<i>": [Grund]}}`.
  Geänderte Einträge bekommen `changed_by_user: true`, `confidence: 1.0`.
* Status aus allen Einträgen (`review_status`): alle entschieden und ≥ 1 übernommen → `applied`, alle verworfen →
  `rejected`, sonst `partial`/`pending`.
* `GET /api/x-ai/review/?status=open` = `pending` + `partial`.

**Benutzerfelder per KI** (`custom_fields`, `server::review::custom_field_plan`): Option
`custom_fields_enabled` + Liste `custom_fields` (Feld-IDs, je Feld einschaltbar). Nach der Klassifikation ein
zweiter Aufruf (`LlmClient::complete_json`, Vorlage `custom_fields`, Schema `{"fields": [{id, value, confidence}]}`).
Nur freigegebene, unterstützte Felder (alle Typen außer `documentlink`), höchstens 20, **nur leere Felder** (kein
Überschreiben). Normalisierung: Datum (gängige Schreibweisen → ISO), Betrag (`€ 1.234,50` → `EUR1234.50`,
Standardwährung des Feldes), Zahl/Ganzzahl (Tausendertrenner, i32-Bereich), Ja/Nein (mehrsprachig), Auswahl
(Bezeichnung oder Option-ID → Option-ID), URL (nur http/https), Text (≤ 128), Langtext. Ungültige Werte fallen weg.
Geplante Änderung: `field: custom_fields`, `existing_id` = Feld-ID, `value` = lesbar, `data` = Speicherwert.
Übernahme: `DocWriter::set_custom_field` (Standard SQL-Upsert inkl. Wiederbelebung weich gelöschter Instanz);
Audit/Rückgängig über `custom_fields: {"<id>": Wert|null}` in `before/after` (null → weich löschen).

**Eigene Prompts** (`prompt_store`, `server::store`, Tabelle `paperrust_ai_prompt`, Migration
`0003_paperrust_ai_prompts_jobs.sql`): Arten `classification` (Platzhalter `{{content}}`*, `{{filename}}`,
`{{candidates}}`, `{{candidate_rules}}`), `title` (`{{content}}`*, `{{filename}}`, `{{language}}`), `custom_fields`
(`{{content}}`*, `{{fields}}`*, `{{language}}`, `{{today}}`), `vision_ocr` (`{{language}}`), `chat_system` (–).
Ohne Zeile gilt die eingebaute Vorlage; für die Klassifikation bleibt dann der upstream-Prompt byte-genau (Golden).
Eingesetzte Werte werden nicht erneut nach Platzhaltern durchsucht (Dokumentinhalt kann keine Platzhalter auslösen).
`title`: nur mit gespeicherter Vorlage → eigener Titel-Aufruf in der Automatik (erste Zeile, ≤ 128).
`chat_system` ersetzt `LLM_SYSTEM_PROMPT` bei „Fragen ans Archiv“ (`LlmClient::with_system_text`).

| id | Route | Rechte | Antwort |
|---|---|---|---|
| `ai.status` | `GET /api/x-ai/status/` | angemeldet | `{enabled, configured, auto_mode, trigger_tag, pending, vision_enabled, custom_fields_enabled, can_configure, can_review}`; `pending` = offene Einträge über sichtbare Dokumente (nur mit `view_document`) |
| `ai.prompts.list` | `GET /api/x-ai/prompts/` | aktiver Superuser | `{"count", "results": [{kind, template, default, custom, placeholders, required, updated}]}` |
| `ai.prompts.update` | `PUT /api/x-ai/prompts/{kind}/` `{"template"}` | Superuser | Eintrag; ungültig → 400 `{"template": ["missing:content", "unknown:x", "empty", "too_long"]}`; unbekannte Art → 404 |
| `ai.prompts.reset` | `DELETE /api/x-ai/prompts/{kind}/` | Superuser | Eintrag mit eingebauter Vorlage |
| `ai.prompts.preview` | `POST /api/x-ai/prompts/{kind}/preview/` `{"document"?, "template"?, "run"?}` | Superuser + `view_document` am Dokument (sonst 404) | `{"prompt", "result", "error"}`; `run` ruft das Modell (Klassifikation: Rohschema, Titel: Text, Benutzerfelder: geprüfte Werte); `error` = Code (`ai_disabled`, `timeout`, `unauthorized`, …) |
| `ai.options.get` / `.patch` | `GET/PATCH /api/x-ai/options/` | Superuser | `{"options": AiOptions, "custom_fields": [{id, name, data_type, supported}]}`; unbekannter Schlüssel/ungültiger Wert → 400 |
| `ai.provider.check` | `POST /api/x-ai/provider/check/` `{"backend"?, "endpoint"?, "model"?, "api_key"?}` | Superuser | `{ok, models[], model_available, error}` (Ollama `/api/tags`, OpenAI `/models`; Zeitlimit ≤ 20 s; Adressprüfung wie LLM-Aufrufe; gespeicherter Schlüssel nur an den gespeicherten Endpunkt; Fehler nur als Code) |
| `ai.jobs.*` | `GET/POST /api/x-ai/jobs/`, `POST …/preview/`, `GET …/{id}/`, `POST …/{id}/cancel/`, `…/{id}/resume/` | Superuser | s. „Archiv nachbearbeiten“ |
| `ai.assist.verdict` | `POST /api/x-ai/assist/verdict/` `{"document"}` | `view_document` (owner-aware), 60/h je Nutzer | `AiVerdict` (s. u.) |
| `ai.assist.workflow_draft` | `POST /api/x-ai/assist/workflow-draft/` `{"description"}` | `documents.add_workflow`, 30/h je Nutzer | `WorkflowDraft` |

`AiOptions` (Tabelle `paperrust_ai_option`, Schlüssel = Feldname; Vorgabe aus `PAPERRUST_AI_VISION_ENABLED`,
`…_VISION_MODEL`, `…_VISION_WHEN`, `…_VISION_MAX_PAGES`, `…_VISION_MAX_PAGES_PER_DAY`, `PAPERRUST_AI_CUSTOM_FIELDS_ENABLED`):
`custom_fields_enabled` (false), `custom_fields` ([]), `vision_enabled` (false), `vision_model` (""), `vision_when`
(`off`|`difficult`|`always`, Standard `difficult`), `vision_limits {max_pages 10, max_image_bytes 8 MiB,
max_pages_per_day 500, max_tokens_per_page 4096}`, `tag_after` (`{"action": "remove"}` | `{"action": "replace",
"tag"}` | `{"action": "keep"}`), `backfill_per_minute` (6, 1…120), `backfill_max_documents` (2000).

**Auslöser-Tag** (`process_trigger_tag`): `tag_after` – `remove` (bisher), `replace` (Ersatz-Tag, global ohne
Eigentümer, bei Bedarf angelegt; nur bei Erfolg gesetzt), `keep` (Tag bleibt; Dokumente mit vorhandenem
Prüflisten-Eintrag werden übersprungen). Der Auslöser-Tag wird auch bei Fehlern entfernt (kein endloses Wiederholen).

**Archiv nachbearbeiten** (`server::jobs`, Tabelle `paperrust_ai_job`): Filter `JobFilter {tags (eines davon),
without_correspondent, without_document_type, without_tags, created_from, created_to, only_unprocessed (kein
Prüflisten-Eintrag), max_documents}` über `documents_document` (nicht gelöscht, keine Versionen, nur Dokumente mit
`change_document` des Auftraggebers, aufsteigende id). `POST /api/x-ai/jobs/` `{"mode", "filter"}`: KI aus → 400;
Modus-Regel wie `/api/x-ai/run/` (M1, 403); schon ein Auftrag aktiv → 409; sonst 201 mit Auftrag `{id, created,
created_by, run_id, mode, filter, status, total, processed, applied, suggested, errors, cancel_requested, heartbeat,
finished, message}`. Ausführung je Dokument `process_document(…, "manual", Auftraggeber, run_id)`, danach Pause
`60 s / backfill_per_minute`, Herzschlag ≤ 30 s. Abbruch über `cancel_requested` (nächster Durchlauf), KI aus →
`cancelled`. Herzschlag > 2 min → `interrupted` (beim Lesen markiert); `resume` setzt ab `last_document_id` fort.
Auftraggeber kein aktiver Superuser mehr → `failed`.

**KI-Texterkennung** (`vision`, `server::assist::{vision_ocr, vision_wanted}`): Seitenbilder (PNG/JPEG/WebP) je
Seite an das Bildmodell (`vision_model`, sonst Textmodell). Ollama `POST /api/chat` mit `messages[0].images =
[base64]`, `options.temperature 0`; OpenAI `POST /chat/completions` mit `content: [text, image_url(data:…)]`,
`max_tokens`. Antwort ohne `<think>`/Code-Zaun; Seiten mit Leerzeile verbunden. Grenzen: Seiten je Dokument (Rest
`skipped_pages`), Bildgröße, Seiten je Tag und Prozess. Entscheidung `vision_decision(enabled, when, requested,
ocr_chars, pages)`: gewählt → ja; `always` → ja; `difficult` → weniger als 50 Zeichen je Seite aus der normalen
Erkennung. **Verdrahtung (2026-11-18):** pl-core `vision::VisionOcr` (`plan`, `recognize`), Umsetzung
pl-server `worker::AiVisionOcr` (= `vision_wanted` + `vision_ocr`, Seitenzahl aus `vision_limits.max_pages`),
eingehängt in pl-consumer `Consumer::vision`. Nach der normalen Erkennung (Einlesen und
`update_document_content_maybe_archive_file`) rendert `Consumer::apply_vision` höchstens so viele Seiten
(pl-ocr `vision_pages`: PDF über PDFium bei 170 dpi, ≤ 6 MP, PNG bzw. JPEG ≤ 8 MiB; PNG/JPEG/WebP direkt;
andere Bilder bzw. Office über das Archiv-PDF) und ersetzt den Inhalt durch nicht leeren KI-Text; Fehler →
Warnung, normaler Text bleibt. Datum wird neu erkannt, wenn es aus dem Text stammte. `requested` = Feld
`ai_ocr` beim Hochladen (`POST /api/documents/post_document/`, Boolean, optional) bzw. `ai_ocr` in
`POST /api/documents/reprocess/` und `bulk_edit` `reprocess` (an die Aufgabe nur gesetzt weitergereicht).
Die Textebene des Archiv-PDFs bleibt von Tesseract.

**KI-Bedingung / Workflow-Entwurf** (`workflow_assist`, `server::assist`): `ai_verdict(state, doc_id, user) ->
Result<AiVerdict, String>` (Klassifikation + Abgleich im Modus „vorschlagen“, ändert nichts, kein Prüflisten-Eintrag;
`AiVerdict {document, title, created, document_type, correspondent, storage_path, tags[]}`, je `Opinion {id, value,
confidence, sure}`), `AiCondition {field, object_id, min_confidence = 0.9}.matches(&verdict)`.
`workflow_draft(state, description, user) -> Result<WorkflowDraft, String>`: Modell-Antwort nach
`draft_schema()`, bereinigt durch `sanitize_draft` → `{workflow: {name, order 0, enabled false, triggers[], actions[]},
missing[], explanation, dropped[]}`; Auslöser nur Typ 1–3 (Quellen 1–4, Dateiname, `match`/Algorithmus 0–5, Tag-/
Korrespondent-/Typ-Filter), Aktionen nur 1 (Zuweisen), 2 (Tags entfernen), 8 (KI-Vorschläge); nur sichtbare IDs;
E-Mail/Webhook/Rechte nie. Nichts wird gespeichert.

## 6. Abdeckung im Vergleich zu paperless-gpt und paperless-ai

Quellen: GitHub-README von `icereed/paperless-gpt` und `clusterzx/paperless-ai` (gelesen 2026-11-07; paperless-ai
laut README nicht mehr aktiv gepflegt). ✔ = vorhanden, ◐ = teilweise, ✘ = fehlt. Stand 2026-11-18.

| Funktion | paperless-gpt | paperless-ai | upstream 3.4.2 | PaperRust (pl-ai) |
|---|---|---|---|---|
| Titel/Tags/Korrespondent/Typ vorschlagen | ✔ | ✔ | ✔ (`ai_suggestions`) | ✔ (Parität; MCP `suggest_metadata`) |
| Erstellungsdatum | ✔ | ? | ✔ | ✔ |
| Speicherpfad | ✘ | ✘ | ✔ | ✔ (nie neu angelegt) |
| Benutzerfelder ausfüllen | ✔ (append/update/replace) | ? | ✘ | ✔ je Feld einschaltbar, typgerecht geprüft; nur leere Felder |
| Dokument-Link-Felder | ✔ | ✘ | ✘ | ✘ |
| Duplikatschutz | ◐ | ◐ | ◐ | **✔ erzwungen** |
| Rechte des Nutzers | ✘ | ✘ | ✔ | ✔ |
| Tag-Trigger (`paperless-gpt-auto`) | ✔ | ✔ | ◐ | ✔ (entfernen/ersetzen/behalten) |
| Automatik ohne Prüfung | ✔ | ✔ | ✔ | ✔ (ab Sicherheitsschwelle) |
| Prüf-Oberfläche | ✔ | ✔ | ✔ | ✔ PaperRust-Design `ai/review` (je Feld übernehmen/ändern/verwerfen, Rückgängig) |
| Archiv nachbearbeiten (paperless-ai „scan“) | ◐ | ✔ | ✘ | ✔ Auftrag mit Filter, Fortschritt, Abbruch, Ratenbegrenzung |
| Undo / Verlauf | ✔ | ? | ◐ | ✔ |
| Vision-OCR per LLM | ✔ (+DocAI, Azure, Docling, Mistral) | ✘ | ◐ | ✔ beim Einlesen/Neuverarbeiten (gewählt je Dokument, „immer“ oder wenig Text) |
| Durchsuchbares PDF aus Vision-OCR | ✔ | ✘ | – | ✘ |
| RAG-Chat | ✘ | ✔ | ✔ | ✔ |
| Ad-hoc-Prompt über Auswahl | ✔ | ✘ | ✘ | ◐ (Prompt-Vorschau an einem Dokument) |
| Prompts anpassbar | ✔ | ✔ | ✘ | ✔ (5 Aufgaben, Platzhalter-Hilfe, Zurücksetzen, Test am Dokument) |
| Anbieter | OpenAI/Azure, Ollama, Gemini, Mistral, Anthropic | Ollama, OpenAI, DeepSeek, OpenRouter, … | Ollama, OpenAI-kompatibel | Ollama, OpenAI-kompatibel (+ Verbindungstest, Modell-Liste) |
| MCP | ✘ | ✘ | ✘ | **✔** |
| Rate-Limits | ✔ | ? | ◐ | ✔ |

Verbleibende Lücken: durchsuchbares PDF aus KI-Text, Dokument-Link-Felder, eigene Anbieter-Adapter für
Gemini/Anthropic/Azure (heute nur über OpenAI-kompatible Endpunkte), Überschreib-Modi für Benutzerfelder,
Schalter „KI-Texterkennung“ in der Oberfläche (API-Feld `ai_ocr` vorhanden).

---

## 7. Tests

* `cargo test -p pl-ai -j 2`: 52 Unit-Tests, 11 Golden (`tests/golden.rs`), **11 Golden W8**
  (`tests/golden_w8.rs`: Tokenzahlen, TokenTextSplitter, `truncate_content`, nltk-Punkt, SentenceSplitter,
  Index-Knoten/`node_content` byte-gleich, Pickle byte-gleich, Python-Index lesen + Abruf mit gleichen Scores,
  Index schreiben/kompaktieren, Chat-Synthese compact/refine mit identischen Prompts und Stream, Drahtformat
  Embeddings/Streaming), 8 MCP-Tests. `cargo clippy -p pl-ai --all-targets -- -D warnings` sauber.
* **E2E Worker (W10)** `crates/pl-server/tests/e2e_w10/` (`reset_e2e.sh` → DB `pl_w10_test`, `run_mock.sh` → Mock :8131,
  `run_server.sh` → PaperRust :8130 `serve --worker`, ohne Django/Redis, `PAPERLESS_CACHE_BACKEND=…LocMemCache`,
  `e2e_ai_test.py`): Upload → `consume_file` → `update_document_in_llm_index` (PgQueue) → `llmindex.db` geschrieben
  → `ai_suggestions` 200 mit Mock-Ergebnis; Automatik `suggest` nach dem Einlesen (Prüflisten-Eintrag
  `consumed`); Auslöser-Tag über den Beat (Tag entfernt, auditlog-Eintrag über docops, Eintrag `tag`);
  `llmindex_index` „LLM index updated successfully.“; Migration beim Start. Lauf 2026-11-08: 20/21, der eine
  Fehlschlag war die Testannahme zum Titel (Mock antwortet mit Ausgabesprache „Übersetzt …“), danach korrigiert
  und **nicht erneut gelaufen**.
* Golden neu erzeugen: `PYTHONUTF8=1 reference/venv/Scripts/python.exe tests/parity/pl-ai/gen_golden_w8.py upstream/src crates/pl-ai/testdata`
  (nur lokaler Mock `tests/parity/pl-ai/mock_llm.py`).
* **Parität** `tests/parity_w8.rs` (`#[ignore]`, eigene Kopie `tests/parity/pl-ai/w8-{env,reset,start,stop}.sh`:
  DB `pl_w8_ref`, Django :8180 mit `PAPERLESS_AI_ENABLED`, Mock-LLM :8181, Redis-Präfix `w8`, Index per
  `manage.py document_llmindex rebuild`):
  `index_nodes_match_django_index` 195/195, `ai_suggestions_match_django` 60/60 + Draht 60/60 + Cache 3/3,
  `chat_matches_django` 75/75 + Draht 75/75, `workflow_apply_ai_suggestions_matches_django` 4/4,
  `rust_built_index_is_read_by_django` 5/5, `review_api_flow` (vorschlagen → übernehmen → rückgängig,
  apply-Modus, Konflikt 409, Rechte). Alles mit Ollama **und** mit `W8_LLM_BACKEND=openai-like
  W8_EMBED_BACKEND=openai-like` (Tool-Calling, SSE-Streaming, base64-Embeddings) gleich; ohne
  Embedding-Backend (`W8_EMBED_BACKEND=none`, MLT-Rückfall) Chat 75/75, ai_suggestions 58/60 (Known differences 12).

## 8. Known differences

1. `get_language_name`: englische Namen; upstream übersetzt `settings.LANGUAGES` per `gettext_lazy` in die
   aktive Sprache (Lokalisierungs-Prompt kann abweichen, wenn Django nicht Englisch spricht).
2. `compat::py_isalnum`: entspricht Python `str.isalnum` bis auf exotische `Other_Alphabetic`-Zeichen.
3. pydantic-Lax-Modus: Ganzzahl-Felder akzeptieren int, ganzzahlige floats, numerische Strings; `bool` abgelehnt.
4. LLM-URL-Prüfung: Prüfung + Pinning der aufgelösten Adresse (`reqwest::resolve`), keine Redirects.
5. MCP-Rate-Limit und Sitzungen nur im Prozess; MCP-Anmeldung per API-Token (kein OAuth 2.1).
6. MCP `search_documents` nutzt die Seitenlogik der API (`offset` wird auf Seiten gerundet).
7. `py_repr_str` (Metadaten-Strings, Knoten-Hash): nicht druckbare Zeichen nach Tabelle (Cc, Zs, Zl, Zp,
   häufige Cf, private Bereiche); seltene unzugeordnete Codepoints werden anders als in Python dargestellt.
8. nltk-Punkt: Python-`\s`/`\w` über `str.isspace`/`isalnum`-Nachbau; `isupper/islower` über Rust
   `char::is_uppercase/is_lowercase` (Unicode-Randfälle bei Titelschreibung).
9. Cache: Fehler des Caches (z. B. Tabelle `paperrust_cache` nicht erreichbar) werden protokolliert und als
   Fehlschlag behandelt (upstream: Redis-Ausnahme → 500). Seit 2026-11-14 kein Redis mehr.
10. HuggingFace-Embeddings (Lücke C4, 2026-11-18): upstream rechnet `HuggingFaceEmbedding`
    (sentence-transformers, Modell aus `DATA_DIR/hf_cache`, `normalize=True`, Stapel 10) im
    Python-Prozess. PaperRust rechnet nicht selbst (das bräuchte eine große neue Abhängigkeit wie
    ONNX Runtime oder candle samt Tokenizer und Modell-Download, nicht im lokalen Registry-Cache),
    sondern spricht die HTTP-Schnittstelle von HuggingFace: `POST <endpoint>/embed` eines
    **text-embeddings-inference**-Servers (TEI, lokal per Docker mit demselben Modell, Standard
    `sentence-transformers/all-MiniLM-L6-v2`) bzw. `…/models/<model>/pipeline/feature-extraction`
    der Inference-API (`*.huggingface.co`, Token aus `AI_LLM_EMBEDDING_API_KEY`/`AI_LLM_API_KEY`),
    Körper `{"inputs": [...], "normalize": true, "truncate": true}`, Antwort `[[f32…]…]`, Stapel 10.
    Unterschiede: `AI_LLM_EMBEDDING_ENDPOINT` ist **Pflicht** (ohne → Konfigurationsfehler mit
    Hinweis; nie ungefragt an einen Fremddienst); ein laufender TEI-/HF-Dienst ist nötig; Vektoren
    gleich bis auf Rundung (gleiches Modell, gleiche Normalisierung), ein vorhandener Index aus ngx
    bleibt nutzbar. Interne Adressen wie bei Ollama nur mit
    `AI_LLM_ALLOW_INTERNAL_ENDPOINTS` (Standard an).
    **Bewusste Abweichung (Stand 2026-11-18, zweite Prüfung):** ein Modell im eigenen Prozess wird
    nicht eingebaut. Gründe: neue große Abhängigkeit (ONNX Runtime bzw. candle + tokenizers, native
    Bibliotheken je Plattform), Modell-Download (~90 MB) beim ersten Start, dauerhaft ~150–300 MiB mehr
    Arbeitsspeicher im Server und CPU-Last im Web-/Worker-Prozess – widerspricht der Vorgabe
    „1. Geschwindigkeit, 2. Speicher“. Die Fehlermeldung ohne Endpunkt nennt als Ausweg ohne Zusatzdienst
    das Ollama-Modell `all-minilm` (gleiche Modellfamilie; Index danach neu aufbauen).
11. Workflow-Aktion/Automatik schreiben standardmäßig per SQL (`SqlDocWriter`): **keine
    django-auditlog-Einträge**, keine Datei-Umbenennung bei Speicherpfad-Wechsel (upstream `post_save`), keine
    auditlog-Einträge für neu angelegte Objekte; neue Tags werden über `pl_objects::objects::update_tree` korrekt
    in den Baum eingefügt. Ein `AiDocWriter` auf pl-consumer `docops` ist noch **nicht** eingehängt (W10: offen,
    pl-consumer bietet `save`/`add_nested_tags` nur auf `DocInstance` + Verbindung, die Dateiumbenennung
    `update_filename_and_move_files` ist privat – CORE-REQUEST an pl-consumer).
16. `apply_ai_suggestions` reiht nach einer Änderung `update_document_in_llm_index` mit `document_id` ein
    (upstream: `kwargs={"document": document}`); ohne PaperRust-Warteschlange schlägt das Einreihen fehl
    (Pickle-Aufgabe) und wird nur protokolliert.
12. Volltext-Rückfall ohne Embedding-Backend (Tantivy MLT, pl-search): MLT ist **schon upstream nicht
    deterministisch** (tantivy wählt Terme bei Gleichstand über eine HashMap, siehe `tests/parity/accepted.json`);
    in der Kopie unterscheiden sich dadurch Dokument 1 und seine Version 201 (Kandidaten/Prompt), auch
    Django gegen Django.
13. Strukturelle Index-Migration v1→v2 (upstream `m0001_v1_to_v2`) ist nicht portiert: ein v1-Index wird in
    Rust neu aufgebaut (Neu-Embedding statt Kopie).
14. `update_document_in_llm_index`/`remove_document_from_llm_index` bekommen die Dokument-ID (upstream:
    gepickeltes Python-Objekt, aus Rust nicht sendbar).
17. Chat (Security Welle 3 AI-N6): enthält die LLM-Ausgabe die Kennung `__PAPERLESS_CHAT_METADATA__`, wird sie
    durch `PAPERLESS CHAT METADATA` ersetzt (`chat::TrailerGuard`), damit eingeschleuste Ausgabe dem Frontend keine
    falschen Referenzen vortäuschen kann (upstream `chat.py:89` gibt sie unverändert weiter). Dafür werden am
    Stückende höchstens 26 Bytes zurückgehalten, wenn sie ein Anfang der Kennung sind; sonst byte- und stückgleich.
18. Antworten von LLM-, Embedding- und API-Endpunkt werden höchstens bis 16 MiB gelesen (auch der Chat-Stream
    insgesamt); größer → Fehler wie bei ungültiger Antwort (upstream unbegrenzt; Security Welle 3 AI-N5).
15. Chat: `Set-Cookie csrftoken` enthält wie Django ein zufälliges Geheimnis, wenn die Anfrage keines hatte
    (Parität vergleicht Status/Typ/Körper/`Cache-Control`).
19. Eigene Prompts (5.4): mit gespeicherter Vorlage `classification` weicht der Klassifikations-Prompt (auch für
    `GET /api/documents/{id}/ai_suggestions/`) bewusst von upstream ab; ohne Vorlage byte-genau. `chat_system`
    ersetzt den Systemtext auch im upstream-Chat-Pfad.
20. Benutzerfelder per KI und Rückgängig: Standard `SqlDocWriter` per SQL (ohne django-auditlog, ohne
    Dateinamen-Aktualisierung); im Server/Worker schreibt pl-server `ConsumerDocWriter` über pl-documents-write
    `cfi::update_or_create` / `cfi::soft_delete` (auditlog, `{{ custom_fields }}`-Dateinamen; Feldtyp aus der
    DB, abweichender Typ → Fehler). Rückgängig löscht neu angelegte Instanzen weich (`deleted_at`).
21. Auslöser-Tag: der Ersatz-Tag (`tag_after = replace`) wird ohne Eigentümer und ohne `add_tag`-Prüfung angelegt
    (vom Administrator konfiguriert).
22. Archiv nachbearbeiten läuft im Worker als Aufgabe `paperrust.ai.backfill` (kwargs `job_id`, Zeitlimit
    7 Tage statt 30 min; nur ohne Warteschlange im eigenen Prozess). Ein Worker-Neustart unterbricht einen
    laufenden Auftrag (fortsetzbar); eingereihte, noch nicht gestartete Aufträge gelten erst nach 6 h als
    unterbrochen. Der Auftrag belegt so lange einen Worker-Platz. Der Tagesbudget-Zähler der
    KI-Texterkennung gilt je Prozess.
23. Kein Proxy aus `HTTP(S)_PROXY`/`ALL_PROXY` für LLM-, Embedding- und MCP-Ingest-Clients
    (Sicherheitsprüfung 2026-11-19 F11; `.no_proxy()` wie die Webhook-Clients). Sonst löst der Proxy selbst
    auf und die Adressprüfung mit festgenagelter IP wäre wirkungslos. upstream (httpx, `trust_env`) nutzt den
    Proxy. Test: `tests/proxy_bypass.rs`.

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`, Ergänzung)

- **62d729e30 (#14754) – Chat über mehr als 32 700 Dokumente.** Quelle: `paperless_ai/chat.py`
  (`_permitted_documents_filter`, Nachfilter in `_stream_chat_with_documents`),
  `vector_store.py::_build_where` (`_MAX_INLINE_IN_VALUES`), `tables.py::DocumentMetaTable.all_document_ids`.
  Rust:
  - `vector_store::build_where`: IN/NOT IN mit mehr als `MAX_IN_VALUES` (32 700) IDs →
    `document_id IN|NOT IN (SELECT value FROM json_each(?))` mit **einem** JSON-Parameter
    (`WhereParam::Json`); v3.4.2 lieferte dann „1 = 0“ (keine Treffer).
  - `VecStore::indexed_document_ids()` (aus `document_meta`).
  - `chat::permitted_documents_filter(indexed, permitted)`: die kürzere Liste – `IN(erlaubt)` oder
    `NOT IN(indiziert − erlaubt)`; bei Gleichstand `IN`. Gewählt unter der Lesesperre des Index.
  - `chat::retain_permitted`: nach dem Abruf nur Knoten mit erlaubter `document_id` (ein NOT IN lässt
    ein inzwischen neu indiziertes, nicht erlaubtes Dokument durch).
  - Angewendet in `server/chat.rs` (Chat ohne `document_id` für eingeschränkte Benutzer) und – weil
    es „gleiche Filter wie der Chat“ nutzt – in `server/mcp_index.rs` (MCP-Suche).
  - PostgreSQL: die erlaubten IDs kommen als Abfrageergebnis (`permitted_document_ids`-Unterabfrage),
    nie als Bind-Parameter; das Limit von 65 535 Parametern wird nicht berührt.
  Tests: `vector_store::tests::large_in_and_not_in_use_json_each` (Grenze 32 700/32 701, echte
  vec0-Abfrage mit 45 000 IDs als IN und NOT IN), `chat::tests::permitted_filter_uses_shorter_list`.
- **7cdcd71a6 (#14728) – Hugging-Face-Modell aus dem Cache laden: nicht anwendbar.** PaperRust lädt
  keine Modelle selbst; `huggingface` spricht einen TEI-/Inference-API-Dienst (Known differences 10),
  der sein Modell selbst verwaltet; Ollama und OpenAI-kompatibel fragen keinen Hub.

## Ausfall-Befund C4 (tests/chaos, 2026-11-18)

- `GET /api/documents/{id}/ai_suggestions/` bei nicht erreichbarem KI-Dienst (`LlmError::Transport`):
  503 `{"ai": ["AI backend not reachable."]}` statt Djangos 500-Seite (upstream: unbehandelte Ausnahme).
- Workflow-Aufgabe `apply_ai_suggestions` und Index-Aufgaben: `ConnectError` wird wie `LLMTimeoutError`
  automatisch wiederholt (upstream nur Zeitüberschreitung).
