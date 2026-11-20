<!-- PaperRust-specific: shrink (pl-pdftools, docs/contract/pl-pdftools.md §4). Choose a level, the server
     shrinks and only creates a new version if the file gets smaller. Size before/after. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { errorText, formatBytes, shrink, type ShrinkPreset, type ShrinkReport } from './pdf-tools-api'

  let { modal, documentId, onqueued }: { modal: ModalRef<any>; documentId: number; onqueued?: () => void } = $props()

  let preset = $state<ShrinkPreset>('standard')
  let busy = $state(false)
  let report = $state<ShrinkReport | null>(null)
  let newVersion = $state(false)

  const presets: { id: ShrinkPreset; label: string; help: string }[] = [
    { id: 'lossless', label: t('pdftools.preset_lossless'), help: t('pdftools.preset_lossless_help') },
    { id: 'standard', label: t('pdftools.preset_standard'), help: t('pdftools.preset_standard_help') },
    { id: 'strong', label: t('pdftools.preset_strong'), help: t('pdftools.preset_strong_help') },
  ]

  async function run() {
    busy = true
    try {
      const r = await shrink(documentId, preset)
      report = r.report
      newVersion = r.new_version
      if (r.new_version) onqueued?.()
    } catch (e) {
      toastService.showError(errorText(e) ?? t('pdftools.error_shrinking'), e)
    } finally {
      busy = false
    }
  }

  let saved = $derived(report && report.size_before > 0 ? Math.round((1 - report.size_after / report.size_before) * 100) : 0)
</script>

<pngx-shrink-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('pdftools.shrink_document')}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
    {#if !report}
        <p class="small text-muted">{t('pdftools.shrink_help')}</p>
        {#each presets as p (p.id)}
            <div class="form-check mb-2">
                <input class="form-check-input" type="radio" name="shrinkPreset" id="preset-{p.id}" value={p.id} bind:group={preset} disabled={busy}>
                <label class="form-check-label" for="preset-{p.id}"><strong>{p.label}</strong><br><span class="small text-muted">{p.help}</span></label>
            </div>
        {/each}
    {:else}
        <table class="table table-sm mb-2">
            <tbody>
                <tr><th scope="row">{t('pdftools.size_before')}</th><td>{formatBytes(report.size_before)}</td></tr>
                <tr><th scope="row">{t('pdftools.size_after')}</th><td>{formatBytes(report.size_after)}{#if saved > 0} <span class="text-success">(−{saved} %)</span>{/if}</td></tr>
                <tr><th scope="row">{t('pdftools.images_changed')}</th><td>{report.images_resampled + report.images_recompressed} / {report.images}</td></tr>
            </tbody>
        </table>
        <div class="alert {newVersion ? 'alert-success' : 'alert-secondary'} small mb-0">{newVersion ? t('pdftools.shrink_queued') : t('pdftools.shrink_not_smaller')}</div>
    {/if}
</div>
<div class="modal-footer">
    {#if !report}
        <button type="button" class="btn btn-outline-secondary" onclick={() => modal.close()} disabled={busy}>{t('common.cancel')}</button>
        <button type="button" class="btn btn-primary" onclick={run} disabled={busy}>
            {#if busy}<span class="spinner-border spinner-border-sm me-1" role="status"></span>{/if}{t('pdftools.shrink')}
        </button>
    {:else}
        <button type="button" class="btn btn-primary" onclick={() => modal.close()}>{t('common.close')}</button>
    {/if}
</div></pngx-shrink-dialog>
