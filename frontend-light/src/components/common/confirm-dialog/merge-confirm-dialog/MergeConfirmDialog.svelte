<!-- Port of common/confirm-dialog/merge-confirm-dialog (order by dragging like cdkDropList/cdkDrag via
     lib/cdk-drag.ts, metadata source, archive version, delete originals). Confirm calls
     onconfirm(setEnabled, { documentIDs, metadataDocumentID, archiveFallback, deleteOriginals }); cancel = dismiss. -->
<script lang="ts" module>
  export interface MergeResult {
    documentIDs: number[]
    metadataDocumentID: number
    archiveFallback: boolean
    deleteOriginals: boolean
  }
</script>

<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { api } from '../../../../api/client'
  import type { Document } from '../../../../api/v1'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import { cdkDropList } from '../../../../lib/cdk-drag'
  import { formatDate } from '../../../../lib/dates'
  import { data } from '../../../../stores/data.svelte'
  import { session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import { reorder } from './merge'

  let {
    modal,
    title = t('ui.confirmation'),
    message = '',
    messageBold = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.confirm'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    documentIDs: initialIDs = [],
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    message?: string
    messageBold?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    documentIDs?: number[]
    onconfirm?: (setEnabled: (b: boolean) => void, r: MergeResult) => void | Promise<void>
  } = $props()

  let buttonsEnabled = $state(true)
  // svelte-ignore state_referenced_locally
  let documentIDs = $state<number[]>(initialIDs.concat())
  let documents = $state<Document[]>([])
  let metadataDocumentID = $state(-1)
  let archiveFallback = $state(false)
  let deleteOriginals = $state(false)
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const user = $derived(session.data?.user)
  const ownsObject = (d: Document) => !d.owner || !!user?.is_superuser || d.owner === user?.id
  const userOwnsAllDocuments = $derived(documents.every(ownsObject))

  onMount(async () => {
    try {
      documents = (await api.getFewDocuments(documentIDs)).results
    } catch {}
  })
  const getDocument = (id: number) => documents.find((d) => d.id === id)
  function onDrop(from: number, to: number) {
    documentIDs = reorder(documentIDs, from, to)
  }
  async function confirm() {
    const r: MergeResult = { documentIDs: documentIDs.concat(), metadataDocumentID, archiveFallback, deleteOriginals }
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), r)
    else modal.close('confirm')
  }
</script>

<pngx-merge-confirm-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}>
    </button>
</div>
<div class="modal-body">
    {#if message}<p>{message}</p>{/if}
    {#if messageBold}<p><b>{messageBold}</b></p>{/if}
    <div class="form-group">
        <label class="form-label" for="metadataDocumentID">{t('dialog.merge.documents')}</label>
        <ul class="list-group" use:cdkDropList={{ item: '.list-group-item', handle: '.list-group-item', dropped: onDrop }}>
            {#each documentIDs as documentID (documentID)}
              {@const document = getDocument(documentID)}
              {#if document}
                <li class="list-group-item d-flex align-items-center cdk-drag" style={"cursor: move"}>
                    <pngx-bs-icon name="grip-vertical" class="me-2">{@html icon('grip-vertical')}</pngx-bs-icon>
                    <div class="d-flex flex-column">
                        <div>
                          {#if document.correspondent}
                            <b>{data.correspondents.get(document.correspondent)?.name ?? t('common.private')}: </b>
                          {/if}{document.title}
                        </div>
                        <small class="text-muted">
                          {formatDate(document.created, 'mediumDate', dateLocale)}
                          {#if document.page_count}
                            | {t('dialog.merge_versions.page_count', { VAR_PLURAL: document.page_count, INTERPOLATION: document.page_count })}
                          {/if}
                        </small>
                    </div>
                </li>
              {/if}
            {/each}
        </ul>
        </div>
    <div class="form-group mt-4">
        <label class="form-label" for="metadataDocumentID">{t('dialog.merge.metadata_from')}</label>
        <select id="metadataDocumentID" class="form-select" bind:value={metadataDocumentID}>
            <option value={-1}>{t('dialog.merge.regenerate_metadata')}</option>
            {#each documents as document (document.id)}
              <option value={document.id}>{document.title}</option>
            {/each}
        </select>
    </div>
    <div class="form-check form-switch mt-4">
      <input class="form-check-input" type="checkbox" role="switch" id="archiveFallbackSwitch" bind:checked={archiveFallback}>
      <label class="form-check-label" for="archiveFallbackSwitch">{t('dialog.merge.archive_fallback')}</label>
    </div>
    <div class="form-check form-switch mt-2">
      <input class="form-check-input" type="checkbox" role="switch" id="deleteOriginalsSwitch" bind:checked={deleteOriginals} disabled={!userOwnsAllDocuments}>
      <label class="form-check-label" for="deleteOriginalsSwitch">{t('dialog.merge.delete_originals')}</label>
    </div>
    {#if !archiveFallback}
      <p class="small text-muted fst-italic mt-4">{t('dialog.merge.only_pdfs')}</p>
    {/if}
</div>
<div class="modal-footer">
    <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
        <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
    </button>
    <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled}>
        {btnCaption}
    </button>
</div></pngx-merge-confirm-dialog>
