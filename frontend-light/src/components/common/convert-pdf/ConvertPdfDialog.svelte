<!-- PaperRust-specific: convert to PDF (images and office files, docs/contract/pl-pdftools.md §7). For one
     document (detail view) or several (multi-selection). Each result becomes a new version; the
     previous version is kept. Afterwards redaction and shrinking work. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { errorText } from '../../document-detail/pdf-tools/pdf-tools-api'
  import { convertMany, convertToPdf, type ConvertSummary } from './convert-pdf-api'

  let {
    modal,
    documentIds,
    showOcr = true,
    onqueued,
  }: { modal: ModalRef<any>; documentIds: number[]; showOcr?: boolean; onqueued?: (ids: number[]) => void } = $props()

  let ocr = $state(false)
  let busy = $state(false)
  let progress = $state(0)
  let summary = $state<ConvertSummary | null>(null)
  const many = $derived(documentIds.length > 1)

  async function run() {
    busy = true
    progress = 0
    try {
      summary = await convertMany(documentIds, ocr, (id, o) => convertToPdf(id, o), errorText, (n) => (progress = n))
      if (summary.done.length > 0) onqueued?.(summary.done)
      if (!many && summary.failed.length > 0) toastService.showError(summary.failed[0].message || t('convert.error'))
    } finally {
      busy = false
    }
  }
</script>

<pngx-convert-pdf-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('convert.title')}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()} disabled={busy}></button>
</div>
<div class="modal-body">
    {#if !summary}
        <p class="small text-muted">{t('convert.help')}</p>
        {#if many}
            <p class="small">{t('convert.bulk_help', { PH: documentIds.length })}</p>
        {/if}
        {#if showOcr}
            <div class="form-check">
                <input class="form-check-input" type="checkbox" id="convertOcr" bind:checked={ocr} disabled={busy}>
                <label class="form-check-label" for="convertOcr">{t('convert.ocr')}<br><span class="small text-muted">{t('convert.ocr_help')}</span></label>
            </div>
        {/if}
        {#if busy && many}
            <div class="small text-muted mt-2">{t('convert.progress', { PH: progress, TOTAL: documentIds.length })}</div>
        {/if}
    {:else}
        {#if summary.done.length > 0}
            <div class="alert alert-success small">{t('convert.queued', { PH: summary.done.length })}</div>
        {/if}
        {#if summary.failed.length > 0}
            <div class="alert alert-warning small mb-0">
                {t('convert.failed', { PH: summary.failed.length })}
                <ul class="mb-0 mt-1">
                    {#each summary.failed.slice(0, 10) as f (f.id)}
                        <li>#{f.id}: {f.message || t('convert.error')}</li>
                    {/each}
                </ul>
            </div>
        {/if}
    {/if}
</div>
<div class="modal-footer">
    {#if !summary}
        <button type="button" class="btn btn-outline-secondary" onclick={() => modal.close()} disabled={busy}>{t('common.cancel')}</button>
        <button type="button" class="btn btn-primary" onclick={run} disabled={busy || documentIds.length === 0}>
            {#if busy}<span class="spinner-border spinner-border-sm me-1" role="status"></span>{/if}{t('convert.convert')}
        </button>
    {:else}
        <button type="button" class="btn btn-primary" onclick={() => modal.close()}>{t('common.close')}</button>
    {/if}
</div></pngx-convert-pdf-dialog>
