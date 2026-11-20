# Workflows

[Deutsch](../workflows.md) · [Documentation overview](README.md)

<a id="workflows"></a>

## Workflows

A workflow is a rule of the form **"when this happens, do that"**. It lets PaperRust do routine
work by itself: set tags, fill in an owner, send a mail, put a document in the trash and much more.

Every workflow has two parts:

- **Triggers** – *when* the workflow starts, and for *which* documents (filters).
- **Actions** – *what* then happens to the document, step by step.

A workflow can have several triggers and several actions. In the PaperRust design you manage
workflows in the **Automation** area – with sentence view, templates, test run, history and
otherwise branch; the [Automation](automatik.md) page explains it. In the classic interface they are
under *Manage → Workflows*. Each workflow has a name, an **Order** and an **On** switch.

This page is the reference for all triggers, filters, actions and placeholders. It uses the terms of
the classic interface; the names in the PaperRust design are listed in the table under
[Automation → Triggers](automatik.md#triggers).

**Order:** if several workflows fit a document, they run one after another by their order number,
and inside a workflow the actions run from top to bottom. If two actions set the same single value
(say, the correspondent), the later one wins. Lists such as tags, custom fields and permissions are
collected instead of replaced.

<a id="workflow-triggers"></a>

### Triggers

<a id="workflow-trigger-types"></a><a id="types"></a>

#### Kinds

There are five kinds of trigger. The names are the ones shown in the workflow dialog:

| Trigger | When it starts | What is already known |
|---|---|---|
| **Import started** | A new file arrives, *before* it is read. | Source, path, file name, mail rule. No text, no tags yet. |
| **Document added** | The new file has been fully processed. | Text, tags, type, correspondent and so on. Source and path no longer. |
| **Document changed** | Someone saves changes to a document. | Everything about the document. |
| **Scheduled** | At a point in time worked out from one of the document's dates. | Everything about the document. |
| **Manual** | Only when you start the workflow yourself. | Everything about the document. |

**Scheduled** in more detail: you choose which date to count from (**Added**, **Created**,
**Changed** or a **Custom field** of type date) and a **Shift in days**. A positive number means "that
many days after", a negative one "that many days before". Example: field "Notice deadline", shift `-14` –
the workflow runs two weeks before the deadline. Normally a scheduled workflow runs only once per
document. With **Repeating** it runs again once the set number of days has passed since the last run.
PaperRust checks scheduled workflows once an hour (five minutes past the hour; change it with
`PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON`).

**Manual** never runs on its own. How to start it is described under
[running a workflow by hand](#running-a-workflow-manually).

<a id="workflow-trigger-filters"></a><a id="filters"></a>

#### Filters

Without filters a trigger applies to every document. Filters narrow it down. A document must meet
**every** filter that is set.

**Filters on arrival** (only for **Import started**):

- **Sources** – where the document comes from: **Import folder**, **API upload**, **Mail import**,
  **Web interface**. Good to know: a scan from the Scan button counts as web interface, a scan sent by
  scan-to-email as API upload (see [Scanner](scanner.md)).
- **Path** – the file's path, `*` stands for any characters. Together with
  `PAPERRUST_CONSUMER_RECURSIVE` (subfolders in the import folder) you can, for example, give each
  subfolder its own owner: path `*/anna/*` → owner Anna.
- **Mail rule** – only documents fetched by this mail rule. This automatically limits the trigger to
  documents from mail import.

**File name** (all triggers): for example `*.pdf` or `*invoice*`. Upper and lower case do not
matter. For existing documents the original file name is checked.

**Filters on the document** (for **Document added**, **Document changed**, **Scheduled** and
**Manual**):

- **Content** – a search pattern for the document's text, using the same matching methods as tags and
  correspondents (for example **Any word**, **Exact text**, **Regular expression**).
- **More filters** (via **Add filter**):
  - Tags: **Has one of these tags**, **Has every one of these tags**, **Has none of these tags**
  - Correspondent: **Has correspondent**, **Has one of these correspondents**, **Has none of these
    correspondents**
  - Document type and storage path: the same three choices each
  - **Meets custom field condition** – a field condition, built like the one in the document list
    filter (for example "amount greater than 100").

For relative dates in field conditions PaperRust only understands simple English forms: `today`,
`yesterday`, `tomorrow`, `now`, `3 days ago`, `in 30 days` and the like.

<a id="running-a-workflow-manually"></a>

#### Running a workflow by hand

You start workflows with a **Manual** trigger yourself:

- **Several documents:** select documents in the document list, then choose **Run workflow** in
  the **Actions** menu (PaperRust design: the **Run workflow** button in the bar above the list).
- **One document:** in the document view choose **Run workflow** in the **Actions** menu
  (PaperRust design: the lightning icon at the top).

Then pick the workflow. Only enabled workflows with a Manual trigger are offered. The trigger's
filters still apply: documents that do not match are left out. If you do not want that, tick
**Run on every selected document**.

The workflow runs in the background. The document view shows the result by itself; a list may need
a reload.

**Who may do this?** You need permission to view workflows. You must also be allowed to change all
selected documents and be their **owner** (or they have no owner). The reason: a workflow can change
owners and permissions or delete documents. Superusers always may.

> [!NOTE]
> For people switching over: paperless-ngx 3.4.2 does not know the Manual trigger yet. The database
> stays compatible anyway. If you go back to paperless-ngx, such workflows stay stored but never run
> there.

<a id="workflow-actions"></a>

### Actions

<a id="workflow-action-types"></a>

#### Kinds

An action is one step the workflow carries out. These kinds exist:

<a id="workflow-action-assignment"></a><a id="assignment"></a>

##### Set values

Fills in values on the document:

- **Set title** – with [placeholders](#workflow-placeholders), e.g. `{{ correspondent }} {{ added_year }}`
- **Add tags**, correspondent, document type, **Set storage path**
- **Set owner**
- **Give view rights** and **Give edit rights** – to users and/or groups
- **Set custom fields** – with or without a value. Without a value the workflow only attaches the
  empty field; if the document already has the field, its value stays as it is. With a value, an
  existing value is replaced.

<a id="workflow-action-removal"></a><a id="removal"></a>

##### Remove values

Takes values off the document – either specific ones or, with **Remove all**, all of them:

- Tags, correspondents, document types, storage paths
- Owner
- View and edit rights
- Custom fields

<a id="workflow-action-email"></a><a id="email"></a>

##### Email

Sends a mail. Mail sending must be [set up](configuration.md#PAPERRUST_EMAIL_HOST) for this. You enter:

- **Mail recipients** – one or more addresses, separated by commas
- **Mail subject** and **Mail text** – with [placeholders](#workflow-placeholders)
- **Attach the document** – whether the file is sent along

PaperRust builds the mail at the moment of the action; the actual sending then carries on in the
background. So processing the document does not wait for the mail server.

<a id="workflow-action-webhook"></a><a id="webhook"></a>

##### Webhook

Sends a message (HTTP POST) to an address, for example a home automation or chat system. You enter:

- **Webhook URL** – only `http://` or `https://`
- the content: either **Send values as form fields** (then you fill in **Webhook fields** as
  name/value pairs) or free text in the **Webhook body**. Both may contain
  [placeholders](#workflow-placeholders).
- **Send the payload as JSON** – otherwise as form data
- **Webhook headers** – extra headers as name/value pairs
- **Attach the document** – whether the file is sent along

Webhooks can be misused to reach machines on your own network. The
[webhook settings](configuration.md#PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES) decide which ports and targets are allowed. If
ordinary users may create workflows on your system, keep these settings tight.

<a id="workflow-action-move-to-trash"></a><a id="move-to-trash"></a>

##### Move to trash

Puts the document in the trash. It stays there until the trash is emptied (by hand or after the
configured delay) – until then it can be restored.

- This action always comes **last**, wherever it sits in the list.
- After it, no further workflows run for this document.
- With the **Import started** trigger there is no document yet: the import is aborted and the file is
  deleted.

<a id="workflow-action-password-removal"></a><a id="password-removal"></a>

##### Remove password

Meant for password-protected PDFs: you store one or more **Passwords** (one per line), and the
workflow tries them in order. The passwords are saved as plain text.

How it works: the original file stays unchanged. The unlocked copy is added to the
document as a new [version](usage.md#document-file-versions), the encrypted one stays in the history.
With the **Import started** trigger the file is first stored as usual (encrypted, mostly without
readable text) and right after that read in again, unlocked, as a second version.

If none of the passwords fits, the document stays as it is; the message is in the log.
If different senders use different PDF passwords, it is best to set up separate workflows.

<a id="workflow-action-remote-ocr"></a><a id="remote-ocr"></a>

##### Remote OCR

Sends the document to an external service for text recognition instead of the built-in OCR. The
action has no settings; having it in the workflow is enough. It only works with the **Import
started** trigger.

Typical use: set [`PAPERRUST_REMOTE_OCR_MODE`](configuration.md#PAPERRUST_REMOTE_OCR_MODE) to
`workflow_only`. Then only what such a workflow picks goes to the service – everything else stays
local. If no service is set up, or it cannot handle the file type, PaperRust falls back to local OCR
and notes this in the log. Setting up the service: [Remote OCR](usage.md#remote-ocr).

<a id="workflow-action-apply-ai-suggestions"></a><a id="apply-ai-suggestions"></a>

##### Use AI suggestions

Asks the configured AI service for suggestions and fills them in – the same suggestions you
otherwise see on the detail page, only automatic. The [AI features](configuration.md#ai) must be
switched on. You enter:

- **Use suggestions for** – any of: title, tags, correspondent, document type, storage path, created on.
  Suggestions for fields you did not pick are dropped.
- **Create what is missing** – off: only existing tags, correspondents and document types are filled
  in. On: missing ones are created as well (except storage paths, which this action never creates).
- **Replace existing values** – off: only empty fields are filled. Since nearly every document already
  has a title and a date, you usually need this option when you pick title or date. Tags are only ever
  added, never replaced.

The action needs the document's text. That is why it works with every trigger **except Import
started**, and documents without text are skipped. Asking the AI takes time, so the action runs in the
background. Its suggestions may arrive after the other actions are already done.

> [!WARNING]
> Every matching document is one request to the AI service. That can cost money, and with an
> outside service the document's text leaves your machine. Many requests at once can also fill the
> queue and slow down new imports. Filter tightly and work in small batches.

More on this: [AI features](ai.md).

<a id="workflow-placeholders"></a><a id="platzhalter"></a>

#### Workflow placeholders

You can use placeholders in the **title**, in the **subject and text of an email** and in the
**body and fields of a webhook**. When the workflow runs, PaperRust fills in the document's values.
A placeholder is written in double curly braces, e.g. `{{ correspondent }}`.

The notation is that of the [Jinja](https://jinja.palletsprojects.com/en/3.1.x/templates/) template
language, so you can also use conditions (`{% if … %}`) and filters (`{{ title | upper }}`).
PaperRust checks a template as soon as you save the workflow and reports typos and unknown
placeholders.

**Always available:**

| Placeholder | Meaning |
|---|---|
| `{{ correspondent }}` | Name of the correspondent (empty if none) |
| `{{ document_type }}` | Name of the document type |
| `{{ owner_username }}` | User name of the owner |
| `{{ added }}` | When the document was added, full date with time and time zone |
| `{{ added_year }}` | Year, four digits (`2025`) |
| `{{ added_year_short }}` | Year, two digits (`25`) |
| `{{ added_month }}` | Month as a number (`03`) |
| `{{ added_month_name }}` | Month name, English (`March`) |
| `{{ added_month_name_short }}` | Short month name, English (`Mar`) |
| `{{ added_day }}` | Day (`07`) |
| `{{ added_time }}` | Time as `HH:MM` |
| `{{ original_filename }}` | Name of the uploaded file, without extension |
| `{{ filename }}` | Current file name in the archive, without extension. Right after adding it may not be final yet – then use `{{ original_filename }}`. |
| `{{ custom_fields }}` | The document's custom fields (explained below) |
| `{{ doc_title }}` | Current title. Inside "Set title" itself it is empty. |

**Only when the document already exists** (so with every trigger except **Import started**):

| Placeholder | Meaning |
|---|---|
| `{{ created }}` | Created date (`2025-03-07`) |
| `{{ created_year }}`, `{{ created_year_short }}` | Year, four or two digits |
| `{{ created_month }}` | Month as a number |
| `{{ created_month_name }}`, `{{ created_month_name_short }}` | Month name long or short, English |
| `{{ created_day }}` | Day |
| `{{ created_time }}` | Time as `HH:MM`; the created date has no time, so always `00:00` |
| `{{ doc_id }}` | Number (ID) of the document |
| `{{ doc_url }}` | Link to the document in the web interface. Only works when [`PAPERRUST_URL`](configuration.md#PAPERRUST_URL) is set. |

**Custom fields:** `custom_fields` holds the type and value for each field name. A field without
spaces in its name can be reached directly, e.g. `{{ custom_fields.InvoiceNumber.value }}`. Safer is
the `get_cf_value` filter, which also handles names with spaces and lets you give a fallback if the
field is missing: `{{ custom_fields | get_cf_value("Customer no", "none") }}`. In "Set title" the
template also knows the fields that the same action is setting. More on these values:
[file name templates](storage-paths.md#additional-variables).

<a id="examples"></a>

##### Examples

Month names in another language come from the `localize_date` filter (give format and language, see
[date localization](storage-paths.md#date-localization)):

```jinja2
{{ created | localize_date('MMMM', 'de_DE') }}
{# in October: "Oktober" #}

{{ created | localize_date('dd/MM/yyyy', 'en_GB') }}
{# "26/10/2023" #}
```

A title on import, with a fallback if no correspondent was recognised:

```jinja2
{% if correspondent %}{{ correspondent }}{% else %}Unknown{% endif %} – {{ added_year }}-{{ added_month }}
```

Mail text for a notification:

```jinja2
New document "{{ doc_title }}" from {{ correspondent }}: {{ doc_url }}
```

<a id="workflow-permissions"></a>

### Workflow permissions

Workflows belong to nobody. Whoever has the permission to edit workflows sees and changes **all**
workflows. Single workflows cannot be locked away from certain users.

Because a workflow can do a lot – such as changing the permissions of many documents – give this
permission to few people only. If you switch from paperless-ngx, the permissions granted there stay.
