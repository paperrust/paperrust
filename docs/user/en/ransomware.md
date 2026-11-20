# Ransomware protection

[Deutsch](../ransomware.md) · [Integrity check](integrity.md) · [Administration](administration.md)

Ransomware encrypts files and demands money for the key. The typical case for PaperRust: PaperRust
runs in Docker on a Synology NAS, a Windows PC on the home network gets infected, and the ransomware
encrypts everything it can write to over network shares – including PaperRust's media folder if that
folder is reachable as a share.

PaperRust helps in two ways:

1. **Guard + read-only mode:** PaperRust notices when files in the archive change without PaperRust and
   then blocks all writes, so that nothing else gets damaged.
2. **Automatic, encrypted, versioned backups:** they bring back the state from before the attack – even
   if the attacker gets hold of the credentials (S3 with Object Lock).

> **Important:** No program can stop an infected PC from encrypting files it is allowed to write. The
> best protection is that it cannot write to them at all (see [NAS advice](#nas-advice)), plus a
> backup it cannot reach.

## What PaperRust does – and what it cannot do

| PaperRust does | PaperRust cannot |
|---|---|
| detect changes to originals, archive files and thumbnails that were not made by PaperRust | stop a PC with write access from encrypting files |
| detect typical extensions (`.locked`, `.encrypted`, …), ransom notes and content that looks encrypted | detect attacks on other shares or programs on the NAS |
| switch to **read-only mode** above a threshold (no consumption, no changes) | decrypt files that are already encrypted |
| raise the alarm as a banner, by mail to the admins and in the log, with the list of affected files | replace a backup that lives on the same device as the original |
| back up the database and files encrypted, deduplicated and versioned | protect you if the NAS itself (admin account, DSM) is taken over – that is what immutable snapshots and an off-site copy are for |
| lock backups on S3 with **Object Lock** against deletion and overwriting | protect a backup on a writable share from the infected PC |

## The guard

The guard runs in PaperRust's worker and watches `MEDIA_ROOT/documents/`:

- **File system events** where available (Linux/Docker: inotify; Windows; macOS). It also sees changes
  that arrive through an SMB share of the NAS, because Samba writes on the same system.
- **Periodic scan** (default every 15 minutes): compares size and modification time of all files with
  the previous state – without reading contents.
- **Sample:** on each scan a few documents (default 20) are fully hashed and compared with the stored
  checksum, in turn over all documents. This also catches encryption that keeps size and modification
  time.

PaperRust itself only changes originals through known operations – new version, renaming via storage
path, reprocessing, emptying the trash – and afterwards file and database always match. So the guard
waits one minute after each change and then checks:

| Finding | Meaning |
|---|---|
| *Changed without PaperRust* | the file belongs to a document, but its SHA-256 checksum no longer matches |
| *Looks encrypted* | as above, and the content is random (high entropy) instead of e.g. `%PDF…` |
| *Ransom note* | files like `README_DECRYPT.txt`, `HOW_TO_RESTORE_FILES…` |
| *Ransomware extension* | new file with `.locked`, `.encrypted`, `.crypt`, `.wncry`, … |
| *File missing* | a document points to a file that no longer exists |
| *Unexpected extension* | e.g. a `.png` in the archive folder, where PaperRust only writes PDFs |
| *Wrong file header* | extension `.pdf`, but no PDF header |

When **20 or more** such findings add up within **10 minutes** (configurable), PaperRust switches to
**read-only mode**:

- Uploading, consuming (folder, mail, scanner), editing and deleting are blocked. Writing API calls get
  `423 Locked` with `{"detail": "PaperRust is in read-only mode …", "code": "read_only_mode"}`.
- Viewing, searching and downloading keep working. The integrity check and the backup keep running
  (but old backups are **not** pruned then, so good states are kept).
- All users see a red banner; superusers get from there to **Administration → Protection**.
- Admins get a mail (if `PAPERRUST_EMAIL_HOST` is set up; recipients: all active superusers with an
  e-mail address, or `PAPERRUST_GUARD_MAIL`).
- The log (`paperrust.log`, logger `paperrust.guard`) records the alarm and the affected files.

### What to do after an alarm

1. **Find the infected computer and disconnect it from the network** (Wi-Fi off, unplug the cable). On
   the NAS, *Log Center → File Transfer* shows which computer wrote last.
2. Open **Administration → Protection** in PaperRust and look at the affected files. On the command
   line, `paperrust manage guard scan` lists every mismatch in the whole archive.
3. **Restore damaged files**, ideally from a Btrfs snapshot of the NAS (fast) or from the PaperRust
   backup (see [Restore](#restore)).
4. Check with `paperrust manage guard scan` that nothing is missing any more, then lift read-only mode
   on the **Protection** page (or with `paperrust manage guard lift`).

A false alarm (e.g. someone renamed files in the media folder by hand) is lifted the same way.

### Guard settings

| Variable | Default | Meaning |
|---|---|---|
| `PAPERRUST_GUARD` | `on` | guard on/off |
| `PAPERRUST_GUARD_THRESHOLD` | `20` | this many suspicious files … |
| `PAPERRUST_GUARD_WINDOW_MINUTES` | `10` | … within this many minutes switch to read-only |
| `PAPERRUST_GUARD_SCAN_INTERVAL` | `900` | seconds between two scans (`0` = none) |
| `PAPERRUST_GUARD_SAMPLE_FILES` | `20` | documents fully hashed per scan |
| `PAPERRUST_GUARD_WATCH` | `on` | use file system events |
| `PAPERRUST_GUARD_EXTRA_EXTENSIONS` | – | more suspicious extensions, comma-separated |
| `PAPERRUST_GUARD_MAIL` | all superusers | recipients of the alarm mail, comma-separated |

All values can also be changed in the UI under *Settings → System → Integrity*; the guard then restarts.

**Memory and load:** the scan keeps about 20 bytes per file (about 60 MB for 1 million documents with
three files each). It reads only directories and file attributes; only changed files and the sample
are fully hashed.

## Encrypted backups

Every night (default 02:15) PaperRust backs up the **database** and **all files** under `MEDIA_ROOT`
to a second target:

- **Target:** a folder (e.g. a mounted USB disk or a share on a *different* device) or
  **S3-compatible storage** (e.g. Synology C2 Object Storage, Backblaze B2, Wasabi, MinIO on a second
  device).
- **Deduplicated:** every file is stored under its SHA-256 checksum. Unchanged files are never
  transferred again – even with a million documents only the new ones travel each night.
- **Encrypted** on the server before anything leaves the device, with [age](https://age-encryption.org)
  (X25519, ChaCha20-Poly1305). To back up, PaperRust only needs the *public* key; to restore you need
  the **passphrase** that only you know.
- **Versions:** every run is its own snapshot. By default 7 daily, 4 weekly and 12 monthly snapshots
  are kept.
- **S3 Object Lock:** with `PAPERRUST_BACKUP_S3_OBJECT_LOCK=compliance` (or `governance`) every object
  gets a retention period (default 30 days). During that period **nobody** can delete or overwrite it –
  not even with stolen credentials. PaperRust extends the retention of objects that later snapshots
  still need.

### Setup

1. Choose the target, e.g. in `docker-compose.env`:

   ```sh
   # folder (e.g. a USB disk mounted into the container)
   PAPERRUST_BACKUP_TARGET=/backup
   # or S3
   PAPERRUST_BACKUP_TARGET=s3://my-bucket/paperrust
   PAPERRUST_BACKUP_S3_ENDPOINT=https://eu-002.s3.synologyc2.net
   PAPERRUST_BACKUP_S3_REGION=eu-002
   PAPERRUST_BACKUP_S3_ACCESS_KEY=…
   PAPERRUST_BACKUP_S3_SECRET_KEY=…
   PAPERRUST_BACKUP_S3_OBJECT_LOCK=compliance
   PAPERRUST_BACKUP_S3_LOCK_DAYS=30
   ```

   For Object Lock the bucket must be **created with Object Lock enabled** (most providers only allow
   that at creation time). The access key only needs read, write, list and `PutObjectRetention` – not
   the permission to bypass retention.

2. Initialize once – this creates the key, protected by your passphrase (at least 12 characters):

   ```sh
   docker compose exec -e PAPERRUST_BACKUP_PASSPHRASE='…' webserver paperrust manage backup init --show-key
   ```

   **Keep the passphrase and the secret key shown (`AGE-SECRET-KEY-1…`) safe and separate**, e.g. printed
   in a safe. Without them the backup is worthless. The passphrase is not stored anywhere.

3. Done: the backup runs on schedule (`PAPERRUST_BACKUP_CRON`, default `15 2 * * *`). By hand:
   **Administration → Protection → Back up now** or `paperrust manage backup run`.

The **Protection** page shows target, schedule, last success, size and all runs with errors.

<a id="restore"></a>

### Restore

```sh
# list snapshots
paperrust manage backup list
# check completeness (all objects present)
PAPERRUST_BACKUP_PASSPHRASE='…' paperrust manage backup check --snapshot latest
# restore the files into a new folder and the database into a new, empty database
PAPERRUST_BACKUP_PASSPHRASE='…' paperrust manage backup restore --snapshot 20261114T021500Z \
    --media /new/media --database
```

- `--database` first creates the schema (`manage migrate`) and then replaces all tables with the backed
  up state. Point `PAPERRUST_DB*` to a **new, empty** database for this.
- `--media DIR` writes all files of the snapshot to `DIR`; each file is checked against its checksum
  after decryption. Existing files with different content are only replaced with `--overwrite`.
- Instead of the passphrase you can use the key file: `--key-file key.txt`.
- For a state before the attack, give that snapshot's name from `backup list`.

### Backup settings

| Variable | Default | Meaning |
|---|---|---|
| `PAPERRUST_BACKUP_TARGET` | – | folder or `s3://bucket/prefix` |
| `PAPERRUST_BACKUP_CRON` | `15 2 * * *` | schedule, `disable` = off |
| `PAPERRUST_BACKUP_DATABASE` | `on` | include the database |
| `PAPERRUST_BACKUP_KEEP_DAILY` / `_WEEKLY` / `_MONTHLY` | `7` / `4` / `12` | retention |
| `PAPERRUST_BACKUP_S3_ENDPOINT` | – | `https://host[:port]` (path-style addressing) |
| `PAPERRUST_BACKUP_S3_REGION` | `us-east-1` | region |
| `PAPERRUST_BACKUP_S3_ACCESS_KEY` / `_SECRET_KEY` | – | credentials (the secret key is write-only in the UI) |
| `PAPERRUST_BACKUP_S3_OBJECT_LOCK` | `off` | `governance` or `compliance` |
| `PAPERRUST_BACKUP_S3_LOCK_DAYS` | `30` | retention per object |
| `PAPERRUST_BACKUP_PASSPHRASE` | – | only for `init`/`check`/`restore`, environment only, never stored |

**Limits:** object names are the SHA-256 checksums of the plain files. Someone with access to the target
can use them to check whether a file *they already know* is in the archive – they see neither contents
nor file names. A local folder as target is **not** protected against ransomware if the infected PC can
write to it; use S3 with Object Lock or immutable snapshots for that.

<a id="nas-advice"></a>

## NAS advice

These steps protect more than any program:

1. **Do not offer the media folder as an SMB share.** PaperRust does not need a share; documents come in
   through the UI, mail or the scanner. If `media` lives in a shared folder, remove write permission for
   everyone except the Docker account (DSM: *Control Panel → Shared Folder → Edit → Permissions*).
2. **Turn on immutable Btrfs snapshots** (DSM 7.2: *Snapshot Replication → Snapshots → Settings →
   Immutable snapshots*, e.g. hourly, immutable for 7 days). An immutable snapshot cannot be deleted
   before it expires – not even by an administrator. So the state from an hour ago is almost always there.
3. **Keep an off-site backup** (3-2-1 rule: three copies, two media, one off-site), e.g. the PaperRust
   backup on S3 with Object Lock or Hyper Backup to a device at a relative's home.
4. **Use the scan intake instead of a writable consume share.** Scanners send by e-mail to PaperRust's
   SMTP intake or are driven by the scan button (see [Scanner](scanner.md)). A writable `consume` share
   is a way in; if you keep it, make it writable only for the scanner account.
5. Disable the DSM admin account, turn on two-factor sign-in, keep DSM and packages up to date.

## Try it

Test the alarm safely – **only with a copy**:

```sh
cp -r media /tmp/media-test && touch /tmp/media-test/.paperrust-test-media
# start PaperRust with PAPERRUST_MEDIA_ROOT=/tmp/media-test and a copy of the database, then:
node crates/pl-guard/tests/simulate-ransomware.mjs /tmp/media-test --count 30
```

The script "encrypts" 30 originals into `*.locked` and drops a ransom note. After about a minute
PaperRust switches to read-only mode. Without the marker file `.paperrust-test-media` the script refuses
to run.
