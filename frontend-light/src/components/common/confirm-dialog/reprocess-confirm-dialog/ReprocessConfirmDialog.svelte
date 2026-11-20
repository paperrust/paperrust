<!-- Port of common/confirm-dialog/reprocess-confirm-dialog (ConfirmDialog + switch "Use remote OCR", only if
     remote OCR is set up and not always active anyway: SettingsService.remoteOCRIsSelectable). Confirm calls
     onconfirm(setEnabled, remoteOcr, aiOcr); cancel = dismiss. PaperRust: switch "AI text recognition" (field ai_ocr),
     only visible when AI and AI text recognition are on (GET /api/x-ai/status/ vision_enabled). -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import { rawValue, session } from '../../../../stores/settings.svelte'
  import { aiStatus } from '../../../../design2/ai/status.svelte'
  let {
    modal,
    title = t('ui.confirmation'),
    messageBold = '',
    message = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.confirm'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
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
    onconfirm?: (setEnabled: (b: boolean) => void, remoteOcr: boolean, aiOcr: boolean) => void | Promise<void>
  } = $props()
  let buttonsEnabled = $state(true)
  let remoteOcr = $state(false)
  let aiOcr = $state(false)
  aiStatus.ensure()
  const bool = (v: unknown) => (typeof v === 'string' ? v === 'true' : !!v)
  const showRemoteOcr = $derived(bool(rawValue(session.settings, 'remote_ocr:configured')) && (rawValue(session.settings, 'remote_ocr:mode') ?? 'always') !== 'always')

  async function confirm() {
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), remoteOcr, aiStatus.vision && aiOcr)
    else modal.close('confirm')
  }
</script>

<pngx-reprocess-confirm-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}>
  </button>
</div>
<div class="modal-body">
  {#if messageBold}
    <p class="text-break"><b>{messageBold}</b></p>
  {/if}
  {#if message}
    <p class="mb-0 text-break">{@html message}</p>
  {/if}
  {#if showRemoteOcr}
    <div class="form-check mt-3">
      <input class="form-check-input" type="checkbox" id="reprocessRemoteOcr" bind:checked={remoteOcr} />
      <label class="form-check-label" for="reprocessRemoteOcr">{t('ui.use_remote_ocr')}</label>
      <div class="form-text">{t('ui.sends_document_configured_remote')}</div>
    </div>
  {/if}
  {#if aiStatus.vision}
    <div class="form-check mt-3">
      <input class="form-check-input" type="checkbox" id="reprocessAiOcr" bind:checked={aiOcr} />
      <label class="form-check-label" for="reprocessAiOcr">{t('ui.use_ai_ocr')}</label>
      <div class="form-text">{t('ui.ai_ocr_hint')}</div>
    </div>
  {/if}
</div>
<div class="modal-footer">
  <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
    <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
  </button>
  <button type="button" class="btn {btnClass}" onclick={confirm} disabled={!buttonsEnabled}>
    {btnCaption}
  </button>
</div></pngx-reprocess-confirm-dialog>
