# Folder view

[Deutsch](../folders.md)

The **folder view** sorts documents like a file manager: every document lies in **at most one** folder. Folders
can have subfolders. Documents without a folder are listed under **Unsorted**. Tags, correspondents, search and
saved views keep working unchanged – the folder is just an additional order.

The folder view exists only in PaperRust (not in paperless-ngx) and only in the PaperRust design.

<a id="einschalten"></a>
## Switching it on

The view is off by default. Switch it on under **Settings → Appearance → Folder view** (per user). The navigation
then shows the item **Folders**. Switching it off hides everything again; folders and assignments are kept.

<a id="bedienen"></a>
## Using it

- **On the left** is the folder tree. At the top **Unsorted (count)**, below it your folders with the number of
  documents directly inside. The arrow expands and collapses subfolders.
- **On the right** is the usual document list, restricted to the selected folder. Filters and search work inside
  the folder. The breadcrumbs on top show the path (`Folders › Invoices › 2026`).
- **New folder:** folder button above the tree. If a folder is selected, a subfolder is created.
- **Rename** and **delete:** pencil and bin buttons, they act on the selected folder. Folder names are unique per
  level (case-insensitive).
- **Deleting never deletes documents.** You choose where the contents go: to the parent folder or to "Unsorted"
  (subfolders then move to the top level).
- **Moving a folder:** drag a folder in the tree onto another folder with the mouse. Dropped on "Unsorted", it moves
  to the top level. A folder cannot go into itself or one of its own subfolders.

<a id="kacheln"></a>
## List or tiles

Above the list on the right you switch between **List** and **Tiles**. PaperRust remembers the choice for your
user account.

- The tile view first shows the **subfolders** of the open folder (under "Unsorted": your top-level folders), then
  the **documents** as thumbnails with their title.
- **Double-click** or **Enter** on a folder tile opens the folder. On a phone a single tap is enough.
- A **click** on a document opens it. The tiles then move closer together and the document appears on the right.
- **Selecting** works like in the list: **Ctrl-click** (Mac: Cmd-click) adds or removes single documents,
  **Shift-click** selects everything between the last clicked and the new document. The arrow keys move through
  the tiles, the space bar selects.

<a id="verschieben"></a>
## Sorting documents in

- **Drag and drop:** drag a document (or several selected ones) from the list or the tiles with the mouse onto a
  folder in the tree, onto a folder tile or onto "Unsorted". The target is highlighted while you drag over it.
  Afterwards **Undo** in the notice brings everything back.
- **Several documents:** select them in the list, then choose **Move to folder** in the bar. "Unsorted" takes the
  documents out of their folder. This also works with the keyboard and on phones (there is no dragging there).
- **One document:** change the **Folder** field in the details.

You need the "change documents" permission for every document and the change permission on the target folder.

<a id="hochladen"></a>
## Uploading straight into a folder

While a folder is open, newly uploaded documents land **in that folder** – whether you drop the files onto the
window or choose **Upload**. While dragging, PaperRust shows "Into folder …". Under "Unsorted" or outside the folder
view they land without a folder as before.

You need the change permission on the folder. If the folder is deleted before processing has finished, the
document lands in "Unsorted".

<a id="rechte"></a>
## Who sees which folders?

- A new folder belongs to you. Folders without an owner are visible to everyone.
- Sharing folders is so far only possible via the API (`/rs/api/v2/folders`, field `set_permissions`).
- Folder permissions are **not inherited** by the documents: a document you may not see stays hidden inside a
  folder too. A document in a folder you cannot see appears under "All documents", but not under "Unsorted".

<a id="sicherung"></a>
## Backup and the way back to paperless-ngx

- `document_exporter` writes the folders to `paperrust-extras.json`; PaperRust's `document_importer` restores them.
- paperless-ngx has no folders. After switching to paperless-ngx the folders are not visible there (tags,
  correspondents etc. stay complete).
- Programs that upload through the REST API can send the additional field `folder` (folder ID). The field exists
  only in PaperRust; paperless-ngx ignores it.
- In the REST API (v1) every document has the additional read-only field `folder` as soon as at least one folder
  exists; the filters are `folder__id`, `folder__id__in`, `folder__id__tree` (with subfolders) and
  `folder__isnull=1` (Unsorted).
