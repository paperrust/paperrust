# Getting started

[Deutsch](../getting-started.md) · [Documentation overview](README.md)

PaperRust manages your documents: scan paper or drop files, PaperRust recognises the text, assigns
correspondent, document type and tags, and makes everything searchable. The REST API is the same as in
paperless-ngx, so apps for paperless-ngx keep working. The interface is PaperRust's own (PaperRust
design); if you want the familiar paperless-ngx look, choose the classic interface under
*Settings → Display*.

<a id="choose"></a>

## 1. Choose how to run it

- **Already using paperless-ngx?** → [Switching from paperless-ngx](umstieg.md). One command, or one
  click in the desktop app; if you want, into a copy, so paperless-ngx stays untouched.
- **Starting fresh on a server** → [Installation without Django](installation.md#standalone).
- **Just one computer, no server** → [Desktop app](installation.md#desktop) (Windows installer, not signed yet).

<a id="first-login"></a>

## 2. Log in and set up

1. Create an administrator (`paperrust manage createsuperuser`) and log in at `PAPERRUST_URL`.
2. The **tour** (user menu → *Start tour*) shows the most important places. Under *Settings* you
   choose language, date format and display.
3. Create the main ordering objects ([terms](usage.md#terms-and-definitions)):
   - **correspondents** – who a document is from (bank, insurer, …),
   - **document types** – what it is (invoice, contract, …),
   - **tags** – free keywords; a tag can be marked as *inbox* tag,
     anything missing can later also be created right in the field of a document,
   - **storage paths** – where the file is stored ([file names](storage-paths.md)).
4. Choose the **matching** for each object (*Auto* learns from your assignments; alternatively word
   lists or regular expressions – [details](advanced.md#matching)).

<a id="add-documents"></a>

## 3. Add documents

- **Upload** by dragging into the UI,
- **consumption directory**: put files into `PAPERRUST_CONSUMPTION_DIR` (e.g. as the scanner target),
- **e-mail**: accounts under *Manage → Mail*, rules under *Automation → Mail fetching* ([Mail](mail.md)),
- **API / apps**: `POST /api/documents/post_document/` ([API](api.md#file-uploads)).

Progress appears at the top right; finished documents show up in the list with the inbox tag.

<a id="next"></a>

## 4. Next

- Search and filter, saved views, share links: [Usage](usage.md)
- Automate: [Automation](automatik.md) (reference: [Workflows](workflows.md))
- Own fields (amount, due date, …): [Custom fields](custom-fields.md)
- Users and permissions: [Permissions](permissions.md)
- Backup: [Administration](administration.md#backup)
- Optional AI: [AI](ai.md)
- With several people: [For companies](firmen.md)
