<!-- Drop files anywhere → upload (shared upload store, progress via XHR + /ws/status/). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { session } from '../../stores/settings.svelte'
  import Icon from './Icon.svelte'

  let depth = $state(0)
  const hasFiles = (e: DragEvent) => !!e.dataTransfer && [...e.dataTransfer.types].includes('Files')
  const can = () => session.can('add', 'document')
</script>

<svelte:window
  ondragenter={(e) => {
    if (!hasFiles(e) || !can()) return
    e.preventDefault()
    depth++
  }}
  ondragover={(e) => {
    if (hasFiles(e) && can()) e.preventDefault()
  }}
  ondragleave={(e) => {
    if (!hasFiles(e)) return
    depth = Math.max(0, depth - 1)
  }}
  ondrop={(e) => {
    if (!hasFiles(e) || !can()) return
    e.preventDefault()
    depth = 0
    const files = Array.from(e.dataTransfer?.files ?? [])
    if (files.length) consumerStatus.uploadFiles(files)
  }}
/>

{#if depth > 0}
  <div class="drop" aria-hidden="true">
    <div class="box"><Icon name="cloud-arrow-up" size="2.5rem" /><p>{t('m.ui.drop_files_upload')}</p></div>
  </div>
{/if}

<style>
  .drop {
    position: fixed;
    inset: 0;
    z-index: 90;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--d2-accent) 14%, var(--d2-overlay));
    pointer-events: none;
  }
  .box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--d2-s3);
    padding: var(--d2-s6) 64px;
    border: 2px dashed var(--d2-accent);
    border-radius: 16px;
    background: var(--d2-surface);
    color: var(--d2-accent-text);
    font-size: var(--d2-fs-xl);
    font-weight: 600;
  }
</style>
