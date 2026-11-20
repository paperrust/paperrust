<!-- Port of document-detail/document-version-dropdown (Host <pngx-document-version-dropdown> in the parent markup).
     Choose version, upload new version (POST /api/documents/<id>/update_version/, then wait for the
     WebSocket result of the task like Angular), existing document as version (merge_as_versions, dialog is
     loaded lazily), change label (PATCH …/versions/<v>/) and delete version (DELETE …/versions/<v>/). -->
<script lang="ts">
  import '../../../styles/components/document-version-dropdown.css'
  import { onDestroy, untrack } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { formatDate } from '../../../lib/dates'
  import { modals } from '../../../lib/modal.svelte'
  import { api } from '../../../api/client'
  import type { DocumentVersionInfo } from '../../../api/v1'
  import { session } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { consumerStatus } from '../../../stores/uploads.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import { UploadState, taskIdFrom, nextVersionLabel } from './versions'

  let {
    documentId,
    versions = [],
    selectedVersionId,
    userIsOwner = false,
    userCanEdit = false,
    dateLocale,
    onversionselected,
    onversionsupdated,
  }: {
    documentId: number | undefined
    versions?: DocumentVersionInfo[]
    selectedVersionId: number | undefined
    userIsOwner?: boolean
    userCanEdit?: boolean
    dateLocale: string
    onversionselected?: (id: number) => void
    onversionsupdated?: (versions: DocumentVersionInfo[]) => void
  } = $props()
  const dd = new Dropdown({ autoClose: 'outside' })
  let newVersionLabel = $state('')
  let versionUploadState = $state(UploadState.Idle)
  let versionUploadError = $state<string | null>(null)
  let savingVersionLabelId = $state<number | null>(null)
  let editingVersionId = $state<number | null>(null)
  let versionLabelDraft = $state('')
  let versionFileInput = $state<HTMLInputElement | undefined>(undefined)
  const canEditLabels = $derived(userIsOwner && userCanEdit)
  let destroyed = false

  // ngOnChanges(documentId) → documentChange$: end running subscriptions, reset status
  let documentChangeCleanups: (() => void)[] = []
  function documentChanged() {
    for (const c of documentChangeCleanups) c()
    documentChangeCleanups = []
  }
  let lastDocumentId: number | undefined = untrack(() => documentId)
  $effect(() => {
    const id = documentId
    untrack(() => {
      if (id === lastDocumentId) return
      lastDocumentId = id
      documentChanged()
      clearVersionUploadStatus()
      cancelEditingVersion()
    })
  })
  onDestroy(() => {
    destroyed = true
    documentChanged()
  })

  const selectVersion = (versionId: number) => onversionselected?.(versionId)
  const isEditingVersion = (versionId: number) => editingVersionId === versionId

  function beginEditingVersion(version: DocumentVersionInfo, e?: Event) {
    e?.preventDefault()
    e?.stopPropagation()
    if (!canEditLabels || savingVersionLabelId !== null) return
    editingVersionId = version.id
    versionLabelDraft = version.version_label ?? ''
  }
  function cancelEditingVersion(e?: Event) {
    e?.preventDefault()
    e?.stopPropagation()
    editingVersionId = null
    versionLabelDraft = ''
  }
  function submitEditedVersionLabel(version: DocumentVersionInfo, e?: Event) {
    e?.preventDefault()
    e?.stopPropagation()
    if (savingVersionLabelId !== null) return
    const next = nextVersionLabel(versionLabelDraft, version.version_label)
    if (next === undefined) {
      cancelEditingVersion()
      return
    }
    saveVersionLabel(version.id, next)
    cancelEditingVersion()
  }

  async function deleteVersion(versionId: number) {
    const docId = documentId
    if (docId === undefined) return
    const wasSelected = selectedVersionId === versionId
    try {
      const result = await api.deleteVersion(docId, versionId)
      const doc = await api.getVersions(docId)
      if (destroyed) return
      if (doc?.versions) onversionsupdated?.(doc.versions)
      if (wasSelected || selectedVersionId === versionId) onversionselected?.(result?.current_version_id ?? docId)
    } catch (e) {
      toastService.showError(t('detail.versions.delete_error'), e)
    }
  }

  async function saveVersionLabel(versionId: number, versionLabel: string | null) {
    const docId = documentId
    if (savingVersionLabelId !== null || docId === undefined) return
    savingVersionLabelId = versionId
    try {
      const updated = await api.updateVersionLabel(docId, versionId, versionLabel)
      onversionsupdated?.(versions.map((v) => (v.id === versionId ? { ...v, version_label: updated.version_label } : v)))
    } catch (e) {
      toastService.showError(t('detail.versions.label_error'), e)
    } finally {
      if (savingVersionLabelId === versionId) savingVersionLabelId = null
    }
  }

  /** Wait for the end of processing (onDocumentConsumptionFinished/Failed, filtered by task ID) */
  function waitForTask(taskId: string): Promise<{ state: 'success' } | { state: 'failed'; message: string } | null> {
    return new Promise((resolve) => {
      let done = false
      const finish = (r: { state: 'success' } | { state: 'failed'; message: string } | null) => {
        if (done) return
        done = true
        offFinished()
        offFailed()
        documentChangeCleanups = documentChangeCleanups.filter((c) => c !== cancel)
        resolve(r)
      }
      const offFinished = consumerStatus.onDocumentConsumptionFinished((s) => s.taskId === taskId && finish({ state: 'success' }))
      const offFailed = consumerStatus.onDocumentConsumptionFailed((s) => s.taskId === taskId && finish({ state: 'failed', message: s.message }))
      const cancel = () => finish(null)
      documentChangeCleanups.push(cancel)
    })
  }

  async function onVersionFileSelected(e: Event) {
    const input = e.target as HTMLInputElement
    if (!input?.files || input.files.length === 0 || documentId === undefined) return
    const uploadDocumentId = documentId
    const file = input.files[0]
    input.value = ''
    const label = newVersionLabel?.trim()
    versionUploadState = UploadState.Uploading
    versionUploadError = null
    try {
      const body = await api.uploadVersion(uploadDocumentId, file, label)
      toastService.showInfo(t('detail.versions.upload_started'))
      newVersionLabel = ''
      versionUploadState = UploadState.Processing
      const taskId = taskIdFrom(body)
      if (!taskId) {
        versionUploadState = UploadState.Failed
        versionUploadError = t('detail.versions.missing_task_id')
        return
      }
      const result = await waitForTask(taskId)
      if (destroyed || uploadDocumentId !== documentId) return
      if (result?.state !== 'success') {
        if (result?.state === 'failed') {
          versionUploadState = UploadState.Failed
          versionUploadError = result.message || t('detail.versions.upload_failed')
        }
        return
      }
      const doc = await api.getVersions(uploadDocumentId)
      if (destroyed || uploadDocumentId !== documentId) return
      if (doc?.versions?.length) {
        onversionsupdated?.(doc.versions)
        // The API returns versions newest first
        onversionselected?.(doc.versions[0].id)
        clearVersionUploadStatus()
      }
    } catch (err: any) {
      if (destroyed || uploadDocumentId !== documentId) return
      versionUploadState = UploadState.Failed
      versionUploadError = err?.message || t('detail.versions.upload_failed')
      toastService.showError(t('detail.versions.upload_error'), err)
    }
  }

  async function addExistingDocumentAsVersion() {
    const docId = documentId
    if (docId === undefined) return
    const C = (await import('./add-existing-document-version-dialog/AddExistingDocumentVersionDialog.svelte')).default
    const ref = modals.open(
      C,
      {
        rootDocumentID: docId,
        onconfirm: async (setEnabled: (b: boolean) => void, existingDocumentID: number) => {
          setEnabled(false)
          const versionLabel = newVersionLabel?.trim()
          try {
            await api.mergeDocumentsAsVersions([docId, existingDocumentID], docId, versionLabel)
            const document = await api.getVersions(docId)
            if (destroyed || docId !== documentId) return
            if (document?.versions?.length) {
              onversionsupdated?.(document.versions)
              // The API returns versions newest first
              onversionselected?.(document.versions[0].id)
            }
            newVersionLabel = ''
            ref.close()
            toastService.showSuccess(t('detail.versions.existing_added'))
          } catch (e) {
            toastService.showError(t('detail.versions.existing_add_error'), e)
          } finally {
            setEnabled(true)
          }
        },
      },
      { backdrop: 'static' },
    )
    // documentChange$ ends the subscription in Angular; here: close the dialog
    const cancel = () => ref.dismiss()
    documentChangeCleanups.push(cancel)
    ref.result.finally(() => (documentChangeCleanups = documentChangeCleanups.filter((c) => c !== cancel)))
  }

  function clearVersionUploadStatus() {
    versionUploadState = UploadState.Idle
    versionUploadError = null
  }
