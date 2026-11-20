# Connecting scanners

[Deutsch](../scanner.md)

PaperRust can take scans in two ways **without storing a password for your NAS or server on the
scanner**. Both hand the result to the normal processing, just like an upload: OCR, matching,
workflows, permissions.

| Way | Where do you start the scan? | What does the scanner need? |
|---|---|---|
| **Scan button** (AirScan/eSCL) | in PaperRust, on a computer or phone | nothing – PaperRust fetches the pages |
| **Scan to email** (device email) | at the printer | its own token that can only upload |

The existing consume folder stays as it is.

> **Status:** Both ways are tested against a simulated test scanner and a local SMTP client.
> **The test with a real HP OfficeJet Pro 9010 is still pending.** Menu names on the printer can
> differ slightly depending on the firmware.

## Way 1: scan button (AirScan / eSCL)

Many network printers and scanners speak **eSCL** (Apple calls it "AirScan"), including the
HP OfficeJet Pro 9010. PaperRust talks to the scanner directly over the network.

### Setup (superuser)

1. Open **Administration → Scanners** (`/rs/app/tools/scanners`).
2. Click **Search the network** and add the scanners it finds with **Add**.
   - Nothing found? Enter the printer's address (e.g. `192.168.1.20`) and click
     **Add by address**. The IP is in the printer menu under *Network* or on the network
     configuration page.
   - In **Docker** with the usual bridge network the search finds nothing (mDNS does not reach the
     container). Always enter the address there.
3. Tip: give the printer a **fixed IP address** in your router, otherwise it changes at some point.

When adding, PaperRust checks that an eSCL scanner really answers at that address. Normal users
see only the name, not the address.

### Scanning

1. Open **Scan** in the sidebar (`/rs/app/tools/scan`). You need the same right as for uploading:
   "add documents".
2. Choose the scanner, the **source** (feeder or flatbed), **both sides** if needed, the
   **resolution** (default 300 dpi) and **colour**. Optionally a title.
3. Click **Scan**. PaperRust fetches page after page until the feeder is empty, turns them into
   **one PDF** and hands it over for processing.

The document **belongs to the user who pressed the button**, as with an upload in the web UI.
Workflows with the trigger "document added" and the source **Web UI** apply.

| Message | What to do |
|---|---|
| Scanner cannot be reached | Is the printer on? In the same network? Address still right? |
| Scanner is busy | Something is running on the printer. Wait a moment. |
| Feeder is empty | Put paper in the feeder or choose "flatbed". |
| Paper jam | Clear the jam on the device and scan again. Nothing was saved. |

## Way 2: scan to email from the printer (device email)

For scans you start **at the printer**. PaperRust has a small built-in **SMTP receiver** for
this. It accepts mail only from your devices and **never forwards anything**.

### 1. Turn on receiving (superuser)

**Settings → System** (or environment variables), then restart PaperRust:

| Setting | Value | Meaning |
|---|---|---|
| [`PAPERRUST_SMTP_INTAKE`](configuration.md#paperrust_smtp_intake) | `on` | receiving on (default: off – nothing listens) |
| `PAPERRUST_SMTP_INTAKE_LISTEN` | `0.0.0.0:2525` | address and port |
| `PAPERRUST_SMTP_INTAKE_ALLOWED_IPS` | e.g. `192.168.1.20` | only these senders (empty = all) |
| `PAPERRUST_SMTP_INTAKE_MAX_MB` | `50` | largest message |

On Linux, port 25 needs administrator rights, so 2525 is the default. Almost every printer lets
you set the port.

**Docker:** also publish the port, e.g. with
[`docker/docker-compose.smtp-intake.yml`](../../../docker/docker-compose.smtp-intake.yml):

```bash
docker compose -f docker-compose.postgres.yml -f docker-compose.smtp-intake.yml up -d
```

In Docker's normal bridge network PaperRust often sees Docker's gateway address as the sender
(e.g. `172.17.0.1`), not the printer's. IP lists then do not work as intended. In that case bind
the port only to the host's LAN address (e.g. `192.168.1.10:2525:2525`) and leave the IP list
empty – the token still protects the receiver.

**Desktop app:** the port is only opened while receiving is on. Windows asks for firewall
permission the first time – allow it for **private networks** only.

### 2. Add the device and create a token (superuser)

1. **Administration → Scanners**, section **Scan to e-mail (devices)**.
2. Enter a **device name** (e.g. "HP office"), the **owner of the documents** and optionally the
   **printer's IP**, then click **Create token**.
3. **Copy the token** (`scan-…`) right away. It is shown only once and stored only as a hash.

The token can **only upload**: it cannot read, change or delete anything. It does not work for the
web UI or the API. You can **revoke** it at any time – it takes effect immediately.

The device can identify itself in two ways:

- **Login (recommended):** token as the **SMTP password**, any user name.
- **Recipient address:** send to `scan-…@your-server` (for devices without SMTP login).

### 3. Set up the HP OfficeJet Pro 9010 (EWS)

> Not yet checked on the real device. The terms come from HP's web interface for this printer
> series and may differ slightly depending on the firmware.

1. Open the printer's IP address in a browser (**Embedded Web Server**, EWS). Continue past the
   certificate warning. Sign in with the printer's administrator password (often on a sticker on
   the device).
