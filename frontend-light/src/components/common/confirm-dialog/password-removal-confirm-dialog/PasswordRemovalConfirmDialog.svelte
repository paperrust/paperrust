<!-- Port of common/confirm-dialog/password-removal-confirm-dialog (ConfirmDialog + choice "Replace current document" /
     "Create new document", for a new document "Copy metadata" and "Delete original").
     Confirm calls onconfirm(setEnabled, { updateDocument, includeMetadata, deleteOriginal }); cancel = dismiss. -->
<script module lang="ts">
  export interface PasswordRemovalOptions {
    updateDocument: boolean
    includeMetadata: boolean
    deleteOriginal: boolean
  }
</script>

<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import type { ModalRef } from '../../../../lib/modal.svelte'

  let {
    modal,
    title = t('dialog.unprotect.title'),
    message = t('dialog.unprotect.message'),
    btnClass = 'btn-primary',
    btnCaption = t('dialog.unprotect.start'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    message?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    onconfirm?: (setEnabled: (b: boolean) => void, options: PasswordRemovalOptions) => void | Promise<void>
  } = $props()

  let buttonsEnabled = $state(true)
  let updateDocument = $state(true)
  let includeMetadata = $state(true)
  let deleteOriginal = $state(false)

  /** onUpdateDocumentChange */
  function setUpdateDocument(v: boolean) {
    updateDocument = v
    if (updateDocument) {
      deleteOriginal = false
      includeMetadata = true
    }
  }

  async function confirm() {
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), { updateDocument, includeMetadata, deleteOriginal })
    else modal.close('confirm')
  }
</script>

<pngx-password-removal-confirm-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}>
  </button>
</div>
<div class="modal-body">
  {#if message}
    <p class="mb-3">{@html message}</p>
  {/if}
  <div class="btn-group mb-3" role="group">
    <input type="radio" class="btn-check" name="passwordRemoveMode" id="removeReplace" checked={updateDocument} onchange={() => setUpdateDocument(true)} />
    <label class="btn btn-outline-primary btn-sm" for="removeReplace">
      <pngx-bs-icon name="pencil">{@html icon('pencil')}</pngx-bs-icon>
      <span class="ms-2">{t('dialog.unprotect.replace_current')}</span>
    </label>
    <input type="radio" class="btn-check" name="passwordRemoveMode" id="removeCreate" checked={!updateDocument} onchange={() => setUpdateDocument(false)} />
    <label class="btn btn-outline-primary btn-sm" for="removeCreate">
      <pngx-bs-icon name="plus">{@html icon('plus')}</pngx-bs-icon>
      <span class="ms-2">{t('dialog.unprotect.create_new')}</span>
    </label>
  </div>
  {#if !updateDocument}
    <div class="d-flex flex-column flex-md-row w-100 gap-3 align-items-center">
      <div class="form-group d-flex">
        <div class="form-check">
          <input class="form-check-input" type="checkbox" id="copyMetaRemove" bind:checked={includeMetadata} />
          <label class="form-check-label" for="copyMetaRemove">{t('dialog.unprotect.copy_metadata')}</label>
        </div>
        <div class="form-check ms-3">
          <input class="form-check-input" type="checkbox" id="deleteOriginalRemove" bind:checked={deleteOriginal} />
          <label class="form-check-label" for="deleteOriginalRemove">{t('dialog.unprotect.delete_original')}</label>
        </div>
      </div>
    </div>
  {/if}
</div>
<div class="modal-footer flex-nowrap gap-2">
  <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
    <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
  </button>
  <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled}>
    {btnCaption}
  </button>
</div></pngx-password-removal-confirm-dialog>
