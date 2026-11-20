# REST API

[Deutsch](../api.md) · [Documentation overview](README.md)

Everything the web interface can do is also available through the REST API: searching, uploading
and changing documents, managing tags, and so on. The API speaks JSON over HTTP.

**Compatible with paperless-ngx v3.4.2:** paths, fields, status codes and error texts are the same.
Apps and scripts that work with paperless-ngx v3.4.2 keep working with PaperRust.

A list of all endpoints that you can try out in the browser lives at `/api/schema/view/`. The
schema itself (OpenAPI) is at `/api/schema/`.

This page explains the parts that are not obvious from that list.

> [!NOTE]
> A few fringe parts of paperless-ngx do not exist in PaperRust and answer with 404: the Django
> admin (`/admin/`), the allauth "headless" login (`/api/auth/headless/…`), social login through
> providers other than OpenID Connect, `OPTIONS` requests that describe fields, and the `.json`/`.api`
> suffixes on paths (e.g. `/api/tags.json`). More under
> [Installation](installation.md#standalone-limits).

<a id="authorization"></a>

## Signing in

Every request has to say who is making it. There are four ways to do that.

1.  **User name and password (Basic)**

    Send the header `Authorization: Basic …`. Behind it is `username:password`, base64-encoded.
    curl does this for you with `-u`:

    ```bash
    curl -u anna:secret https://paperrust.example/api/documents/
    ```

2.  **Browser session**

    If you are signed in to PaperRust in your browser, you are signed in to the API too. No extra
    header is needed. Handy for testing in the API browser.

3.  **API token** (recommended for apps and scripts)

    A token is a long key that stands in for your password. To get one:

    - In the web interface: user menu → **My profile** → field **API token**. The button with the
      round arrow ("Create new API token") makes a new one; the old one stops working.
    - Through the API: `POST /api/token/` with `username` and `password` (JSON or form). The
      answer contains the token.

    ```bash
    curl -X POST -H "Content-Type: application/json" \
         -d '{"username": "anna", "password": "secret"}' \
         https://paperrust.example/api/token/
    # {"token": "9f2c…"}
    ```

    Then send this header with every request:

    ```
    Authorization: Token 9f2c…
    ```

4.  **Sign-in by a reverse proxy in front (remote user)**

    If a reverse proxy already signs the user in, PaperRust can take the user name from a header.
    This is off by default; switch it on with
    [`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER_API).

Signing in with OpenID Connect works in the web interface, see
[OpenID Connect](advanced.md#openid-connect-and-social-authentication). The allauth "headless" API,
which third-party apps could use for that, does not exist in PaperRust.

<a id="searching-for-documents"></a>

## Searching for documents

Searching happens on the normal list endpoint `/api/documents/`. All four parameters query the
search index; the parameter decides what kind of search:

| Parameter | What it does |
|---|---|
| `text=…` | looks for the term in title and content (simple part-of-word search) |
| `title_search=…` | looks only in the title |
| `query=…` | full-text search with a query language (words, `AND`/`OR`, fields), see [Searching](usage.md#basic-usage_searching) |
| `more_like_id=…` | finds documents similar to the document with this ID |

Example: documents from 2024 that mention "electricity":

```
GET /api/documents/?query=electricity%20created:2024&page_size=25
```

Page through results as usual with `page` and `page_size`.

With such a search, every hit carries an extra field `__search_hit__`:

```json
{
  "id": 42,
  "title": "Electricity bill March",
  "__search_hit__": {
    "score": 7.81,
    "highlights": "… monthly payment for <span class=\"match\">electricity</span> in …",
    "rank": 0
  }
}
```

- `score`: how well the hit matches. Higher is better; only useful compared with the other hits of
  the same search.
- `highlights`: a short snippet of the text, with the matches wrapped in `<span class="match">`.
- `rank`: position in the result list, counting from 0.

<a id="filtering-by-custom-fields"></a>

### Filtering by custom fields

`custom_field_query` filters by the values of custom fields. The value is a small JSON expression:
`[field, operator, value]`. Name the field by its name or ID.

Examples (URL-encode before sending):

| Looking for | Expression |
|---|---|
| field "Customer no." is exactly "K-1001" | `["Customer no.", "exact", "K-1001"]` |
| field "Amount" greater than 100 | `["Amount", "gt", 100]` |
| field "Due" in January 2025 | `["Due", "range", ["2025-01-01", "2025-01-31"]]` |
| field "Paid" is yes | `["Paid", "exact", true]` |
| select field "Account" is "Checking" or "Savings" | `["Account", "in", ["Checking", "Savings"]]` |
| document does not have the field "Contract" at all | `["Contract", "exists", false]` |
| document link "Attachments" points to 12 and 15 | `["Attachments", "contains", [12, 15]]` |
| "Note" empty or not set | `["OR", [["Note", "isnull", true], ["Note", "exact", ""]]]` |

Expressions can be combined with `"AND"`, `"OR"` and `"NOT"`, as in the last row.

Which operators fit which field type:

| Field type | Operators |
|---|---|
| all | `exact`, `in`, `isnull`, `exists` |
| text, URL, monetary | also `icontains`, `istartswith`, `iendswith` (case-insensitive) |
| integer, number, date | also `gt`, `gte`, `lt`, `lte`, `range` |
| document link | also `contains` (contains all the listed documents) |

**Relative dates:** for date fields you can write something like `"today"` or `"in 30 days"`
instead of a fixed date. Example – due within the next two weeks:

```json
["Due", "range", ["today", "in 14 days"]]
```

The expression is worked out again on every request, in the time zone from `PAPERRUST_TIME_ZONE`.
PaperRust understands simple English and German forms: `today`, `yesterday`, `tomorrow`, `now`,
`in N days`, `N weeks ago`, `heute`, `gestern`, `morgen`, `vorgestern`, `übermorgen`,
`vor N Tagen` (units from second to year). German applies when `PAPERRUST_DATE_PARSER_LANGUAGES`
is empty or contains `de`. Other forms are rejected with an error message. In workflow triggers
(`filter_custom_field_query`) only the English forms work.

<a id="apisearchautocomplete"></a>

### `/api/search/autocomplete/`

Suggests matching words while someone types.

- `term`: the start of the word, e.g. `inv`.
- `limit`: at most this many suggestions (default 10).

The answer is a plain list. First comes the word found in the most documents the user may see:

```json
["invoice", "invoices", "inventory"]
```

<a id="file-uploads"></a><a id="posting-documents"></a>

## Uploading documents

Send new files to `POST /api/documents/post_document/` as a multipart form. The file goes in the
field `document`; everything else is optional:

| Field | Content |
|---|---|
| `title` | title; without it PaperRust uses the file name |
| `created` | date, e.g. `2025-03-14` or `2025-03-14 09:30:00+01:00` |
| `correspondent`, `document_type`, `storage_path` | the ID of each |
| `tags` | tag ID; send the field several times for several tags |
| `archive_serial_number` | archive serial number (ASN) |
| `custom_fields` | list of field IDs (fields are added empty) or an object `{"field-id": value}` |

```bash
curl -H "Authorization: Token 9f2c…" \
     -F document=@electricity-bill.pdf \
     -F title="Electricity bill March" \
     -F tags=3 -F tags=8 \
     https://paperrust.example/api/documents/post_document/
# "b1d7c0e2-5a4f-4c1e-9a77-2f3e8d6c1a90"
```

The answer comes right away (HTTP 200) and only holds the ID of the intake task. The actual work
(reading the text, assigning tags and so on) then runs in the background. Ask for its state with
that ID:

```
GET /api/tasks/?task_id=b1d7c0e2-5a4f-4c1e-9a77-2f3e8d6c1a90
```

Once the task is done, the ID of the new document is listed there as well.

Large files are fine: PaperRust writes the upload straight to disk instead of holding it all in
memory. To recognise the file type it reads only the first 7 MiB.

<a id="document-versions"></a>

## Document versions

A document can have several copies of its file, for example the original and a redacted copy. The
first one is the **root document**; further ones hang off it as versions.

- **Shared by all versions:** title, correspondent, tags, document type, custom fields, storage
  path, permissions.
- **Own to each version:** the file itself, file type, checksums, archive file and recognised text.

Unless you say otherwise, the newest version is used. Pick a specific one with
`?version={version_id}`.

| Request | Effect |
|---|---|
| `GET /api/documents/{id}/` | the document's data; `content` is the text of the newest (or chosen) version |
| `PATCH /api/documents/{id}/` | changes to the text go to the newest (or chosen) version, everything else to the root document |
| `GET …/download/`, `…/preview/`, `…/thumb/`, `…/metadata/` | accept `?version=` |
| `POST /api/documents/{id}/update_version/` | upload a new version: field `document`, optional `version_label` |
| `POST /api/documents/merge_as_versions/` | turn existing documents into versions of one document: `documents` (at least two IDs), `root_document_id` (one of them); `version_label` only with exactly one source document |
| `PATCH /api/documents/{id}/versions/{version_id}/` | change a version's label (`version_label`) |
| `DELETE /api/documents/{id}/versions/{version_id}/` | delete a version (not the root) |

<a id="permissions"></a>

## Permissions

For documents, tags, correspondents and all other objects you can send the owner and permissions
when creating or changing them. Users and groups are given as IDs:

```json
{
  "owner": 4,
  "set_permissions": {
    "view":   { "users": [7, 9], "groups": [2] },
    "change": { "users": [7],    "groups": [] }
  }
}
```

`view` means see, `change` means edit. Sending `set_permissions` **replaces** the object's current
permissions. Only the owner or a superuser may do that.

<a id="retrieving-full-permissions"></a>

### Retrieving full permissions

Normally the API only returns a short field: `user_can_change` says whether the signed-in user may
edit the object. Add `full_perms=true` and you get the whole `permissions` field instead, built the
same way as `set_permissions` above.

<a id="bulk-editing"></a>

## Bulk editing

Two endpoints change many objects at once. The work runs in the background.

<a id="documents"></a>

### Documents

`POST /api/documents/bulk_edit/` with a list of documents, a method and its parameters. Example –
add tag 5 and remove tag 2 on three documents:

```json
{
  "documents": [101, 102, 117],
  "method": "modify_tags",
  "parameters": { "add_tags": [5], "remove_tags": [2] }
}
```

| `method` | `parameters` |
|---|---|
| `set_correspondent` | `{"correspondent": id}` |
| `set_document_type` | `{"document_type": id}` |
| `set_storage_path` | `{"storage_path": id}` |
| `add_tag` / `remove_tag` | `{"tag": id}` |
| `modify_tags` | `{"add_tags": [ids], "remove_tags": [ids]}` |
| `modify_custom_fields` | `{"add_custom_fields": {field-id: value}` (or a list of IDs, then empty), `"remove_custom_fields": [ids]}` |
| `set_permissions` | `{"set_permissions": {…}, "owner": id or null, "merge": false}` – format as under [Permissions](#permissions). `merge: true` adds to the existing permissions, `false` (default) replaces them. |
| `delete` | none |
| `reprocess` | none; optional `{"remote_ocr": true}` for [remote OCR](usage.md#remote-ocr) |

<a id="document-editing-operations"></a>

#### Editing files (rotate, merge and so on)

Since API version 10 these actions have their own endpoints, e.g. `/api/documents/rotate/`,
`/api/documents/merge/`, `/api/documents/edit_pdf/`, `/api/documents/remove_password/` and
`/api/documents/delete/`. Their fields are listed in the API browser. They still work through
`bulk_edit`, but only for the sake of old apps; new apps should use the dedicated endpoints.

<a id="objects"></a>

### Objects

`POST /api/bulk_edit_objects/` changes permissions or deletes – for tags, correspondents, document
types or storage paths:

```json
{
  "objects": [3, 4, 9],
  "object_type": "tags",
  "operation": "set_permissions",
  "owner": 4,
  "permissions": { "view": { "users": [7], "groups": [] }, "change": { "users": [], "groups": [] } },
  "merge": false
}
```

- `object_type`: `tags`, `correspondents`, `document_types` or `storage_paths`.
- `operation`: `set_permissions` or `delete`.
- `owner`, `permissions` and `merge` are only needed for `set_permissions`; `merge` works as above.
- Instead of a long ID list you can send `"all": true` with optional `"filters"` (API version 10).

<a id="api-versioning"></a>

## API versions

So that old apps do not break as the API evolves, it has version numbers. The app says with every
request which version it speaks and gets answers in exactly that format.

- PaperRust knows versions **9** and **10**.
- Without a version, **10** applies.
- The version goes in the `Accept` header:

  ```
  Accept: application/json; version=9
  ```

- An unknown version gets `406 Not Acceptable` with an explanation.

**Does my app fit the server?** Send any signed-in request. The answer carries two headers:

```
X-Api-Version: 10
X-Version: 3.4.2
```

`X-Api-Version` is the server's newest API version. `X-Version` reports **3.4.2**, the
paperless-ngx version PaperRust is compatible with – not PaperRust's own version number. That way
apps that check for paperless-ngx versions recognise the server correctly. Answers without
sign-in (e.g. share links under `/share/…`) do not carry these headers.

<a id="api-version-deprecation-policy"></a>

### How long old versions stay

When a new API version arrives, the previous one stays usable for at least one more year. After
that it may go away.

<a id="api-changelog"></a>

### API changes

A short overview of what changed with which version:

| Version | What is new |
|---|---|
| <a id="version-1"></a>1 | Starting point. |
| <a id="version-2"></a>2 | Tags have `color` (hex colour, e.g. `#4caf50`) and the read-only `text_color` (black or white, depending on the background). The old field `colour` is gone. |
| <a id="version-3"></a>3 | Endpoints for permissions; `/api/ui_settings/` has a new layout. |
| <a id="version-4"></a>4 | Consumption templates became workflows, with their own endpoints. |
| <a id="version-5"></a>5 | Documents and objects can be deleted in bulk. |
| <a id="version-6"></a>6 | Tasks are now acknowledged through `/api/tasks/acknowledge/`. |
| <a id="version-7"></a>7 | Select fields return their options as objects with `id` and `label`. A value is set by the option's `id`, no longer by its position. |
| <a id="version-8"></a>8 | On notes, the field `user` holds a small user object instead of just the ID. |
| <a id="version-9"></a>9 | `created` on a document is a plain date without time. `created_date` is deprecated. |
| <a id="version-10"></a>10 | See the list below. |

**Version 10 in detail** (version 9 keeps the old behaviour for as long as it is supported):

- Saved views no longer have `show_on_dashboard`/`show_in_sidebar`; that now lives in the UI
  settings (`/api/ui_settings/`).
- Rotate, merge, PDF editing and so on have their own endpoints (see
  [above](#document-editing-operations)).
- `bulk_edit_objects` accepts `all` and `filters`.
- The list parameter `all` is deprecated.
- The search parameter `title_content` is deprecated; use `text` (title and content) or
  `title_search` (title only).
- Tasks: `/api/tasks/` is paginated. Fields are called `task_type` (was `task_name`) and
  `trigger_source` (was `type`). New are `/api/tasks/summary/`, `/api/tasks/status_counts/`,
  `/api/tasks/active/` (read only) and `POST /api/tasks/run/` for starting permitted tasks.

<a id="paperrust-extensions"></a>

## PaperRust extensions

On top of the compatible API, PaperRust has endpoints of its own. They do not change the compatible
endpoints; apps written for paperless-ngx do not notice them.

| Path | What for |
|---|---|
| `GET /__rs/health` | sign of life: 200 while the server is running (for Docker, monitoring) |
| `/__rs/brand/…` | logo and favicon (custom branding) |
| `/api/x-ai/…` | AI: review list (`/api/x-ai/review/`), start a run by hand (`POST /api/x-ai/run/`), MCP scopes – see [AI](ai.md) |
| `/mcp` | built-in MCP server for AI assistants (sign-in with an API token); only with `PAPERRUST_AI_MCP_ENABLED=yes` |
| `/api/x-scan/…` | scanners, scan jobs and scan devices – see [Scanners](scanner.md) |
| `/api/documents/{id}/redact/…`, `/api/documents/{id}/shrink/` | PDF tools, see below |
| `/rs/api/v2/…` | new PaperRust API (e.g. [ASN labels](asn-labels.md)); can be switched off with `PAPERRUST_API_V2=off` |

<a id="pdf-werkzeuge"></a>

### PDF tools

Redacting and shrinking, as described under [PDF tools](pdf-tools.md).

- **Permission:** "change document", both in general and on this document.
- **Source:** always the newest version of the document, and it has to be a PDF.
- **Result:** a new version of the same document. It is read in the background like an upload; the
  answer contains the `task_id`.
- **Pages** count from 1. **Rectangles** are `[x0, y0, x1, y1]` in PDF points (1/72 inch), origin
  bottom left – the same values pdf.js returns from `convertToPdfPoint`.

| Request | Body (example) | Answer |
|---|---|---|
| `POST /api/documents/{id}/redact/search/` – find matches, change nothing | `{"terms": ["Jane Sample", "DE89 3704"]}` | `document`, `version`, `source` (`original` or `archive`), `truncated`, `hits`: list with `page`, `term`, `context`, `rects` |
| `POST /api/documents/{id}/redact/` – redact | `{"areas": [{"page": 2, "rect": [72, 600, 300, 620]}], "terms": ["Jane Sample"], "remove_metadata": true, "version_label": "Redacted copy"}` | `task_id`, `document`, `source_version`, `report` (counts: `marks`, `pages`, `glyphs`, `images_removed`, `images_cleared`, `paths_removed`, `paths_clipped`, `annotations_removed`, `metadata_removed`, `term_hits`) |
| `POST /api/documents/{id}/shrink/` – shrink | `{"preset": "standard", "version_label": "Small"}` | `task_id`, `document`, `new_version` (`true`/`false`), `report` (`preset`, `size_before`, `size_after`, `images`, `images_resampled`, `images_recompressed`, `unused_objects_removed`, `streams_compressed`) |
| `POST /api/documents/{id}/convert_pdf/` | `{"ocr": false, "version_label": "PDF"}` – image or office file as a new PDF version ([Convert to PDF](pdf-tools.md#in-pdf-umwandeln)); `ocr` for images only | `{"task_id", "document", "source_version", "kind": "image"\|"office", "ocr"}`; 400 `This document is already a PDF.` / `This document cannot be converted to PDF.`, 503 without OCR engine or with office conversion switched off |

Without `version_label` the new version is called `Redacted` or `Shrunk`. Shrink levels:
`lossless`, `standard` (default), `strong`. If the file does not get smaller, no new version is
made; then `task_id` is `null` and `new_version` is `false`.

**Errors:**

| Code | Meaning |
|---|---|
| 400 | wrong input (field errors) or `{"detail": "…"}`, e.g. page does not exist, nothing to redact, redaction could not be verified, not a PDF |
| 403 | permission missing |
| 404 | unknown document |
| 422 | tool gave up (file broken or too large) |
| 503 | tool `paperrust-pdftool` not installed |
| 504 | time limit exceeded |
