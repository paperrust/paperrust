<!-- Port of common/confirm-dialog/merge-as-versions-confirm-dialog (choose root document, order versions by
     dragging, oldest first; last = current version). Confirm calls onconfirm(setEnabled, documentIDs, rootDocumentID);
     cancel = dismiss. -->
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
  import { reorderVersions, versionIds } from '../merge-confirm-dialog/merge'

  let {
    modal,
    title = t('ui.confirmation'),
    message = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.confirm'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    documentIDs: initialIDs = [],
    rootDocumentID: initialRoot = -1,
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    message?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    documentIDs?: number[]
    rootDocumentID?: number
    onconfirm?: (setEnabled: (b: boolean) => void, documentIDs: number[], rootDocumentID: number) => void | Promise<void>
  } = $props()

  let buttonsEnabled = $state(true)
  // svelte-ignore state_referenced_locally
  let documentIDs = $state<number[]>(initialIDs.concat())
  // svelte-ignore state_referenced_locally
  let rootDocumentID = $state(initialRoot)
  let documents = $state<Document[]>([])
  const versionDocumentIDs = $derived(versionIds(documentIDs, rootDocumentID))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())

  onMount(async () => {
    try {
      documents = (await api.getFewDocuments(documentIDs)).results
    } catch {}
  })
  const getDocument = (id: number) => documents.find((d) => d.id === id)
  function onDrop(from: number, to: number) {
    documentIDs = reorderVersions(documentIDs, rootDocumentID, from, to)
  }
  async function confirm() {
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), documentIDs.concat(), rootDocumentID)
    else modal.close('confirm')
  }
</script>

<pngx-merge-as-versions-confirm-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}></button>
</div>
<div class="modal-body">
  <p>{message}</p>
  <div class="form-group">
    <label class="form-label" for="rootDocumentID">{t('dialog.merge_versions.main_document')}</label>
    <select id="rootDocumentID" class="form-select" bind:value={rootDocumentID}>
      {#each documents as document (document.id)}
        <option value={document.id}>{document.title}</option>
      {/each}
    </select>
  </div>
  <div class="form-group mt-4">
    <span class="form-label d-inline-block">{t('dialog.merge_versions.versions_oldest_first')}</span>
    <ul class="list-group" use:cdkDropList={{ item: '.list-group-item', handle: '.list-group-item', dropped: onDrop }}>
      {#each versionDocumentIDs as documentID, i (documentID)}
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
            {#if i === versionDocumentIDs.length - 1}
              <span class="badge bg-primary ms-auto">{t('dialog.merge_versions.current_version')}</span>
            {/if}
          </li>
        {/if}
      {/each}
    </ul>
    {#if versionDocumentIDs.length > 1}
      <div class="form-text">{t('dialog.merge_versions.drag_to_reorder')}</div>
    {/if}
  </div>
</div>
<div class="modal-footer">
  <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
    <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
  </button>
  <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled}>
    {btnCaption}
  </button>
</div></pngx-merge-as-versions-confirm-dialog>
