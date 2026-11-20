# Usage

[Deutsch](../usage.md) · [Overview](README.md)

This page covers day-to-day work with PaperRust: what the terms mean, where things are in the
interface, how documents get in, how you find them again, and how to keep paper and archive in step.
A scanner is optional; if you want to connect one, see [Connecting a scanner](scanner.md).

<a id="terms-and-definitions"></a><a id="begriffe"></a>

## Terms

Two things work together behind the scenes: **consumption** (it takes in new files, reads the text
and assigns metadata) and the **web interface** (where you work with your documents). Both live in
the same program.

What a document can carry:

| Term | Meaning |
| --- | --- |
| **Document** | a consumed file plus everything PaperRust knows about it |
| **Correspondent** | who sent the document or who receives it – a person, company or authority |
| **Tag** | a label. A document can carry any number of tags. That is why there are no folders: a document is never stuck in just one drawer. |
| **Document type** | what kind of document it is, e.g. invoice, contract, bank statement |
| **Storage path** | where and under which name the file is kept on disk, see [Storage paths](storage-paths.md#storage-paths) |
| **Added** | when PaperRust consumed the document. This date stays fixed. |
| **Created** | when the document was issued, e.g. the invoice date or the date of a letter |
| **ASN** (archive serial number) | the number under which the paper original sits in your binder, see [Recommended workflow](#usage-recommended-workflow) |
| **Content** | the recognised text. It feeds the search and automatic matching. |
| **Custom fields** | your own extra data such as an amount or contract number, see [Custom fields](custom-fields.md) |

<a id="the-web-ui"></a>

## The web interface

Open PaperRust in your browser at `/rs/app/` (the root address `/` redirects there).
Unless you have chosen otherwise, the **PaperRust design** opens (address `/rs/app/m/`):

- **Navigation on the left:** **Dashboard**, **Inbox** (new documents nobody has worked on yet),
  **All documents**, your saved views, **Automation** (workflows, mail fetching, AI suggestions –
  see [Automation](automatik.md)), **Manage** (tags, correspondents, document types, storage paths,
  custom fields, users and groups, configuration, tasks, logs, trash, scan, protection, share links
  and so on) and **Settings**. Things you are not allowed to use are not shown.
- **At the top:** the search field (also with **Ctrl + K**, see [Global search](#global-search)), the
  **Upload** button, the notifications and the user menu (My profile, Light/dark, Keyboard
  shortcuts, Documentation, **Start tour**, Switch to classic interface, Sign out).
- **Workspace:** list, preview and details sit side by side. Only the single areas scroll; the header
  bar and the navigation stay in place. On a phone this becomes a bar at the bottom.

New to PaperRust? The **tour** shows you the most important places in twelve steps. Start it from
the user menu (**Start tour**) or under *Settings*.

The **classic interface** looks like paperless-ngx. Choose it under *Settings → Display → Interface
design* (applies to your account) or once via "Switch to classic interface" in the user menu. Both
work on the same data. Where handling differs, this page names both ways.

<a id="sprache"></a><a id="language"></a>

### Interface language

The interface speaks **10 languages**: English, German, French, Italian, Spanish, Dutch, Portuguese
(Brazil), Polish, Czech and Swedish. PaperRust wrote all of these texts itself.

Choose under *Settings → General → Language*. Without a choice, PaperRust uses the browser language.
If that is not one of the ten (say Japanese or Danish), the texts appear in English, while dates and
numbers still use your language's format. Swiss German (`de-CH`) gets the German texts.

For people switching over: paperless-ngx had more than 30 languages. If you had picked one outside
the ten, you see English after switching until you choose again.

<a id="dashboard"></a>

### Dashboard

The **dashboard** (called "Startseite" in the German interface) appears after login. At first it is
nearly empty. You fill it with [saved views](#saved-views) that you show as a tile on the dashboard –
for example "In the inbox" or "To do". You can upload files here, or simply drop them anywhere in
the interface.

<a id="document-list"></a>

### Document list

Under **Documents** you see every document you have access to – as a table or as cards. Above the
list you filter by tags, correspondents, document types, storage paths, dates, custom fields and
more. The text box in the filter bar searches title and content, by ASN, or with the
[advanced search](#document-searches).

Tick documents to change many at once: assign tags or a correspondent, change the type, storage path,
custom fields or "Visible to" (permissions), delete, run workflows and more. In the PaperRust design
the **PDF** menu collects the file actions (Rotate, Merge, Merge as versions, Process again, Convert
to PDF); **Send** offers a share link for several documents or an e-mail, **Download** the originals
or archive files.

<a id="document-detail"></a>

### Document details

Clicking a document opens the detail view with preview and details. Tabs show the recognised
content, file metadata, notes, permissions and history. The PaperRust design saves changes by
itself.

**Create right in the field:** If a tag, correspondent, document type or storage path is missing,
type its name into the selection field and choose **"Create …"**. PaperRust creates the entry and
assigns it right away.

**The action menu** of the document (in the PaperRust design at the top right) contains: PDF editor,
Redact, Shrink, Convert to PDF, Process again, Print, Download original, send by e-mail, Share links
and Run workflow. Which of these appear depends on the file type and your permissions. The PDF tools
are explained in [PDF tools](pdf-tools.md).

<a id="document-file-versions"></a>

### Document file versions

A document can have several **file versions** – like a stack with the newest file on top. This
happens, for example, when you rotate or delete pages, hand in a better copy later, or combine two
documents.

- Each version has its own file and its own recognised text.
- The title and all other data (tags, correspondent, type, storage path, custom fields) belong to
  the document as a whole (the "root"), not to a single version.
- Search and the displayed content always use the newest version.
- In the document details you can pick an older version. Preview, content, file metadata and
  download then show that version.
- On disk, version files get the normal file name with the ending `_v1`, `_v2` and so on.
- If you delete a version (not the root), all data stays; the next newest version takes its place.

**Combining:** Tick at least two documents in the document list, then pick **Merge as versions**.
You decide which document is the root – its data and permissions stay, the others become versions.
The root may already have versions; the others may not. To do it one at a time, use a document's
**Versions** menu: **Existing** picks another document and attaches it as a version.

Keep in mind:

- The attached documents lose their ASN. A root without its own ASN inherits the first one;
  otherwise the numbers become free again, and the history notes this.
- Combining cannot be undone in the interface.
- If the root is deleted, all its versions go to the trash with it.

<a id="management-lists"></a>

### Management lists

Under **Manage** you look after tags, correspondents, document types, storage paths and custom
fields (the **Attributes** area), plus saved views, share links and mail accounts. In every list
you can add, change and delete entries and set their permissions. Users and groups, configuration
(application and server), tasks, logs and the trash are in the same area, shown depending on your
permissions. Workflows and mail rules are in the [Automation](automatik.md) area in the PaperRust
design, and under *Manage* in the classic interface.

<a id="nested-tags"></a>

### Nested tags

Tags can be placed inside each other, for example `Insurance` → `Car` → `Liability`. A tag then has a
parent tag; up to 5 levels are possible. This has effects:

- If a document gets `Liability`, it automatically also gets `Car` and `Insurance`.
- If you take `Car` away from a document, `Liability` goes too.
- If you later move an existing tag under another one, every document with that tag gets the new
  parent tag as well.

<a id="adding-documents-to-paperrust"></a>

## Adding documents to PaperRust

Whichever way a file arrives, PaperRust always does the same with it:

1. **Recognise text (OCR)** if the file has none yet. PDFs created on a computer usually already have
   it.
2. **Create an archive copy:** a PDF/A with selectable text. Whether this happens is set by
   [`PAPERRUST_ARCHIVE_FILE_GENERATION`](configuration.md#PAPERRUST_ARCHIVE_FILE_GENERATION):
   `auto` (the default) leaves PDFs that already have text alone, `never` never creates an archive
   copy. More under [OCR settings](configuration.md#ocr).
3. **Match:** tags, correspondent, document type and so on are assigned automatically, then the
   document is saved.

PaperRust always keeps the original unchanged; the archive copy sits next to it. You can only change
the file stack on purpose, for example with the [PDF actions](#pdf-actions) – and those create a new
version.

Ways in: the consumption folder, uploading in the interface, [scan intake](scanner.md),
[mail](mail.md) and the [REST API](#rest-api).

<a id="consumption-directory"></a><a id="eingangsordner"></a><a id="the-consumption-directory"></a>

### The consumption folder

The consumption folder is a mailbox: whatever you put in, PaperRust picks up, reads and files in its
own storage (the media folder). **Afterwards the file is gone from the consumption folder** – it now
lives safely in PaperRust, at the place your settings and storage paths decide.

PaperRust checks the folder regularly. How often is set by
[`PAPERRUST_CONSUMER_POLLING_INTERVAL`](configuration.md#PAPERRUST_CONSUMER_POLLING_INTERVAL); the
default `0` means once per second. PaperRust's own background service (`paperrust serve --worker`)
does this, so you don't need to start anything extra.

How files reach the folder is up to you. If PaperRust runs on your own computer, just drag them in.
On a server, a network share (e.g. with [Samba](https://www.samba.org/)) or an FTP server such as
[ProFTPD](http://www.proftpd.org/) works well. It is easier, and needs no server password stored in
the scanner, with [scan intake](scanner.md).

<a id="web-ui-upload"></a>

### Uploading in the web interface

Click **Upload** at the top, or drag files onto any part of the interface. You can follow progress in
the processing indicator at the top right.

<a id="usage-mobile_upload"></a><a id="mobile-upload"></a>

### Uploading from a phone

The interface also works in a phone's browser, where you can upload photos and files. Many apps
written for paperless-ngx use the same REST API and therefore usually work with PaperRust too – but
this has not been checked for every app. For network document scanners, see
[Connecting a scanner](scanner.md).

<a id="rest-api"></a>

### REST API

Programs can send files straight to the API, see [Uploading documents](api.md#file-uploads).

<a id="duplicate-documents"></a>

### Duplicate documents

If a file comes in that already exists (same checksum of the original or archive copy), **PaperRust
still consumes it** and writes a warning to the log. That way nothing is lost if you wanted the copy
on purpose.

To find duplicates:

- The document details show a **Duplicates** tab listing all identical documents you may see
  (including those in the trash).
- In the document list, choose **Duplicates** in the text filter's drop-down.
- Through the API: `has_duplicates=true`.

To keep duplicates out altogether, set
[`PAPERRUST_CONSUMER_DELETE_DUPLICATES`](configuration.md#PAPERRUST_CONSUMER_DELETE_DUPLICATES) to
`true`. The new file is then deleted, and the task ends with the error "Document already exists" and
a link to the existing document. (That is how paperless-ngx behaved before version 3.)

<a id="document-suggestions"></a>

## Document suggestions

PaperRust learns from your existing documents which tags, correspondents, document types and storage
paths fit which content. This is a small local learning method, not a language AI. In the document
details the suggestions appear next to the fields; you accept them with one click or ignore them.

For documents with an inbox tag, PaperRust fetches suggestions automatically when you open them. If
you don't want that, turn off "Fetch suggestions for inbox documents automatically" in *Settings*;
then suggestions only come via the **Suggest** button. If an AI service is set up, it can supply
suggestions as well, see [AI features](ai.md).

<a id="sharing"></a><a id="freigaben"></a><a id="sharing-documents-from-paperrust"></a>

## Sharing documents

Inside PaperRust you share documents through [permissions](permissions.md#object-permissions):
whoever has view or change permission sees the document. Outside there are three ways:
[share links](#share-links), [sending by email](#email-sharing), and workflows that automatically
send an [email](workflows.md#workflow-action-email) or a [webhook](workflows.md#workflow-action-webhook).

<a id="share-links"></a>

### Share links

A share link is an address from which someone can download a file **without logging in**. It looks
like this: `{paperrust-url}/share/{random-code}`.

- **One document:** in the document details, **Send** → share link. Optionally with an expiry date.
- **Several documents:** select them in the document list and choose "Create a share link for
  several documents". In the background PaperRust packs a ZIP archive and hands you one link.
  You see the state (pending, processing, ready, failed) under "Manage share links for several
  documents"; there you can rebuild failed bundles or delete links.
- You can also manage all links under *Manage → Share links*.
- Once a link has expired or been deleted, visitors land on the login page.

> [!TIP]
> If a reverse proxy with its own login sits in front of PaperRust, you may need to exempt the
> `/share/` path from it, otherwise outsiders cannot reach the file. Think carefully about what you
> make public.

<a id="share-links-desktop"></a>

#### Share links in the desktop app

The desktop app is only reachable on your own computer (`http://127.0.0.1:…`). A share link
therefore **only works on this computer**; the dialog says so too. To pass a document on anyway:

- **download** it and pass on the file, or
- **send it by email** (**Send** → **Email**, see [sending by email](#email-sharing); this needs a
  configured mail server), or
- turn on **sharing in your home network**: in `desktop.toml` (folder `%LOCALAPPDATA%\PaperRust` or
  `~/.local/share/paperrust`) add `lan_sharing = true` under `[server]` and restart the app. It is
  then also reachable at `http://<network-address>:<port>`, and share links work on devices in the
  same network, e.g. on your phone over Wi-Fi. Working in the app still requires login; only the
  share links work without it. Windows asks on first start – allow **private networks** only. If the
  detected address is wrong, set `lan_address = "192.168.1.20"` (or a host name). Do not use this on
  public Wi-Fi.

Links reachable from the internet come with a PaperRust server at its own address (also used for
remote access and syncing the app), see `docs/REMOTE-APP.md` in the project.

<a id="email-sharing"></a>

### Sending by email

Once a [mail server for sending](configuration.md#email-sending) is set up, **Send** in the document
details offers an **Email** option. The document then goes as an attachment to the address you
enter. To do this automatically, for example for every new invoice, use a
[workflow action](workflows.md#workflow-action-email).

<a id="pdf-actions"></a>

## PDF actions

For PDFs (and only for them) PaperRust offers simple edits:

| Action | Where |
| --- | --- |
| Rotate pages | document list (selection → **Rotate**) and PDF editor |
| Delete or reorder pages | PDF editor |
| Split a document | PDF editor ("Split here") |
| Merge documents | document list, select several documents |

Open the **PDF editor** from the document details (PaperRust design: action menu → **PDF editor**;
for several documents: the **PDF** menu above the list). It shows every page as a thumbnail; there you
rotate, move and delete pages or set split points.

Rotating and deleting pages create a **new file version**; the previous file stays as an older
[version](#document-file-versions). The new file is a different file, though: a digital signature no
longer holds for it.

Redacting, shrinking and repairing PDFs is covered on the [PDF tools](pdf-tools.md) page.

<a id="document-history"></a>

## Document history

PaperRust records every change to a document: who changed which field and when. You see this in the
**History** tab of the document details. Changes made by workflows or other automatic steps are
listed as "System". It can be turned off with
[`PAPERRUST_AUDIT_LOG_ENABLED`](configuration.md#PAPERRUST_AUDIT_LOG_ENABLED) (default: on).

<a id="papierkorb"></a><a id="document-trash"></a>

## Trash

Deleted documents first go to the **trash** (under *Manage → Trash*). There, the file and all
data stay complete, and you can bring the document back at any time.

A document is only gone for good once you delete it in the trash or the waiting period runs out. Set
the period with [`PAPERRUST_EMPTY_TRASH_DELAY`](configuration.md#PAPERRUST_EMPTY_TRASH_DELAY); the
default is 30 days.

For a last safety net, set [`PAPERRUST_EMPTY_TRASH_DIR`](configuration.md#PAPERRUST_EMPTY_TRASH_DIR).
On final deletion the original file is then moved into that folder. The archive copy and all
database data are still gone.

<a id="basic-searching"></a><a id="best-practices"></a>

## Good habits

PaperRust doesn't force any order on you. A few ideas that work well:

- **Document type = what it is.** Invoice, contract, payslip. If you used to have a "Receipts"
  binder, create a document type for it. Not every document needs a type – a one-off isn't worth one.
- **Tag = topic or state.** A document can have many tags, but a sheet of paper can only sit in one
  binder. A car insurance invoice can carry `Car`, `Insurance` and `Taxes 2026` at the same time.
- **Tags for temporary projects.** Planning a move and need your lease, pay slips and a copy of your
  ID? A `Moving` tag gathers everything without touching the rest of your order. Afterwards, delete
  the tag or keep it.
- **Correspondents** where they help you find things – the bank, the health insurer, the landlord.
- **Saved views** for anything you need often. They are like labelled binders on your desk that fill
  themselves.

Tags that prove useful in practice:

- `Inbox` – new and not looked at yet
- `To do` – something still needs to happen (pay, reply, cancel)
- `Incomplete` – data is missing that you will add later
- `By mail` – came in through mail fetching
- one tag per account, vehicle or family member

<a id="saved-views"></a><a id="gespeicherte-ansichten"></a>

## Saved views

Once filters, sort order and columns in the document list are set the way you need them, save them
via *Views → Save as…*. A saved view

- can appear in the sidebar, on the dashboard, or both,
- has its own page size and display (table, small or large cards),
- can be shared with other users through permissions.

Edit and reorder them under *Manage → Saved views*. Through the API: `/api/saved_views/`.

<a id="searching"></a><a id="suche"></a><a id="basic-usage_searching"></a>

## Search

<a id="global-search"></a>

### Global search

The search field at the top looks everywhere at once: documents, saved views, tags, correspondents,
document types, storage paths, users, groups, mail accounts and rules, workflows and custom fields.
It needs at least 3 characters. It shows up to 3 hits per kind, and only things you are allowed to
see.

It searches documents in full text (title and content). If you turn on "Leave out advanced search
results" in *Settings*, it searches titles only. The button next to the search field opens the full
result list in the document list.

In the PaperRust design the search field is also a **command bar** (open it with **Ctrl + K**):

- **Commands:** jump to a page, upload, new workflow, light/dark, quick filters and so on.
- **Find settings:** type e.g. "date" or "OCR". Matching interface settings appear at the bottom and
  – for administrators – all server settings (the search looks at the name, the variable name and
  keywords). A click opens the right tab and makes the setting light up briefly.
- **Ask a question:** "Ask a question: …" sends the text to the assistant (questions to the archive).
  Without AI it shows the best-matching passages from your documents; with AI an answer with sources.

<a id="document-searches"></a>

### Document search

Document search helps with questions like "Where is the invoice for the vacuum cleaner from last
spring?". PaperRust compares what you type with title, content, correspondent, document type and
tags, and sorts the hits: the best match comes first.

Without special characters, a document must contain **all** the words you typed. Also:

- Word **order** doesn't matter: `electricity bill` = `bill electricity`.
- **Accents** don't matter: `creme` also finds `crème`.
- **Punctuation and separators** are removed on consumption. So `4711` also finds `KD-4711/03`.

The plain search does not look at notes, custom fields or barcodes; they have their own field names
(see below). For more precise searches, use the following rules.

<a id="combining-terms"></a>

#### Combining terms

```
rent AND (2025 OR 2026)
insurance NOT car
"please transfer the amount"
```

- Write `AND`, `OR` and `NOT` in **capitals**. Parentheses group terms.
- Words next to each other without an operator are joined with `AND`.
- In quotes, PaperRust looks for exactly that phrase.

> [!WARNING]
> A leading minus does **not** exclude: `insurance -car` finds documents that contain both words. The
> right way is `insurance NOT car`.

<a id="searching-by-field"></a>

#### Searching by field

With `fieldname:value` you search a single field only:

```
correspondent:utility type:invoice
correspondent:"city council"
tag:taxes,2025
page_count:[10 to 50]
checksum:3a7bd3e2*
```

| Field | Searches |
| --- | --- |
| `title` | title |
| `content` | recognised text |
| `correspondent` | correspondent |
| `document_type` or `type` | document type |
| `storage_path` or `path` | storage path |
| `tag` | tags |
| `original_filename` | original file name |
| `asn` | archive serial number |
| `page_count` | number of pages |
| `num_notes` | number of notes |
| `checksum` | checksum of the original file |
| `created` | created date |
| `added` | date added |
| `modified` | date of last change |

Worth knowing:

- The field name only applies to the word **right after it**. If the value has several words, put it
  in quotes.
- Several tags separated by commas mean "all of them": `tag:taxes,2025` only finds documents with
  both.
- `asn`, `page_count` and `num_notes` are numbers. Ranges like `asn:[100 to 200]` work, wildcards
  don't.
- `checksum` only finds the complete checksum in lowercase. For the beginning, add `*`.
- For `created`, `added` and `modified`, see [Searching by date](#searching-by-date).
- An unknown field name is treated like a normal word.

<a id="wildcards"></a>

#### Wildcards

```
insur*
title:Statement*
202?
19[6-9]?
```

- `*` stands for any number of characters, `?` for exactly one.
- `[…]` means one character out of a set or range, `[!…]` one character **outside** it. So
  `19[6-9]?` matches 1960 to 1999.
- Square brackets only act as a wildcard together with `*` or `?`; on their own they are searched as
  text. Exception: a range for exactly one character with no other wildcard (e.g. `title:202[0-5]`)
  is rejected with an error message. With a wildcard it works: `title:202[0-5]*`.

> [!NOTE]
> If a [search language](configuration.md#PAPERRUST_SEARCH_LANGUAGE) with stemming is set, PaperRust
> stores words in their base form. `invoice*` then also finds "invoices". A prefix longer than the
> stored stem sometimes finds nothing, though. If a wildcard finds nothing, shorten the beginning.

<a id="searching-by-date"></a>

#### Searching by date

```
created:2024
added:"previous month"
created:[2020-01 to 2022-06]
modified:[-3 days to now]
```

These words each stand for a whole period (with or without quotes): `today`, `yesterday`,
`tomorrow`, `previous week`, `this month`, `previous month`, `previous quarter`, `this year`,
`previous year`.

Other forms:

| Example | Means | Quotes |
| --- | --- | --- |
| `created:2024`, `created:2024-07`, `created:2024-07-15` | that year, month or day | not needed |
| `added:march` | March of the current year | not needed |
| `added:"next friday"`, `added:"last friday"` | that single day | needed |
| `created:"3 march 2021"` | that single day | needed |
| `added:"2024-07-15T08:30:00Z"` | exactly that moment | needed |

Write **periods** as `[from to until]`. Each end may use a form from the table, or a relative value
such as `-2 weeks`, `now-30d` or `now`. Quotes are optional here; if you add some, use single ones
(`['-2 weeks' to now]`) – double quotes cause an error.

> [!WARNING]
> `now`, `noon`, `midnight` and relative values like `-2 weeks` only work as the bound of a range. On
> their own they mean one single instant, which no document matches. Write
> `added:[-2 weeks to now]`, not `added:"-2 weeks"`. A weekday on its own (`friday`) and phrases like `3 days ago` or `this week`
> are not understood.

Dates follow the time zone from `PAPERRUST_TIME_ZONE`.

<a id="searching-custom-fields"></a>

#### Searching custom fields

The name and value of every custom field are in the search index, but only reachable with a field
name:

```
custom_fields.value:4711
custom_fields.name:"Customer number"
custom_fields.name:"Customer number" custom_fields.value:4711
```

- `custom_fields.value:` searches the value of any field; the short form is `custom_fields:`.
- `custom_fields.name:` searches the field's name (several words in quotes).
- Both together find documents where exactly that field has that value.

Separators are dropped here too: a value `KD-4711/03` is stored as `kd`, `4711` and `03`, and each
piece can be found on its own.

> [!NOTE]
> Date fields among the custom fields have no relative values like `[now to 2 weeks]`. Use the
> document list filters for those.

<a id="searching-notes"></a>

#### Searching notes

Notes, too, are only found with a field name:

```
notes.note:callback
notes.user:maria
notes.user:maria notes.note:accountant
```

`notes.note:` searches the text, `notes.user:` the author. `notes:` is the short form of
`notes.note:`.

<a id="searching-barcodes"></a>

#### Searching barcodes

If PaperRust [stores barcode contents](advanced.md#barcode-contents), find them like this:

```
barcodes.value:RF18539007547034
barcodes.format:qrcode
barcodes:paymentcode
```

`barcodes.value:` (short form `barcodes:`) searches the content, `barcodes.format:` the kind of code.
Separators are dropped, so you can look for each piece of a code separately.

All rules can be combined freely. Forms not listed here may not work as expected.

> [!NOTE]
> To let the search also find words with small typos, set
> [`PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD`](configuration.md#PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD).
> Such approximate hits are then listed below the exact ones.

<a id="keyboard-shortcuts--hotkeys"></a>

## Keyboard shortcuts

Press <kbd>Shift</kbd> + <kbd>?</kbd> to see the keyboard shortcuts. The list only shows the ones that
work on the current page.

<a id="usage-recommended-workflow"></a><a id="the-recommended-workflow"></a>

## Recommended workflow

A simple routine that also handles paper you have to keep. The core idea: **you only ever search in
PaperRust.** The paper binder is just storage, sorted by one running number.

<a id="preparations-in-paperrust"></a>

### Preparing PaperRust

Create two tags:

- an **inbox tag** (tick "Inbox tag" when creating it) – every new document gets it automatically,
- a **To do** tag for documents where something still needs doing.

<a id="processing-of-the-physical-documents"></a>

### Handling the paper

Collect new mail in a tray. Every so often, go through the pile sheet by sheet:

1. **Must the original be kept?** (contracts, certificates, deeds …)
2. **Yes:** write the next free number on it or stick on an [ASN label](asn-labels.md), scan it, put
   it into **one** binder – sorted by number only.
3. **No:** scan it and throw it away.

With ASN labels, PaperRust reads the number itself on consumption and fills it in (if barcode
detection is turned on). When a binder is full, write the range on the spine (e.g. "ASN 1–350"),
put it away and start the next one.

Need an original later? Search in PaperRust, read the ASN, take the binder with that range, pull out
the sheet.

<a id="processing-of-documents-in-paperrust"></a>

### Handling it in PaperRust

Open the documents with the inbox tag and check each one briefly:

1. Enter the ASN (unless it was detected automatically).
2. Set correspondent and document type – not required, but very helpful for finding things.
3. Give it a clear title, e.g. "Bike purchase contract".
4. Check the date. PaperRust reads it from the text; with a poor scan or several dates it is
   sometimes off.
5. Remove the inbox tag (PaperRust can also do this automatically on save, see settings).

> [!TIP]
> The more documents you sort like this, the better the automatic matching gets. You can also set
> fixed rules, see [Matching](advanced.md#matching).

<a id="task-management"></a>

### Keeping track of tasks

For documents where you still have to do something:

- **If you work in PaperRust regularly:** give them the **To do** tag and put a saved view "To do"
  on the dashboard. Once the matter is done, remove the tag.
- **If you only use PaperRust as an archive:** next to the tray for new mail, put a second one for
  "To do". Once the matter is done, the sheet goes to the normal pile.

<a id="remote-ocr"></a>

## Remote OCR

> [!IMPORTANT]
> Off until you explicitly turn it on.

Instead of recognising text itself, PaperRust can send documents to an online service. At the moment
this only works with Microsoft's
[Azure AI Document Intelligence](https://azure.microsoft.com/en-us/products/ai-services/ai-document-intelligence).
You need an Azure account; the service costs money (there is a small free tier) and has limits on
file size and page count. Microsoft has nothing to do with PaperRust. Keep in mind: your documents
leave your machine.

PaperRust calls the same interface as paperless-ngx (model `prebuilt-read`, text and searchable
PDF). This has been checked with simulated responses, not yet against the real Azure service.

Set it up via the [remote OCR settings](configuration.md#PAPERRUST_REMOTE_OCR_ENGINE), as an
environment variable or under *Manage → Configuration*.

Two modes ([`PAPERRUST_REMOTE_OCR_MODE`](configuration.md#PAPERRUST_REMOTE_OCR_MODE)):

- **Default:** every document of a suitable file type goes to the service, and local OCR is skipped.
- **`workflow_only`:** everything is recognised locally unless a
  [workflow action "Remote OCR"](workflows.md#workflow-action-remote-ocr) turns the service on for a
  document. You can then also choose remote OCR for single documents when using **Process again**.

<a id="architecture"></a>

## Architecture

Everything is in one program, `paperrust`:

- **`paperrust serve`** – the web server: web interface, REST API, login, downloads and live
  messages.
- **`paperrust serve --worker`** – additionally the background work in the same process: consumption
  (OCR, barcodes, matching), mail fetching, workflows, scheduled tasks and the consumption folder.
  With `--worker-only`, only the background work runs, without a web server.
- **`paperrust manage <command>`** – management commands such as `migrate` or `document_exporter`,
  see [Administration](administration.md).
- **PostgreSQL** stores everything, including the queue of background tasks. PaperRust does not need
  Redis, Celery, Django or Python.

For a careful switch, PaperRust could also sit in front of a running paperless-ngx installation;
this is only possible up to PaperRust v0.1.0, see [Mixed operation](installation.md#mixed-operation).
