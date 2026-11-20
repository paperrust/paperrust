# For companies: operation and permissions

[Deutsch](../firmen.md) · [Documentation overview](README.md)

PaperRust also works for offices and small companies: one server, many staff, and everyone only sees
what they are allowed to see. This page sums up what is available for this **today**. Plans for more
(rolling out the app, device management, connecting accounting software) are in the concept
[docs/FIRMEN.md](../../FIRMEN.md) (German) – none of that has been built yet.

## Operation

- **One server, no screen needed:** PaperRust runs as a Docker container or system service
  (`paperrust serve --worker`) next to PostgreSQL, e.g. on a company server, a NAS or in the cloud
  ([Installation](installation.md)). It needs little memory and restarts in seconds.
- **Access:** in the browser or with the desktop app in "connect to a server" mode; the app can also
  keep selected documents available offline ([docs/REMOTE-APP.md](../../REMOTE-APP.md), German).
  Every device gets its own key, which the user can revoke themselves.
- **Run it behind a reverse proxy** with HTTPS; set `PAPERRUST_URL`
  ([Configuration](configuration.md)).
- **Several processes** (web server and worker separate) work with `PAPERRUST_CACHE=postgres`
  ([Configuration](configuration.md#message-broker)).
- **Settings** are set by an administrator in the interface (*Manage → Configuration → Server*);
  only the database and `SECRET_KEY` need to be in the environment
  ([Settings in the UI](configuration.md#settings-ui)).

## Sign-in

- **Company account:** sign in via OpenID Connect (e.g. Microsoft Entra ID, Keycloak, Authentik) or
  directly via Microsoft, Google, GitHub, GitLab, Nextcloud, Discord, Gitea; if you want, only that way
  ([Advanced → SSO](advanced.md#openid-connect-and-social-authentication)).
- **Sign-in via the proxy** (remote user header), for example with Authelia
  ([Advanced](advanced.md#remote-user-authentication)).
- **Two-factor sign-in** (TOTP) per user ([Permissions](permissions.md#two-factor-authentication)).
- **E-mail confirmation** of new accounts can be made mandatory
  ([Advanced](advanced.md#email-verification)).

## Permissions

- **Roles:** *Read only*, *Edit*, *Manage*, *Administrator* – or your own roles. You give a role to a
  user or to a whole group (e.g. "Accounting")
  ([Permissions → Roles](permissions.md#roles)).
- **Per document:** "Visible to" – only me, my team or certain people. Also for many documents at
  once.
- **Automatically:** workflows can set owner and permissions during import
  ([Automation](automatik.md)).
- The server checks every request; search, AI and the apps only see what the user may see.

## Tracing and protecting

- **History** per document: who changed what and when (audit log, as in paperless-ngx).
- **Integrity check** of the files on a schedule ([Integrity check](integrity.md)).
- **Ransomware protection:** guard with read-only mode and encrypted backups with versions, also on
  S3 with Object Lock ([Ransomware protection](ransomware.md)).
- **AI** only if switched on; for confidential documents use your own model in the company network
  ([AI](ai.md)).

## What is still missing

- A pre-configured installation of the app for IT (server address fixed in advance), a lean app
  without a database, a device overview for administrators (block a lost device).
- Offline changes in the app that are sent later.
- Connection to ERP/accounting software.

All of this is described in the concept [docs/FIRMEN.md](../../FIRMEN.md), but not built.