</script>

<div class="btn-group {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
  <button class="btn btn-sm btn-outline-secondary dropdown-toggle" aria-label={t('detail.versions')} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon name="file-earmark-diff">{@html icon('file-earmark-diff')}</pngx-bs-icon>
    <span class="d-none d-lg-inline ps-1">{t('detail.versions')}</span>
    {#if versions.length > 1}
      <span class="badge text-bg-secondary ms-1">{versions.length}</span>
    {/if}
  </button>
  {#if dd.open}
  <div class="dropdown-menu shadow show" use:dd.menuEl>
    <div class="px-3 py-2 mb-2">
      {#if versionUploadState === UploadState.Idle}
        <div class="input-group input-group-sm mb-2">
          <label class="input-group-text" for="newVersionLabel">{t('detail.label')}</label>
          <input
            id="newVersionLabel"
            class="form-control"
            type="text"
            bind:value={newVersionLabel}
            placeholder={t('detail.optional')}
            disabled={!userIsOwner || !userCanEdit}
          />
        </div>
        <input bind:this={versionFileInput} type="file" class="visually-hidden" onchange={onVersionFileSelected} />
        <div class="btn-group btn-group-sm w-100">
          <button
            class="btn btn-sm btn-outline-secondary w-100"
            onclick={() => versionFileInput?.click()}
            disabled={!userIsOwner || !userCanEdit}
            title={t('detail.upload_new_version')}
          >
            <pngx-bs-icon name="file-earmark-plus">{@html icon('file-earmark-plus')}</pngx-bs-icon><span class="ps-1">{t('common.upload')}</span>
          </button>
          <button
            class="btn btn-sm btn-outline-secondary w-100"
            onclick={addExistingDocumentAsVersion}
            disabled={!userIsOwner || !userCanEdit}
            title={t('detail.use_existing_document')}
          >
            <pngx-bs-icon name="file-earmark">{@html icon('file-earmark')}</pngx-bs-icon><span class="ps-1">{t('detail.existing')}</span>
          </button>
        </div>
      {:else if versionUploadState === UploadState.Uploading}
        <div class="small text-muted mt-1 d-flex align-items-center">
          <output class="spinner-border spinner-border-sm me-2" aria-hidden="true"></output>
          <span>{t('detail.versions.uploading')}</span>
        </div>
      {:else if versionUploadState === UploadState.Processing}
        <div class="small text-muted mt-1 d-flex align-items-center">
          <output class="spinner-border spinner-border-sm me-2" aria-hidden="true"></output>
          <span>{t('detail.versions.processing')}</span>
        </div>
      {:else if versionUploadState === UploadState.Failed}
        <div class="small text-danger mt-1 d-flex align-items-center justify-content-between">
          <span>{t('detail.versions.version_upload_failed')}</span>
          <button type="button" class="btn btn-link btn-sm p-0 ms-2" onclick={clearVersionUploadStatus}>{t('common.dismiss')}</button>
        </div>
        {#if versionUploadError}
          <div class="small text-muted mt-1">{versionUploadError}</div>
        {/if}
      {/if}
    </div>
    {#each versions as version, i (version.id)}
      <div class="dropdown-item border-top px-0" class:pe-3={versions.length === 1}>
        <div class="d-flex align-items-center w-100 py-2 version-item">
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="btn btn-link link-underline link-underline-opacity-0 d-flex align-items-center small text-start p-0 version-link"
            onclick={() => selectVersion(version.id)}
          >
            <div class="check mx-3">
              {#if selectedVersionId === version.id}
                <pngx-bs-icon name="check-circle">{@html icon('check-circle')}</pngx-bs-icon>
              {:else}
                <pngx-bs-icon class="text-muted" name="circle">{@html icon('circle')}</pngx-bs-icon>
              {/if}
            </div>
            <div class="d-flex flex-column">
              {#if isEditingVersion(version.id)}
                <label class="visually-hidden" for="versionLabel{version.id}">{t('detail.versions.version_label')}</label>
              {/if}
              <div class="input-group input-group-sm mb-1">
                {#if isEditingVersion(version.id)}
                  <input
                    id="versionLabel{version.id}"
                    class="form-control"
                    type="text"
                    bind:value={versionLabelDraft}
                    placeholder={t('detail.versions.version_label')}
                    disabled={savingVersionLabelId !== null}
                    onkeydown={(e) => {
                      if (e.key === 'Enter') submitEditedVersionLabel(version, e)
                      else if (e.key === 'Escape') cancelEditingVersion(e)
                    }}
                    onclick={(e) => e.stopPropagation()}
                  />
                {:else}
                  <span class="input-group-text overflow-hidden version-label">
                    {#if version.version_label}
                      {version.version_label}
                    {:else}
                      <span class="fst-italic">{t('detail.version')}&nbsp;{versions.length - i}&nbsp;<span class="text-muted small">(#{version.id})</span></span>
                    {/if}
                  </span>
                {/if}
                {#if canEditLabels}
                  <button
                    type="button"
                    class="btn btn-outline-secondary"
                    disabled={savingVersionLabelId !== null}
                    onclick={(e) => (isEditingVersion(version.id) ? submitEditedVersionLabel(version, e) : beginEditingVersion(version, e))}
                  >
                    {#if isEditingVersion(version.id)}
                      <pngx-bs-icon width=".8rem" height=".8rem" name="check-lg">{@html icon('check-lg', '.8rem', '.8rem')}</pngx-bs-icon>
                    {:else}
                      <pngx-bs-icon width=".8rem" height=".8rem" name="pencil">{@html icon('pencil', '.8rem', '.8rem')}</pngx-bs-icon>
                    {/if}
                  </button>
                {/if}
              </div>
              <div class="d-flex text-muted small align-items-center mt-1">
                {formatDate(version.added, 'short', dateLocale)}
                <div class="badge bg-light text-muted ms-auto">
                  {(version.checksum ?? '').slice(0, 8)}
                </div>
              </div>
            </div>
          </div>
          {#if !version.is_root && session.can('delete', 'document')}
            <pngx-confirm-button><ConfirmButton
              buttonClasses="btn-sm btn-link text-danger mx-1"
              iconName="trash"
              confirmMessage={t('detail.versions.delete_confirm')}
              disabled={!userIsOwner || !userCanEdit}
              onconfirm={() => deleteVersion(version.id)}
            /></pngx-confirm-button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
  {/if}
</div>
