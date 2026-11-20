<!-- Short messages at the bottom right (shared toastService of the Light UI). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { toastService, type Toast } from '../../stores/toasts.svelte'
  import Icon from './Icon.svelte'

  const timers = new Map<string, ReturnType<typeof setTimeout>>()
  $effect(() => {
    for (const p of toastService.popups) {
      if (!p.id || timers.has(p.id)) continue
      timers.set(
        p.id,
        setTimeout(() => {
          toastService.removePopup(p)
          timers.delete(p.id!)
        }, p.delay || 5000),
      )
    }
  })
  const close = (p: Toast) => toastService.removePopup(p)
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each toastService.popups as p (p.id)}
    <div class="toast {p.level ?? 'info'}">
      <Icon name={p.level === 'error' ? 'exclamation-triangle' : p.level === 'success' ? 'check-circle' : 'info-circle'} size="1.1rem" />
      <div class="txt">
        <p>{p.content}</p>
        {#if p.error}<p class="err">{typeof p.error === 'string' ? p.error : (p.error?.message ?? JSON.stringify(p.error?.body ?? p.error)).slice(0, 300)}</p>{/if}
        {#if p.action && p.actionName}<button class="d2-btn sm" onclick={() => { p.action?.(); close(p) }}>{p.actionName}</button>{/if}
      </div>
      <button class="d2-btn ghost sm icon" aria-label={t('common.close')} onclick={() => close(p)}><Icon name="x-lg" /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: var(--d2-s4);
    bottom: var(--d2-s4);
    z-index: 80;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    width: min(380px, calc(100vw - 32px));
    pointer-events: none;
  }
  :global(.mobile) ~ .toasts {
    bottom: calc(var(--d2-tabbar-h) + 8px);
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: var(--d2-s2);
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-left: 4px solid var(--d2-info);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    pointer-events: auto;
  }
  .toast.success {
    border-left-color: var(--d2-ok);
  }
  .toast.error {
    border-left-color: var(--d2-danger);
  }
  .txt {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    min-width: 0;
    font-size: var(--d2-fs-sm);
  }
  .err {
    color: var(--d2-text-3);
    word-break: break-word;
  }
</style>
