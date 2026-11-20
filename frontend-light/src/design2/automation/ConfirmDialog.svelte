<!-- Confirmation before consequential steps (delete, apply retroactively, discard): small dialog with focus trap,
     Esc = cancel. No browser dialog. -->
<script lang="ts">
  import { onMount, type Snippet } from 'svelte'
  import { t } from '../../lib/i18n'
  let {
    title,
    confirmLabel,
    danger = false,
    busy = false,
    onconfirm,
    oncancel,
    children,
  }: { title: string; confirmLabel: string; danger?: boolean; busy?: boolean; onconfirm: () => void; oncancel: () => void; children?: Snippet } = $props()
  let el: HTMLDivElement
  const id = `au-confirm-${Math.random().toString(36).slice(2, 8)}`
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null
    el.querySelector<HTMLElement>('button.cancel')?.focus()
    return () => prev?.focus?.()
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      oncancel()
    } else if (e.key === 'Tab') {
      const f = [...el.querySelectorAll<HTMLElement>('button:not([disabled]), input, a[href]')]
      if (!f.length) return
      if (e.shiftKey && document.activeElement === f[0]) {
        f[f.length - 1].focus()
        e.preventDefault()
      } else if (!e.shiftKey && document.activeElement === f[f.length - 1]) {
        f[0].focus()
        e.preventDefault()
      }
    }
  }
</script>

<div class="bd" role="presentation" onclick={oncancel}></div>
<div class="dlg" role="alertdialog" aria-modal="true" aria-labelledby={id} tabindex="-1" bind:this={el} {onkeydown}>
  <h2 {id}>{title}</h2>
  {#if children}<div class="txt">{@render children()}</div>{/if}
  <div class="btns">
    <button type="button" class="d2-btn cancel" onclick={oncancel} disabled={busy}>{t('common.cancel')}</button>
    <button type="button" class="d2-btn" class:primary={!danger} class:danger onclick={onconfirm} disabled={busy}>{confirmLabel}</button>
  </div>
</div>

<style>
  .bd {
    position: fixed;
    inset: 0;
    z-index: 70;
    background: var(--d2-overlay);
  }
  .dlg {
    position: fixed;
    top: 50%;
    left: 50%;
    z-index: 71;
    width: min(440px, calc(100vw - 32px));
    transform: translate(-50%, -50%);
    padding: var(--d2-s5);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    outline: none;
  }
  h2 {
    margin: 0 0 var(--d2-s3);
    font-size: var(--d2-fs-lg);
  }
  .txt {
    color: var(--d2-text-2);
  }
  .txt :global(p) {
    margin: 0 0 var(--d2-s2);
  }
  .btns {
    display: flex;
    justify-content: flex-end;
    gap: var(--d2-s2);
    margin-top: var(--d2-s4);
  }
  .danger {
    border-color: var(--d2-danger);
    background: var(--d2-danger-soft);
    color: var(--d2-danger);
    font-weight: 600;
  }
</style>