2. Tab **Scan** → **Scan to Email** → **Outgoing Email Profiles** → **New**.
3. Enter:

   | Printer field | Value |
   |---|---|
   | Email address (sender) | e.g. `printer@home.local` (anything) |
   | Display name | e.g. `HP office` |
   | SMTP server | IP or name of the PaperRust machine, e.g. `192.168.1.10` |
   | SMTP port | `2525` (or your `PAPERRUST_SMTP_INTAKE_LISTEN` port) |
   | Use SSL/TLS | **off** – unless you set up a certificate (see below) |
   | Server requires authentication | **on** |
   | User name | anything, e.g. `hp` |
   | Password | the token `scan-…` |

4. **Test** or **Save**. Then add a recipient under **Scan to Email**, e.g.
   `paperrust@home.local` (the address does not matter, the login does).
5. At the printer: **Scan → Email**, choose the profile and recipient, file type **PDF**, scan.

The printer sends the PDF as an attachment. PDF, JPEG and TIFF are accepted (detected by content,
not file name); other attachments are ignored. If a mail has no suitable attachment at all,
PaperRust rejects it and the printer shows an error.

The document belongs to the **owner you chose for the device**. Workflows with the source
**API upload** apply (not "consume folder" – adjust such workflows if the printer used to scan into
the consume folder).

### 4. Remove the old network folder access from the printer

If the printer used to scan **into a network folder (SMB)**, your NAS password is stored on it.
Once scan to email works:

1. In the EWS: **Scan** → **Scan to Network Folder** → select the old profile → **Delete**.
2. On the NAS, **disable the account** the printer used **or change its password** – ideally it was
   an account just for the printer.
3. Check whether the printer has the password anywhere else (address book, quick sets).

## Security

- **No relay:** the SMTP receiver never forwards mail and never sends anything. Without a valid
  token, recipients are refused with "Relaying denied".
- **Upload only:** device tokens are valid only here. They are stored only as a SHA-256 hash, never
  appear in the log and can be revoked immediately. If the owner is deactivated, the token stops
  working.
- **Limits:** message size, messages per minute and device
  (`PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE`, default 30), after 5 failed logins PaperRust blocks
  the IP for 10 minutes, at most 16 connections at once.
- **Sender IPs:** can be restricted globally (`PAPERRUST_SMTP_INTAKE_ALLOWED_IPS`) and per device.
- **Encryption:** without a certificate the token travels **unencrypted**. Then run it only in your
  own trusted LAN and **never** forward the port to the internet. With a certificate:
  - Set `PAPERRUST_SMTP_INTAKE_TLS_CERT` and `PAPERRUST_SMTP_INTAKE_TLS_KEY` (PEM files) → the
    receiver offers **STARTTLS**. With `PAPERRUST_SMTP_INTAKE_REQUIRE_TLS=on`, login is only
    possible encrypted.
  - Some printers (often with "SSL" and port 465) want **direct TLS** instead of STARTTLS:
    `PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS=on` and a port such as `0.0.0.0:465` or another free port.
    Whether the HP OfficeJet Pro 9010 uses STARTTLS or direct TLS has not been checked yet.
  - A self-signed certificate is usually enough; many printers do not check it.
- **Scan button:** PaperRust only connects to scanners a superuser added (users cannot enter
  addresses). With `https` (`_uscans`) the printer's certificate is not checked, because printers
  almost always use self-signed certificates.

Technical details: [docs/contract/pl-scan.md](../../contract/pl-scan.md).
