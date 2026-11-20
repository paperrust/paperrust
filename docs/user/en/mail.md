# Incoming mail

[Deutsch](../mail.md) · [Documentation overview](README.md)

<a id="incoming-mail"></a><a id="incoming-email"></a>

## Taking documents from e-mail

Many invoices and letters arrive by mail these days. PaperRust can check your mailbox on a schedule
and file attachments (or whole mails) as documents. You need two things. In the PaperRust design you
set up accounts under *Manage → Mail* and rules under *Automation → Mail fetching* (there you also
see which workflows run after a rule – see [Automation](automatik.md#mail)). In the classic interface
both are under *Manage → Mail*.

1. **A mail account** – the login for the mailbox (IMAP server, port, SSL or STARTTLS encryption,
   user name, password).
2. **At least one rule** – it says which mails are taken and what happens to them afterwards.

On every check PaperRust works like this:

1. Sign in to the mail server.
2. For each rule (in the order you set), look for matching mails in the chosen folder.
3. Queue the matching files of every new mail for consumption. The documents get the values from
   the rule (tags, document type, correspondent, title, owner).
4. Once all files of a mail have been consumed: apply the rule's **action** to that mail (for
   example, mark it as read).

Mails that match no rule are left alone.

By default PaperRust checks **every 10 minutes**. Change the schedule with
[`PAPERRUST_EMAIL_TASK_CRON`](configuration.md#PAPERRUST_EMAIL_TASK_CRON). To check right away, use
the **Check now** button on the account, or run
[`paperrust manage mail_fetcher`](administration.md#mail) on the command line.

<a id="mail-rules"></a>

### What a rule sets

**Which mails**

- **Folder** – usually `INBOX`. Subfolders are written with the server's separator, often `.` or
  `/` (sometimes `|`). If PaperRust cannot find a folder, it writes every folder of the account to
  the log – that shows you the right spelling.
- **Maximum age (days)** – older mails are skipped.
- **Sender / Recipient / Subject / Body contains** – empty fields are ignored. A mail must meet
  **every** filled-in condition.

**What is taken**

- **What to import:** attachments only, the whole mail as an `.eml` file, or both (the mail as
  `.eml` plus each attachment on its own).
- **Attachment type:** real attachments only, or all files including inline ones (such as pictures
  inside the mail text).
- **Only files named / Skip files named:** patterns like `*.pdf` or `*invoice*`, several separated
  by commas, case does not matter.
- Only file types PaperRust can read are taken (PDF, images, text, Office, …). A mail without a
  suitable file is recorded as "processed, nothing taken".

**What the document gets**

- **Take title from:** the mail subject, the attachment file name, or nothing (then the usual
  consumption rules apply).
- The rule's **tags** and **document type**.
- **Take correspondent from:** leave empty, sender address, sender name, or one fixed
  correspondent. With address or name, PaperRust creates the correspondent if it does not exist yet.
- **Make the rule owner the document owner** – applies to attachments. A mail taken as `.eml`
  always gets the rule's owner.

**Order**

- Rules run by their **Order** field.
- **Stop here:** once this rule has taken something, the remaining rules for this account are
  skipped.
- Tip: a last "catch-all" rule without filters that sets a tag like "Mail – check" collects
  everything the earlier rules missed.

<a id="mail-actions"></a>

### What happens to the mail afterwards

The action makes sure the same mail is not taken a second time some other way. It only happens to
mails from which something was actually consumed.

| Action | Effect | Which mails the rule looks at |
|---|---|---|
| Mark as read (default) | the mail is marked as read | unread ones only – so a mail you already read yourself is not taken |
| Flag | sets the flag ("important") | unflagged ones only |
| Tag | sets a custom keyword (IMAP "keyword", a label on Gmail) | mails without that keyword only |
| Move to this folder | moves the mail to the given folder | all in the rule's folder |
| Delete | deletes the mail at once | all in the rule's folder – **use with care** |

For "Tag" and "Move", enter the keyword or the target folder under **Action value**. Not every mail
server supports custom keywords.

**Coloured flags in Apple Mail:** use `apple:<colour>` as the keyword, e.g. `apple:green`. Possible
values are `red`, `orange`, `yellow`, `blue`, `green`, `violet` and `grey`.

> [!TIP]
> "Mark as read" is the safest choice – nothing gets lost. If you like things tidy: create a folder
> "PaperRust" in your mailbox, move mails there (by hand or with a filter in your mail program) and
> let the rule move them on to an "Archived" folder once they are taken.

<a id="processed-mail"></a><a id="verarbeitete-mails"></a>

### Processed mails

PaperRust remembers every mail a rule has already handled – even when nothing was taken or an
error occurred. A mail is recognised by its number on the server (the IMAP `UID`). That way no mail
is handled twice.

You find the list on each rule under **Show processed mails**. It also tells you whether it worked
or why not.

- **If consuming a file fails**, the mail is recorded as failed and the action is **not** carried
  out. The mail stays where it was.
- **To handle a mail again:** delete its entry from the list. It is picked up on the next check
  (as long as it still matches the rule).

<a id="oauth-email-setup"></a>

### Gmail and Outlook with OAuth

For Gmail and Outlook you can sign in with OAuth2 instead of a password.

1. Register your own app with the provider (Google or Microsoft) and get a client ID and client
   secret there. As the redirect address enter `<your PaperRust address>/api/oauth/callback/`.
2. Enter client ID and secret in PaperRust ([OAuth settings](configuration.md#email_oauth)).
3. Set [`PAPERRUST_OAUTH_CALLBACK_BASE_URL`](configuration.md#PAPERRUST_OAUTH_CALLBACK_BASE_URL) or
   [`PAPERRUST_URL`](configuration.md#PAPERRUST_URL) to the address you use to open PaperRust in
   the browser.
4. Under *Settings → Mail* choose **Connect a Gmail account** or **Connect an Outlook account** and
   sign in with the provider.
5. In the newly created account, enter your e-mail address and save.

After that the account behaves like any other; PaperRust renews the sign-in by itself.

> [!NOTE]
> The OAuth path is new in PaperRust and has so far only been tested with test values, not with
> real Google or Microsoft accounts. If it fails, the log has the details. The proven path is an
> account with a password (on Gmail, an app password).

<a id="mail-notes"></a>

### Good to know

- **Mail servers in your own network:** allowed (default). With
  [`PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS`](configuration.md#PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS)`=false`
  PaperRust only accepts public addresses.
- **Time limit:** if the server does not answer a step within 60 seconds, PaperRust stops this check
  and tries again next time.
- **Encrypted mails (GPG):** With
  [`PAPERRUST_ENABLE_GPG_DECRYPTOR`](configuration.md#PAPERRUST_ENABLE_GPG_DECRYPTOR)`=true`
  PaperRust decrypts them before consuming them. For this the `gpg` program with your key must be
  installed (it is not included in the Docker image). If it is missing, an error is written to the log.
- **`.eml` files:** PaperRust reads the mail text itself (without Tika) and turns it into a PDF
  (headers, text, and for HTML mails also the HTML part with embedded images). Which parts go into
  the PDF is set by the rule's **PDF layout** setting or by
  [`PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT`](configuration.md#PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT).
  This needs `paperrust-pdftool` (included).
