<!-- Port of common/email-document-dialog (POST /api/documents/email/, toasts like Angular). -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { api } from '../../../api/client'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'

  let { modal, documentIds, hasArchiveVersion = true }: { modal: ModalRef<any>; documentIds: number[]; hasArchiveVersion?: boolean } = $props()

  // svelte-ignore state_referenced_locally
  let useArchiveVersion = $state(hasArchiveVersion)
  let loading = $state(false)
  let emailAddress = $state('')
  let emailSubject = $state('')
  let emailMessage = $state('')

  async function emailDocuments() {
    loading = true
    try {
      await api.emailDocuments(documentIds, emailAddress, emailSubject, emailMessage, useArchiveVersion)
      loading = false
      emailAddress = emailSubject = emailMessage = ''
      modal.close()
      toastService.showSuccess(t('ui.email_sent'))
    } catch (e) {
      loading = false
      toastService.showError(documentIds.length > 1 ? t('ui.error_emailing_documents') : t('ui.error_emailing_document'), e)
    }
  }
</script>

<pngx-email-document-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('ui.email_documents', { VAR_PLURAL: documentIds.length, INTERPOLATION: documentIds.length })}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
    <div class="mb-1">
        <label for="email" class="form-label">{t('ui.email_address_es')}</label>
        <input type="email" class="form-control" id="email" bind:value={emailAddress}>
    </div>
    <div class="mb-1">
        <label for="email" class="form-label">{t('common.subject')}</label>
        <input type="email" class="form-control" id="subject" bind:value={emailSubject}>
    </div>
    <div>
        <label for="message" class="form-label">{t('ui.message')}</label>
        <textarea class="form-control" id="message" rows="3" bind:value={emailMessage}></textarea>
    </div>
</div>
<div class="modal-footer">
    <div class="input-group">
        <div class="input-group-text flex-grow-1">
            <input class="form-check-input mt-0 me-2" type="checkbox" role="switch" id="useArchiveVersion" disabled={!hasArchiveVersion} bind:checked={useArchiveVersion}>
            <label class="form-check-label w-100 text-start" for="useArchiveVersion">{t('ui.use_archive_version')}</label>
        </div>
        <button type="submit" class="btn btn-outline-primary" onclick={emailDocuments} disabled={loading || emailAddress.length === 0 || emailMessage.length === 0 || emailSubject.length === 0}>
            {#if loading}
                <div class="spinner-border spinner-border-sm me-2" role="status"></div>
            {/if}
            {t('ui.send_email')}
        </button>
    </div>
    <div class="text-light fst-italic small mt-2">
        {t('ui.some_email_servers_may')}
    </div>
</div></pngx-email-document-dialog>
