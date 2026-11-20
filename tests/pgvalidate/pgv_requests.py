"""Requests as the Angular frontend (upstream/src-ui, v3.4.2) sends them – phase 6.

Source of the parameters: services/rest/abstract-paperless-service.ts (list/listAll/getFew),
services/document-list-view.service.ts (reload: truncate_content + include_selection_data),
saved-view-widget (page_size 10, truncate_content), data/filter-rule-type.ts (filter variables),
tasks.component.ts (acknowledged=false, status=…, page_size 25/10), search.service.ts.

Every entry: (name, method, path, body). `{DOC}` is replaced by a document ID visible to the user
(first ID of the default list).
"""

# The frontend always sends include_selection_data=true in the document list. Because with 100k
# documents that costs minutes in Rust, the sort/filter variants run WITHOUT and only two
# variants (+sel) WITH selection_data; total cost of the UI request = variant + selection_data share.
DL = "/api/documents/?page={page}&page_size=50&ordering={ord}&truncate_content=true"
SEL = "&include_selection_data=true"

REQUESTS = [
    # ---- App-Start (app.component / settings.service / permissions)
    ("ui_settings", "GET", "/api/ui_settings/", None),
    ("profile", "GET", "/api/profile/", None),
    ("tags.listAll", "GET", "/api/tags/?page=1&page_size=100000", None),
    ("correspondents.listAll", "GET", "/api/correspondents/?page=1&page_size=100000", None),
    ("document_types.listAll", "GET", "/api/document_types/?page=1&page_size=100000", None),
    ("storage_paths.listAll", "GET", "/api/storage_paths/?page=1&page_size=100000", None),
    ("custom_fields.listAll", "GET", "/api/custom_fields/?page=1&page_size=100000", None),
    ("saved_views.listAll", "GET", "/api/saved_views/?page=1&page_size=100000", None),
    ("users.listAll", "GET", "/api/users/?page=1&page_size=100000", None),
    ("groups.listAll", "GET", "/api/groups/?page=1&page_size=100000", None),
    ("tasks.status_counts.unack", "GET", "/api/tasks/status_counts/?acknowledged=false", None),
    # ---- Dashboard
    ("statistics", "GET", "/api/statistics/", None),
    ("dashboard.widget.recent", "GET", "/api/documents/?page=1&page_size=10&ordering=-created&truncate_content=true", None),
    ("dashboard.widget.inbox", "GET", "/api/documents/?page=1&page_size=10&ordering=-added&truncate_content=true&is_in_inbox=1", None),
    # ---- Document list (default + sortings + pages)
    ("doclist.default", "GET", DL.format(page=1, ord="-created"), None),
    ("doclist.default+sel", "GET", DL.format(page=1, ord="-created") + SEL, None),
    ("doclist.page200", "GET", DL.format(page=200, ord="-created"), None),
    ("doclist.title", "GET", DL.format(page=1, ord="title"), None),
    ("doclist.-added", "GET", DL.format(page=1, ord="-added"), None),
    ("doclist.correspondent", "GET", DL.format(page=1, ord="correspondent__name"), None),
    ("doclist.-num_notes", "GET", DL.format(page=1, ord="-num_notes"), None),
    ("doclist.asn", "GET", DL.format(page=1, ord="archive_serial_number"), None),
    ("doclist.owner", "GET", DL.format(page=1, ord="owner"), None),
    ("doclist.page_count", "GET", DL.format(page=1, ord="-page_count"), None),
    ("doclist.cf_sort", "GET", DL.format(page=1, ord="-custom_field_7"), None),
    # ---- Document list with filters
    ("doclist.f.tag_all", "GET", DL.format(page=1, ord="-created") + "&tags__id__all=6", None),
    ("doclist.f.tag_all+sel", "GET", DL.format(page=1, ord="-created") + "&tags__id__all=6" + SEL, None),
    ("doclist.f.tag_in", "GET", DL.format(page=1, ord="-created") + "&tags__id__in=7,8,9", None),
    ("doclist.f.tag_none", "GET", DL.format(page=1, ord="-created") + "&tags__id__none=18", None),
    ("doclist.f.untagged", "GET", DL.format(page=1, ord="-created") + "&is_tagged=0", None),
    ("doclist.f.corr_in", "GET", DL.format(page=1, ord="-created") + "&correspondent__id__in=1,2", None),
    ("doclist.f.doctype", "GET", DL.format(page=1, ord="-created") + "&document_type__id=1", None),
    ("doclist.f.title_content", "GET", DL.format(page=1, ord="-created") + "&title_content=rechnung", None),
    ("doclist.f.created_range", "GET", DL.format(page=1, ord="-created") + "&created__date__gte=2024-01-01&created__date__lte=2024-12-31", None),
    ("doclist.f.cf_query", "GET", DL.format(page=1, ord="-created") + "&custom_field_query=%5B%227%22%2C%22gt%22%2C100%5D", None),
    ("doclist.f.owner_none", "GET", DL.format(page=1, ord="-created") + "&owner__id__none=2", None),
    ("doclist.f.shared_by", "GET", DL.format(page=1, ord="-created") + "&shared_by__id=2", None),
    ("doclist.f.asn_isnull", "GET", DL.format(page=1, ord="-created") + "&archive_serial_number__isnull=1", None),
    ("doclist.f.mime", "GET", DL.format(page=1, ord="-created") + "&mime_type=application%2Fpdf", None),
    ("doclist.f.has_duplicates", "GET", DL.format(page=1, ord="-created") + "&has_duplicates=true", None),
    ("doclist.f.saved_view3", "GET", DL.format(page=1, ord="-created") + "&storage_path__id=1&title_content=rent", None),
    ("doclist.select_all_ids", "GET", "/api/documents/?page=1&page_size=100000&fields=id&tags__id__all=6", None),
    ("selection_data", "POST", "/api/documents/selection_data/", "SELECTION"),
    # ---- Document detail
    ("doc.retrieve", "GET", "/api/documents/{DOC}/?full_perms=true", None),
    ("doc.notes", "GET", "/api/documents/{DOC}/notes/", None),
    ("doc.history", "GET", "/api/documents/{DOC}/history/", None),
    ("doc.next_asn", "GET", "/api/documents/next_asn/", None),
    # ---- Administration (object lists with full_perms, sorting by name)
    ("tags.manage", "GET", "/api/tags/?page=1&page_size=25&ordering=name&full_perms=true", None),
    ("tags.manage.filter", "GET", "/api/tags/?page=1&page_size=25&ordering=name&full_perms=true&name__icontains=tag", None),
    ("correspondents.manage", "GET", "/api/correspondents/?page=1&page_size=25&ordering=name&full_perms=true", None),
    ("document_types.manage", "GET", "/api/document_types/?page=1&page_size=25&ordering=name&full_perms=true", None),
    ("custom_fields.manage", "GET", "/api/custom_fields/?page=1&page_size=25&ordering=name&full_perms=true", None),
    ("share_links", "GET", "/api/share_links/?page=1&page_size=25", None),
    ("trash", "GET", "/api/trash/?page=1&page_size=25", None),
    # ---- Tasks
    ("tasks.status_counts", "GET", "/api/tasks/status_counts/?acknowledged=false", None),
    ("tasks.attention", "GET", "/api/tasks/?page=1&page_size=10&acknowledged=false&status=failure&status=revoked", None),
    ("tasks.progress", "GET", "/api/tasks/?page=1&page_size=10&acknowledged=false&status=pending&status=started", None),
    ("tasks.completed", "GET", "/api/tasks/?page=1&page_size=25&acknowledged=false&status=success", None),
    # ---- Search
    ("search.autocomplete", "GET", "/api/search/autocomplete/?term=rech", None),
    ("search.global.db", "GET", "/api/search/?query=invoice&db_only=true", None),
    ("doclist.fulltext", "GET", "/api/documents/?page=1&page_size=50&query=rechnung&truncate_content=true", None),
]

USERS = {
    "admin": "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001",
    "alice": "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002",
    "bob": "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003",
}
