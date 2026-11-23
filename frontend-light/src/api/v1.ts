// Transitional client for the v1 API (paperless-ngx compatible), read-only in F0.
// Same requests as the Angular UI (parameters/order) so server caches and parity work.
// Login: browser session (cookie), CSRF from cookie `csrftoken` → header `X-CSRFToken`.

export interface Paged<T> {
  count: number
  next: string | null
  previous: string | null
  results: T[]
  all?: number[]
  selection_data?: unknown
}

export interface UiSettingsResponse {
  user: { id: number; username: string; first_name?: string; last_name?: string; is_staff?: boolean; is_superuser?: boolean }
  settings: Record<string, any>
  permissions: string[]
}

export interface Document {
  id: number
  title: string
  correspondent: number | null
  document_type: number | null
  storage_path: number | null
  tags: number[]
  created: string
  added: string
  archive_serial_number: number | null
  owner: number | null
  is_shared_by_requester?: boolean
  notes: unknown[]
  page_count: number | null
  custom_fields?: { field: number; value: unknown }[]
  content?: string
  modified?: string
  mime_type?: string
  original_file_name?: string
  archived_file_name?: string | null
  user_can_change?: boolean
  permissions?: ObjectPermissions
  versions?: DocumentVersionInfo[]
  duplicate_documents?: { id: number; title: string; deleted_at?: string | null }[]
  /** PaperRust folder view: folder id, null = "Unsorted" (only sent once the instance has folders) */
  folder?: number | null
  /** Additional Angular fields of the OpenDocumentsService (sessionStorage) */
  __changedFields?: string[]
  __selectedVersionId?: number
}

export interface ObjectPermissions {
  view: { users: number[]; groups: number[] }
  change: { users: number[]; groups: number[] }
}

export interface DocumentVersionInfo {
  id: number
  added?: string
  version_label?: string | null
  checksum?: string
  is_root?: boolean
  /** PaperRust: version is the result of a redaction (list paperrust_redaction) */
  redacted?: boolean
}

/** Note text format (upstream 3.5.1, PR #14626). Older servers (ngx 3.4.2) send none: treat as plain. */
export type NoteFormat = 'plain' | 'markdown'

export interface DocumentNote {
  id: number
  note: string
  format?: NoteFormat
  created: string
  user?: number | { id: number; username: string; first_name?: string; last_name?: string }
}

export interface MetadataEntry {
  namespace: string
  prefix: string
  key: string
  value: string
}

export interface DocumentMetadata {
  original_checksum?: string
  archive_checksum?: string
  original_size?: number
  archive_size?: number
  original_mime_type?: string
  media_filename?: string
  original_filename?: string
  has_archive_version?: boolean
  original_metadata?: MetadataEntry[]
  archive_metadata?: MetadataEntry[]
  archive_media_filename?: string
  lang?: string
  barcodes?: DocumentBarcode[]
}

/** data/document-barcode.ts */
export interface DocumentBarcode {
  page: number
  format: string
  value: string
}

export interface DocumentSuggestions {
  title?: string
  tags?: number[]
  suggested_tags?: string[]
  correspondents?: number[]
  suggested_correspondents?: string[]
  document_types?: number[]
  suggested_document_types?: string[]
  storage_paths?: number[]
  suggested_storage_paths?: string[]
  dates?: string[]
}

export interface AuditLogEntry {
  id: number
  timestamp: string
  action: string
  changes: Record<string, any>
  actor?: { id: number; username: string } | null
}

export interface Group {
  id: number
  name: string
}

export interface MatchingObject {
  id: number
  name: string
  document_count?: number
}
export interface Tag extends MatchingObject {
  color: string
  text_color: string
  is_inbox_tag?: boolean
  parent?: number | null
}
export interface User {
  id: number
  username: string
  first_name?: string
  last_name?: string
  is_superuser?: boolean
  groups?: number[]
}
export interface SavedView {
  id: number
  name: string
  icon?: string | null
  sort_field: string | null
  sort_reverse: boolean
  filter_rules: { rule_type: number; value: string | null }[]
}
export interface CustomField {
  id: number
  name: string
  data_type: string
  extra_data?: { default_currency?: string; select_options?: { id: string; label: string }[] } & Record<string, any>
  document_count?: number
}

