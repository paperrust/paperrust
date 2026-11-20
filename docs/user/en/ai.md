# AI features

[Deutsch](../ai.md) · [Documentation overview](README.md)

PaperRust can use a language model (LLM) to file documents faster and to answer questions about your
archive. You do not need add-ons such as paperless-gpt or paperless-ai for this. All AI features are
**off by default**. The usual automatic matching without AI ([matching](advanced.md#matching)) always
stays in place; AI only comes on top. Background and design (German): [docs/AI.md](../../AI.md).

> [!IMPORTANT]
> Once switched on, PaperRust passes the text of your documents, plus their metadata, to the
> configured model. With a cloud provider your documents therefore leave your own server, and costs may arise. For
> confidential papers use a local model (Ollama or your own OpenAI-compatible server).

<a id="enable"></a>

## Enabling

The easiest way is the interface: in the PaperRust design under *Settings → AI* (with
**Test connection**, see [below](#ai-settings)), in the classic interface under
*Settings → Configuration → AI* (in the PaperRust design also *Manage → Configuration → Application*).
Or use environment variables. A value set in the interface takes precedence over the environment
variable.

```sh
PAPERRUST_AI_ENABLED=true
PAPERRUST_AI_LLM_BACKEND=ollama                 # or openai-like (OpenAI or a compatible service)
PAPERRUST_AI_LLM_MODEL=llama3.1
PAPERRUST_AI_LLM_ENDPOINT=http://127.0.0.1:11434
PAPERRUST_AI_LLM_EMBEDDING_BACKEND=ollama        # turns on the AI index (chat, better suggestions)
PAPERRUST_AI_LLM_EMBEDDING_MODEL=nomic-embed-text
```

- **Language model:** `ollama` needs `PAPERRUST_AI_LLM_ENDPOINT` (address of the Ollama server).
  `openai-like` usually needs `PAPERRUST_AI_LLM_API_KEY` and – except for OpenAI itself – the
  service's endpoint.
- **Embeddings** (for the AI index): `ollama` or `openai-like`. Without their own values they use the
  language model's key and endpoint; separate values go into
  [`PAPERRUST_AI_LLM_EMBEDDING_API_KEY`](configuration.md#PAPERRUST_AI_LLM_EMBEDDING_API_KEY) and
  [`PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT`](configuration.md#PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT).
  `huggingface` works too, but differently from paperless-ngx: PaperRust does not run the model
  itself. Instead it asks a service with a HuggingFace interface – your own
  `text-embeddings-inference` server (e.g. as a Docker container in your own network) or the
  HuggingFace Inference API. For this, `PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT` is required; without it
  you get a clear error message, and no document is sent to an outside service. With the same model
  (default `sentence-transformers/all-MiniLM-L6-v2`) an AI index taken over from paperless-ngx keeps
  working. Running the model inside the PaperRust process is deliberately not supported (it costs a lot
  of memory and speed). If you use Ollama anyway, `ollama` with the model `all-minilm` is simpler (same
  model family; rebuild the AI index afterwards).
- **Language of suggestions:** [`PAPERRUST_AI_LLM_OUTPUT_LANGUAGE`](configuration.md#PAPERRUST_AI_LLM_OUTPUT_LANGUAGE);
  if unset, the user's interface language.

All variables: [Configuration → AI](configuration.md#ai).

<a id="replaces"></a>

## Instead of paperless-ai and paperless-gpt

Many paperless-ngx users run add-on programs such as **paperless-ai** or **paperless-gpt** for this.
Their core features are built into PaperRust – without a second container, without a separate API
token, and with the same permissions as in the interface:

| What you used it for | In PaperRust |
|---|---|
| Set title, tags, correspondent, type, date automatically | [Automatic processing](#auto) (*Only suggest* or *Apply certain suggestions*) |
| Check suggestions first, then apply them | [AI suggestions](#ai-review) in Automation: apply, change, dismiss or undo per field |
| Only process certain documents (tag "ai", "paperless-gpt") | **process by tag** (afterwards remove, replace or keep the tag) |
| Process older documents afterwards | **Process existing archive** (slowly in the background, can be paused) |
| Fill custom fields (amount, invoice number …) | **Fill custom fields**, switchable per field, values checked by field type |
| Text recognition with an image model (LLM OCR) | **AI text recognition** (only difficult pages, or always) |
| Own prompts | **Own prompts** per task, with a test on a document |
| Chat with your documents | [Chat](#chat) or "Ask a question" in the search, with sources |
| Rules with AI | AI condition and the action "Get AI suggestions" in [workflows](automatik.md#ai-condition) |
| Connecting agents | [MCP server](#mcp) |

What is different: PaperRust never creates lots of new tags without asking (only with *Create missing
tags, correspondents and types*, with a limit per run and a check for duplicates). Every AI change is
listed in the history with before/after and can be undone. And the text of a document can never make
the AI delete something, change permissions or send something ([Security](#security)). PaperRust
does not take over settings from paperless-ai or paperless-gpt; you set them once under
*Settings → AI*.

<a id="upstream-features"></a><a id="ai-features"></a>

## What the AI does

<a id="ai-suggestions"></a>

### Suggestions on a document

On the document page the AI proposes a title, a date, tags, a correspondent, a document type and a
storage path. Suggestions are only **shown** – nothing changes until you click them. PaperRust recognises
existing tags, correspondents and so on by name (small spelling differences included).

In *Settings* (documents section) you choose:

- **Suggestions come from:** *Both*, *ML only* (the built-in matching without AI) or *AI only*. With
  *Both* the results are merged.
- **Fetch suggestions for inbox documents automatically:** if this is off, ask for suggestions with
  the **Suggest** button.

When the AI index is on, the model also sees similar documents from your archive and makes better
suggestions.

<a id="chat"></a>

### Chatting with your documents

With AI switched on there is a chat: in the PaperRust design via "Ask a question" in the search
field (Ctrl + K) or the **Assistant**, and on a document ("Ask about this document"); in the classic
interface in the top bar. On a document page you ask that one document, elsewhere all documents you may see. The answer is based on the matching passages from the AI index
and links to its sources. The chat needs the AI index.

<a id="llm-index"></a>

### The AI index

The AI index stores a kind of "meaning fingerprint" (embeddings) for every document. That lets
PaperRust find passages that fit a question by content, even when they use different words (the
technical term is RAG).

- It is only built when AI is on **and** an embedding service is set.
- It lives under `DATA_DIR/llm_index` in the same format as in paperless-ngx; when switching over, an
  existing index can be reused.
- It is kept up to date on every document change and daily on schedule
  ([`PAPERRUST_LLM_INDEX_TASK_CRON`](configuration.md#PAPERRUST_LLM_INDEX_TASK_CRON)). To rebuild it
  by hand, see [Administration → AI index](administration.md#llm-index).

<a id="workflow-ai"></a>

### In workflows

The workflow action **Use AI suggestions** applies the suggestions automatically, for example right
after consumption ([workflows](workflows.md#workflow-action-apply-ai-suggestions)).

Suggestions, chat and the workflow action behave as in paperless-ngx; this was compared against a
paperless-ngx instance with a test model (details in docs/AI.md).

<a id="paperrust-extensions"></a>

## PaperRust extensions

These features exist only in PaperRust. They live under their own paths (`/api/x-ai/…`, `/mcp`);
all other API paths stay as in paperless-ngx.

<a id="taxonomy"></a>

### No duplicate correspondents, tags and types

Before an AI path (MCP, automatic processing) creates a correspondent, tag, document type, storage path
or custom field, PaperRust looks for equal or similar existing objects: case, umlauts/accents,
punctuation, legal forms (GmbH, AG, SA, Sàrl, Ltd, Inc, …), filler words, word order, similarity
measures. If a candidate is above the threshold (`PAPERRUST_AI_MATCH_THRESHOLD`, default 0.8), nothing
is created – the existing object is used. Confirmed matches ("Swisscom (Schweiz) AG" = "Swisscom") are
remembered as aliases.

<a id="mcp"></a>

### MCP server for external agents ("second brain")

With `PAPERRUST_AI_MCP_ENABLED=yes` PaperRust offers an MCP server at `/mcp` (Streamable HTTP, server
name `paperrust`). AI agents (Claude, Cursor, VS Code, your own agents) can use the existing document
archive as their knowledge base and file new documents – no second knowledge system to maintain.
Login with an API token (`Authorization: Bearer <token>` or `Token <token>`); the agent **always acts
with that user's permissions** (model and object permissions as in the UI). **No deleting** and no
owner/permission changes via MCP.

**Tools** (by group):

| Group | Tools |
|---|---|
| `read` | `search_documents`, `get_document`, `get_document_content` (by page/chunk), `list_recent_documents`, `list_saved_views`, `run_saved_view`, `ask_documents` (questions with citations), `get_custom_fields`, `list_notes`, `list_taxonomy`, `find_similar`, `suggest_metadata`, `get_task_status` |
| `write` | `update_document`, `set_custom_field`, `add_note`, `create_taxonomy` (with enforced matching), `add_alias` |
| `ingest` | `add_document` (file, text/Markdown or – only if allowed – URL) |

Plus **resources** `paperrust://document/{id}` (text) and `paperrust://document/{id}/metadata` (JSON)
and the **prompts** `summarize_document`, `file_document` and `ask_archive`.

**Asking the archive (`ask_documents`).** There is no second index: PaperRust uses the **existing
[AI index](#llm-index)** (the one the document chat uses; `PAPERRUST_AI_ENABLED`,
`PAPERRUST_AI_LLM_EMBEDDING_BACKEND` etc., built by the `llmindex_index` task). Retrieval is hybrid:
full-text index + AI index (same chunks, same similarity, same permission filtering as the chat); the
configured LLM writes the answer with the chat prompts. Every answer carries citations `[n]` with
document id, title, page (if the text has page breaks) and snippet.

| State | Behaviour |
|---|---|
| AI on, AI index built | hybrid (full text + AI index), LLM answer, citations |
| AI on, index not built yet | notice, index build is queued, answer from full-text passages |
| AI off | ranked full-text passages with citations only, no LLM answer |

Newly filed documents enter the index through the normal path (update after consumption, periodic
`llmindex_index` task); there is no separate embedding pipeline.

**Filing documents (`add_document`).** The file goes through the **normal processing pipeline** (the
same task as an upload: workflows, OCR, automatic matching, AI index). Allowed are the upload types
(PDF, images, text/CSV, Office/ODF), detected from the content, not the name. Text and
Markdown are stored as text documents. Correspondent, document type, tags and custom fields can be
given by id **or by name** – names are matched against existing objects with the same matching as
`create_taxonomy`; a name without match is an error (nothing is created silently), unless
`create_missing=true` (then it is created duplicate-protected; needs the `write` group as well).
Returns a task id; poll `get_task_status`. URLs only with `PAPERRUST_AI_MCP_ALLOW_URL_FETCH=yes`
(default off): http(s) only, public addresses only (same check as for LLM endpoints, address pinned
against DNS rebinding), every redirect re-checked, size limited.

**Settings**

| Variable | Default | Meaning |
|---|---|---|
| `PAPERRUST_AI_MCP_ENABLED` | `no` | serve `/mcp` |
| `PAPERRUST_AI_MCP_TOOL_GROUPS` | `read,write` | enabled groups (admin switch); `ingest` is opt-in |
| `PAPERRUST_AI_MCP_ALLOW_URL_FETCH` | `no` | `add_document` with `url` |
| `PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` | `1572864` | largest file for `add_document` (base64 in the call; larger files via `POST /api/documents/post_document/` with the same token) |
| `PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` | `30` | `add_document` per token and hour |
| `PAPERRUST_AI_MCP_RATE_LIMIT` | `120` | requests per minute and token |
| `PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` | `30` | new objects per token and hour |
| `PAPERRUST_AI_MCP_ALLOWED_ORIGINS` | empty | allowed browser origins |

At most 20 sessions per user, failed logins limited.

**Access control for companies: scopes per token.** On top of the user's permissions an
administrator can restrict per token (or as a default per user) which groups are allowed – e.g. a
research agent's token only `read`, a filing agent's token `read,ingest`. Effective is always the
intersection of `PAPERRUST_AI_MCP_TOOL_GROUPS` and the token's scopes; scopes never extend the user's
permissions. Without an entry all enabled groups apply. Only the SHA-256 of the token is stored (table
`paperrust_ai_mcp_scope`, works for API and device tokens). Management (superusers only):

```sh
# read-only token
curl -X POST https://paperrust.example.com/api/x-ai/mcp-scopes/ \
  -H "Authorization: Token <admin-token>" -H "Content-Type: application/json" \
  -d '{"token": "<agent-token>", "scopes": ["read"], "label": "research agent"}'
# default for all tokens of a user
curl -X POST .../api/x-ai/mcp-scopes/ ... -d '{"user_id": 7, "scopes": ["read", "ingest"]}'
curl .../api/x-ai/mcp-scopes/ -H "Authorization: Token <admin-token>"          # list
curl -X DELETE .../api/x-ai/mcp-scopes/<id>/ -H "Authorization: Token <admin-token>"
```

Tip: create a dedicated user for agents with exactly the permissions needed and use its token.

**Audit.** Every write action via MCP (`add_document`, `update_document`, `set_custom_field`,
`add_note`, `create_taxonomy`, `add_alias`) and every scope change is recorded with user, token prefix
(never the token) and before/after in `paperrust_ai_audit`.

<a id="mcp-clients"></a>

#### Connecting agents

URL `https://<your-server>/mcp`, login with `Authorization: Bearer <token>`.

**Claude Code**

```sh
claude mcp add --transport http paperrust https://paperrust.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

**Claude Desktop** (`claude_desktop_config.json`; stdio → HTTP bridge `mcp-remote`, needs Node.js):

```json
{
  "mcpServers": {
    "paperrust": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://paperrust.example.com/mcp", "--header", "Authorization:${PAPERRUST_AUTH}"],
      "env": { "PAPERRUST_AUTH": "Bearer <token>" }
    }
  }
}
```

**Cursor** (`.cursor/mcp.json`):

```json
{ "mcpServers": { "paperrust": { "url": "https://paperrust.example.com/mcp",
  "headers": { "Authorization": "Bearer <token>" } } } }
```

**VS Code** (`.vscode/mcp.json`, the token is asked for on start):

```json
{
  "servers": { "paperrust": { "type": "http", "url": "https://paperrust.example.com/mcp",
    "headers": { "Authorization": "Bearer ${input:paperrust-token}" } } },
  "inputs": [ { "type": "promptString", "id": "paperrust-token", "description": "PaperRust API token", "password": true } ]
}
```

**ChatGPT (connectors)** requires OAuth for custom MCP servers; PaperRust only supports bearer tokens so
far. Do not connect ChatGPT directly until then.

Other clients that only speak stdio connect via `npx mcp-remote <url> --header …` as well. Browser-based
clients send an `Origin` header – add it to `PAPERRUST_AI_MCP_ALLOWED_ORIGINS`. Expose the server via
HTTPS only (the token travels in a header).

<a id="auto"></a>

### Automatic processing

`PAPERRUST_AI_AUTO_MODE`:

| Value | Effect |
|---|---|
| `off` (default) | nothing automatic |
| `suggest` | suggestions go to the [AI suggestions](#ai-review) (`/api/x-ai/review/`) |
| `apply` | suggestions above `PAPERRUST_AI_AUTO_MIN_CONFIDENCE` (0.9) are applied, the rest goes to the review list |

Triggers: after consumption, by tag (`PAPERRUST_AI_AUTO_TRIGGER_TAG`, checked per
`PAPERRUST_AI_TRIGGER_TAG_CRON`) or manually (`POST /api/x-ai/run/`, at most 10 documents per call and
`PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` per user). New objects only with
`PAPERRUST_AI_AUTO_CREATE_MISSING=yes`, at most `PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` per run and only with
the `add_*` permission of the trigger user or owner. Every AI change is logged with before/after and can
be undone (with conflict check).

<a id="ai-settings"></a>

### Settings → AI (PaperRust design)

Administrators configure everything in one place: *Settings → AI*.

- **Use AI** – the main switch. Off = no AI feature has any effect.
- **Provider** – *Ollama* (own computer or network, documents stay with you) or an *OpenAI-compatible
  service* (OpenAI, LM Studio, vLLM, LocalAI …). **Test connection** fetches the provider's model list; pick
  the model from it.
- **What may happen automatically** – *Nothing automatic*, *Only suggest* (everything goes to the AI
  suggestions) or *Apply certain suggestions* (threshold selectable; the rest waits for you). Also: create
  missing tags/correspondents/types (off = existing ones only), **process by tag** (e.g. tag "AI"; afterwards
  remove the tag, replace it with another one – such as "AI done" – or keep it).
- **Fill custom fields** – per custom field. The AI reads e.g. amount, invoice number or due date from the
  document; values are checked for the field type (date, amount with currency, number, yes/no, select, text).
  Only empty fields are filled.
- **AI text recognition** – an image model (e.g. `qwen2.5vl`, `minicpm-v`, `llava`, `gpt-4o-mini`) reads
  difficult scans and handwriting, limited per document and per day. *When:* off, only for difficult
  pages (little recognised text, the default) or always. It runs after the normal text recognition
  during consumption and when you **Process again**; when uploading and processing again you can
  also switch it on for a single document. Without its own image model, PaperRust uses the text
  model. Settings: [`PAPERRUST_AI_VISION_*`](configuration.md#PAPERRUST_AI_VISION_ENABLED).
- **Advanced** – answer language, limits per run, pace of archive processing.
- **Own prompts** – editable per task (classification, title, custom fields, AI text recognition, ask the
  archive) with placeholder help (`{{content}}`, `{{filename}}` …), **Reset** and **Try on a document** (show
  the prompt or try it with the AI).
- **Process existing archive** – the AI goes through stored documents (e.g. "only without correspondent" or
  "only documents the AI has not seen yet"). *How many documents?* shows the number and the approximate
  duration first; the run is slow on purpose, shows its progress, can be stopped and continued after a restart.

<a id="ai-review"></a>

### Checking AI suggestions

With AI switched on, the **Automation** area has an **AI suggestions** tab; next to "Automation" the
navigation shows the number of open suggestions (also as a dashboard widget). The old address
`…/ai/review` redirects there. For each document you see the thumbnail, the old and new value per field and how certain
the AI is (*certain*, *likely*, *unsure*). Per field: **Apply**, **Change** (enter your own value) or
**Dismiss**; for all: *Apply certain ones*, *Apply all*, *Dismiss all*. **Undo** restores the previous state as
long as nobody has changed the document since. You only see suggestions for documents you may view and can
only decide for documents you may change.


<a id="security"></a>

## Security

- Document content is **untrusted** (prompt injection in PDFs): automatic processing may only suggest
  and assign – no deleting, no permission changes, no mails or webhooks.
- Permissions are always checked server-side; other users' data never reaches a prompt.
- For its tools the MCP server calls the PaperRust API and passes on the agent's token. The address
  it uses is `PAPERRUST_AI_API_BASE`; it must point only at PaperRust itself. If unset, PaperRust uses
  `http://127.0.0.1:<PAPERRUST_PORT>` (default port 8000). That fits the Docker image, which listens
  on this port. When you start `paperrust` directly, however, it listens on `PAPERRUST_BIND`
  (default `127.0.0.1:8080`) – then set `PAPERRUST_AI_API_BASE` to match, e.g.
  `http://127.0.0.1:8080`.
- Language model and embedding addresses inside your own network (e.g. a local Ollama) are allowed
  by default. With
  [`PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS`](configuration.md#PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS)`=false`
  PaperRust only accepts public addresses.

