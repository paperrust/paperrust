<!-- Upload list at the bottom left: progress per file (upload 0–20 %, processing 20–100 %), result with link. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { consumerStatus, FileStatusPhase, FILE_STATUS_MESSAGES } from '../../stores/uploads.svelte'
  import Icon from './Icon.svelte'

  let collapsed = $state(false)
  const list = $derived(consumerStatus.consumerStatus)
  const open = $derived(consumerStatus.getConsumerStatusNotCompleted().length)
  const msgOf = (m: string) => (FILE_STATUS_MESSAGES[m] ? t(FILE_STATUS_MESSAGES[m]) : m)
</script>

<section class="tray" aria-label={t('m.ui.uploads')} id="d2-upload-tray">
  <div class="head">
    <strong>{open ? t('m.ui.uploading_file_s', { PH: open }) : t('m.ui.uploads_finished')}</strong>
    <button class="d2-btn ghost sm icon" aria-label={collapsed ? t('m.ui.expand') : t('m.ui.collapse')} aria-expanded={!collapsed} onclick={() => (collapsed = !collapsed)}><Icon name={collapsed ? 'chevron-left' : 'chevron-down'} /></button>
    {#if !open}<button class="d2-btn ghost sm icon" aria-label={t('common.dismiss_all')} onclick={() => consumerStatus.dismissCompleted()}><Icon name="x-lg" /></button>{/if}
  </div>
  {#if !collapsed}
    <ul class="d2-scroll">
      {#each list as s, i (i)}
        <li class:fail={s.phase === FileStatusPhase.FAILED} class:ok={s.phase === FileStatusPhase.SUCCESS}>
          <div class="row">
            <Icon name={s.phase === FileStatusPhase.FAILED ? 'exclamation-triangle' : s.phase === FileStatusPhase.SUCCESS ? 'check-circle' : 'file-earmark-arrow-up'} />
            <span class="fn" title={s.filename}>{s.filename}</span>
            {#if s.phase === FileStatusPhase.SUCCESS && s.documentId}
              <a class="d2-btn sm" href={router.href('documents/' + s.documentId)}>{t('common.open')}</a>
            {/if}
            {#if s.phase >= FileStatusPhase.SUCCESS}
              <button class="d2-btn ghost sm icon" aria-label={t('common.dismiss')} onclick={() => consumerStatus.dismiss(s)}><Icon name="x-lg" /></button>
            {/if}
          </div>
          <div class="bar" role="progressbar" aria-label={s.filename} aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(s.getProgress() * 100)}>
            <span style:width={`${Math.round(s.getProgress() * 100)}%`}></span>
          </div>
          <p class="msg">{msgOf(s.message) || (s.phase === FileStatusPhase.UPLOADING ? t('common.uploading') : '')}</p>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .tray {
    position: fixed;
    left: var(--d2-s4);
    bottom: var(--d2-s4);
    z-index: 70;
    width: min(360px, calc(100vw - 32px));
    max-height: 50vh;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
  }
  :global(.mobile) ~ .tray {
    bottom: calc(var(--d2-tabbar-h) + 8px);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--d2-s1);
    padding: var(--d2-s2) var(--d2-s2) var(--d2-s2) var(--d2-s3);
    border-bottom: 1px solid var(--d2-border);
  }
  .head strong {
    flex: 1;
    font-size: var(--d2-fs-sm);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: var(--d2-s2) var(--d2-s3);
  }
  li + li {
    margin-top: var(--d2-s3);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
  .fn {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar {
    height: 4px;
    margin-top: 6px;
    border-radius: 2px;
    background: var(--d2-surface-3);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--d2-accent);
    transition: width 0.2s;
  }
  .ok .bar span {
    background: var(--d2-ok);
  }
  .fail .bar span {
    background: var(--d2-danger);
  }
  .msg {
    margin-top: 2px;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
  }
  .fail .msg {
    color: var(--d2-danger);
  }
</style>