export interface ShareLink {
  id: number
  created: string
  expiration?: string | null
  slug: string
  document: number
  document_title?: string
  file_version: string
}

/** DocumentSelectionQuery (document.service.ts) */
export interface DocumentSelectionQuery {
  documents?: number[]
  all?: boolean
  filters?: Record<string, unknown>
  excluded_documents?: number[]
}

/** ShareLinkBundleSummary (data/share-link-bundle.ts) */
export interface ShareLinkBundle {
  id: number
  slug: string
  created: string
  expiration?: string | null
  documents: number[]
  document_count: number
  file_version: 'archive' | 'original'
  status: 'pending' | 'processing' | 'ready' | 'failed'
  built_at?: string | null
  size_bytes?: number | null
  last_error?: { bundle_id?: number; message?: string; exception_type?: string; timestamp?: string } | null
}

/** AbstractPaperlessService.getOrderingQueryParam */
function ordering(sortField: string | null, sortReverse: boolean): string | undefined {
  return sortField ? (sortReverse ? '-' : '') + sortField : undefined
}

/** BulkEditSourceMode (services/rest/document.service.ts) */
export type BulkEditSourceMode = 'latest_version' | 'explicit_selection'

/** EditPdfDocumentsRequest (services/rest/document.service.ts) */
export interface EditPdfDocumentsRequest {
  operations: { page: number; rotate?: number; doc?: number }[]
  delete_original?: boolean
  update_document?: boolean
  include_metadata?: boolean
  source_mode?: BulkEditSourceMode
}

/** RemovePasswordDocumentsRequest (services/rest/document.service.ts) */
export interface RemovePasswordDocumentsRequest {
  password: string
  update_document?: boolean
  delete_original?: boolean
  include_metadata?: boolean
  source_mode?: BulkEditSourceMode
}

/** `WorkflowTrigger.type` "Manual" (same value as paperless-ngx dev). */
export const MANUAL_TRIGGER_TYPE = 5

/** Workflow selection for the manual run (only the fields needed). */
export interface ManualWorkflow {
  id: number
  name: string
}

export class ApiError extends Error {
  constructor(
    public status: number,
    public url: string,
    public body: unknown,
  ) {
    super(`HTTP ${status} ${url}`)
  }
}

export type Fetch = (input: string, init?: RequestInit) => Promise<Response>

export function getCookie(name: string, cookie: string = typeof document !== 'undefined' ? document.cookie : ''): string | null {
  for (const part of cookie.split(';')) {
    const [k, ...v] = part.trim().split('=')
    if (k === name) return decodeURIComponent(v.join('='))
  }
  return null
}

