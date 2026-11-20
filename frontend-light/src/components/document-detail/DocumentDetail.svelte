<!-- Port of document-detail/document-detail.component.html (S5 document view). Host <pngx-document-detail>
     in App.svelte. Everything in the Light UI (no links to the classic UI): PDF editor (common/pdf-editor),
     remove password (password-removal-confirm-dialog), versions (document-version-dropdown), barcodes
     (document-barcodes). Dialogs (create, share links, email, reprocess, PDF editor, remove password)
     and UTIF are only loaded on demand. -->
<script lang="ts">
  import '../../styles/components/document-detail.css'
  import { onDestroy, onMount, untrack } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { formatDate } from '../../lib/dates'
  import { Dropdown } from '../../lib/dropdown.svelte'
  import { modals, type ModalRef } from '../../lib/modal.svelte'
  import { withSearchHash } from '../../stores/searchQuery'
  import { toastService } from '../../stores/toasts.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { restoreDocuments } from '../admin/trash/trash'
  import type { Component } from 'svelte'
  import { api } from '../../api/client'
  import { ApiError, type Document, type DocumentMetadata, type DocumentNote, type DocumentSuggestions, type DocumentVersionInfo, type CustomField, type ManualWorkflow } from '../../api/v1'
  import { loadManualWorkflows } from '../../lib/manual-workflows'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { list } from '../../stores/listView.svelte'
  import { openDocuments } from '../../stores/openDocuments.svelte'
  import { router } from '../../router.svelte'
  import { FILTER_FULLTEXT_MORELIKE } from '../../lib/filter-rule-type'
  import type { FilterRule } from '../../lib/filter-rules'
  import type { DateStruct } from '../../lib/date-input'
  import PageHeader from '../common/page-header/PageHeader.svelte'
  import ConfirmDialog from '../common/confirm-dialog/ConfirmDialog.svelte'
  import InputText from '../common/input/text/InputText.svelte'
  import InputNumber from '../common/input/number/InputNumber.svelte'
  import InputDate from '../common/input/date/InputDate.svelte'
  import InputSelect from '../common/input/select/InputSelect.svelte'
  import InputTags from '../common/input/tags/InputTags.svelte'
  import InputCheck from '../common/input/check/InputCheck.svelte'
  import InputUrl from '../common/input/url/InputUrl.svelte'
  import InputTextarea from '../common/input/textarea/InputTextarea.svelte'
  import InputMonetary from '../common/input/monetary/InputMonetary.svelte'
  import InputDocumentLink from '../common/input/document-link/InputDocumentLink.svelte'
  import PermissionsForm from '../common/input/permissions/permissions-form/PermissionsForm.svelte'
  import SuggestionsDropdown from '../common/suggestions-dropdown/SuggestionsDropdown.svelte'
  import CustomFieldsDropdown from '../common/custom-fields-dropdown/CustomFieldsDropdown.svelte'
  import DocumentVersionDropdown from './document-version-dropdown/DocumentVersionDropdown.svelte'
  import MetadataCollapse from './metadata-collapse/MetadataCollapse.svelte'
  import DocumentBarcodes from './document-barcodes/DocumentBarcodes.svelte'
  import DocumentHistory from './document-history/DocumentHistory.svelte'
  import DocumentNotes from '../document-notes/DocumentNotes.svelte'
  import PdfViewer from '../common/pdf-viewer/PdfViewer.svelte'
  import { PdfRenderMode } from '../common/pdf-viewer/pdfjs'
  import { isConvertible, isImage } from '../common/convert-pdf/convert-pdf-api'
  import { mergeSuggestions, type SuggestionSource } from '../common/suggestions-dropdown/suggestions'
  import {
    NAV_IDS,
    type NavKey,
    getRenderType,
    formFromDocument,
    deepEqual,
    changedFields,
    filterRulesFor,
    setZoom as zoomFrom,
    increaseZoom as zoomIn,
    decreaseZoom as zoomOut,
    currentZoom as zoomCurrent,
    zoomSettings,
    fileSize,
    type DocumentFormValue,
  } from './document-detail'

  // ---- Route ---------------------------------------------------------------------------
  const params = $derived.by(() => {
    const m = /^documents\/(\d+)(?:\/([a-z]+))?$/.exec(router.path)
    return { id: m ? +m[1] : NaN, section: m?.[2] }
  })

  // ---- State -------------------------------------------------------------------------
  let documentId = $state<number | undefined>(undefined)
  let doc = $state<Document | undefined>(undefined)
  let metadata = $state<DocumentMetadata | undefined>(undefined)
  let suggestions = $state<DocumentSuggestions | null>(null)
  let suggestionsLoading = $state(false)
  let suggestionSourceOverride = $state<SuggestionSource | null>(null)
  let fetchedSuggestionSources = $state<SuggestionSource[]>([])
  let error = $state<any>(undefined)
  let networkActive = $state(false)
  let title = $state<string | undefined>(undefined)
  let previewUrl = $state<string | undefined>(undefined)
  let pdfSource = $state<string | undefined>(undefined)
  let previewRevision = $state(0)
  let pdfPassword = $state<string | undefined>(undefined)
  let thumbUrl = $state<string | undefined>(undefined)
  let previewText = $state<string | undefined>(undefined)
  let previewLoaded = $state(false)
  let tiffURL = $state<string | null>(null)
  let tiffError = $state<string | null>(null)
  let selectedVersionId = $state<number | undefined>(undefined)
  let previewCurrentPage = $state(1)
  let previewNumPages = $state<number | undefined>(undefined)
  let zoom = $state(zoomFrom(session.get(SETTINGS_KEYS.PDF_VIEWER_ZOOM_SETTING)))
  let requiresPassword = $state(false)
  let password = $state<string | undefined>(undefined)
  let downloading = $state(false)
  let useFormattedFilename = $state(false)
  let activeNav = $state<NavKey>('details')
  let form = $state<DocumentFormValue | undefined>(undefined)
  let store = $state<DocumentFormValue | undefined>(undefined)
  let dirtyFields = $state(new Set<string>())
  let formDisabled = $state(false)
  let wide = $state(typeof matchMedia !== 'undefined' ? matchMedia('(min-width: 768px)').matches : true)
  let titleTimer: ReturnType<typeof setTimeout> | undefined
  let loadSeq = 0

  // ---- Settings / permissions -----------------------------------------------------------
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const useNativePdfViewer = $derived(session.get(SETTINGS_KEYS.USE_NATIVE_PDF_VIEWER))
  const aiEnabled = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const showThumbnailOverlay = $derived(session.get(SETTINGS_KEYS.DOCUMENT_EDITING_OVERLAY_THUMBNAIL))
  const hiddenFields = $derived<string[]>(session.get(SETTINGS_KEYS.DOCUMENT_DETAILS_HIDDEN_FIELDS) ?? [])
  const isFieldHidden = (f: string) => hiddenFields.includes(f)
  const defaultSuggestionSource = $derived<SuggestionSource>(aiEnabled ? session.get(SETTINGS_KEYS.DOCUMENT_EDITING_SUGGESTION_SOURCE) : 'ml')
  const suggestionSource = $derived<SuggestionSource>(!aiEnabled ? 'ml' : (suggestionSourceOverride ?? defaultSuggestionSource))
  const isMobile = typeof navigator !== 'undefined' && /Mobi|Android|iPhone|iPad/i.test(navigator.userAgent)
  const emailEnabled = $derived(session.get(SETTINGS_KEYS.EMAIL_ENABLED))
  /** Owner as in the saved state (do not lock immediately when editing) */
  const docForPerms = $derived(doc ? { ...doc, owner: store?.permissions_form && 'owner' in store.permissions_form ? store.permissions_form.owner : doc.owner } : undefined)
  const userIsOwner = $derived(!doc || session.ownsObject(docForPerms))
  const userCanEditDoc = (d: any) => session.can('change', 'document') && session.hasObjectPermissions('change', d)
  const userCanEdit = $derived(!doc || userCanEditDoc(docForPerms))
  const showPermissions = $derived(session.can('view', 'user') && userIsOwner)
  const notesEnabled = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED) && session.can('view', 'note'))
  const historyEnabled = $derived(session.get(SETTINGS_KEYS.AUDITLOG_ENABLED) && userIsOwner && session.can('view', 'history'))
  const createDisabled = (type: string) => !session.can('add', type)
  const isDirty = $derived(!!form && !!store && !deepEqual(form, store))
  const saveDisabled = $derived(!userCanEdit || networkActive || !isDirty)
  const hasArchiveVersion = $derived(metadata?.has_archive_version ?? !!doc?.archived_file_name)
  const archiveContentRenderType = $derived(hasArchiveVersion ? getRenderType('application/pdf') : getRenderType(metadata?.original_mime_type || doc?.mime_type))
  const originalContentRenderType = $derived(getRenderType(metadata?.original_mime_type || doc?.mime_type))
  const isRTL = $derived(!!metadata?.lang && ['ar', 'he', 'fe'].includes(metadata.lang))
  const users = $derived([...data.users.values()])
  const tags = $derived([...data.tags.values()])
  const customFields = $derived<CustomField[]>(data.customFields)
  const getCustomField = (field: number) => customFields.find((f) => f.id === field)
  const getCustomFieldError = (i: number) => {
    const e = error?.custom_fields?.[i]
    return e?.['non_field_errors'] ?? e?.['value']
  }
  const docTitle = (v: string | undefined) => (v ? v : t('common.no_title'))

  // ---- Back/forward (DocumentListViewService) ----------------------------------------------
  const lastPage = () => Math.ceil((list.collectionSize ?? 0) / list.pageSize)
  const indexInList = () => (list.initialized ? list.documents.findIndex((d) => d.id == documentId) : -1)
  const hasNext = $derived.by(() => {
    if (!list.initialized) return false
    const i = indexInList()
    return i != -1 && (list.currentPage < lastPage() || i + 1 < list.documents.length)
  })
  const hasPrevious = $derived.by(() => {
    if (!list.initialized) return false
    const i = indexInList()
    return i != -1 && !(i == 0 && list.currentPage == 1)
  })
  async function getNext(): Promise<number | undefined> {
    const i = indexInList()
    if (i != -1 && i + 1 < list.documents.length) return list.documents[i + 1].id
    if (i != -1 && list.currentPage < lastPage()) {
      list.currentPage += 1
      await list.reload(false)
      return list.documents[0]?.id
    }
  }
  async function getPrevious(): Promise<number | undefined> {
    const i = indexInList()
    if (i > 0) return list.documents[i - 1].id
    if (list.currentPage > 1) {
      list.currentPage -= 1
      await list.reload(false)
      return list.documents[list.documents.length - 1]?.id
    }
  }
  async function nextDoc() {
    const id = await getNext()
    if (id) router.navigate(`documents/${id}`)
  }
  async function previousDoc() {
    const id = await getPrevious()
    if (id) router.navigate(`documents/${id}`)
  }

  // ---- Loading -------------------------------------------------------------------------------
  $effect(() => {
    const { id, section } = params
    if (isNaN(id)) return
    untrack(() => {
      if (!section) {
        router.navigate(`documents/${id}/details`, true)
        return
      }
      const key = (Object.keys(NAV_IDS) as NavKey[]).find((k) => k === section)
      if (key) activeNav = key
      if (id !== documentId) loadDocument(id)
    })
  })

  function updatePdfSource() {
    pdfSource = previewUrl
    pdfPassword = password
  }

  async function loadDocument(id: number, forceRemote = false) {
    const seq = ++loadSeq
    closeIncomingUpdateModal()
    pendingIncomingUpdate = null
    lastLocalSaveModified = null
    selectedVersionId = id
    previewUrl = api.previewUrl(id)
    updatePdfSource()
    thumbUrl = api.thumbUrlVersion(id)
    let fetched: Document | null = null
    try {
      fetched = await api.getDocument(id)
    } catch (e) {
      if (seq !== loadSeq) return
      if (e instanceof ApiError && e.status === 404) {
        try {
          const r = await api.getRootId(id)
          if (r?.root_id && r.root_id !== id) {
            router.navigate(`documents/${r.root_id}/${params.section || 'details'}`, true)
            return
          }
        } catch {}
      }
      router.navigate('404', true)
      return
    }
    if (seq !== loadSeq) return
    const remote = fetched!
    documentId = remote.id
    suggestions = null
    suggestionSourceOverride = null
    fetchedSuggestionSources = []
    const open = openDocuments.getOpenDocument(remote.id)
    if (open && remote.duplicate_documents) {
      open.duplicate_documents = remote.duplicate_documents
      openDocuments.save()
    }
    if (open) {
      open.versions = remote.versions
      if (!open.__changedFields?.includes('content')) open.content = remote.content
    }
    let useDoc: Document = open || remote
    const syncRemote = (o: Document) => {
      Object.assign(o, remote)
      o.__changedFields = []
      openDocuments.setDirty(o, false)
      openDocuments.save()
      useDoc = o
    }
    if (open && forceRemote) syncRemote(open)
    else if (open) {
      if (new Date(remote.modified!) > new Date(open.modified!)) {
        // keep local changes and ask, otherwise silently sync
        if (openDocuments.isDirty(open) || open.__changedFields?.length) showIncomingUpdateModal(remote.modified!)
        else syncRemote(open)
      }
    } else {
      await openDocuments.openDocument(remote)
    }
    updateComponent(useDoc, remote)
    if (forceRemote) previewRevision++
  }

  // ---- WebSocket document_updated (handleIncomingDocumentUpdated) ---------------------------
  let pendingIncomingUpdate: { document_id: number; modified?: string } | null = null
  let lastLocalSaveModified: string | null = null
  let incomingUpdateModal: ModalRef<string> | null = null

  function showIncomingUpdateModal(modified: string) {
    if (incomingUpdateModal) return
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('detail.document_updated'),
        messageBold: t('detail.document_updated_2', { PH: new Date(modified).toLocaleString() }),
        message: t('detail.reload_discard_local_unsaved'),
        btnClass: 'btn-warning',
        btnCaption: t('common.reload'),
        cancelBtnCaption: t('common.dismiss'),
        onconfirm: (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          ref.close()
          reloadRemoteVersion()
        },
      },
      { backdrop: 'static' },
    )
    incomingUpdateModal = ref
    ref.result.finally(() => {
      if (incomingUpdateModal === ref) incomingUpdateModal = null
    })
  }
  function closeIncomingUpdateModal() {
    if (!incomingUpdateModal) return
    incomingUpdateModal.close()
    incomingUpdateModal = null
  }
  function flushPendingIncomingUpdate() {
    if (!pendingIncomingUpdate || networkActive) return
    const p = pendingIncomingUpdate
    pendingIncomingUpdate = null
    handleIncomingDocumentUpdated(p)
  }
  function handleIncomingDocumentUpdated(data: { document_id: number; modified?: string }) {
    if (!documentId || !doc || data.document_id !== documentId) return
    if (networkActive) {
      pendingIncomingUpdate = data
      return
    }
    // same timestamp as our own last save → own change, no message
    if (data.modified && lastLocalSaveModified && data.modified === lastLocalSaveModified) {
      lastLocalSaveModified = null
      return
    }
    lastLocalSaveModified = null
    if (openDocuments.isDirty(doc)) showIncomingUpdateModal(data.modified!)
    else {
      loadDocument(documentId, true)
      toastService.showInfo(t('detail.document_reloaded_latest_changes'))
    }
  }
  function reloadRemoteVersion() {
    if (!documentId) return
    closeIncomingUpdateModal()
    loadDocument(documentId, true)
    toastService.showInfo(t('detail.document_reloaded'))
  }

  function updateComponent(d: Document, original: Document) {
    doc = d
    const versions = d.versions ?? []
    const selected = versions.find((v) => v.id === d.__selectedVersionId) ?? versions[0]
    selectedVersionId = selected?.id ?? d.id
    previewLoaded = false
    requiresPassword = false
    loadMetadata()
    if (
      session.get(SETTINGS_KEYS.DOCUMENT_EDITING_AUTO_SUGGEST) &&
      session.hasObjectPermissions('change', d) &&
      session.can('change', 'document') &&
      d.tags.some((id) => data.tags.get(id)?.is_inbox_tag)
    ) {
      getSuggestions()
    }
    title = docTitle(d.title)
    // prepareForm + setupDirtyTracking
    form = { ...formFromDocument(d), permissions_form: { owner: d.owner, set_permissions: d.permissions as any } }
    store = formFromDocument(original)
    store.permissions_form = { owner: original.owner, set_permissions: original.permissions as any }
    dirtyFields = new Set((d.__changedFields ?? []).map((f) => (f === 'owner' || f === 'set_permissions' ? 'permissions_form' : f)))
    formDisabled = !userCanEditDoc(d)
    if (activeNav === 'duplicates' && !d.duplicate_documents?.length) activeNav = 'details'
    if (archiveContentRenderType === 'text') loadPreviewText()
  }

  function loadPreviewText() {
    const url = previewUrl!
    fetch(url, { credentials: 'same-origin' })
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error(`Http failure response for ${url}: ${r.status} ${r.statusText}`))))
      .then(
        (txt) => (previewText = txt),
        (err) => (previewText = t('detail.error_occurred_loading_content', { PH: err.message ?? err.toString() })),
      )
  }

  async function tryRenderTiff() {
    const url = previewUrl!
    const id = documentId
    try {
      const r = await fetch(url, { credentials: 'same-origin' })
      if (!r.ok) throw new Error(`Http failure response for ${url}: ${r.status} ${r.statusText}`)
      const [buf, { tiffToDataUrl }] = await Promise.all([r.arrayBuffer(), import('./tiff')])
      if (id !== documentId || url !== previewUrl) return
      try {
        tiffURL = tiffToDataUrl(buf)
      } catch (err: any) {
        tiffError = t('detail.error_occurred_loading_tiff', { PH: String(err) })
      }
    } catch (err: any) {
      if (id === documentId) tiffError = t('detail.error_occurred_loading_tiff', { PH: String(err) })
    }
  }

  function nonLatestVersionId(): number | null {
    const versions = doc?.versions ?? []
    if (!versions.length || !selectedVersionId) return null
    return selectedVersionId === versions[0].id ? null : selectedVersionId
  }

  function loadMetadata() {
    const id = documentId!
    api.getMetadata(id, nonLatestVersionId()).then(
      (m) => {
        if (id !== documentId) return
        metadata = m
        tiffURL = null
        tiffError = null
        if (archiveContentRenderType === 'tiff') tryRenderTiff()
        if (archiveContentRenderType !== 'pdf' || useNativePdfViewer) previewLoaded = true
        if (archiveContentRenderType === 'text' && previewText === undefined) loadPreviewText()
      },
      (e) => {
        metadata = {} // allow display to fallback to <object> tag
        toastService.showError(t('detail.error_retrieving_metadata'), e)
      },
    )
  }

  function selectVersion(versionId: number) {
    if (!doc) return
    selectedVersionId = versionId
    doc.__selectedVersionId = versionId
    openDocuments.save()
    previewLoaded = false
    previewUrl = api.previewUrl(documentId!, false, versionId)
    updatePdfSource()
    thumbUrl = api.thumbUrlVersion(documentId!, versionId)
    loadMetadata()
    api.getDocument(documentId!, versionId, 'content').then(
      (d) => {
        const content = d?.content ?? ''
        if (store) store.content = content
        if (form) form.content = content
        dirtyFields.delete('content')
        dirtyFields = new Set(dirtyFields)
      },
      (e) => toastService.showError(t('detail.error_retrieving_version_content'), e),
    )
  }

  function onVersionSelected(versionId: number) {
    if (versionId === selectedVersionId) return
    const exists = doc?.versions?.some((v) => v.id === selectedVersionId)
    if (networkActive && exists) return
    if (!exists || form?.content === store?.content) {
      selectVersion(versionId)
      return
    }
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('common.unsaved_changes'),
        messageBold: t('detail.unsaved_changes_content_version'),
        message: t('detail.switching_versions_discard_them'),
        btnClass: 'btn-secondary',
        btnCaption: t('detail.discard_switch'),
        alternativeBtnClass: 'btn-primary',
        alternativeBtnCaption: t('detail.save_switch'),
      },
      { backdrop: 'static' },
    )
    ref.result.then((r) => {
      if (r === 'confirm') selectVersion(versionId)
      else if (r === 'alternative') save(false, () => selectVersion(versionId))
    })
  }

  // ---- Form ---------------------------------------------------------------------------
  function setField<K extends keyof DocumentFormValue>(key: K, value: DocumentFormValue[K]) {
    if (!form) return
    form[key] = value
    dirtyFields.add(key)
    dirtyFields = new Set(dirtyFields)
    formChanged()
  }
  function formChanged() {
    error = null
    if (!doc || !form) return
    // documentForm.valueChanges → Object.assign(document, mapFormToDoc(values))
    const { permissions_form, ...rest } = $state.snapshot(form)
    Object.assign(doc, rest, { owner: permissions_form?.owner, set_permissions: permissions_form?.set_permissions })
    openDocuments.setDirty(doc, !deepEqual($state.snapshot(form), $state.snapshot(store)), changedFields(doc.id, form, dirtyFields))
  }
  function titleKeyUp(e: KeyboardEvent) {
    const v = (e.target as HTMLInputElement).value
    clearTimeout(titleTimer)
    titleTimer = setTimeout(() => {
      if (v !== (e.target as HTMLInputElement).value) return
      title = v
      setField('title', v)
    }, 1000)
  }
  function setCustomField(i: number, value: unknown) {
    if (!form) return
    form.custom_fields[i] = { ...form.custom_fields[i], value }
    dirtyFields.add('custom_fields')
    dirtyFields = new Set(dirtyFields)
    formChanged()
  }
  function addField(field: CustomField) {
    if (!doc || !form) return
    doc.custom_fields = [...(doc.custom_fields ?? []), { field: field.id, value: null }]
    setField('custom_fields', [...form.custom_fields, { field: field.id, value: null }])
  }
  function removeField(i: number) {
    if (!doc || !form) return
    doc.custom_fields = (doc.custom_fields ?? []).filter((_, j) => j !== i)
    setField('custom_fields', form.custom_fields.filter((_, j) => j !== i))
  }

  // ---- Suggestions ------------------------------------------------------------------------------
  async function getSuggestions(source: SuggestionSource = suggestionSource) {
    const sources = (source === 'both' ? (['ml', 'ai'] as SuggestionSource[]) : [source]).filter((s) => !fetchedSuggestionSources.includes(s))
    if (!sources.length) return
    const id = documentId!
    suggestionsLoading = true
    await Promise.all(
      sources.map((s) =>
        (s === 'ai' ? api.getAiSuggestions(id) : api.getSuggestions(id)).then(
          (r) => {
            if (id !== documentId) return
            fetchedSuggestionSources = [...fetchedSuggestionSources, s]
            suggestions = mergeSuggestions(suggestions, r)
          },
          (e) => toastService.showError(t('detail.error_retrieving_suggestions'), e),
        ),
      ),
    )
    suggestionsLoading = false
  }

  // ---- Versions (onVersionsUpdated) -------------------------------------------------------------
  function onVersionsUpdated(versions: DocumentVersionInfo[]) {
    if (!doc) return
    doc.versions = versions
    const open = openDocuments.getOpenDocument(documentId!)
    if (open) {
      open.versions = versions
      openDocuments.save()
    }
  }

  // ---- Actions ---------------------------------------------------------------------------------
  function discard() {
    api.getDocument(documentId!, selectedVersionId).then(
      (d) => {
        Object.assign(doc!, d)
        title = d.title
        form = { ...formFromDocument(d), permissions_form: { owner: d.owner, set_permissions: d.permissions as any } }
        dirtyFields = new Set()
        openDocuments.setDirty(d, false)
      },
      () => router.navigate('404', true),
    )
  }

  function save(close = false, savedCallback: (() => void) | null = null) {
    if (!doc || !form) return
    networkActive = true
    ;(document.activeElement as HTMLElement)?.dispatchEvent(new Event('change'))
    const changes = changedFields(doc.id, $state.snapshot(form) as DocumentFormValue, dirtyFields)
    ;(changes as any).remove_inbox_tags = !!session.get(SETTINGS_KEYS.DOCUMENT_EDITING_REMOVE_INBOX_TAGS)
    api.patchDocument(changes, nonLatestVersionId() ? selectedVersionId : null).then(
      (docValues) => {
        closeIncomingUpdateModal()
        lastLocalSaveModified = docValues.modified ?? null
        // in case data changed while saving eg removing inbox_tags
        for (const k of Object.keys(form!) as (keyof DocumentFormValue)[]) if (k in docValues) (form as any)[k] = (docValues as any)[k]
        form!.custom_fields = (docValues.custom_fields ?? []).map((f) => ({ field: f.field, value: f.value }))
        store = { ...$state.snapshot(form!), tags: [...docValues.tags], custom_fields: [...form!.custom_fields] } as DocumentFormValue
        dirtyFields = new Set()
        openDocuments.setDirty(doc!, false)
        openDocuments.save()
        toastService.showSuccess(t('detail.document_saved_successfully', { PH: form!.title }))
        networkActive = false
        error = null
        if (close) {
          pendingIncomingUpdate = null
          closeDoc(() => openDocuments.refreshDocument(documentId!))
        } else {
          openDocuments.refreshDocument(documentId!)
          flushPendingIncomingUpdate()
        }
        data.refreshViewCounts()
        savedCallback?.()
      },
      (e) => {
        networkActive = false
        lastLocalSaveModified = null
        if (!session.hasObjectPermissions('change', doc)) {
          // document was 'given away'
          openDocuments.setDirty(doc!, false)
          toastService.showSuccess(t('detail.document_saved_successfully', { PH: doc!.title }))
          closeDoc()
        } else {
          error = e instanceof ApiError ? e.body : e
          toastService.showError(t('detail.error_saving_document_2', { PH: doc!.title }), e)
        }
        flushPendingIncomingUpdate()
      },
    )
  }

  async function saveEditNext() {
    if (!doc || !form) return
    networkActive = true
    store = $state.snapshot(form) as DocumentFormValue
    const changes = changedFields(doc.id, $state.snapshot(form) as DocumentFormValue, dirtyFields)
    ;(changes as any).remove_inbox_tags = !!session.get(SETTINGS_KEYS.DOCUMENT_EDITING_REMOVE_INBOX_TAGS)
    try {
      const updated = await api.patchDocument(changes, nonLatestVersionId() ? selectedVersionId : null)
      data.refreshViewCounts()
      const nextId = await getNext()
      if (nextId && updated) {
        openDocuments.setDirty(doc, false)
        const closed = await openDocuments.closeDocument(doc)
        closeIncomingUpdateModal()
        error = null
        networkActive = false
        pendingIncomingUpdate = null
        lastLocalSaveModified = null
        if (closed) router.navigate(`documents/${nextId}`)
      } else networkActive = false
    } catch (e) {
      networkActive = false
      lastLocalSaveModified = null
      error = e instanceof ApiError ? e.body : e
      toastService.showError(t('detail.error_saving_document'), e)
      flushPendingIncomingUpdate()
    }
  }

  async function closeDoc(closedCallback: (() => void) | null = null) {
    if (!doc) {
      router.navigate('documents')
      return
    }
    const closed = await openDocuments.closeDocument(doc)
    if (!closed) return
    closedCallback?.()
    const before = previousPath()
    if (before === 'dashboard') router.navigate('dashboard') // e.g. opened from a dashboard widget
    else if (list.activeSavedViewId) router.navigate(`view/${list.activeSavedViewId}`)
    else router.navigate(before)
  }

  /** ComponentRouterService.getComponentURLBefore: last page outside the document view */
  function previousPath() {
    const prev = (window as any).__pngxPreviousPath as string | undefined
    return prev && !/^documents\/\d+/.test(prev) ? prev : 'documents'
  }

  function deleteDoc() {
    if (!doc) return
    const d = doc
    modals.open(
      ConfirmDialog,
      {
        title: t('common.confirm'),
        messageBold: t('detail.really_want_move_document', { PH: d.title }),
        message: t('detail.documents_restored_prior_permanent'),
        btnClass: 'btn-danger',
        btnCaption: t('common.move_trash'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await api.deleteDocument(d.id)
            // TrashService.notifyMovedToTrash (with "Undo")
            toastService.show({
              content: t('detail.moved_trash', { PH: d.title }),
              delay: 10000,
              level: 'success',
              actionName: t('detail.undo'),
              action: () =>
                restoreDocuments([d.id]).then(
                  () => {
                    toastService.show({ content: t('common.document_s_restored'), delay: 5000, level: 'success' })
                    list.reload()
                  },
                  (e) => toastService.showError(t('common.error_restoring_document_s'), e),
                ),
            })
            modals.open_[modals.open_.length - 1]?.close()
            openDocuments.setDirty(d, false)
            closeDoc()
          } catch (e) {
            toastService.showError(t('common.error_deleting_document'), e)
            setEnabled(true)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  // ---- Reprocess, share links, email (dialogs loaded lazily) ---------------------------
  async function reprocess() {
    if (!doc) return
    const d = doc
    const C = (await import('../common/confirm-dialog/reprocess-confirm-dialog/ReprocessConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('detail.reprocess_confirm'),
        messageBold: t('detail.operation_permanently_recreate_archive'),
        message: t('detail.archive_file_re_generated'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void, remoteOcr: boolean, aiOcr: boolean) => {
          setEnabled(false)
          try {
            await api.reprocessDocuments([d.id], remoteOcr, aiOcr)
            toastService.showInfo(t('detail.reprocess_operation_begin_in', { PH: d.title }))
            ref.close()
          } catch (e) {
            setEnabled(true)
            toastService.showError(t('common.error_executing_operation'), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
  // ---- PDF editor (editPdf), remove password (removePassword); dialogs loaded lazily ----------------
  async function editPdf() {
    if (!doc) return
    const d = doc
    const sourceDocumentId = selectedVersionId ?? d.id
    const C = (await import('../common/pdf-editor/PdfEditor.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('common.pdf_editor'),
        btnCaption: t('common.proceed'),
        documentID: d.id,
        versionID: sourceDocumentId,
        password,
        onconfirm: async (setEnabled: (b: boolean) => void, r: { operations: { page: number; rotate: number; doc: number }[]; deleteOriginal: boolean; updateDocument: boolean; includeMetadata: boolean }) => {
          setEnabled(false)
          try {
            await api.editPdfDocuments([sourceDocumentId], {
              operations: r.operations,
              delete_original: r.deleteOriginal,
              update_document: r.updateDocument,
              include_metadata: r.includeMetadata,
              source_mode: 'explicit_selection',
            })
            toastService.showInfo(t('dialog.pdf_editor.started', { PH: d.title }))
            ref.close()
            if (r.deleteOriginal) openDocuments.closeDocument(d)
          } catch (e) {
            setEnabled(true)
            toastService.showError(t('dialog.pdf_editor.error'), e)
          }
        },
      },
      { backdrop: 'static', size: 'xl' },
    )
  }

  async function removePassword() {
    if (!doc) return
    if (requiresPassword || !password) {
      toastService.showError(t('dialog.password_removal.password_required'))
      return
    }
    const d = doc
    const pw = password
    const C = (await import('../common/confirm-dialog/password-removal-confirm-dialog/PasswordRemovalConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('dialog.unprotect.title'),
        message: t('dialog.unprotect.message'),
        btnCaption: t('dialog.unprotect.start'),
        onconfirm: async (setEnabled: (b: boolean) => void, o: { updateDocument: boolean; includeMetadata: boolean; deleteOriginal: boolean }) => {
          const sourceDocumentId = selectedVersionId ?? d.id
          setEnabled(false)
          networkActive = true
          try {
            await api.removePasswordDocuments([sourceDocumentId], {
              password: pw,
              update_document: o.updateDocument,
              include_metadata: o.includeMetadata,
              delete_original: o.deleteOriginal,
              source_mode: 'explicit_selection',
            })
            toastService.showInfo(t('dialog.password_removal.started', { PH: d.title }))
            networkActive = false
            ref.close()
            if (!o.updateDocument && o.deleteOriginal) openDocuments.closeDocument(d)
            else if (o.updateDocument) openDocuments.refreshDocument(d.id)
          } catch (e) {
            setEnabled(true)
            networkActive = false
            toastService.showError(t('dialog.password_removal.error'), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  // ---- Manual workflow run (PaperRust; idea from paperless-ngx dev #14704) --------------------
  let manualWorkflows = $state<ManualWorkflow[]>([])
  $effect(() => {
    void loadManualWorkflows(session.can('view', 'workflow')).then((w) => (manualWorkflows = w))
  })
  async function runWorkflow() {
    if (!doc) return
    const d = doc
    const C = (await import('../common/confirm-dialog/run-workflow-confirm-dialog/RunWorkflowConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('common.run_workflow_2'),
        message: t('common.run_workflow', { PH: d.title }),
        btnCaption: t('common.run'),
        workflows: manualWorkflows,
        // one document: ignore the triggers' filters by default (idea from paperless-ngx dev #14760)
        forceDefault: true,
        singleDocument: true,
        onconfirm: async (setEnabled: (b: boolean) => void, workflowId: number, force: boolean) => {
          setEnabled(false)
          try {
            await api.runWorkflow([d.id], workflowId, !force)
            toastService.showInfo(t('common.workflow_run_in_background', { PH: d.title }))
            ref.close()
          } catch (e) {
            setEnabled(true)
            toastService.showError(t('common.error_executing_operation'), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
  async function openShareLinks() {
    const C = (await import('../common/share-links-dialog/ShareLinksDialog.svelte')).default
    modals.open(C, { documentId: selectedVersionId ?? doc!.id, hasArchiveVersion: hasArchiveVersion })
  }
  // PaperRust: redact/shrink as a new version (pdf-tools/)
  async function openRedact() {
    const C = (await import('./pdf-tools/RedactDialog.svelte')).default
    modals.open(C, { documentId: doc!.id, src: api.previewUrl(doc!.id, true) }, { backdrop: 'static', size: 'xl' })
  }
  async function openShrink() {
    const C = (await import('./pdf-tools/ShrinkDialog.svelte')).default
    modals.open(C, { documentId: doc!.id }, { backdrop: 'static' })
  }
  // PaperRust: convert to PDF (image/office → new PDF version, common/convert-pdf/)
  const canConvertToPdf = $derived(isConvertible(metadata?.original_mime_type || doc?.mime_type))
  async function openConvertPdf() {
    const C = (await import('../common/convert-pdf/ConvertPdfDialog.svelte')).default
    modals.open(C, { documentIds: [doc!.id], showOcr: isImage(metadata?.original_mime_type || doc?.mime_type) }, { backdrop: 'static' })
  }
  async function openEmailDocument() {
    const C = (await import('../common/email-document-dialog/EmailDocumentDialog.svelte')).default
    modals.open(C, { documentIds: [selectedVersionId ?? doc!.id], hasArchiveVersion: hasArchiveVersion }, { backdrop: 'static' })
  }

  // ---- Create objects (createTag/-DocumentType/-Correspondent/-StoragePath, CustomFieldsDropdown.createField) ----
  const byId = <T extends { id: number }>(xs: T[]) => new Map(xs.map((x) => [x.id, x]))
  function openCreateDialog(component: Component<any>, newName: string | null | undefined, onsucceeded: (o: any) => void, extra: Record<string, any> = {}, staticBackdrop = true) {
    modals.open(component, { dialogMode: 'create', object: newName ? { name: newName } : null, onsucceeded, ...extra }, staticBackdrop ? { backdrop: 'static' } : {})
  }
  const dropSuggestion = (key: 'suggested_tags' | 'suggested_document_types' | 'suggested_correspondents', name: string | null | undefined) => {
    if (suggestions) suggestions = { ...suggestions, [key]: ((suggestions as any)[key] ?? []).filter((n: string) => n !== name) }
  }
  async function createTag(newName: string | null = null) {
    const C = (await import('../common/edit-dialog/tag-edit-dialog/TagEditDialog.svelte')).default
    openCreateDialog(C, newName, async (newTag: { id: number; name: string }) => {
      dropSuggestion('suggested_tags', newTag.name)
      const r = await api.listAll<any>('tags')
      data.tags = byId(r.results)
      if (form && !form.tags.includes(newTag.id)) setField('tags', [...form.tags, newTag.id])
    })
  }
  async function createDocumentType(newName: string | null = null) {
    const C = (await import('../common/edit-dialog/document-type-edit-dialog/DocumentTypeEditDialog.svelte')).default
    openCreateDialog(C, newName, async (o: { id: number }) => {
      const r = await api.listAll<any>('document_types')
      data.documentTypes = byId(r.results)
      setField('document_type', o.id)
      dropSuggestion('suggested_document_types', newName)
    })
  }
  async function createCorrespondent(newName: string | null = null) {
    const C = (await import('../common/edit-dialog/correspondent-edit-dialog/CorrespondentEditDialog.svelte')).default
    openCreateDialog(C, newName, async (o: { id: number }) => {
      const r = await api.listAll<any>('correspondents')
      data.correspondents = byId(r.results)
      setField('correspondent', o.id)
      dropSuggestion('suggested_correspondents', newName)
    })
  }
  async function createStoragePath(newName: string | null = null) {
    const C = (await import('../common/edit-dialog/storage-path-edit-dialog/StoragePathEditDialog.svelte')).default
    openCreateDialog(C, newName, async (o: { id: number }) => {
      const r = await api.listAll<any>('storage_paths')
      data.storagePaths = byId(r.results)
      setField('storage_path', o.id)
    })
  }
  async function createField(newName: string | undefined) {
    const C = (await import('../common/edit-dialog/custom-field-edit-dialog/CustomFieldEditDialog.svelte')).default
    openCreateDialog(
      C,
      newName,
      async (newField: CustomField) => {
        toastService.showSuccess(t('common.saved_field', { PH: newField.name }))
        const r = await api.listAll<CustomField>('custom_fields')
        data.customFields = r.results
        setTimeout(() => addField(newField), 100)
      },
      { onfailed: (e: any) => toastService.showError(t('common.error_saving_field'), e) },
      false,
    )
  }

  function quickFilter(rules: FilterRule[]) {
    list.setFilterRules(rules)
    router.navigate('documents')
  }
  const moreLike = () => quickFilter([{ rule_type: FILTER_FULLTEXT_MORELIKE, value: String(documentId) }])
  const filterDocuments = (items: any[], type?: 'correspondent' | 'document_type' | 'storage_path' | 'tag') =>
    quickFilter(filterRulesFor(items as (DateStruct | { id: number })[], type))

  async function download(original = false) {
    downloading = true
    const url = api.downloadUrlFor(documentId!, original, nonLatestVersionId(), useFormattedFilename)
    try {
      const r = await fetch(url, { credentials: 'same-origin' })
      if (!r.ok) throw new Error(`${r.status}`)
      const cd = r.headers.get('Content-Disposition') ?? ''
      const m = /filename\*=UTF-8''([^;]+)/i.exec(cd) ?? /filename="?([^";]+)"?/i.exec(cd)
      const filename = m ? decodeURIComponent(m[1]) : doc!.title
      const blob = await r.blob()
      downloading = false
      const a = document.createElement('a')
      const href = URL.createObjectURL(blob)
      a.href = href
      a.download = filename
      a.click()
      URL.revokeObjectURL(href)
    } catch (e) {
      downloading = false
      toastService.showError(t('detail.error_downloading_document'), e)
    }
  }

  let printIframe: HTMLIFrameElement | null = null
  let printBlobUrl: string | null = null
  function cleanupPrint() {
    printIframe?.remove()
    printIframe = null
    if (printBlobUrl) URL.revokeObjectURL(printBlobUrl)
    printBlobUrl = null
  }
  async function printDocument() {
    cleanupPrint()
    const r = await fetch(api.downloadUrlFor(doc!.id, false, nonLatestVersionId()), { credentials: 'same-origin' })
    if (!r.ok) return toastService.showError(t('detail.error_loading_document_printing'))
    printBlobUrl = URL.createObjectURL(await r.blob())
    const iframe = document.createElement('iframe')
    printIframe = iframe
    Object.assign(iframe.style, { position: 'fixed', right: '0', bottom: '0', width: '0', height: '0', border: '0', visibility: 'hidden' })
    iframe.src = printBlobUrl
    document.body.appendChild(iframe)
    iframe.onload = () =>
      setTimeout(() => {
        try {
          iframe.contentWindow?.focus()
          iframe.contentWindow?.print()
        } catch (err) {
          toastService.showError(t('detail.print_failed'), err)
          setTimeout(() => cleanupPrint(), 100)
        }
      })
  }

  function notesUpdated(notes: DocumentNote[]) {
    if (!doc) return
    doc.notes = notes
    openDocuments.refreshDocument(documentId!)
  }

  function navTo(key: NavKey, e: Event) {
    e.preventDefault()
    if (activeNav === key) return
    activeNav = key
    router.navigate(`documents/${documentId}/${key}`)
  }

  // ---- PDF preview ------------------------------------------------------------------------
  function pdfPreviewLoaded(pdf: { numPages: number }) {
    previewNumPages = pdf.numPages
    if (password) requiresPassword = false
    previewLoaded = true
  }
  function onPdfError(e: any) {
    if (e?.name == 'PasswordException') {
      requiresPassword = true
      previewLoaded = true
    }
  }
  function onPasswordKeyUp(e: KeyboardEvent) {
    if (e.key == 'Enter') {
      password = (e.target as HTMLInputElement).value
      updatePdfSource()
    }
  }
  const zoomTitle = (s: string) => (s === 'page-fit' ? t('detail.page_fit') : `${parseFloat(s) * 100}%`)

  // ---- Keyboard shortcuts (HotKeyService) ---------------------------------------------------------
  function hotkey(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey
    if ((!(e.altKey || mod) || e.key.toLowerCase().includes('arrow')) && (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) return
    let handler: (() => void) | null = null
    if (mod && !e.shiftKey && e.key === 'ArrowRight') handler = () => hasNext && nextDoc()
    else if (mod && !e.shiftKey && e.key === 'ArrowLeft') handler = () => hasPrevious && previousDoc()
    else if (e.key === 'Escape' && !mod) {
      if (modals.open_.length || document.getElementsByClassName('dropdown-menu show').length > 0) return
      handler = () => closeDoc()
    } else if (mod && !e.shiftKey && e.key.toLowerCase() === 's') handler = () => openDocuments.isDirty(doc) && save()
    else if (mod && e.shiftKey && e.key.toLowerCase() === 's')
      handler = () => {
        if (openDocuments.isDirty(doc)) hasNext ? saveEditNext() : save(true)
      }
    if (!handler) return
    e.preventDefault()
    handler()
  }

  // @ViewChild('pdfPreview'): preview column visible and "Preview" tab active → switch to "Details"
  $effect(() => {
    if (wide && activeNav === 'preview' && documentId) setTimeout(() => untrack(() => {
      activeNav = 'details'
      router.navigate(`documents/${documentId}/details`)
    }))
  })

  onMount(() => {
    const mq = matchMedia('(min-width: 768px)')
    const onMq = () => (wide = mq.matches)
    mq.addEventListener('change', onMq)
    // WebsocketStatusService.onDocumentUpdated
    const offUpdated = consumerStatus.onDocumentUpdated((m) => handleIncomingDocumentUpdated(m))
    return () => {
      mq.removeEventListener('change', onMq)
      offUpdated()
      closeIncomingUpdateModal()
    }
  })
  onDestroy(() => {
    cleanupPrint()
    clearTimeout(titleTimer)
  })

  const ddDownload = new Dropdown()
  const ddActions = new Dropdown()
  const ddSend = new Dropdown()
  const ddSaveTop = new Dropdown({ placement: 'bottom-end top-end' })
  const ddSaveBottom = new Dropdown({ placement: 'bottom-end top-end' })
  const ddSave = (where: 'top' | 'bottom') => (where === 'top' ? ddSaveTop : ddSaveBottom)
</script>

<svelte:window onkeydown={hotkey} />

<pngx-page-header>
<PageHeader title={title ?? ''} id={documentId}>
  {#if session.can('delete', 'document')}
  <button type="button" class="btn btn-sm btn-outline-danger me-md-4" onclick={deleteDoc} disabled={!userIsOwner} aria-label={t('common.delete')}>
    <pngx-bs-icon width="1.2em" height="1.2em" name="trash">{@html icon('trash', '1.2em', '1.2em')}</pngx-bs-icon><span class="d-none d-lg-inline ps-1">{t('common.delete')}</span>
  </button>
  {/if}

  <pngx-document-version-dropdown><DocumentVersionDropdown
    {documentId}
    versions={doc?.versions ?? []}
    {selectedVersionId}
    {userIsOwner}
    {userCanEdit}
    {dateLocale}
    onversionselected={onVersionSelected}
    onversionsupdated={onVersionsUpdated}
  /></pngx-document-version-dropdown>

  <div class="btn-group ms-md-4">
    <button onclick={() => download()} class="btn btn-sm btn-outline-primary" disabled={downloading} aria-label={t('common.download')}>
      {#if downloading}
        <div class="spinner-border spinner-border-sm" role="status"></div>
      {:else}
        <pngx-bs-icon width="1.2em" height="1.2em" name="download">{@html icon('download', '1.2em', '1.2em')}</pngx-bs-icon>
      {/if}
      <span class="d-none d-lg-inline ps-1">{t('common.download')}</span>
    </button>

    <div class="btn-group {ddDownload.placementClass}" class:show={ddDownload.open} role="group" use:ddDownload.hostEl>
      <button class="btn btn-sm btn-outline-primary dropdown-toggle" disabled={downloading} aria-label={t('common.download_options')} aria-expanded={ddDownload.open} use:ddDownload.anchorEl onclick={ddDownload.toggle}></button>
      {#if ddDownload.open}
      <div class="dropdown-menu shadow show" use:ddDownload.menuEl>
        {#if metadata?.has_archive_version}
          <button class="dropdown-item" tabindex="0" onclick={() => download(true)} disabled={downloading}>{t('detail.download_original')}</button>
          <div class="dropdown-divider"></div>
        {/if}
        <form class="px-3 py-1">
          <div class="form-check">
            <input type="checkbox" class="form-check-input" id="downloadUseFormatting" bind:checked={useFormattedFilename} />
            <label class="form-check-label" for="downloadUseFormatting">{t('common.use_formatted_filename')}</label>
          </div>
        </form>
      </div>
      {/if}
    </div>
  </div>

  <div class="ms-md-auto {ddActions.placementClass}" class:show={ddActions.open} use:ddActions.hostEl>
    <button class="btn btn-sm btn-outline-primary dropdown-toggle" id="actionsDropdown" aria-label={t('common.actions')} aria-expanded={ddActions.open} use:ddActions.anchorEl onclick={ddActions.toggle}>
      <pngx-bs-icon name="three-dots">{@html icon('three-dots')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.actions')}</div>
    </button>
    {#if ddActions.open}
    <div aria-labelledby="actionsDropdown" class="dropdown-menu shadow show" use:ddActions.menuEl>
      <button class="dropdown-item" tabindex="0" onclick={reprocess} disabled={!userCanEdit || !userIsOwner}>
        <pngx-bs-icon width="1em" height="1em" name="arrow-counterclockwise" class="me-2">{@html icon('arrow-counterclockwise', '1em', '1em')}</pngx-bs-icon><span>{t('common.reprocess')}</span>
      </button>

      {#if manualWorkflows.length > 0}
        <button class="dropdown-item" tabindex="0" onclick={runWorkflow} disabled={!userCanEdit || !userIsOwner} id="runWorkflow">
          <pngx-bs-icon width="1em" height="1em" name="play-fill" class="me-2">{@html icon('play-fill', '1em', '1em')}</pngx-bs-icon><span>{t('common.run_workflow_2')}</span>
        </button>
      {/if}

      <button class="dropdown-item" tabindex="0" onclick={printDocument} hidden={useNativePdfViewer || isMobile}>
        <pngx-bs-icon width="1em" height="1em" name="printer" class="me-2">{@html icon('printer', '1em', '1em')}</pngx-bs-icon><span>{t('detail.print')}</span>
      </button>

      <button class="dropdown-item" tabindex="0" onclick={moreLike}>
        <pngx-bs-icon width="1em" height="1em" name="diagram-3" class="me-2">{@html icon('diagram-3', '1em', '1em')}</pngx-bs-icon><span>{t('common.more_like')}</span>
      </button>

      <button class="dropdown-item" tabindex="0" onclick={editPdf} disabled={!userIsOwner || !userCanEdit || originalContentRenderType !== 'pdf'}>
        <pngx-bs-icon name="pencil" class="me-2">{@html icon('pencil')}</pngx-bs-icon>{t('common.pdf_editor')}
      </button>

      <!-- PaperRust: convert to PDF (common/convert-pdf/), afterwards the PDF tools work -->
      <button class="dropdown-item" tabindex="0" onclick={openConvertPdf} disabled={!userIsOwner || !userCanEdit} hidden={!canConvertToPdf}>
        <pngx-bs-icon name="file-earmark-pdf" class="me-2">{@html icon('file-earmark-pdf')}</pngx-bs-icon>{t('convert.menu')}
      </button>
      <!-- PaperRust: PDF tools (pdf-tools/, pl-pdftools) -->
      <button class="dropdown-item" tabindex="0" onclick={openRedact} disabled={!userIsOwner || !userCanEdit || originalContentRenderType !== 'pdf' || requiresPassword}>
        <pngx-bs-icon name="file-earmark-lock" class="me-2">{@html icon('file-earmark-lock')}</pngx-bs-icon>{t('pdftools.redact_menu')}
      </button>
      <button class="dropdown-item" tabindex="0" onclick={openShrink} disabled={!userIsOwner || !userCanEdit || originalContentRenderType !== 'pdf' || requiresPassword}>
        <pngx-bs-icon name="file-earmark-minus" class="me-2">{@html icon('file-earmark-minus')}</pngx-bs-icon>{t('pdftools.shrink_menu')}
      </button>

      {#if userIsOwner && (requiresPassword || password)}
        <button class="dropdown-item" tabindex="0" onclick={removePassword} disabled={!password}>
          <pngx-bs-icon name="unlock" class="me-2">{@html icon('unlock')}</pngx-bs-icon>{t('detail.remove_password')}
        </button>
      {/if}
    </div>
    {/if}
  </div>

  <div class="ms-md-auto {ddSend.placementClass}" class:show={ddSend.open} use:ddSend.hostEl>
    <button class="btn btn-sm btn-outline-primary dropdown-toggle" id="sendDropdown" aria-label={t('common.send')} aria-expanded={ddSend.open} use:ddSend.anchorEl onclick={ddSend.toggle}>
      <pngx-bs-icon name="send">{@html icon('send')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.send')}</div>
    </button>
    {#if ddSend.open}
    <div aria-labelledby="sendDropdown" class="dropdown-menu shadow show" use:ddSend.menuEl>
      {#if session.can('add', 'sharelink')}
      <button class="dropdown-item" tabindex="0" onclick={openShareLinks}>
        <pngx-bs-icon name="link" class="me-2">{@html icon('link')}</pngx-bs-icon><span>{t('common.share_links')}</span>
      </button>
      {/if}
      {#if emailEnabled}
        <button class="dropdown-item" tabindex="0" onclick={openEmailDocument}>
          <pngx-bs-icon name="envelope" class="me-2">{@html icon('envelope')}</pngx-bs-icon><span>{t('common.email')}</span>
        </button>
      {/if}
    </div>
    {/if}
  </div>

</PageHeader>
</pngx-page-header>

{#snippet saveButtons(where: 'top' | 'bottom')}
  {@const dd = ddSave(where)}
  <div class="btn-group">
    {#if session.can('change', 'document')}
      <button type="submit" class="order-3 btn btn-sm" class:btn-primary={!saveDisabled} class:btn-outline-primary={saveDisabled} title={t('common.save')} disabled={saveDisabled}>
        <pngx-bs-icon class="d-md-none" width="1.2em" height="1.2em" name="floppy">{@html icon('floppy', '1.2em', '1.2em')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.save')}</span>
      </button>
      {#if hasNext}
        <button type="button" class="order-1 btn btn-sm btn-outline-primary d-none d-md-inline-block" onclick={saveEditNext} disabled={saveDisabled}>{t('detail.save_next')}</button>
      {:else}
        <button type="button" class="order-2 btn btn-sm btn-outline-primary d-none d-md-inline-block" onclick={() => save(true)} disabled={saveDisabled}>{t('detail.save_close')}</button>
      {/if}
      <button type="button" class="order-0 btn btn-sm btn-outline-secondary" onclick={discard} title={t('common.discard')} disabled={saveDisabled}>
        <pngx-bs-icon class="d-md-none" width="1.2em" height="1.2em" name="arrow-counterclockwise">{@html icon('arrow-counterclockwise', '1.2em', '1.2em')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.discard')}</span>
      </button>
      <div class="btn-group order-4 d-md-none {dd.placementClass}" class:show={dd.open} role="group" use:dd.hostEl>
        <button type="button" class="btn btn-sm btn-outline-primary dropdown-toggle dropdown-toggle-split" aria-label={t('detail.more_save_options')} disabled={saveDisabled} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}></button>
        {#if dd.open}
        <div class="dropdown-menu shadow show" use:dd.menuEl>
          {#if hasNext}
            <button type="button" class="dropdown-item" tabindex="0" onclick={saveEditNext}>{t('detail.save_next')}</button>
          {/if}
          <button type="button" class="dropdown-item" tabindex="0" onclick={() => save(true)}>{t('detail.save_close')}</button>
        </div>
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet previewContent()}
  <div class="thumb-preview position-absolute pe-none text-center" class:fade={previewLoaded}>
    {#if showThumbnailOverlay}
      <img src={thumbUrl} class="mx-auto" width={zoom.scale === 'page-fit' ? 'auto' : '100%'} height={zoom.scale === 'page-fit' ? '100%' : 'auto'} alt={t('detail.document_loading')} />
    {/if}
    <div class="position-absolute top-0 start-0 m-2 p-2 d-flex align-items-center justify-content-center">
      <div>
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
      </div>
    </div>
  </div>
  {#if doc}
    {#if archiveContentRenderType === 'pdf'}
        {#if !useNativePdfViewer}
          <div class="preview-sticky pdf-viewer-container" tabindex="-1">
            <pngx-pdf-viewer><PdfViewer
              src={pdfSource}
              sourceRevision={previewRevision}
              password={pdfPassword}
              renderMode={PdfRenderMode.All}
              page={previewCurrentPage} onpagechange={(p) => (previewCurrentPage = p)}
              zoomScale={zoom.scale}
              zoom={zoom.level}
              onloaderror={onPdfError}
              onafterloadcomplete={pdfPreviewLoaded} />
            </pngx-pdf-viewer>
            <div class="preview-controls position-absolute bottom-0 start-50 translate-middle-x z-1 mb-3 d-flex gap-2 p-1 bg-body border rounded shadow">
              {#if previewNumPages}
                <div class="input-group input-group-sm w-auto flex-nowrap">
                  <input class="form-control flex-grow-0 w-auto" type="number" min="1" max={previewNumPages} value={previewCurrentPage} oninput={(e) => (previewCurrentPage = +e.currentTarget.value)} aria-label={t('common.page')} />
                  <div class="input-group-text">{t('detail.of', { INTERPOLATION: previewNumPages })}</div>
                </div>
              {/if}
              <div class="input-group input-group-sm w-auto flex-nowrap">
                <button class="btn btn-outline-secondary" type="button" onclick={() => (zoom = zoomOut(zoom))} aria-label={t('common.zoom_out')}>
                  <pngx-bs-icon width="1em" height="1em" name="dash">{@html icon('dash', '1em', '1em')}</pngx-bs-icon>
                </button>
                <select class="form-select" onchange={(e) => (zoom = zoomFrom(e.currentTarget.value))} value={zoomCurrent(zoom)} aria-label={t('detail.zoom')}>
                  {#each zoomSettings() as setting (setting)}
                    <option value={setting}>
                      {zoomTitle(setting)}
                    </option>
                  {/each}
                </select>
                <button class="btn btn-outline-secondary" type="button" onclick={() => (zoom = zoomIn(zoom))} aria-label={t('common.zoom_in')}>
                  <pngx-bs-icon width="1em" height="1em" name="plus">{@html icon('plus', '1em', '1em')}</pngx-bs-icon>
                </button>
              </div>
            </div>
          </div>
        {:else}
          <object data={withSearchHash(previewUrl)} class="preview-sticky" width="100%" title=""></object>
        {/if}
    {:else if archiveContentRenderType === 'text'}
        <div class="preview-sticky bg-light p-3 overflow-auto whitespace-preserve">{previewText}</div>
    {:else if archiveContentRenderType === 'image'}
        <div class="preview-sticky">
          <img src={previewUrl} width="100%" height="100%" alt={title} />
        </div>
    {:else if archiveContentRenderType === 'tiff'}
        {#if !tiffError}
          <div class="preview-sticky">
            <img src={tiffURL} width="100%" height="100%" alt={title} />
          </div>
        {:else}
          <div class="preview-sticky bg-light p-3 overflow-auto whitespace-preserve">{tiffError}</div>
        {/if}
    {:else if archiveContentRenderType === 'other'}
        <object data={withSearchHash(previewUrl)} class="preview-sticky" width="100%" title=""></object>
    {/if}
    {#if requiresPassword}
      <div class="password-prompt">
        <form>
          <!-- svelte-ignore a11y_autofocus -->
          <input autocomplete="off" autofocus={true} class="form-control" placeholder={t('detail.enter_password')} type="password" onkeyup={onPasswordKeyUp} />
        </form>
      </div>
    {/if}
  {/if}
{/snippet}

{#snippet navLink(key: NavKey, label: string, cls = '')}
  <!-- ngbNavLink -->
  <a href="" class="nav-link {cls}" class:active={activeNav === key} role="tab" id="ngb-nav-{NAV_IDS[key]}" aria-controls={activeNav === key ? `ngb-nav-${NAV_IDS[key]}-panel` : null} aria-selected={activeNav === key} aria-disabled="false" onclick={(e) => navTo(key, e)}>{label}</a>
{/snippet}

<div class="row">
  <div class="col-md-6 col-xl-5 mb-4">

    <form onsubmit={(e) => { e.preventDefault(); save() }}>

      <div class="btn-toolbar mb-1 pb-3 gap-2 row-gap-2 border-bottom">
        <div class="btn-group">
          <button type="button" class="btn btn-sm btn-outline-secondary" title={t('common.close')} onclick={() => closeDoc()}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="x">{@html icon('x', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
          <button type="button" class="btn btn-sm btn-outline-secondary" title={t('common.previous')} onclick={previousDoc} disabled={!hasPrevious}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="arrow-left">{@html icon('arrow-left', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
          <button type="button" class="btn btn-sm btn-outline-secondary" title={t('common.next')} onclick={nextDoc} disabled={!hasNext}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="arrow-right">{@html icon('arrow-right', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
        </div>

        {#if session.can('change', 'document')}
          <div class="btn-group ms-3">
            <pngx-suggestions-dropdown><SuggestionsDropdown
              disabled={!userCanEdit || suggestionsLoading}
              loading={suggestionsLoading}
              {suggestions}
              {aiEnabled}
              source={suggestionSource}
              defaultSource={defaultSuggestionSource}
              fetchedSources={fetchedSuggestionSources}
              applied={{ tags: form?.tags, correspondent: form?.correspondent, documentType: form?.document_type, storagePath: form?.storage_path, title: form?.title, created: form?.created }}
              ongetsuggestions={(s) => getSuggestions(s)}
              onsourcechange={(s) => (suggestionSourceOverride = s)}
              onaddtag={(n) => createTag(n)}
              onadddocumenttype={(n) => createDocumentType(n)}
              onaddcorrespondent={(n) => createCorrespondent(n)} />
            </pngx-suggestions-dropdown>
          </div>

          <div class="ms-auto">
            {@render saveButtons('top')}
          </div>
        {/if}
      </div>

      <ul class="nav nav-underline flex-nowrap flex-md-wrap overflow-auto" role="tablist">
        <li class="nav-item" role="presentation">
          {@render navLink('details', t('common.details'))}
        </li>

        <li class="nav-item" role="presentation">
          {@render navLink('content', t('common.content'))}
        </li>

        <li class="nav-item" role="presentation">
          {@render navLink('metadata', t('detail.metadata'))}
        </li>

        <li class="d-md-none nav-item" role="presentation">
          {@render navLink('preview', t('common.preview'))}
        </li>

        {#if notesEnabled}
          <li class="nav-item" role="presentation">
            <a href="" class="text-nowrap nav-link" class:active={activeNav === 'notes'} role="tab" id="ngb-nav-{NAV_IDS.notes}" aria-controls={activeNav === 'notes' ? `ngb-nav-${NAV_IDS.notes}-panel` : null} aria-selected={activeNav === 'notes'} aria-disabled="false" onclick={(e) => navTo('notes', e)}>{t('common.notes')}{' '}{#if doc?.notes.length}
              <span class="badge text-bg-secondary ms-1">{doc.notes.length}</span>
            {/if}</a>
          </li>
        {/if}

        {#if historyEnabled}
          <li class="nav-item" role="presentation">
            {@render navLink('history', t('common.history'))}
          </li>
        {/if}

        {#if showPermissions}
          <li class="nav-item" role="presentation">
            {@render navLink('permissions', t('common.permissions'))}
          </li>
        {/if}

        {#if doc?.duplicate_documents?.length}
          <li class="nav-item" role="presentation">
            <a href="" class="text-nowrap nav-link" class:active={activeNav === 'duplicates'} role="tab" id="ngb-nav-{NAV_IDS.duplicates}" aria-selected={activeNav === 'duplicates'} aria-disabled="false" onclick={(e) => navTo('duplicates', e)}>
              {t('common.duplicates')}
              <span class="badge text-bg-secondary ms-1">{doc.duplicate_documents.length}</span>
            </a>
          </li>
        {/if}
      </ul>

      <div class="mt-3 tab-content">
        <div class="tab-pane fade show active" role="tabpanel" id="ngb-nav-{NAV_IDS[activeNav]}-panel" aria-labelledby="ngb-nav-{NAV_IDS[activeNav]}">
        {#if activeNav === 'details' && form}
            <div>
              <pngx-input-text><InputText title={t('common.title')} value={form.title} horizontal={true} suggestion={suggestions?.title} onkeyup={titleKeyUp} oninput={(v) => setField('title', v)} onchange={(v) => { title = docTitle(v ?? ''); setField('title', v ?? '') }} error={error?.title} disabled={formDisabled} /></pngx-input-text>
              {#if !isFieldHidden('archive_serial_number')}
                <pngx-input-number><InputNumber title={t('common.archive_serial_number')} error={error?.archive_serial_number} horizontal={true} value={form.archive_serial_number} onchange={(v) => setField('archive_serial_number', v)} disabled={formDisabled} /></pngx-input-number>
              {/if}
              <pngx-input-date><InputDate title={t('common.date_created')} value={form.created} suggestions={suggestions?.dates} showFilter={true} horizontal={true} onfilterdocuments={(d) => filterDocuments(d)}
              error={error?.created} onchange={(v) => setField('created', v ?? null)} disabled={formDisabled} /></pngx-input-date>
              {#if !isFieldHidden('correspondent')}
                {#if session.can('view', 'correspondent')}
                <pngx-input-select><InputSelect items={[...data.correspondents.values()]} title={t('common.correspondent')} value={form.correspondent} allowNull={true} showFilter={true} horizontal={true} onfilterdocuments={(i) => filterDocuments(i, 'correspondent')}
                oncreatenew={(n) => createCorrespondent(n)} hideAddButton={createDisabled('correspondent')} suggestions={suggestions?.correspondents} onchange={(v) => setField('correspondent', v)} disabled={formDisabled} /></pngx-input-select>
                {/if}
              {/if}
              {#if !isFieldHidden('document_type')}
                {#if session.can('view', 'documenttype')}
                <pngx-input-select><InputSelect items={[...data.documentTypes.values()]} title={t('common.document_type')} value={form.document_type} allowNull={true} showFilter={true} horizontal={true} onfilterdocuments={(i) => filterDocuments(i, 'document_type')}
                oncreatenew={(n) => createDocumentType(n)} hideAddButton={createDisabled('documenttype')} suggestions={suggestions?.document_types} onchange={(v) => setField('document_type', v)} disabled={formDisabled} /></pngx-input-select>
                {/if}
              {/if}
              {#if !isFieldHidden('storage_path')}
                {#if session.can('view', 'storagepath')}
                <pngx-input-select><InputSelect items={[...data.storagePaths.values()]} title={t('common.storage_path')} value={form.storage_path} allowNull={true} showFilter={true} horizontal={true} onfilterdocuments={(i) => filterDocuments(i, 'storage_path')}
                oncreatenew={(n) => createStoragePath(n)} hideAddButton={createDisabled('storagepath')} suggestions={suggestions?.storage_paths} placeholder={t('common.default')} onchange={(v) => setField('storage_path', v)} disabled={formDisabled} /></pngx-input-select>
                {/if}
              {/if}
              {#if !isFieldHidden('tags')}
                {#if session.can('view', 'tag')}
                <pngx-input-tags><InputTags {tags} value={form.tags} suggestions={suggestions?.tags} showFilter={true} horizontal={true} onfilterdocuments={(x) => filterDocuments(x, 'tag')} hideAddButton={createDisabled('tag')} oncreate={() => createTag()} onchange={(v) => setField('tags', v)} disabled={formDisabled} /></pngx-input-tags>
                {/if}
              {/if}
              {#each doc?.custom_fields ?? [] as fieldInstance, i (fieldInstance.field)}
                {@const cf = getCustomField(fieldInstance.field)}
                {@const fv = form.custom_fields[i]?.value as any}
                <div>
                  {#if cf?.data_type === 'string'}
                      <pngx-input-text><InputText value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} oninput={(v) => setCustomField(i, v)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-text>
                  {:else if cf?.data_type === 'date'}
                      <pngx-input-date><InputDate value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-date>
                  {:else if cf?.data_type === 'integer'}
                      <pngx-input-number><InputNumber value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      showAdd={false}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-number>
                  {:else if cf?.data_type === 'float'}
                      <pngx-input-number><InputNumber value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      showAdd={false}
                      step={0.1}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-number>
                  {:else if cf?.data_type === 'monetary'}
                      <pngx-input-monetary><InputMonetary value={fv}
                      title={cf.name}
                      defaultCurrency={cf.extra_data?.default_currency}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-monetary>
                  {:else if cf?.data_type === 'boolean'}
                      <pngx-input-check><InputCheck value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-check>
                  {:else if cf?.data_type === 'url'}
                      <pngx-input-url><InputUrl value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-url>
                  {:else if cf?.data_type === 'documentlink'}
                      <pngx-input-document-link><InputDocumentLink value={fv}
                      title={cf.name}
                      parentDocumentID={documentId}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-document-link>
                  {:else if cf?.data_type === 'select'}
                      <pngx-input-select><InputSelect value={fv}
                      title={cf.name}
                      items={cf.extra_data?.select_options}
                      bindLabel="label"
                      allowNull={true}
                      horizontal={true}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      error={getCustomFieldError(i)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-select>
                  {:else if cf?.data_type === 'longtext'}
                      <pngx-input-textarea><InputTextarea value={fv}
                      title={cf.name}
                      removable={userCanEdit}
                      onremoved={() => removeField(i)}
                      horizontal={true}
                      error={getCustomFieldError(i)} oninput={(v) => setCustomField(i, v)} onchange={(v) => setCustomField(i, v)} disabled={formDisabled} /></pngx-input-textarea>
                  {/if}
                </div>
              {/each}
            </div>

            <div class="d-flex border-top pt-3">
              {#if session.can('change', 'document')}
                {#if session.can('view', 'customfield')}
                <pngx-custom-fields-dropdown><CustomFieldsDropdown
                  fields={customFields}
                  disabled={!userCanEdit}
                  existingFields={doc?.custom_fields}
                  oncreate={(n) => createField(n)}
                  onadded={addField} />
                </pngx-custom-fields-dropdown>
                {/if}
              {/if}
              <div class="ms-auto">
                {@render saveButtons('bottom')}
              </div>
            </div>
        {:else if activeNav === 'content' && form}
            <div>
              <textarea class="form-control" id="content" rows="20" value={form.content} oninput={(e) => setField('content', e.currentTarget.value)} class:rtl={isRTL} disabled={formDisabled}></textarea>
            </div>
        {:else if activeNav === 'metadata'}
            {#if doc}
              <table class="table table-borderless">
                <tbody>
                  <tr>
                    <td>{t('detail.date_modified')}</td>
                    <td>{formatDate(doc.modified, session.get(SETTINGS_KEYS.DATE_FORMAT), dateLocale)}</td>
                  </tr>
                  <tr>
                    <td>{t('detail.date_added')}</td>
                    <td>{formatDate(doc.added, session.get(SETTINGS_KEYS.DATE_FORMAT), dateLocale)}</td>
                  </tr>
                  <tr>
                    <td>{t('detail.media_filename')}</td>
                    <td>{metadata?.media_filename ?? ''}</td>
                  </tr>
                  <tr>
                    <td>{t('common.original_filename')}</td>
                    <td>{metadata?.original_filename ?? ''}</td>
                  </tr>
                  <tr class="align-middle">
                    <td>{t('detail.original_sha256_checksum')}</td>
                    <td><span class="badge bg-light text-dark font-monospace text-wrap text-break text-start">{metadata?.original_checksum ?? ''}</span></td>
                  </tr>
                  <tr>
                    <td>{t('detail.original_file_size')}</td>
                    <td>{fileSize(metadata?.original_size)}</td>
                  </tr>
                  <tr>
                    <td>{t('detail.original_mime_type')}</td>
                    <td>{metadata?.original_mime_type ?? ''}</td>
                  </tr>
                  {#if metadata?.has_archive_version}
                    <tr class="align-middle">
                      <td>{t('detail.archive_sha256_checksum')}</td>
                      <td><span class="badge bg-light font-monospace text-wrap text-break text-start">{metadata?.archive_checksum ?? ''}</span></td>
                    </tr>
                  {/if}
                  {#if metadata?.has_archive_version}
                    <tr>
                      <td>{t('detail.archive_file_size')}</td>
                      <td>{fileSize(metadata?.archive_size)}</td>
                    </tr>
                  {/if}
                </tbody>
              </table>
            {/if}

            {#if (metadata?.barcodes?.length ?? 0) > 0}
              <h6>{t('common.barcodes')}</h6>
              <pngx-document-barcodes><DocumentBarcodes barcodes={metadata?.barcodes ?? []} /></pngx-document-barcodes>
            {/if}
            {#if (metadata?.original_metadata?.length ?? 0) > 0}
              <pngx-metadata-collapse><MetadataCollapse title={t('detail.original_document_metadata')} metadata={metadata?.original_metadata} /></pngx-metadata-collapse>
            {/if}
            {#if (metadata?.archive_metadata?.length ?? 0) > 0}
              <pngx-metadata-collapse><MetadataCollapse title={t('detail.archived_document_metadata')} metadata={metadata?.archive_metadata} /></pngx-metadata-collapse>
            {/if}
        {:else if activeNav === 'preview'}
          {#if !wide}
              <div class="position-relative">
                {@render previewContent()}
              </div>
          {/if}
        {:else if activeNav === 'notes' && notesEnabled && documentId}
              <pngx-document-notes><DocumentNotes {documentId} notes={(doc?.notes ?? []) as DocumentNote[]} addDisabled={!userCanEdit} {dateLocale} onupdated={notesUpdated} /></pngx-document-notes>
        {:else if activeNav === 'history' && historyEnabled && documentId}
              <div class="mb-3">
                <pngx-document-history><DocumentHistory {documentId} {dateLocale} /></pngx-document-history>
              </div>
        {:else if activeNav === 'permissions' && showPermissions && form}
              <div class="mb-3">
                <pngx-permissions-form><PermissionsForm title="" {users} value={form.permissions_form ?? undefined} disabled={formDisabled} onchange={(v) => setField('permissions_form', v)} /></pngx-permissions-form>
              </div>
        {:else if activeNav === 'duplicates' && doc?.duplicate_documents?.length}
              <div class="d-flex flex-column gap-2">
                <div class="fst-italic">{t('detail.duplicate_documents_detected_label')}</div>
                <div class="list-group">
                  {#each doc.duplicate_documents as duplicate (duplicate.id)}
                    <a
                      class="list-group-item list-group-item-action d-flex justify-content-between align-items-center"
                      href={router.href(`documents/${duplicate.id}/details`)}
                      class:disabled={duplicate.deleted_at}
                    >
                      <span class="d-flex align-items-center gap-2">
                        <span>{duplicate.title || '#' + duplicate.id}</span>
                        {#if duplicate.deleted_at}
                          <span class="badge text-bg-secondary">{t('detail.in_trash')}</span>
                        {/if}
                      </span>
                      <span class="text-secondary">#{duplicate.id}</span>
                    </a>
                  {/each}
                </div>
              </div>
        {/if}
        </div>
      </div>

    </form>
  </div>

  <div class="col-md-6 col-xl-7 mb-3 d-none d-md-block position-relative">
    {@render previewContent()}
  </div>

</div>
