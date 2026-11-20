<!-- Port of common/confirm-dialog/rotate-confirm-dialog (ConfirmDialog + preview image with rotate buttons).
     Confirm calls onconfirm(setEnabled, degrees); cancel = dismiss. Without delay progress bar (like ConfirmDialog). -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { api } from '../../../../api/client'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import { rotationDegrees } from '../../../document-list/bulk-editor/bulk-editor'
  let {
    modal,
    title = t('ui.confirmation'),
    messageBold = '',
    message = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.confirm'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    documentID = undefined,
    showPDFNote = true,
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    messageBold?: string
    message?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    documentID?: number
    showPDFNote?: boolean
    onconfirm?: (setEnabled: (b: boolean) => void, degrees: number) => void | Promise<void>
  } = $props()
  let buttonsEnabled = $state(true)
  let rotation = $state(0)
  const degrees = $derived(rotationDegrees(rotation))

  function rotate(clockwise = true) {
    rotation += clockwise ? 90 : -90
  }
  async function confirm() {
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), degrees)
    else modal.close('confirm')
  }
</script>

<pngx-rotate-confirm-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}>
    </button>
</div>
<div class="modal-body">
    <div class="row">
        <div class="col-2 d-flex justify-content-end">
            <button class="btn btn-secondary mt-auto" onclick={() => rotate(false)} aria-label={t('dialog.rotate.counterclockwise')}>
                <pngx-bs-icon name="arrow-counterclockwise">{@html icon('arrow-counterclockwise')}</pngx-bs-icon>
            </button>
        </div>
        <div class="col-8 d-flex align-items-center">
            {#if documentID}
                <img class="w-75 m-auto" style="transition: all 0.25s ease; transform: rotate({rotation}deg)" src={api.thumbUrl(documentID)} alt="" />
            {/if}
        </div>
        <div class="col-2 d-flex">
            <button class="btn btn-secondary mt-auto" onclick={() => rotate()} aria-label={t('dialog.rotate.clockwise')}>
                <pngx-bs-icon name="arrow-clockwise">{@html icon('arrow-clockwise')}</pngx-bs-icon>
            </button>
        </div>
    </div>
    {#if showPDFNote}
        <p class="small text-muted fst-italic mt-4">{t('dialog.rotate.only_pdfs')}</p>
    {/if}
</div>
<div class="modal-footer flex-nowrap">
    <div class="col">
        {#if message}
            <p>{message}</p>
        {/if}
        {#if messageBold}
          <p class="mb-0 small"><b>{messageBold}</b></p>
        {/if}
    </div>
    <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
        <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
    </button>
    <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled || degrees === 0}>
      {btnCaption}
    </button>
</div></pngx-rotate-confirm-dialog>