/** Query string in fixed order (like Angular HttpParams), omit empty values. */
export function queryString(params: Record<string, string | number | boolean | null | undefined>): string {
  const parts: string[] = []
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') continue
    parts.push(`${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
  }
  return parts.length ? `?${parts.join('&')}` : ''
}

export interface ClientOptions {
  base?: string // Origin + prefix, default '' (same origin)
  fetch?: Fetch
  cookie?: () => string
  onUnauthorized?: (status: number) => void
}

export class V1Client {
  private base: string
  private f: Fetch
  private cookie: () => string
  private onUnauthorized?: (status: number) => void

  constructor(opts: ClientOptions = {}) {
    this.base = (opts.base ?? '').replace(/\/$/, '')
    this.f = opts.fetch ?? ((i, init) => fetch(i, init))
    this.cookie = opts.cookie ?? (() => (typeof document !== 'undefined' ? document.cookie : ''))
    this.onUnauthorized = opts.onUnauthorized
  }

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { Accept: 'application/json; version=10' }
    if (body !== undefined) headers['Content-Type'] = 'application/json'
    if (!/^(GET|HEAD|OPTIONS)$/.test(method)) {
      const csrf = getCookie('csrftoken', this.cookie())
      if (csrf) headers['X-CSRFToken'] = csrf
    }
    const url = this.base + path
    const res = await this.f(url, {
      method,
      headers,
      credentials: 'same-origin',
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    if (res.status === 401 || res.status === 403) this.onUnauthorized?.(res.status)
    if (!res.ok) {
      let detail: unknown = null
      try {
        detail = await res.json()
      } catch {}
      throw new ApiError(res.status, url, detail)
    }
    if (res.status === 204) return undefined as T
    return (await res.json()) as T
  }

  get<T>(path: string) {
    return this.request<T>('GET', path)
  }

  uiSettings() {
    return this.get<UiSettingsResponse>('/api/ui_settings/')
  }

  /** Like AbstractPaperlessService.listAll: one page with page_size=100000. */
  listAll<T>(endpoint: string) {
    return this.get<Paged<T>>(`/api/${endpoint}/${queryString({ page: 1, page_size: 100000 })}`)
  }

  /** Like DocumentService.listFiltered (document list). */
  listDocuments(o: { page: number; pageSize: number; sortField: string; sortReverse: boolean; extra?: Record<string, string | number | boolean | null | undefined> }) {
    return this.get<Paged<Document>>(
      `/api/documents/${queryString({
        page: o.page,
        page_size: o.pageSize,
        ordering: (o.sortReverse ? '-' : '') + o.sortField,
        truncate_content: true,
        include_selection_data: true,
        ...(o.extra ?? {}),
      })}`,
    )
  }

  taskStatusCounts() {
    return this.get<{ all: number; needs_attention: number; in_progress: number; completed: number }>(
      `/api/tasks/status_counts/${queryString({ acknowledged: false })}`,
    )
  }

  private origin() {
    return this.base || (typeof location !== 'undefined' ? location.origin : '')
  }

  thumbUrl(id: number) {
    return `${this.origin()}/api/documents/${id}/thumb/`
  }

  downloadUrl(id: number, original = false) {
    return `${this.origin()}/api/documents/${id}/download/${original ? '?original=true' : ''}`
  }

  previewUrl(id: number, original = false, versionID: number | null = null) {
    const url = new URL(`${this.origin()}/api/documents/${id}/preview/`)
    if (original) url.searchParams.append('original', 'true')
    if (versionID) url.searchParams.append('version', String(versionID))
    return url.toString()
  }

  // ---- Document view (DocumentService/DocumentNotesService like Angular) ----------------
  /** DocumentService.get: always full_perms=true, optional version/fields */
  getDocument(id: number, versionID: number | null = null, fields: string | null = null) {
    return this.get<Document>(`/api/documents/${id}/${queryString({ full_perms: true, version: versionID ?? undefined, fields: fields ?? undefined })}`)
  }
  getRootId(id: number) {
    return this.get<{ root_id: number }>(`/api/documents/${id}/root/`)
  }
  getMetadata(id: number, versionID: number | null = null) {
    return this.get<DocumentMetadata>(`/api/documents/${id}/metadata/${queryString({ version: versionID ?? undefined })}`)
  }
  getSuggestions(id: number) {
    return this.get<DocumentSuggestions>(`/api/documents/${id}/suggestions/`)
  }
  getAiSuggestions(id: number) {
    return this.get<DocumentSuggestions>(`/api/documents/${id}/ai_suggestions/`)
  }
  getHistory(id: number) {
    return this.get<AuditLogEntry[]>(`/api/documents/${id}/history/`)
  }
  getNextAsn() {
    return this.get<number>(`/api/documents/next_asn/`)
  }
  /** DocumentService.patch (changed fields + remove_inbox_tags) */
  patchDocument(o: Record<string, unknown> & { id: number }, versionID: number | null = null) {
    return this.request<Document>('PATCH', `/api/documents/${o.id}/${queryString({ version: versionID ?? undefined })}`, o)
  }
  deleteDocument(id: number) {
    return this.request<void>('DELETE', `/api/documents/${id}/`)
  }
  addNote(id: number, note: string, format: NoteFormat = 'plain') {
    // older servers ignore `format` and store the note as plain text
    return this.request<DocumentNote[]>('POST', `/api/documents/${id}/notes/`, { note, format })
  }
  deleteNote(id: number, noteId: number) {
    return this.request<DocumentNote[]>('DELETE', `/api/documents/${id}/notes/?id=${noteId}`)
  }
  thumbUrlVersion(id: number, versionID: number | null = null) {
    const url = new URL(this.thumbUrl(id))
    if (versionID) url.searchParams.append('version', String(versionID))
    return url.toString()
  }
  downloadUrlFor(id: number, original = false, versionID: number | null = null, followFormatting = false) {
    const url = new URL(`${this.origin()}/api/documents/${id}/download/`)
    if (original) url.searchParams.append('original', 'true')
    if (versionID) url.searchParams.append('version', String(versionID))
    if (followFormatting) url.searchParams.append('follow_formatting', 'true')
    return url.toString()
  }

  // ---- Document view: reprocess, email, share links (P4 S5b) -------------------
  /** DocumentService.reprocessDocuments; PaperRust: aiOcr → ai_ocr (AI text recognition, only if selected) */
  reprocessDocuments(documents: number[], remoteOcr = false, aiOcr = false) {
    return this.reprocessSelection({ documents }, remoteOcr, aiOcr)
  }
  // ---- Manual workflow run (PaperRust; idea from paperless-ngx dev #14704) ------------------
  /** Active workflows with trigger "Manual" (`trigger_type=5`, `enabled=true`). */
  listManualWorkflows() {
    return this.get<Paged<ManualWorkflow>>(`/api/workflows/${queryString({ page: 1, page_size: 100000, trigger_type: MANUAL_TRIGGER_TYPE, enabled: true })}`)
  }
  /** bulk_edit `run_workflow`; `filterDocuments = false` also applies the workflow to non-matching documents. */
  runWorkflow(documents: number[], workflow: number, filterDocuments = true) {
    return this.request<unknown>('POST', '/api/documents/bulk_edit/', { documents, method: 'run_workflow', parameters: { workflow, filter_documents: filterDocuments } })
  }
  /** DocumentService.emailDocuments */
  emailDocuments(documents: number[], addresses: string, subject: string, message: string, useArchiveVersion: boolean) {
    return this.request<unknown>('POST', '/api/documents/email/', { documents, addresses, subject, message, use_archive_version: useArchiveVersion })
  }
  /** ShareLinkService.getLinksForDocument */
  getShareLinks(documentId: number) {
    return this.get<ShareLink[]>(`/api/documents/${documentId}/share_links/`)
  }
  /** ShareLinkService.createLinkForDocument */
  createShareLink(document: number, file_version: 'archive' | 'original' = 'archive', expiration: Date | null = null) {
    return this.request<ShareLink>('POST', '/api/share_links/', { document, file_version, expiration: expiration?.toISOString() })
  }
  deleteShareLink(id: number) {
    return this.request<void>('DELETE', `/api/share_links/${id}/`)
  }

  // ---- Bulk edit (DocumentService, bulk-editor.component.ts) ---------------------------
  /** DocumentService.bulkEdit: selection ({documents} or {all, filters, excluded_documents}) + method + parameters */
  bulkEdit(selection: DocumentSelectionQuery, method: string, parameters: Record<string, unknown>) {
    return this.request<unknown>('POST', '/api/documents/bulk_edit/', { ...selection, method, parameters })
  }
  /** DocumentService.getSelectionData */
  selectionData<T = unknown>(selection: DocumentSelectionQuery) {
    return this.request<T>('POST', '/api/documents/selection_data/', selection)
  }
  /** DocumentService.deleteDocuments (to the trash) */
  deleteDocuments(selection: DocumentSelectionQuery) {
    return this.request<unknown>('POST', '/api/documents/delete/', { ...selection })
  }
  /** DocumentService.reprocessDocuments with selection query */
  reprocessSelection(selection: DocumentSelectionQuery, remoteOcr = false, aiOcr = false) {
    return this.request<unknown>('POST', '/api/documents/reprocess/', { ...selection, remote_ocr: remoteOcr, ...(aiOcr && { ai_ocr: true }) })
  }
  /** DocumentService.rotateDocuments (source_mode default latest_version as upstream) */
  rotateDocuments(selection: DocumentSelectionQuery, degrees: number, sourceMode = 'latest_version') {
    return this.request<unknown>('POST', '/api/documents/rotate/', { ...selection, degrees, source_mode: sourceMode })
  }
  /** DocumentService.mergeDocuments */
  mergeDocuments(ids: number[], request: { metadata_document_id?: number; delete_originals?: boolean; archive_fallback?: boolean } = {}) {
    return this.request<unknown>('POST', '/api/documents/merge/', { documents: ids, ...request })
  }
  /** AbstractPaperlessService.getFew (id__in, ordering -id, page_size 1000) */
  getFewDocuments(ids: number[]) {
    return this.get<Paged<Document>>(`/api/documents/${queryString({ id__in: ids.join(','), ordering: '-id', page_size: 1000 })}`)
  }

  // ---- Share links and bundles (ShareLinkService, ShareLinkBundleService) -------------------
  /** AbstractPaperlessService.list for share_links */
  listShareLinks(page: number, pageSize: number, sortField: string | null, sortReverse: boolean) {
    return this.get<Paged<ShareLink>>(`/api/share_links/${queryString({ page, page_size: pageSize, ordering: ordering(sortField, sortReverse) })}`)
  }
  /** AbstractPaperlessService.list for share_link_bundles */
  listShareLinkBundles(page: number, pageSize: number, sortField: string | null, sortReverse: boolean) {
    return this.get<Paged<ShareLinkBundle>>(`/api/share_link_bundles/${queryString({ page, page_size: pageSize, ordering: ordering(sortField, sortReverse) })}`)
  }
  /** ShareLinkBundleService.createBundle */
  createShareLinkBundle(payload: { document_ids: number[]; file_version: 'archive' | 'original'; expiration_days: number | null }) {
    return this.request<ShareLinkBundle>('POST', '/api/share_link_bundles/', payload)
  }
  /** ShareLinkBundleService.rebuildBundle */
  rebuildShareLinkBundle(id: number) {
    return this.request<ShareLinkBundle>('POST', `/api/share_link_bundles/${id}/rebuild/`, {})
  }
  deleteShareLinkBundle(id: number) {
    return this.request<void>('DELETE', `/api/share_link_bundles/${id}/`)
  }

  // ---- Document view: PDF editor, remove password, versions (DocumentService) ----------
  /** DocumentService.editPdfDocuments → POST /api/documents/edit_pdf/ */
  editPdfDocuments(documents: number[], request: EditPdfDocumentsRequest) {
    return this.request<unknown>('POST', '/api/documents/edit_pdf/', { documents, ...request })
  }
  /** DocumentService.removePasswordDocuments → POST /api/documents/remove_password/ */
  removePasswordDocuments(documents: number[], request: RemovePasswordDocumentsRequest) {
    return this.request<unknown>('POST', '/api/documents/remove_password/', { documents, ...request })
  }
  /** DocumentService.getVersions: only id + versions */
  getVersions(id: number) {
    return this.get<Pick<Document, 'id' | 'versions'>>(`/api/documents/${id}/${queryString({ fields: 'id,versions' })}`)
  }
  /** DocumentService.deleteVersion → DELETE /api/documents/{root}/versions/{version}/ */
  deleteVersion(rootDocumentId: number, versionId: number) {
    return this.request<{ result: string; current_version_id: number }>('DELETE', `/api/documents/${rootDocumentId}/versions/${versionId}/`)
  }
  /** DocumentService.updateVersionLabel → PATCH /api/documents/{root}/versions/{version}/ */
  updateVersionLabel(rootDocumentId: number, versionId: number, versionLabel: string | null) {
    return this.request<DocumentVersionInfo>('PATCH', `/api/documents/${rootDocumentId}/versions/${versionId}/`, { version_label: versionLabel })
  }
  /** DocumentService.mergeDocumentsAsVersions → POST /api/documents/merge_as_versions/ */
  mergeDocumentsAsVersions(documents: number[], rootDocumentId: number, versionLabel?: string) {
    return this.request<unknown>('POST', '/api/documents/merge_as_versions/', {
      documents,
      root_document_id: rootDocumentId,
      ...(versionLabel ? { version_label: versionLabel } : {}),
    })
  }
  /** DocumentService.uploadVersion → POST multipart /api/documents/{id}/update_version/ (response: task id) */
  async uploadVersion(id: number, file: File, versionLabel?: string): Promise<unknown> {
    const formData = new FormData()
    formData.append('document', file, file.name)
    if (versionLabel) formData.append('version_label', versionLabel)
    const headers: Record<string, string> = { Accept: 'application/json; version=10' }
    const csrf = getCookie('csrftoken', this.cookie())
    if (csrf) headers['X-CSRFToken'] = csrf
    const url = this.base + `/api/documents/${id}/update_version/`
    const res = await this.f(url, { method: 'POST', headers, credentials: 'same-origin', body: formData })
    if (res.status === 401 || res.status === 403) this.onUnauthorized?.(res.status)
    let body: unknown = null
    try {
      body = await res.json()
    } catch {}
    if (!res.ok) throw new ApiError(res.status, url, body)
    return body
  }

  // ---- Saved views (SavedViewService) and global search (SearchService) ----------
  /** SavedViewService.get → GET /api/saved_views/<id>/ */
  getSavedView<T = SavedView>(id: number) {
    return this.get<T>(`/api/saved_views/${id}/`)
  }
  /** SavedViewService.create → POST /api/saved_views/ */
  createSavedView<T = SavedView>(view: Record<string, unknown>) {
    return this.request<T>('POST', '/api/saved_views/', view)
  }
  /** SavedViewService.patch → PATCH /api/saved_views/<id>/ */
  patchSavedView<T = SavedView>(view: Record<string, unknown> & { id: number }) {
    return this.request<T>('PATCH', `/api/saved_views/${view.id}/`, view)
  }
  /** SearchService.globalSearch → GET /api/search/?query=…[&db_only=true] */
  globalSearch<T = unknown>(query: string, dbOnly = false) {
    return this.get<T>(`/api/search/${queryString({ query, db_only: dbOnly ? true : undefined })}`)
  }

  // ---- Upload (UploadDocumentsService) ------------------------------------------------
  /** DocumentService.uploadDocument: POST multipart with upload progress (HttpClient reportProgress → XHR) */
  uploadDocument(
    formData: FormData,
    cb: { progress: (loaded: number, total: number) => void; done: (body: unknown) => void; error: (status: number, statusText: string, body: unknown) => void },
  ) {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', this.base + '/api/documents/post_document/')
    xhr.setRequestHeader('Accept', 'application/json; version=10')
    const csrf = getCookie('csrftoken', this.cookie())
    if (csrf) xhr.setRequestHeader('X-CSRFToken', csrf)
    xhr.withCredentials = true
    xhr.upload.onprogress = (e) => cb.progress(e.loaded, e.total)
    xhr.onload = () => {
      let body: unknown = xhr.responseText
      try {
        body = JSON.parse(xhr.responseText)
      } catch {}
      if (xhr.status >= 200 && xhr.status < 300) cb.done(body)
      else {
        if (xhr.status === 401 || xhr.status === 403) this.onUnauthorized?.(xhr.status)
        cb.error(xhr.status, xhr.statusText, body)
      }
    }
    xhr.onerror = () => cb.error(0, xhr.statusText || 'Unknown Error', null)
    xhr.send(formData)
    return xhr
  }
}
