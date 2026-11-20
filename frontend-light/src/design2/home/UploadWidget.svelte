<!-- "Upload" component: choose files; dropping works anywhere (DropZone), progress in the upload list.
     Switch "AI text recognition" (ai_ocr) for the next uploads, only if AI and AI text recognition are on. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { session } from '../../stores/settings.svelte'
  import Widget from './Widget.svelte'
  import Icon from '../components/Icon.svelte'
  import { aiStatus } from '../ai/status.svelte'
  aiStatus.ensure()

  let input: HTMLInputElement | undefined = $state()
  const can = $derived(session.can('add', 'document'))
  const busy = $derived(consumerStatus.getConsumerStatusNotCompleted().length)
  function onchange(e: Event) {
    const el = e.target as HTMLInputElement
    const files = Array.from(el.files ?? [])
    el.value = ''
    if (files.length) consumerStatus.uploadFiles(files)
  }
</script>

<Widget id="upload" title={t('common.upload')} icon="cloud-arrow-up">
  {#if !can}
    <p class="d2-muted">{t('m.home.upload_no_permission')}</p>
  {:else}
    <div class="up">
      <Icon name="file-earmark-arrow-up" size="2rem" />
      <input type="file" multiple class="d2-sr" tabindex="-1" aria-hidden="true" bind:this={input} {onchange} id="d2-w-upload-input" />
      <button class="d2-btn" onclick={() => input?.click()}><Icon name="file-earmark-arrow-up" />{t('m.home.choose_files')}</button>
      <p class="d2-muted">{t('dashboard.drop_files_anywhere')}</p>
      {#if aiStatus.vision}
        <label class="ai" title={t('ui.ai_ocr_hint')}><input type="checkbox" id="d2-w-upload-ai-ocr" bind:checked={consumerStatus.aiOcr} />{t('ui.use_ai_ocr')}</label>
        <p class="d2-muted hint">{t('ui.ai_ocr_upload_hint')} {t('ui.ai_ocr_hint')}</p>
      {/if}
      {#if busy}<p class="d2-muted" aria-live="polite">{t('m.home.uploads_running', { PH: busy })}</p>{/if}
    </div>
  {/if}
</Widget>

<style>
  .up {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--d2-s2);
    padding: var(--d2-s3);
    border: 1px dashed var(--d2-border-strong, var(--d2-border));
    border-radius: var(--d2-r2);
    color: var(--d2-text-2);
    text-align: center;
  }
  p {
    margin: 0;
  }
  .ai {
    display: inline-flex;
    align-items: center;
    gap: var(--d2-s2);
    color: var(--d2-text);
    cursor: pointer;
  }
  .hint {
    font-size: var(--d2-fs-sm, 0.85em);
  }
</style>
