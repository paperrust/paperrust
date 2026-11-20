# Automation (workflows)

[Deutsch](../automatik.md) · [Documentation overview](README.md) · Reference for all options: [Workflows](workflows.md)

In the PaperRust design the navigation on the left has an **Automation** area. It holds everything
PaperRust does by itself:

| Tab | What for |
|---|---|
| **Workflows** | rules of the form "when …, then …" – for example adding tags when a document is added |
| **Mail fetching** | which emails are fetched and which workflows run afterwards |
| **AI suggestions** | review list with the AI's suggestions (only shown when the [AI](ai.md) is switched on) |

You only see this area if you may view workflows or mail rules. With AI switched on, the number of
open AI suggestions is shown next to **Automation**.

The classic interface still shows the same workflows under *Manage → Workflows*. Both interfaces
save in exactly the same way, so you can switch at any time.

<a id="list"></a>

## The list of workflows

Each workflow is shown in the list as **one sentence**, for example:

> When a document has been added and the text contains "invoice", then add the tag "Invoice" and
> set the document type "Invoice".

Below it you see how often it ran this week and when it last ran (marked red if there was an error).
The switch on the left turns a workflow on or off. The **More actions** menu offers *Duplicate* (the
copy starts switched off), *History* and *Delete*.

Workflows run **from top to bottom**. You change the order with the handle (drag it) or with
**Alt + arrow up/down**.

<a id="new"></a>

## Creating a workflow

**New workflow** offers three ways:

- **Templates** – ready-made building blocks to adjust: *Sort invoices*, *Tax folder*, *Notify*,
  *Open password PDFs*, *Pre-sort with AI* (only with AI) and *Start empty*.
- **Suggestions from your archive** – without AI, PaperRust looks at what you have almost always
  filed the same way so far ("for correspondent X always document type Y"). **Use** turns a
  suggestion into a draft. Suggestions that a workflow already covers are hidden.
- **Describe it to the AI** (only with AI) – write in your own words what should happen, e.g.
  "Invoices from Swisscom get the tag 'Invoice' and go into the 'Phone' folder". **Create draft**
  turns this into a workflow that you check and adjust. Nothing is saved until you click **Save**.

<a id="editor"></a>

## The editor: cards from top to bottom

At the top, the workflow is shown as a sentence. The parts of the sentence can be clicked; simple
workflows can be built entirely in the sentence (**+ Condition**, **+ Action**). Below it are the
cards:

| Card | Question | Note |
|---|---|---|
| **When** | When should the workflow start? | one or more triggers |
| **Only if** | For which documents? | conditions; without a condition: "Always" |
| **Only if the AI thinks** | Does the document fit, according to the AI? | only with AI, see below |
| **Then** | What should happen? | actions, from top to bottom |
| **Otherwise** | What if the conditions do not apply? | optional (**+ Otherwise**) |

Clicking a card opens all its options on the right. Options you rarely need are under **Advanced**.
If something is missing (name, trigger, action), the editor marks the place.

<a id="triggers"></a>

### Triggers ("When")

| In the PaperRust design | In the classic interface | When |
|---|---|---|
| **Import starts** | Import started | before a file is processed – only the file name, path and source are known |
| **Document added** | Document added | as soon as a new document has been processed |
| **Document changed** | Document changed | when someone saves a document |
| **On a schedule** | Scheduled | a number of days before or after a date, repeated if you want |
| **Manually** | Manual | only when someone starts the workflow on a document |
| **Email arrives** | Import started with the filter "Mail rule" | when a mail rule fetches a document |

With **Email arrives** you choose a mail rule or create a new one right there. Internally this is
still a mail rule plus a workflow – just as paperless-ngx stores it.

<a id="conditions"></a>

### Conditions ("Only if")

