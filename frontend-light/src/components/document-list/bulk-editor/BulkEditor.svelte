<!-- Port of document-list/bulk-editor/bulk-editor.component.html/.ts (bar on selection): confirmation dialogs
     (setting "Confirmation dialogs for bulk edit"), apply on close, reprocess, rotate,
     merge (also as versions), permissions, field values, share bundles, email. Selection query as upstream
     ({documents} or {all, filters, excluded_documents}). -->
<script lang="ts">
  import '../../../styles/components/bulk-editor.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import { queryString } from '../../../api/v1'
  import type { CustomField, ManualWorkflow } from '../../../api/v1'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { list, type SelectionData, type SelectionDataItem } from '../../../stores/listView.svelte'
  import { openDocuments } from '../../../stores/openDocuments.svelte'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { FilterableDropdownSelectionModel, ToggleableItemState, flattenTags } from '../../../lib/filter-editor/selection-model'
  import FilterableDropdown from '../../common/filterable-dropdown/FilterableDropdown.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import { modals, type ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { router } from '../../../router.svelte'
  import { queryParamsFromFilterRules } from '../../../lib/query-params'
  import { restoreDocuments } from '../../admin/trash/trash'
  import { loadManualWorkflows } from '../../../lib/manual-workflows'
  import { isImage } from '../../common/convert-pdf/convert-pdf-api'
  import { customFieldsMessage, selectionQuery, singleMessage, tagsMessage, type Named, type SingleKind } from './bulk-editor'

  type ChangedItems = { itemsToAdd: Named[]; itemsToRemove: Named[] }
  type SetEnabled = (b: boolean) => void

  let { disabled = false }: { disabled?: boolean } = $props()

  const ddActions = new Dropdown()
  const ddSend = new Dropdown()
  const ddDownload = new Dropdown({ autoClose: 'outside' })

  const user = $derived(session.data?.user)
  const selectedDocs = $derived(list.documents.filter((d) => list.selected.has(d.id)))
  const ownsObject = (d: { owner: number | null }) => !d || !d.owner || !!user?.is_superuser || d.owner === user?.id
  const userCanEditAll = $derived(session.can('change', 'document') && selectedDocs.every((d) => ownsObject(d) || d.user_can_change))
  const userOwnsAll = $derived(selectedDocs.every(ownsObject))
  const userCanEdit = $derived(session.can('change', 'document'))
  const userCanAdd = $derived(session.can('add', 'document'))
  const userCanDelete = $derived(session.can('delete', 'document'))
  const emailEnabled = $derived(!!session.settings.email_enabled)
  const canSendSelection = $derived(list.hasSelection && (!list.allSelected || list.selectedCount === list.selected.size))
  const applyOnClose = $derived(!!session.get(SETTINGS_KEYS.BULK_EDIT_APPLY_ON_CLOSE))
  const showConfirmationDialogs = $derived(!!session.get(SETTINGS_KEYS.BULK_EDIT_CONFIRMATION_DIALOGS))
  let awaitingDownload = $state(false)
  let dlArchive = $state(true)
  let dlOriginals = $state(false)
  let dlFormatting = $state(false)
  // upstream (ngOnInit): deselecting one file kind selects the other again
  function setDlArchive(v: boolean) {
    dlArchive = v
    if (!v) dlOriginals = true
  }
  function setDlOriginals(v: boolean) {
    dlOriginals = v
    if (!v) dlArchive = true
  }

  // Selection models as in the bulk editor (editing = true); tags/fields multiple (manyToOne = true), otherwise one
  function makeModel(manyToOne: boolean, items: () => any[]) {
    const m = new FilterableDropdownSelectionModel(manyToOne, { t })
    m.editing = true
    m.items = items()
    return m
  }
  const tagModel = makeModel(true, () => flattenTags([...data.tags.values()]))
  const correspondentModel = makeModel(false, () => [...data.correspondents.values()])
  const documentTypeModel = makeModel(false, () => [...data.documentTypes.values()])
  const storagePathModel = makeModel(false, () => [...data.storagePaths.values()])
  const customFieldModel = makeModel(true, () => data.customFields)
  $effect(() => {
    tagModel.items = flattenTags([...data.tags.values()])
    correspondentModel.items = [...data.correspondents.values()]
    documentTypeModel.items = [...data.documentTypes.values()]
    storagePathModel.items = [...data.storagePaths.values()]
    customFieldModel.items = data.customFields
  })

  type Kind = 'tags' | 'correspondents' | 'document_types' | 'storage_paths' | 'custom_fields'

  // createTag/-Correspondent/-DocumentType/-StoragePath/-CustomField: create dialog (loaded lazily) with the name,
  // then reload the list and select the new object in the selection model. Offered only with create permission.
  const DIALOGS = {
    tags: ['tag', () => import('../../common/edit-dialog/tag-edit-dialog/TagEditDialog.svelte')],
    correspondents: ['correspondent', () => import('../../common/edit-dialog/correspondent-edit-dialog/CorrespondentEditDialog.svelte')],
    document_types: ['documenttype', () => import('../../common/edit-dialog/document-type-edit-dialog/DocumentTypeEditDialog.svelte')],
    storage_paths: ['storagepath', () => import('../../common/edit-dialog/storage-path-edit-dialog/StoragePathEditDialog.svelte')],
    custom_fields: ['customfield', () => import('../../common/edit-dialog/custom-field-edit-dialog/CustomFieldEditDialog.svelte')],
  } as const
  const byId = <T extends { id: number }>(xs: T[]) => new Map(xs.map((x) => [x.id, x]))
  function creator(kind: Kind, model: FilterableDropdownSelectionModel) {
    if (!session.can('add', DIALOGS[kind][0])) return undefined
    return async (name: string) => {
      const C = (await DIALOGS[kind][1]()).default
      return modals.open(C, {
        dialogMode: 'create',
        object: { name },
        onsucceeded: async (o: { id: number }) => {
          try {
            const r = await api.listAll<any>(kind)
            if (kind === 'tags') data.tags = byId(r.results)
            else if (kind === 'correspondents') data.correspondents = byId(r.results)
            else if (kind === 'document_types') data.documentTypes = byId(r.results)
            else if (kind === 'storage_paths') data.storagePaths = byId(r.results)
            else data.customFields = r.results
            model.items = kind === 'tags' ? flattenTags(r.results) : r.results
          } catch {
            // List stays; the new object arrives with the next load
          }
          model.toggle(o.id)
        },
      }, { backdrop: 'static' })
    }
  }
  let documentCounts = $state<Partial<Record<Kind, SelectionDataItem[]>>>({})
  const getSelectionQuery = () => selectionQuery(list)

  /** applySelectionData */
  function applySelectionData(items: SelectionDataItem[], model: FilterableDropdownSelectionModel) {
    const map = new Map<number, ToggleableItemState>()
    for (const i of items) {
      if (i.document_count == list.selectedCount) map.set(i.id, ToggleableItemState.Selected)
      else if (i.document_count > 0) map.set(i.id, ToggleableItemState.PartiallySelected)
    }
    model.init(map)
  }
  /** open…Dropdown: everything selected without exceptions → selection data of the list, otherwise query */
  async function opened(kind: Kind, model: FilterableDropdownSelectionModel) {
    let items: SelectionDataItem[] | undefined
    if (list.allSelected && list.excluded.size === 0) {
      items = (list.selectionData as Record<string, any> | null)?.[`selected_${kind}`] ?? []
    } else {
      try {
        const r = await api.selectionData<SelectionData>(getSelectionQuery())
        items = (r as Record<string, any>)[`selected_${kind}`]
      } catch {
        return
      }
    }
    documentCounts = { ...documentCounts, [kind]: items ?? [] }
    applySelectionData(items ?? [], model)
  }

  /** DocumentListViewService.reduceSelectionToFilter (private there): keep only IDs that still match the filter */
  function reduceSelectionToFilter() {
    if (list.allSelected) {
      list.excluded = new Set()
      return
    }
    if (list.selected.size === 0) return
    const params = queryParamsFromFilterRules(list.filterRules) as Record<string, string | number | boolean | null | undefined>
    api
      .get<{ results: { id: number }[] }>(`/api/documents/${queryString({ page: 1, page_size: 100000, fields: 'id', ...params })}`)
      .then((r) => {
        const ids = new Set(r.results.map((x) => x.id))
        list.selected = new Set([...list.selected].filter((id) => ids.has(id)))
      })
      .catch(() => {})
  }
  /** handleOperationSuccess */
  function handleOperationSuccess(ref: ModalRef<any> | null, clearSelection = false) {
    if (clearSelection) list.selectNone()
    list.reload()
    reduceSelectionToFilter()
    list.selected.forEach((id) => openDocuments.refreshDocument(id))
    data.refreshViewCounts()
    ref?.close()
  }
  /** executeDocumentAction / executeBulkEditMethod (+ handleOperationError) */
  async function execute(
    ref: ModalRef<any> | null,
    setEnabled: SetEnabled | null,
    request: () => Promise<unknown>,
    options: { clearSelection?: boolean; successMessage?: string } = {},
  ) {
    setEnabled?.(false)
    try {
      await request()
    } catch (e) {
      setEnabled?.(true)
      toastService.showError(t('common.error_executing_bulk_operation'), e)
      return
    }
    handleOperationSuccess(ref, options.clearSelection ?? false)
    if (options.successMessage) toastService.showSuccess(options.successMessage)
  }
  const bulk = (ref: ModalRef<any> | null, setEnabled: SetEnabled | null, method: string, parameters: Record<string, unknown>) =>
    execute(ref, setEnabled, () => api.bulkEdit(getSelectionQuery(), method, parameters))

  /** Confirmation dialog (btn-warning, "Confirm") – or, with the setting switched off, execute directly */
  function confirmThen(title: string, message: string, method: string, parameters: Record<string, unknown>) {
    if (!showConfirmationDialogs) {
      void bulk(null, null, method, parameters)
      return
    }
    const ref = modals.open(
      ConfirmDialog,
      {
        title,
        message,
        btnClass: 'btn-warning',
        btnCaption: t('common.confirm'),
        onconfirm: (setEnabled: SetEnabled) => bulk(ref, setEnabled, method, parameters),
      },
      { backdrop: 'static' },
    )
  }

  function setTags(changed: ChangedItems) {
    if (changed.itemsToAdd.length == 0 && changed.itemsToRemove.length == 0) return
    confirmThen(t('bulk.confirm_tags'), tagsMessage(changed.itemsToAdd, changed.itemsToRemove, list.selectedCount), 'modify_tags', {
      add_tags: changed.itemsToAdd.map((i) => i.id),
      remove_tags: changed.itemsToRemove.map((i) => i.id),
    })
  }
  /** setCorrespondents / setDocumentTypes / setStoragePaths */
  const single = (kind: SingleKind, method: string) => (changed: ChangedItems) => {
    if (changed.itemsToAdd.length == 0 && changed.itemsToRemove.length == 0) return
    const item = changed.itemsToAdd.length > 0 ? changed.itemsToAdd[0] : null
    const { title, message } = singleMessage(kind, item, list.selectedCount)
    confirmThen(title, message, method, { [kind]: item ? item.id : null })
  }
  function setCustomFields(changed: ChangedItems) {
    if (changed.itemsToAdd.length == 0 && changed.itemsToRemove.length == 0) return
    confirmThen(t('bulk.confirm_custom_fields'), customFieldsMessage(changed.itemsToAdd, changed.itemsToRemove, list.selectedCount), 'modify_custom_fields', {
      add_custom_fields: changed.itemsToAdd.map((i) => i.id),
      remove_custom_fields: changed.itemsToRemove.map((i) => i.id),
    })
  }

  /** setCustomFieldValues */
  async function setCustomFieldValues(changed: ChangedItems) {
    const C = (await import('./custom-fields-bulk-edit-dialog/CustomFieldsBulkEditDialog.svelte')).default
    modals.open(
      C,
      {
        customFields: (customFieldModel.items as CustomField[]).filter((f) => f.id !== null),
        fieldsToAddIds: changed.itemsToAdd.map((i) => i.id),
        fieldsToRemoveIds: changed.itemsToRemove.map((i) => i.id),
        selection: getSelectionQuery(),
        selectionCount: list.selectedCount,
        onsucceeded: () => {
          toastService.showSuccess(t('bulk.custom_fields_saved'))
          list.reload()
          reduceSelectionToFilter()
          list.selected.forEach((id) => openDocuments.refreshDocument(id))
        },
        onfailed: (e: unknown) => toastService.showError(t('bulk.custom_fields_save_failed'), e),
      },
      { backdrop: 'static', size: 'lg' },
    )
  }

  // Manual workflow run (PaperRust; idea from paperless-ngx dev #14704): menu item only if there are active
  // workflows with trigger "Manual". Only for owners of all selected documents (idea from paperless-ngx
  // dev #14762): workflow actions can change owner/permissions; the server checks the same.
  let manualWorkflows = $state<ManualWorkflow[]>([])
  $effect(() => {
    void loadManualWorkflows(session.can('view', 'workflow')).then((w) => (manualWorkflows = w))
  })
  const canRunWorkflow = $derived(userCanEdit && userOwnsAll && manualWorkflows.length > 0)
  async function runWorkflowSelected() {
    ddActions.close()
    const C = (await import('../../common/confirm-dialog/run-workflow-confirm-dialog/RunWorkflowConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('common.run_workflow_2'),
        message: t('common.run_workflow_selected_document', { PH: list.selectedCount }),
        btnCaption: t('common.run'),
        workflows: manualWorkflows,
        onconfirm: async (setEnabled: (b: boolean) => void, workflowId: number, force: boolean) => {
          setEnabled(false)
          try {
            await api.runWorkflow([...list.selected], workflowId, !force)
            ref.close()
            list.reload()
          } catch (e) {
            setEnabled(true)
            toastService.showError(t('common.error_executing_bulk_operation'), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
  async function downloadSelected() {
    awaitingDownload = true
    try {
      const content = dlArchive && dlOriginals ? 'both' : dlArchive ? 'archive' : 'originals'
      const res = await fetch('/api/documents/bulk_download/', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-CSRFToken': document.cookie.match(/csrftoken=([^;]+)/)?.[1] ?? '' },
        body: JSON.stringify({ ...getSelectionQuery(), content, follow_formatting: dlFormatting }),
      })
      const blob = await res.blob()
      const a = document.createElement('a')
      a.href = URL.createObjectURL(blob)
      a.download = 'documents.zip'
      a.click()
      URL.revokeObjectURL(a.href)
    } finally {
      awaitingDownload = false
    }
  }

  /** applyDelete + deleteSelected (TrashService.notifyMovedToTrash with "Undo") */
  function applyDelete() {
    const run = (ref: ModalRef<any> | null, setEnabled: SetEnabled | null) => {
      const documents = list.allSelected ? null : Array.from(list.selected)
      const count = list.selectedCount
      return execute(ref, setEnabled, async () => {
        await api.deleteDocuments(getSelectionQuery())
        const canUndo = !!documents && documents.length > 0
        toastService.show({
          content: t('bulk.moved_to_trash', { PH: count }),
          delay: 10000,
          level: 'success',
          actionName: canUndo ? t('detail.undo') : undefined,
          action: canUndo
            ? () =>
                restoreDocuments(documents).then(
                  () => {
                    toastService.show({ content: t('common.document_s_restored'), delay: 5000, level: 'success' })
                    list.reload()
                  },
                  (e) => toastService.showError(t('common.error_restoring_document_s'), e),
                )
            : undefined,
        })
      })
    }
    if (!showConfirmationDialogs) {
      void run(null, null)
      return
    }
    const ref = modals.open(
      ConfirmDialog,
      {
        title: t('common.confirm'),
        messageBold: t('common.move_selected_document_s', { PH: list.selectedCount }),
        message: t('detail.documents_restored_prior_permanent'),
        btnClass: 'btn-danger',
        btnCaption: t('common.move_trash'),
        onconfirm: (setEnabled: SetEnabled) => run(ref, setEnabled),
      },
      { backdrop: 'static' },
    )
  }

  /** reprocessSelected */
  async function reprocessSelected() {
    ddActions.close()
    const C = (await import('../../common/confirm-dialog/reprocess-confirm-dialog/ReprocessConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('detail.reprocess_confirm'),
        messageBold: t('bulk.reprocess_confirm', { PH: list.selectedCount }),
        message: t('bulk.reprocess_message'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: (setEnabled: SetEnabled, remoteOcr: boolean, aiOcr: boolean) => execute(ref, setEnabled, () => api.reprocessSelection(getSelectionQuery(), remoteOcr, aiOcr)),
      },
      { backdrop: 'static' },
    )
  }

  /** rotateSelected */
  async function rotateSelected() {
    ddActions.close()
    const C = (await import('../../common/confirm-dialog/rotate-confirm-dialog/RotateConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('bulk.rotate_title'),
        messageBold: t('bulk.rotate_confirm', { PH: list.selectedCount }),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        documentID: Array.from(list.selected)[0],
        onconfirm: (setEnabled: SetEnabled, degrees: number) => execute(ref, setEnabled, () => api.rotateDocuments(getSelectionQuery(), degrees)),
      },
      { backdrop: 'static', size: 'lg' },
    )
  }

  /** mergeSelected */
  async function mergeSelected() {
    ddActions.close()
    const C = (await import('../../common/confirm-dialog/merge-confirm-dialog/MergeConfirmDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        title: t('bulk.merge_title'),
        messageBold: t('bulk.merge_confirm', { PH: list.selectedCount }),
        btnCaption: t('common.proceed'),
        documentIDs: Array.from(list.selected),
        onconfirm: (setEnabled: SetEnabled, r: { documentIDs: number[]; metadataDocumentID: number; archiveFallback: boolean; deleteOriginals: boolean }) => {
          const args: { metadata_document_id?: number; delete_originals?: boolean; archive_fallback?: boolean } = {}
          if (r.metadataDocumentID > -1) args.metadata_document_id = r.metadataDocumentID
          if (r.deleteOriginals) args.delete_originals = true
          if (r.archiveFallback) args.archive_fallback = true
          void execute(ref, setEnabled, () => api.mergeDocuments(r.documentIDs, args), { clearSelection: !!args.delete_originals })
          toastService.showInfo(t('bulk.merge_queued'))
        },
      },
      { backdrop: 'static' },
    )
  }

  /** PaperRust: convert to PDF (image/office → one new PDF version each, common/convert-pdf/) */
  async function convertSelectedToPdf() {
    ddActions.close()
    const C = (await import('../../common/convert-pdf/ConvertPdfDialog.svelte')).default
    const ids = Array.from(list.selected)
    const known = selectedDocs.filter((d) => ids.includes(d.id))
    // Show the OCR option as soon as an image is included (or the type is unknown)
    const showOcr = known.length < ids.length || known.some((d) => isImage(d.mime_type))
    modals.open(C, { documentIds: ids, showOcr, onqueued: (done: number[]) => done.forEach((id) => openDocuments.refreshDocument(id)) }, { backdrop: 'static' })
  }

  /** mergeSelectedAsVersions */
  async function mergeSelectedAsVersions() {
    ddActions.close()
    const C = (await import('../../common/confirm-dialog/merge-as-versions-confirm-dialog/MergeAsVersionsConfirmDialog.svelte')).default
    const documentIDs = Array.from(list.selected)
    const ref = modals.open(
      C,
      {
        title: t('list.merge_versions'),
        message: t('bulk.merge_versions_message'),
        btnCaption: t('common.proceed'),
        documentIDs,
        rootDocumentID: documentIDs[0],
        onconfirm: (setEnabled: SetEnabled, ids: number[], rootId: number) =>
          execute(ref, setEnabled, () => api.mergeDocumentsAsVersions(ids, rootId), { clearSelection: true, successMessage: t('bulk.merged_as_versions') }),
      },
      { backdrop: 'static' },
    )
  }

  /** setPermissions */
  async function setPermissions() {
    const C = (await import('../../common/permissions-dialog/PermissionsDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        onconfirm: ({ permissions, merge }: { permissions: object; merge: boolean }, setEnabled: SetEnabled) =>
          bulk(ref, setEnabled, 'set_permissions', { ...permissions, merge }),
      },
      { backdrop: 'static' },
    )
  }

  /** createShareLinkBundle (create + toasts in the dialog) */
  async function createShareLinkBundle() {
    ddSend.close()
    const C = (await import('../../common/share-link-bundle-dialog/ShareLinkBundleDialog.svelte')).default
    modals.open(C, { documents: list.documents.filter((d) => list.selected.has(d.id)), onmanage: manageShareLinkBundles }, { backdrop: 'static', size: 'lg' })
  }
  /** manageShareLinkBundles */
  function manageShareLinkBundles() {
    ddSend.close()
    router.navigate('share-links?type=bundles')
  }
  /** emailSelected */
  async function emailSelected() {
    ddSend.close()
    const allHaveArchiveVersion = list.documents.filter((d) => list.selected.has(d.id)).every((doc) => !!doc.archived_file_name)
    const C = (await import('../../common/email-document-dialog/EmailDocumentDialog.svelte')).default
    modals.open(C, { documentIds: Array.from(list.selected), hasArchiveVersion: allHaveArchiveVersion }, { backdrop: 'static' })
  }
</script>

<div class="d-flex flex-wrap gap-4">
  {#if session.can('change', 'document')}
  <div class="d-flex flex-wrap align-items-center gap-2">
    <label class="me-2">{t('list.edit_label')}</label>
    {#if session.can('view', 'tag')}
      <pngx-filterable-dropdown title="Tags" filterPlaceholder="Filter tags" shortcutKey="t"><FilterableDropdown title={t('common.tags')} icon="tag-fill" name="tags"
        filterPlaceholder={t('list.filter_tags')}
        disabled={!userCanEditAll || disabled}
        editing
        {applyOnClose}
        model={tagModel}
        shortcutKey="t"
        createRef={creator('tags', tagModel)}
        documentCounts={documentCounts.tags}
        onopened={() => opened('tags', tagModel)}
        onapply={setTags} /></pngx-filterable-dropdown>
    {/if}
    {#if session.can('view', 'correspondent')}
      <pngx-filterable-dropdown title="Correspondent" filterPlaceholder="Filter correspondents" shortcutKey="y"><FilterableDropdown title={t('common.correspondent')} icon="person-fill" name="correspondent"
        filterPlaceholder={t('list.filter_correspondents')}
        disabled={!userCanEditAll || disabled}
        editing
        {applyOnClose}
        model={correspondentModel}
        shortcutKey="y"
        createRef={creator('correspondents', correspondentModel)}
        documentCounts={documentCounts.correspondents}
        onopened={() => opened('correspondents', correspondentModel)}
        onapply={single('correspondent', 'set_correspondent')} /></pngx-filterable-dropdown>
    {/if}
    {#if session.can('view', 'documenttype')}
      <pngx-filterable-dropdown title="Document type" filterPlaceholder="Filter document types" shortcutKey="u"><FilterableDropdown title={t('common.document_type')} icon="file-earmark-fill" name="document_type"
        filterPlaceholder={t('list.filter_document_types')}
        disabled={!userCanEditAll || disabled}
        editing
        {applyOnClose}
        model={documentTypeModel}
        shortcutKey="u"
        createRef={creator('document_types', documentTypeModel)}
        documentCounts={documentCounts.document_types}
        onopened={() => opened('document_types', documentTypeModel)}
        onapply={single('document_type', 'set_document_type')} /></pngx-filterable-dropdown>
    {/if}
    {#if session.can('view', 'storagepath')}
      <pngx-filterable-dropdown title="Storage path" filterPlaceholder="Filter storage paths" shortcutKey="i"><FilterableDropdown title={t('common.storage_path')} icon="folder-fill" name="storage_path"
        filterPlaceholder={t('list.filter_storage_paths')}
        disabled={!userCanEditAll || disabled}
        editing
        {applyOnClose}
        model={storagePathModel}
        shortcutKey="i"
        createRef={creator('storage_paths', storagePathModel)}
        documentCounts={documentCounts.storage_paths}
        onopened={() => opened('storage_paths', storagePathModel)}
        onapply={single('storage_path', 'set_storage_path')} /></pngx-filterable-dropdown>
    {/if}
    {#if session.can('view', 'customfield')}
      <pngx-filterable-dropdown title="Custom fields" filterPlaceholder="Filter custom fields" extraButtonTitle="Set values"><FilterableDropdown title={t('common.custom_fields')} icon="ui-radios" name="custom_fields"
        filterPlaceholder={t('list.filter_custom_fields')}
        disabled={!userCanEditAll}
        editing
        {applyOnClose}
        model={customFieldModel}
        createRef={creator('custom_fields', customFieldModel)}
        documentCounts={documentCounts.custom_fields}
        extraButtonTitle={t('list.set_values')}
        onextrabutton={setCustomFieldValues}
        onopened={() => opened('custom_fields', customFieldModel)}
        onapply={setCustomFields} /></pngx-filterable-dropdown>
    {/if}
    <div class="btn-group">
      <button type="button" class="btn btn-sm btn-outline-primary me-2" onclick={setPermissions} disabled={!userOwnsAll || !userCanEditAll} aria-label={t('common.permissions')}>
        <pngx-bs-icon name="person-fill-lock">{@html icon('person-fill-lock')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.permissions')}</div>
      </button>
    </div>
  </div>
  {/if}
  <div class="d-flex align-items-center gap-2 ms-auto">
    <div class="btn-toolbar">
      <div class={ddActions.placementClass} class:show={ddActions.open} use:ddActions.hostEl>
        <button class="dropdown-toggle btn btn-sm btn-outline-primary" id="dropdownSelect" disabled={!userCanEdit && !userCanAdd} aria-label={t('common.actions')} aria-expanded={ddActions.open} use:ddActions.anchorEl onclick={ddActions.toggle}>
          <pngx-bs-icon name="three-dots">{@html icon('three-dots')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.actions')}</div>
        </button>
        {#if ddActions.open}
          <div aria-labelledby="dropdownSelect" class="dropdown-menu shadow show" use:ddActions.menuEl>
            <button class="dropdown-item" tabindex="0" onclick={reprocessSelected} disabled={!userCanEditAll && !userCanEditAll}>
              <pngx-bs-icon name="body-text" class="me-2">{@html icon('body-text')}</pngx-bs-icon>{t('common.reprocess')}
            </button>
            <button class="dropdown-item" tabindex="0" onclick={rotateSelected} disabled={!userOwnsAll && !userCanEditAll}>
              <pngx-bs-icon name="arrow-clockwise" class="me-2">{@html icon('arrow-clockwise')}</pngx-bs-icon>{t('list.rotate')}
            </button>
            <button class="dropdown-item" tabindex="0" onclick={mergeSelected} disabled={!userCanAdd || list.allSelected || list.selectedCount < 2}>
              <pngx-bs-icon name="journals" class="me-2">{@html icon('journals')}</pngx-bs-icon>{t('list.merge')}
            </button>
            <button class="dropdown-item" tabindex="0" onclick={mergeSelectedAsVersions} disabled={!userOwnsAll || !userCanEditAll || !userCanDelete || list.allSelected || list.selectedCount < 2}>
              <pngx-bs-icon name="journal-bookmark-fill" class="me-2">{@html icon('journal-bookmark-fill')}</pngx-bs-icon>{t('list.merge_versions')}
            </button>
            <button class="dropdown-item" tabindex="0" onclick={convertSelectedToPdf} disabled={!userOwnsAll || !userCanEditAll || list.allSelected || list.selectedCount < 1}>
              <pngx-bs-icon name="file-earmark-pdf" class="me-2">{@html icon('file-earmark-pdf')}</pngx-bs-icon>{t('convert.menu')}
            </button>
            {#if manualWorkflows.length > 0}
              <button class="dropdown-item" tabindex="0" onclick={runWorkflowSelected} disabled={!canRunWorkflow} id="runWorkflowSelected">
                <pngx-bs-icon name="play-fill" class="me-2">{@html icon('play-fill')}</pngx-bs-icon>{t('common.run_workflow_2')}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    </div>
    <div class="btn-toolbar {ddSend.placementClass}" class:show={ddSend.open} use:ddSend.hostEl>
      <button
        class="dropdown-toggle btn btn-sm btn-outline-primary"
        id="dropdownSend"
        disabled={disabled || !canSendSelection}
        aria-label={t('common.send')}
        aria-expanded={ddSend.open}
        use:ddSend.anchorEl
        onclick={ddSend.toggle}
      >
        <pngx-bs-icon name="send">{@html icon('send')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.send')}
        </div>
      </button>
      {#if ddSend.open}
        <div aria-labelledby="dropdownSend" class="dropdown-menu shadow show" use:ddSend.menuEl>
          {#if session.can('add', 'sharelinkbundle')}
            <button class="dropdown-item" tabindex="0" onclick={createShareLinkBundle} disabled={!canSendSelection}>
              <pngx-bs-icon name="link" class="me-2">{@html icon('link')}</pngx-bs-icon>{t('list.create_share_link_bundle')}
            </button>
          {/if}
          {#if session.can('view', 'sharelinkbundle')}
            <button class="dropdown-item" tabindex="0" onclick={manageShareLinkBundles}>
              <pngx-bs-icon name="list-ul" class="me-2">{@html icon('list-ul')}</pngx-bs-icon>{t('list.manage_share_link_bundles')}
            </button>
          {/if}
          {#if emailEnabled && (session.can('add', 'sharelinkbundle') || session.can('view', 'sharelinkbundle'))}
            <div class="dropdown-divider"></div>
          {/if}
          {#if emailEnabled}
            <button class="dropdown-item" tabindex="0" onclick={emailSelected} disabled={!canSendSelection}>
              <pngx-bs-icon name="envelope" class="me-2">{@html icon('envelope')}</pngx-bs-icon>{t('common.email')}
            </button>
          {/if}
        </div>
      {/if}
    </div>
    <div class="btn-group btn-group-sm">
      <button class="btn btn-sm btn-outline-primary" disabled={awaitingDownload} onclick={downloadSelected} aria-label={t('common.download')}>
        {#if !awaitingDownload}
          <pngx-bs-icon name="arrow-down">{@html icon('arrow-down')}</pngx-bs-icon>
        {/if}
        {#if awaitingDownload}
          <div class="spinner-border spinner-border-sm" role="status">
            <span class="visually-hidden">Preparing download...</span>
          </div>
        {/if}
        <div class="d-none d-sm-inline ms-1">{t('common.download')}</div>
      </button>
      <div class="me-2 d-flex btn-group {ddDownload.placementClass}" class:show={ddDownload.open} role="group" use:ddDownload.hostEl>
        <button type="button" class="dropdown-toggle btn btn-sm btn-outline-primary dropdown-toggle-split rounded-end" aria-label={t('common.download_options')} aria-expanded={ddDownload.open} use:ddDownload.anchorEl onclick={ddDownload.toggle}></button>
        {#if ddDownload.open}
          <div aria-labelledby="dropdownSelect" class="dropdown-menu shadow show" use:ddDownload.menuEl>
            <form class="px-3 py-1">
              <p class="mb-1">{t('list.include_label')}</p>
              <div class="form-group ps-3 mb-2">
                <div class="form-check">
                  <input type="checkbox" class="form-check-input" id="downloadFileType_archive" checked={dlArchive} onchange={(e) => setDlArchive(e.currentTarget.checked)} />
                  <label class="form-check-label" for="downloadFileType_archive">{t('list.archived_files')}</label>
                </div>
                <div class="form-check">
                  <input type="checkbox" class="form-check-input" id="downloadFileType_originals" checked={dlOriginals} onchange={(e) => setDlOriginals(e.currentTarget.checked)} />
                  <label class="form-check-label" for="downloadFileType_originals">{t('list.original_files')}</label>
                </div>
              </div>
              <div class="form-check">
                <input type="checkbox" class="form-check-input" id="downloadUseFormatting" bind:checked={dlFormatting} />
                <label class="form-check-label" for="downloadUseFormatting">{t('common.use_formatted_filename')}</label>
              </div>
            </form>
          </div>
        {/if}
      </div>
    </div>

    <div class="btn-group btn-group-sm">
      {#if session.can('delete', 'document')}
        <button type="button" class="btn btn-sm btn-outline-danger" onclick={applyDelete} disabled={!userOwnsAll}>
          <pngx-bs-icon name="trash" class="me-1">{@html icon('trash')}</pngx-bs-icon>{t('common.delete')}
        </button>
      {/if}
    </div>
  </div>
</div>
