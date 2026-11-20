<!-- Port of dashboard/widgets/upload-file-widget/upload-file-widget.component.html.
     Host <pngx-upload-file-widget> in the parent markup. PaperRust: switch "AI text recognition" (ai_ocr) for the next
     uploads, only if AI and AI text recognition are on. -->
<script lang="ts">
  import '../../../../styles/components/upload-file-widget.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { session } from '../../../../stores/settings.svelte'
  import { consumerStatus } from '../../../../stores/uploads.svelte'
  import WidgetFrame from '../widget-frame/WidgetFrame.svelte'
  import { aiStatus } from '../../../../design2/ai/status.svelte'
  aiStatus.ensure()

  let fileUpload: HTMLInputElement | undefined = $state()

  function onFileSelected(event: Event) {
    const input = event.target as HTMLInputElement
    consumerStatus.uploadFiles(Array.from(input.files ?? []))
    input.value = ''
  }
</script>

{#if session.can('add', 'document')}
  <pngx-widget-frame><WidgetFrame cardless={true}>
    <div tourAnchor="tour.upload-widget">
      <form class="justify-content-center d-flex flex-column align-items-center ng-untouched ng-pristine ng-valid">
        <button type="button" class="btn btn-outline-dark bg-light shadow-sm w-100 h-100 pt-3 pb-3" onclick={() => fileUpload?.click()}>
          <pngx-bs-icon class="text-primary me-1" name="plus-circle">{@html icon('plus-circle')}</pngx-bs-icon>
          <span class="text-primary">{t('common.upload_documents')}</span>
          <div class="text-muted smaller fst-italic">{t('dashboard.drop_files_anywhere')}</div>
        </button>
        <input type="file" class="visually-hidden" onchange={onFileSelected} multiple bind:this={fileUpload}>
        {#if aiStatus.vision}
          <div class="form-check mt-2 align-self-start">
            <input class="form-check-input" type="checkbox" id="uploadAiOcr" bind:checked={consumerStatus.aiOcr} />
            <label class="form-check-label" for="uploadAiOcr">{t('ui.use_ai_ocr')}</label>
            <div class="form-text">{t('ui.ai_ocr_upload_hint')} {t('ui.ai_ocr_hint')}</div>
          </div>
        {/if}
      </form>
    </div>
  </WidgetFrame></pngx-widget-frame>
{/if}