The common conditions are available as buttons: **Text contains**, **Tag**, **Correspondent**,
**Document type**, **Storage path**, **File name**, **Source**. Under **Advanced** you find the path
during import, the mail rule, conditions for custom fields and the kind of text comparison (exactly,
roughly, all words, one of the words, regular expression, learned). The workflow only runs if **all**
conditions apply.

With the **Import starts** trigger there is no text yet. So set conditions on the content with
**Document added**.

<a id="ai-condition"></a>

### AI condition ("Only if the AI thinks")

Only shown when the AI is switched on. The AI reads the document and checks, for example, whether it
is an invoice or belongs to a certain correspondent. With the slider you set how sure it must be
("At least this sure: 80 %"). Higher means less often, but more reliable. If the AI is sure enough,
the actions under **Then** run; otherwise those under **Otherwise**.

- With the **Import starts** trigger there is no document yet; there the AI condition is skipped.
- If the AI is off or cannot be reached, the condition is not checked; the history notes this.

<a id="actions"></a>

### Actions ("Then" and "Otherwise")

Quick choices: **Add tag**, **Set correspondent**, **Set document type**, **Set storage path**,
**Set title**, **Send email**, **Move to trash**, **Get AI suggestions** (only with AI). **Other
action …** has the rest: remove values, owner and permissions, custom fields, webhook, **Remove
password** and **recognise the text with the external service** (remote OCR). What each action does
exactly and which placeholders you can use in the title is described under
[Workflows → Actions](workflows.md#workflow-actions).

You change the order of the actions with the handle or with Alt + arrow key.

The **otherwise branch** is a PaperRust extension. paperless-ngx does not have it; the classic
interface only shows the main part.

<a id="dryrun"></a>

## Test run and applying

**Test run** shows what the workflow would do with your newest documents – without changing
anything, without sending emails and without calling webhooks.

- You choose how many documents are checked and **which trigger** to test as ("Test as").
- The result: how many documents would be changed, how many fall into the otherwise branch, how many
  are not affected, and where an error occurred. For each document you see **before/after**.
- **Import starts** is simulated: existing documents are checked as if they were being imported now.
  For **On a schedule** the test run only checks the conditions, not the time.
- In the test run the AI condition counts as met. With **Really check the AI condition** PaperRust
  asks the AI (at most 10 documents).

Afterwards you can select documents and click **Apply to N**. Then the actions really run in the
background, including emails and webhooks. This cannot be undone automatically, so PaperRust asks
first. A new workflow must be saved before you can apply it.

<a id="history"></a>

## History

**History** shows every run of a workflow: when, on which document, with which result (OK, skipped,
error, otherwise branch) and what changed. Errors are clearly visible; the filter shows only errors,
only successful runs or only skipped ones. Documents you are not allowed to see appear without a
title.

How long the history is kept is set by
[`PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS`](configuration.md#PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS)
(default 90 days, 0 = without limit).

<a id="mail"></a>

## Mail fetching

The **Mail fetching** tab shows a **flow** for each mail rule: which emails are fetched (account,
folder, filters), what is imported (attachments, the whole email) and which workflows run afterwards.

- **New mail flow** creates a rule. Its questions are *Which emails?*, *What is imported?*, *Values
  for the document* and *What happens to the email afterwards?*.
- **Add a workflow for this rule** creates a workflow with the *Email arrives* trigger.
- You set up mail **accounts** under *Manage → Mail* (**Manage accounts**). Details on accounts and
  rules: [Mail](mail.md).
- If you delete a rule, its workflows are kept; but they no longer start with this rule.

<a id="ai-review"></a>

## AI suggestions

Only with the AI switched on. Here you check, field by field, what the AI suggests: apply, change or
dismiss; anything you applied can be undone. Details:
[AI → Checking AI suggestions](ai.md#ai-review).

<a id="rights"></a>

## Permissions

Who may view, create or change workflows is controlled by the normal permissions for workflows (and,
for mail fetching, for mail rules), see [Workflow permissions](workflows.md#workflow-permissions).
When applying, PaperRust checks that you may change every selected document.
