# Users, groups and permissions

[Deutsch](../permissions.md) · [Documentation overview](README.md)

<a id="permissions"></a><a id="rechte"></a>

## How permissions work

Every time someone opens or changes something, PaperRust asks two questions:

1. **May this person use this area at all?** That is decided by the
   [global permissions](#global-permissions), for example "may view documents" or "may create
   tags".
2. **May they see or change this particular item?** That is decided by the
   [object permissions](#object-permissions). Every document, tag, correspondent and so on has an
   owner and can give other users or groups the right to view or to edit it.

Access only works when both answers are yes. The check always happens on the server, not just in
the web interface.

> [!TIP]
> Object permissions only cover the item itself. Making a tag private does not make the documents
> carrying that tag private.

To change a document's permissions, open it and use the **Permissions** tab. For many documents at
once, select them in the list and choose **Permissions**. There you decide whether the new
permissions replace the current ones or are added to them.

<a id="default-permissions"></a>

### Permissions of new items

- **Created in the web interface** (tag, document type, storage path …): the person who creates it
  becomes the owner; nobody else gets any rights. You can change this in *Settings* under
  **Permissions for new items** (owner, view rights, edit rights).
- **Picked up from the consumption folder:** the document starts without an owner and without any
  extra permissions. Use [workflows](workflows.md#workflows) ("Set owner", "Give view rights" …)
  to decide who owns it and who may read it.

<a id="users-and-groups"></a><a id="benutzer-und-gruppen"></a>

## Users and groups

In the PaperRust design you manage users and groups under *Manage → Users and groups*, in the
classic interface under *Settings → Users and groups*. To manage them you need the global
permissions "User" or "Group" yourself.

- A user receives every permission of the groups they belong to. The interface shows which
  permissions come from a group.
- A new user has **no permissions of their own**. They can only do what their groups allow.
- An easy approach: create groups by job (e.g. "Accounting", "Read only") and put users into them
  instead of setting permissions one by one.

<a id="roles"></a>

### Roles (PaperRust design)

Instead of ticking every permission one by one, you give a user or a group a **role**:

| Role | May |
|---|---|
| **Read only** | view documents, notes, tags, correspondents, document types, storage paths, fields and saved views |
| **Edit** | in addition add and change documents, write notes, create own views and share links |
| **Manage** | in addition delete; maintain tags, correspondents, types, storage paths, fields, workflows and mail accounts/rules; see tasks |
| **Administrator** | everything (superuser), including users and the system |

- **Adjust individual rights** (expandable) shows five topics, each with *View*, *Edit* and
  *Delete*. If you change something here, the user then has the role "Individual"; "Back to role …"
  restores the role.
- **Own roles:** An administrator can switch the role mode from "Classic" to "Custom" (link in the
  user management). Then *Manage → Roles* appears, where they create their own roles; the four roles
  serve as templates, "Administrator" stays fixed.
- What is stored is still the normal permissions, as in paperless-ngx. Users from paperless-ngx show
  up as "Individual" if their permissions do not exactly match a role; nothing is changed by this.
  If you switch back to paperless-ngx, only the role names are lost.

<a id="superusers"></a>

### Superusers

A superuser holds every permission and sees every item, no matter who owns it. Only a superuser can
turn another user into a superuser. Some areas are reserved for superusers, for example managing
[scanners](scanner.md).

> [!TIP]
> Use a normal account for everyday work – as a superuser you also see everyone else's documents.
> You can create a (further) superuser [on the command line](administration.md#create-superuser),
> or an existing superuser ticks the box in the user dialog.

<a id="admin-status"></a>

### Admin

The **Admin** box in the user dialog lets a user see the system status and the logs. It grants
nothing else.

> [!NOTE]
> For people switching over: in paperless-ngx this box ("staff") also opened the Django admin at
> `/admin/`. PaperRust has no such page; everything goes through the normal interface.

<a id="global-permissions"></a><a id="detailed-explanation-of-global-permissions"></a>

### Global permissions

Each area has four levels: **View**, **Add**, **Change** and **Delete**. They apply to the area as
a whole. Whether a given document is visible is then decided by its object permissions.

| Area | What it covers |
|---|---|
| AppConfig | The *Configuration* page (settings for the whole installation). Takes effect from _Change_.<br/>**Give it only to people you trust:** some values, such as OCR arguments, are handed straight to the processing programs. |
| Correspondent | Correspondents |
| CustomField | Custom fields |
| Document | Documents |
| DocumentType | Document types |
| GlobalStatistics | Counts and statistics about the whole archive. Does not open single documents. |
| Group | Groups |
| MailAccount | Mail accounts |
| MailRule | Mail rules |
| Note | Notes on documents |
| PaperlessTask | The list of file tasks; _Change_ allows dismissing entries |
| SavedView | Saved views |
| ShareLink | Share links (there is no Change level here) |
| StoragePath | Storage paths |
| SystemMonitoring | System status, task overview and the matching API paths. Users with **Admin** see the system status even without it. |
| Tag | Tags |
| UISettings | A user's stored interface settings.<br/>**Anyone using the web interface needs at least _View_ here.** |
| User | Managing other users (*Users and groups*, `/api/users/`). Not needed for one's own profile (*My profile*, `/api/profile/`). |
| Workflow | Workflows. There is one shared set of workflows; whoever may see them sees all of them. What this means for permissions: [workflow permissions](workflows.md#workflow-permissions). |

<a id="object-permissions"></a><a id="detailed-explanation-of-object-permissions"></a>

### Object permissions

| Permission | Meaning |
|---|---|
| Owner | Sees and changes the item. Only the owner hands out rights to others. For documents, only the owner may create share links and add or remove custom fields. |
| View | Read, not change. Without it the interface shows _Private_ instead of the name – for example a tag you may not see, on a document you may see. |
| Edit | Change; includes View. |

**Items without an owner** can be seen and edited by anyone with the matching global permission.
This keeps older archives without owners working.

**Linked items you may not see:** if a document carries a tag (or correspondent, document type,
storage path) that is _Private_ to you, that link stays in place when you save. You may send the
known ID along even though the item is missing from search and pick lists. You still cannot view
or change the item itself.

<a id="password-reset"></a>

## Forgotten password

For "Forgot password" on the sign-in page to work, PaperRust must be able to send mail. Enter a
mail server ([`PAPERRUST_EMAIL_HOST`](configuration.md#PAPERRUST_EMAIL_HOST) and the related
values) and set [`PAPERRUST_URL`](configuration.md#PAPERRUST_URL) – otherwise the link in the mail
points to the wrong address. The page does not reveal whether an account with that address exists.

<a id="two-factor-authentication"></a>

## Two-factor sign-in

With two-factor sign-in (2FA) you need, besides your password, a one-time code from an
authenticator app (TOTP).

To turn it on:

1. Open *My profile* and go to the two-factor sign-in (TOTP) section.
2. Scan the QR code with your authenticator app (or type in the secret).
3. Enter the code the app shows and confirm.
4. PaperRust now shows **10 recovery codes**. Store them safely right away – they are never shown
   again. Each code stands in for the app once, should your phone be missing.

From then on PaperRust asks for a code at every sign-in.

If the device is gone and the recovery codes are lost too, a superuser can turn off two-factor
sign-in for that user under *Users and groups*.

**Signing in through OpenID Connect:** if the sign-in service already checks a second factor,
PaperRust does not ask again. It recognises this from the service's `amr` value; which values count
as "second factor checked" is set by
[`PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR`](configuration.md#PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR)
(default: `mfa`, separate several with commas).
