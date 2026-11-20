<!-- Port of document-detail/document-version-dropdown/add-existing-document-version-dialog (choose exactly one existing
     document as version). Confirm calls onconfirm(setEnabled, existingDocumentID); cancel = dismiss.
     Deviation: InputDocumentLink has no "minimal" mode and no own placeholder (shows "Search for documents"). -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import InputDocumentLink from '../../../common/input/document-link/InputDocumentLink.svelte'

  let {
    modal,
    rootDocumentID,
    onconfirm,
  }: {
    modal: ModalRef<string>
    rootDocumentID: number | undefined
    onconfirm?: (setEnabled: (b: boolean) => void, existingDocumentID: number) => void | Promise<void>
  } = $props()

  let selectedDocumentIDs = $state<number[]>([])
  let buttonsEnabled = $state(true)

  function confirm() {
    if (selectedDocumentIDs.length !== 1) return
    onconfirm?.((b) => (buttonsEnabled = b), selectedDocumentIDs[0])
  }
</script>

<pngx-add-existing-document-version-dialog><div class="modal-header">
  <h4 class="modal-title">{t('detail.versions.add_existing_title')}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}></button>
</div>
<div class="modal-body">
  <pngx-input-document-link><InputDocumentLink bind:value={selectedDocumentIDs} parentDocumentID={rootDocumentID} /></pngx-input-document-link>
  <div class="form-text mt-2">{t('detail.versions.add_existing_hint')}</div>
</div>
<div class="modal-footer">
  <button type="button" class="btn btn-outline-secondary" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>{t('common.cancel')}</button>
  <button type="button" class="btn btn-primary" onclick={confirm} disabled={!buttonsEnabled || selectedDocumentIDs.length !== 1}>{t('detail.versions.add_version')}</button>
</div></pngx-add-existing-document-version-dialog>
