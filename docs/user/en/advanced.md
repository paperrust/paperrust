# Advanced topics

[Deutsch](../advanced.md) · [Overview](README.md)

This page covers features that save you manual work: automatic assignment, your own scripts during
intake, barcodes, double-sided scanning with a single-sided feeder, and signing in through an
existing login service (SSO).

<a id="matching"></a><a id="matching-tags-correspondents-document-types-and-storage-paths"></a><a id="zuordnung"></a>

## Automatic assignment

Tags, correspondents, document types and storage paths can attach themselves to new documents.
Each of these objects has two fields for this:

- **Match:** a search text, e.g. `city utilities`.
- **Algorithm:** how the search text is compared with the document text.

During intake, PaperRust goes through all objects and checks whether their search text fits the
recognised text of the new document. If it does, the object is set. Example: a tag `Power` with the
search text `city utilities springfield` and the algorithm _Exact text_ ends up on every new
document that contains exactly these words in this order.

The algorithms at a glance:

| Algorithm | Fits when … |
| --- | --- |
| **None** | never – the object is not set automatically. |
| **Any word** | at least one of the words appears in the text. `Barclays Lloyds` fits documents with "Barclays" _or_ "Lloyds". |
| **Every word** | each of the words appears in the text, in any order. |
| **Exact text** | the whole search text appears exactly like that in the text. |
| **Regular expression** | the pattern finds a hit somewhere in the text. |
| **Similar word** | a piece of text is very close to the search text (at least 90 out of 100 points). Helps with small recognition errors. Punctuation is removed first. |
| **Automatic** | PaperRust has learned it from your existing documents. No search text needed, see [below](#automatic-matching). |

With _Any word_ and _Every word_, double quotes keep several words together:
`"National Rail" NR` fits "National Rail" or "NR", but not "National Post Railway".

With the **Ignore case** switch, upper and lower case do not matter. An empty search text never
fits (except with _Automatic_).

To try it out: enter search text and algorithm on the object, save, then add a new document. If
everything fits, the document carries the assignment right after intake. Existing documents are not
changed by this.

> [!NOTE]
> Write regular expressions in the usual Python syntax. PaperRust translates them internally. Very
> rare special forms (e.g. variable-length lookbehinds or recursive patterns) are not known to
> PaperRust; such patterns count as invalid and never hit. Instead of a time limit there is a fixed
> cap on the search effort, so a clumsy pattern cannot stall intake.

<a id="automatic-matching"></a>

### Let it learn automatically

When an object is set to _Automatic_, PaperRust looks at which documents you have given it so far
and learns from that. A small neural network does the work.

Example: all statements from your health insurer carry the correspondent "Health insurer", and it is
set to _Automatic_. After a number of documents, PaperRust recognises new statements by itself.

For this to work, keep in mind:

- **It does not learn instantly.** The model is retrained regularly, hourly by default (setting
  [`PAPERRUST_TRAIN_TASK_CRON`](configuration.md#PAPERRUST_TRAIN_TASK_CRON)). Changes take effect
  after the next training run.
- **The inbox does not count.** Documents with an inbox tag are skipped while learning. That way
  PaperRust only learns from documents you have already sorted.
- **The content has to relate to the object.** A statement usually contains a name and a member
  number – that can be learned. A tag like "Do later" has nothing to do with the text and cannot be
  learned.
- **It needs enough examples.** A correspondent with a single document will hardly be recognised.
  The more documents, the better.
- **It also needs counter-examples.** If there are only two correspondents and both are set to
  _Automatic_, every new document gets one of them – even if neither fits. As your collection grows,
  this usually sorts itself out.

The learned model lives in its own file (default `DATA_DIR/paperrust_classifier.model`, changeable
with `PAPERRUST_CLASSIFIER_MODEL_FILE`). After switching from paperless-ngx, the model is built anew
on the first training run; the old ngx model file is not used. Individual suggestions may therefore
come out a little differently than before.

<a id="consume-hooks"></a><a id="hooking-into-the-consumption-process"></a>

## Your own scripts during intake

You can have PaperRust start a program of your own for every document – once **before** intake
and/or once **after**. The script can be written in any language.

How to set it up:

1. Write the script and put it somewhere PaperRust can read it.
2. Make the script executable (on Linux: `chmod +x` and a first line like `#!/bin/sh`). PaperRust
   starts the file directly, without an extra shell and without arguments.
3. Enter the path: [`PAPERRUST_PRE_CONSUME_SCRIPT`](configuration.md#PAPERRUST_PRE_CONSUME_SCRIPT)
   for before, [`PAPERRUST_POST_CONSUME_SCRIPT`](configuration.md#PAPERRUST_POST_CONSUME_SCRIPT)
   for after.

Good to know:

- **PaperRust waits for the script.** A slow script slows down every intake. If something long
  should run in the background, start it from the script as a separate process and end the script
  right away.
- **Output goes to the log.** Each line of normal output is written to the server log as info, each
  line of error output as a warning.
- **The exit code matters.** If the script ends with a code other than 0, the task counts as failed
  (message "Error while executing … script"). If the script file does not exist, the task fails as
  well.
- Missing values usually appear in the variables as the text `None`; for the owner, as an empty
  text.

<a id="pre-consume-script"></a><a id="pre-consumption-script"></a>

### Script before intake

Runs as soon as PaperRust has accepted a new file and made a working copy – before text
recognition, barcodes and everything else. The script receives these environment variables:

| Variable | Content |
| --- | --- |
| `DOCUMENT_SOURCE_PATH` | path of the delivered original file |
| `DOCUMENT_WORKING_PATH` | path of the working copy PaperRust continues with |
| `TASK_ID` | ID of the intake task (may be empty) |

If you want to change the file, change **only the working copy** (`DOCUMENT_WORKING_PATH`). If you
change the original in the inbox folder, PaperRust may treat it as a new file and read it a second
time. Also change the file the same way every time (same input → same result), otherwise PaperRust
can no longer reliably detect duplicates.

Example: faxes arrive with a cover sheet. A script removes the first page from files in the `fax`
subfolder (using the `qpdf` tool, which you install yourself):

`/opt/paperrust/drop-fax-cover.sh`

```sh
#!/bin/sh
# Only touch files from the "fax" subfolder
case "$DOCUMENT_SOURCE_PATH" in
  */fax/*)
    echo "Removing cover sheet from $DOCUMENT_SOURCE_PATH"
    qpdf --replace-input "$DOCUMENT_WORKING_PATH" --pages . 2-z --
    ;;
esac
```

Entry in the configuration:

```bash
PAPERRUST_PRE_CONSUME_SCRIPT=/opt/paperrust/drop-fax-cover.sh
```

<a id="post-consume-script"></a><a id="post-consumption-script"></a>

### Script after intake

Runs once the document is fully stored. It can no longer undo the intake and should not change the
stored files (use the editing functions of the web interface for that). The script receives these
environment variables:

| Variable | Content |
| --- | --- |
| `DOCUMENT_ID` | number of the document |
| `DOCUMENT_FILE_NAME` | file name for downloads, without folders (date, correspondent, title, extension) |
| `DOCUMENT_TYPE` | document type |
| `DOCUMENT_CREATED` | issue date (`YYYY-MM-DD`) |
| `DOCUMENT_MODIFIED` | time of the last change |
| `DOCUMENT_ADDED` | time it was added |
| `DOCUMENT_SOURCE_PATH` | path of the stored original file |
| `DOCUMENT_ARCHIVE_PATH` | path of the archive version (PDF/A), if there is one |
| `DOCUMENT_THUMBNAIL_PATH` | path of the thumbnail |
| `DOCUMENT_DOWNLOAD_URL` | download path, e.g. `/api/documents/42/download/` |
| `DOCUMENT_THUMBNAIL_URL` | thumbnail path, e.g. `/api/documents/42/thumb/` |
| `DOCUMENT_OWNER` | user name of the owner |
| `DOCUMENT_CORRESPONDENT` | correspondent |
| `DOCUMENT_TAGS` | tags, separated by commas |
| `DOCUMENT_ORIGINAL_FILENAME` | original file name |
| `TASK_ID` | ID of the intake task (may be empty) |

The two URL values are paths without a host name; put your own address in front.

Example: every new document is recorded in a simple list that you can open in a spreadsheet:

`/opt/paperrust/new-documents.sh`

```sh
#!/bin/sh
LIST=/srv/paperrust/new-documents.csv
[ -f "$LIST" ] || echo "Time;ID;Correspondent;Tags;File" > "$LIST"
echo "$(date '+%Y-%m-%d %H:%M');$DOCUMENT_ID;$DOCUMENT_CORRESPONDENT;$DOCUMENT_TAGS;$DOCUMENT_FILE_NAME" >> "$LIST"
```

```bash
PAPERRUST_POST_CONSUME_SCRIPT=/opt/paperrust/new-documents.sh
```

> [!NOTE]
> PaperRust builds `DOCUMENT_FILE_NAME` more simply than paperless-ngx: date, correspondent and
> title with extension, invalid characters replaced, without extra shortening.

<a id="pdf-recovery"></a><a id="automatic-recovery-of-invalid-pdfs"></a>

## Repairing damaged PDFs

Some PDFs are badly built, for example by old scanners or broken downloads. PaperRust tries to
repair such a file before intake in two cases:

- **File type detected wrongly:** if a file ends in `.pdf` but is not recognised as a PDF, PaperRust
  rebuilds its internal references and checks the file type again.
- **PDF cannot be read:** PaperRust then tries a lenient repair, see
  [PDF tools: repair during intake](pdf-tools.md#reparieren).

In both cases PaperRust continues with the repaired copy. **The unchanged original file is what
gets stored.** If the repair fails, intake carries on normally and reports an error if needed. No
extra program such as `qpdf` is required for this.

<a id="barcodes"></a>

## Barcodes

Barcodes on your pages can give PaperRust instructions: start a new document here, assign this
archive number, set this tag. How to turn this on is described in the
[configuration](configuration.md#barcodes). The functions can be turned on one by one, but work
together as described below.

> **Tip:** PaperRust prints matching ASN labels and separator sheets itself – see
> [ASN labels](asn-labels.md).

Good to know:

- Barcodes are searched in PDFs, and in TIFF files only when
  [turned on](configuration.md#PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT).
- Only the **content** of a code matters, not its kind. The usual linear and 2D codes are
  recognised, among them QR Code, Data Matrix, Aztec, PDF417, Code 128, Code 39, Code 93, Codabar,
  EAN-8, EAN-13, UPC-A, UPC-E and Interleaved 2 of 5.
- To look for barcodes, pages are turned into images with the PDFium PDF library. Without PDFium,
  barcode detection is off.
- After switching from paperless-ngx: split documents contain the same pages as with paperless-ngx,
  but the files are not byte-for-byte identical and therefore have different checksums.

<a id="document-splitting"></a>

### Splitting documents

You can scan a whole stack at once and PaperRust splits it into separate documents. Put a
**separator sheet** between the documents, with a barcode whose content equals the configured
separator string (default `PATCHT`).

Default behaviour:

- The separator sheet is **thrown away**.
- The next document starts with the page **after** the separator sheet.

If [`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES)
is turned on, the page with the separator barcode is kept and becomes the **first page** of the new
document.

<a id="archive-serial-number-assignment"></a>

### Assigning an archive number (ASN)

If a barcode's content starts with the ASN prefix (default `ASN`), PaperRust takes the number after
it as the document's archive number. `ASN00123` becomes archive number 123. The number on the
sticker leads you quickly to the paper in your binder.

If splitting is turned on as well, a new document starts at every ASN barcode. Unlike the separator
sheet, **this page stays** in the document – it is usually the first page of the letter you put the
sticker on.

<a id="tag-assignment"></a>

### Tags by barcode

PaperRust can turn barcodes into tags. By default a barcode `TAG:Invoice` becomes the tag
"Invoice"; if the tag does not exist yet, PaperRust creates it. Which contents become which tags is
set with [`PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING`](configuration.md#PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING);
the function is turned on with
[`PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE`](configuration.md#PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE).

<a id="splitting-on-tag-barcodes"></a>

#### Splitting at tag barcodes

Normally tag barcodes only set tags. With
[`PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT`](configuration.md#PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT) `=true`
every page with a tag barcode also starts a new document:

- The page with the tag barcode **stays** in the new document.
- Each part gets **only the tags of its own pages**.
- A stack can have as many of these split points as you like.
- ASN barcodes work at the same time; each part gets its own number and its tags.

This lets you split and sort a mixed stack in one pass.

**Example:** 6 pages, `TAG:Invoice` on page 3, `TAG:Receipt` on page 5. Result: three documents –
pages 1–2 without a tag, pages 3–4 with "Invoice", pages 5–6 with "Receipt".

<a id="barcode-contents"></a>

### Storing barcode contents

If you like, PaperRust remembers the content of **all** barcodes it finds on the document, such as
payment codes or QR codes on invoices. Turn it on with
[`PAPERRUST_CONSUMER_STORE_BARCODE_VALUES`](configuration.md#PAPERRUST_CONSUMER_STORE_BARCODE_VALUES).

- In the web interface they appear on the **Metadata** tab with page, kind and content; the content
  can be copied.
- The API endpoint `/api/documents/{id}/metadata/` lists them under `barcodes`.
- You can [search for them](usage.md#searching-barcodes), for instance with `barcodes:DE89370400440532013000`.
- Only the first
  [`PAPERRUST_CONSUMER_BARCODE_MAX_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_MAX_PAGES)
  pages are searched. For existing documents: have them reprocessed.
- Each version of a document has its own barcodes; those of the newest version are shown and
  searched.

<a id="collate"></a><a id="automatic-collation-of-double-sided-documents"></a>

## Double-sided scanning with a single-sided feeder

> [!NOTE]
> If your scanner can scan both sides by itself, you do not need this.

This function is off at first. To turn it on, see [configuration](configuration.md#collate).

<a id="summary"></a>

### What it is for

Many scanners with a document feeder only scan one side of the sheet. For double-sided letters you
then scan the stack twice: once the front sides, once the back sides. PaperRust joins the two scans
into **one** document and puts the pages in the right order.

<a id="usage-example"></a>

### How it works

Example: a letter with 3 sheets, so 6 pages.

1. Put the stack in as usual, page 1 first. The scanner captures pages **1, 3, 5**.
2. Put the file into the double-sided subfolder of the inbox folder (default name `double-sided`).
   **You have to create this folder yourself.** PaperRust accepts the scan and sets it aside.
3. Turn the whole stack over without re-sorting the sheets and scan again. Now pages **6, 4, 2**
   come through.
4. Put this file into the subfolder as well. PaperRust reverses the order of the second scan,
   interleaves both scans and reads the result (pages 1 to 6) in as usual.

> [!TIP]
> You may leave out empty back sides **at the end** in the second scan. If page 6 is empty, pages 2
> and 4 are enough. Empty pages **in the middle** must be scanned, otherwise the order gets mixed up.

<a id="things-that-could-go-wrong"></a>

### When something goes wrong

- **Second scan has more pages than the first:** then the scans do not belong together (often the
  feeder swallowed sheets in the first pass). PaperRust discards both and reports an error. Start
  over: front sides first, then back sides.
- **Order and timing:** the two scans must arrive one after the other, separately, while PaperRust
  is running. By default PaperRust checks the inbox folder every second. If you have set
  [`PAPERRUST_CONSUMER_POLLING_INTERVAL`](configuration.md#PAPERRUST_CONSUMER_POLLING_INTERVAL)
  higher, keep the value shorter than the time between your two scans.
- **Forgot the second scan:** a single front-side scan waits for **30 minutes** at most. After that
  the next scan counts as a new first scan again, and the old one is discarded. That way pages from
  different days are not mixed by mistake.

<a id="interaction-with-subdirs-as-tags"></a>

### Together with "subfolders as tags"

If you use [subfolders as tags](configuration.md#consume_config), this also works with the
double-sided folder. It may sit anywhere in the path: `double-sided/Taxes/2025` and
`Taxes/2025/double-sided` both produce a document with the tags "Taxes" and "2025" – without a tag
"double-sided".

<a id="interaction-with-document-splitting"></a>

### Together with separator sheets

[Separator sheets](#document-splitting) work here too. If the separator sheet is printed on one side
only, its back side ends up as an (empty) first page in the next document. The fix: a separator
sheet that carries the separator barcode on **both** sides. Then both sides are dropped.

<a id="sso-and-third-party-authentication-with-paperrust"></a><a id="sso"></a>

## Signing in through other services (SSO)

Normally users sign in to PaperRust directly with name and password. If you already run a central
login service (e.g. Authentik, Keycloak, Authelia), PaperRust can use it. There are two ways.

<a id="remote-user-authentication"></a>

### Through a reverse proxy (remote user)

A reverse proxy in front of PaperRust handles the login and passes the user name in an HTTP header.
PaperRust trusts this header and signs the user in; if the user does not exist yet, it is created.

- Turn it on with [`PAPERRUST_ENABLE_HTTP_REMOTE_USER`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER)
  (for the web interface) or [`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER_API)
  (for the API too).
- Header name: [`PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME`](configuration.md#PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME)
  (default: `Remote-User`).
- Where to go after signing out: [`PAPERRUST_LOGOUT_REDIRECT_URL`](configuration.md#PAPERRUST_LOGOUT_REDIRECT_URL).

> [!WARNING]
> PaperRust does not check where the header comes from. Only turn this on if PaperRust can be
> reached exclusively through the proxy and the proxy always removes a header sent from outside.
> Otherwise anyone can pose as any user.

<a id="openid-connect-and-social-authentication"></a>

### Through OpenID Connect (OIDC)

PaperRust can connect directly to a login service that speaks **OpenID Connect**. The sign-in page
then shows a button for this service.

Setup in two steps:

1. **At the login service**, create a new application (client). As redirect address enter:
   `https://<your-paperrust-address>/accounts/oidc/<provider_id>/login/callback/`
   (you choose `<provider_id>` yourself, see below). Note the client ID and secret.
2. **In PaperRust**, set two settings, here with Authentik as the example:

```conf
PAPERRUST_APPS="allauth.socialaccount.providers.openid_connect"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"openid_connect": {"APPS": [{
  "provider_id": "authentik",
  "name": "Company login",
  "client_id": "<CLIENT_ID>",
  "secret": "<CLIENT_SECRET>",
  "settings": {"server_url": "https://auth.example.org/application/o/paperrust"}
}]}}'
```

`name` is the button label. `server_url` is the address of the service (without a trailing slash);
PaperRust appends `/.well-known/openid-configuration`. You can also enter the full address with
`/.well-known/` right away. Several services are possible: just add more entries to the `APPS`
list. All related settings are in the [configuration](configuration.md#PAPERRUST_SOCIALACCOUNT_PROVIDERS),
for example whether new users may register themselves
([`PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS`](configuration.md#PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS),
[`PAPERRUST_SOCIAL_AUTO_SIGNUP`](configuration.md#PAPERRUST_SOCIAL_AUTO_SIGNUP)) and whether groups
are taken over from the login service ([`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS`](configuration.md#PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS)).

**Connecting an existing account:** sign in as usual, open **My profile** in the user menu and
connect the account at the login service there. After that, signing in via the button works too.

<a id="social-login"></a>

**GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea:** These providers also work directly through their
own interface (as in paperless-ngx with the allauth packages). Put the matching app into
`PAPERRUST_APPS` (several separated by commas) and the provider with client ID and secret into
`PAPERRUST_SOCIALACCOUNT_PROVIDERS`, for example:

```conf
PAPERRUST_APPS="allauth.socialaccount.providers.github,allauth.socialaccount.providers.google"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"github": {"APPS": [{"client_id": "<ID>", "secret": "<SECRET>"}]},
  "google": {"APPS": [{"client_id": "<ID>", "secret": "<SECRET>"}]}}'
```

Callback address (redirect URL) to enter at the provider:
`https://<your-paperrust-address>/accounts/<provider>/login/callback/`
(e.g. `…/accounts/github/login/callback/`). PaperRust does not know other allauth providers (such as
Facebook). They are skipped with a note in the log; signing in with a password and with the other
providers keeps working.

<a id="email-verification"></a>

**Confirming the e-mail address:** With
[`PAPERRUST_ACCOUNT_EMAIL_VERIFICATION`](configuration.md#PAPERRUST_ACCOUNT_EMAIL_VERIFICATION)`=mandatory`
every account must confirm its e-mail address before it can sign in. PaperRust sends an e-mail with
a link for this. This needs [sending e-mail](configuration.md#email-sending) to be set up and an
e-mail address on every account; an account without an address then cannot sign in (note in the
log). With `optional`, a confirmation is only sent on registration, but signing in also works
without it.

What does not work:

- The signature of the ID token is not checked additionally. The token comes directly and encrypted
  from the login service; paperless-ngx handles it the same way.

<a id="disabling-regular-login"></a>

### Signing in only through the login service

Once SSO is working, you can turn off the normal sign-in with name and password
([`PAPERRUST_DISABLE_REGULAR_LOGIN`](configuration.md#PAPERRUST_DISABLE_REGULAR_LOGIN)). With
[`PAPERRUST_REDIRECT_LOGIN_TO_SSO`](configuration.md#PAPERRUST_REDIRECT_LOGIN_TO_SSO), users go
straight to the login service instead of the sign-in page.

<a id="gpg-decryptor"></a><a id="decryption-of-encrypted-emails-before-consumption"></a>

## Decrypting encrypted e-mails

With [`PAPERLESS_ENABLE_GPG_DECRYPTOR`](configuration.md#PAPERRUST_ENABLE_GPG_DECRYPTOR) PaperRust
decrypts GPG-encrypted mails before consuming them, using the keys from
[`PAPERLESS_EMAIL_GNUPG_HOME`](configuration.md#PAPERRUST_EMAIL_GNUPG_HOME) (otherwise GnuPG's default
folder). The program `gpg` must be installed for this – it is not included in the PaperRust Docker
image. If it is missing or decryption fails, the mail is not processed and an error is written to the
log.

<a id="requirements"></a><a id="setup"></a><a id="troubleshooting"></a>
