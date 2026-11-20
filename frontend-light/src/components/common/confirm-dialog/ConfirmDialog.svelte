<!-- Port of common/confirm-dialog/confirm-dialog.component.html (without delay progress bar).
     Return via modal.close('confirm' | 'alternative'), cancel = dismiss. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  let {
    modal,
    title = t('ui.confirmation'),
    messageBold = '',
    message = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.confirm'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    alternativeBtnClass = 'btn-secondary',
    alternativeBtnCaption = '',
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
    alternativeBtnClass?: string
    alternativeBtnCaption?: string
    /** like confirmClicked: returns false if the dialog should stay open */
    onconfirm?: (setEnabled: (b: boolean) => void) => void | Promise<void>
  } = $props()
  let buttonsEnabled = $state(true)

  function cancel() {
    modal.dismiss()
  }
  async function confirm() {
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b))
    else modal.close('confirm')
  }
</script>

<pngx-confirm-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={cancel}>
  </button>
</div>
<div class="modal-body">
  {#if messageBold}
    <p class="text-break"><b>{messageBold}</b></p>
  {/if}
  {#if message}
    <p class="mb-0 text-break">{@html message}</p>
  {/if}
</div>
<div class="modal-footer">
  <button type="button" class="btn {cancelBtnClass}" onclick={cancel} disabled={!buttonsEnabled}>
        <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
      </button>
  <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled}>
    <span>
      {btnCaption}
      <span class="visually-hidden">0 seconds</span>
    </span>
  </button>
  {#if alternativeBtnCaption}
    <button type="button" class="btn {alternativeBtnClass}" onclick={() => modal.close('alternative')} disabled={!buttonsEnabled}>
      {alternativeBtnCaption}
    </button>
  {/if}
</div></pngx-confirm-dialog>
