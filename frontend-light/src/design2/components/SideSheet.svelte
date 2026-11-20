<!-- Side sheet instead of a centered dialog (docs/DESIGN2.md §4.6): on the right, full height, focus stays in the sheet. -->
<script lang="ts">
  import { onMount, type Snippet } from 'svelte'
  import { t } from '../../lib/i18n'
  import Icon from './Icon.svelte'
  import { sheetCatchesEscape, sheetStack } from '../lib/focus'
  let { title, onclose, width = '480px', children, footer = undefined }: { title: string; onclose: () => void; width?: string; children: Snippet; footer?: Snippet } = $props()
  let el: HTMLDivElement
  const id = `d2-sheet-${Math.random().toString(36).slice(2, 8)}`
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null
    ;(el.querySelector<HTMLElement>('[autofocus], input, select, textarea, button:not(.close)') ?? el).focus()
    // Esc also when focus was lost (e.g. the clicked button was removed): the topmost sheet
    // catches it before the global key handling (capture phase on document).
    const me = {}
    const pop = sheetStack.push(me)
    const esc = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !sheetCatchesEscape(e.key, document.activeElement, document.body, sheetStack.isTop(me))) return
      e.stopPropagation()
      e.preventDefault()
      onclose()
    }
    document.addEventListener('keydown', esc, true)
    return () => {
      document.removeEventListener('keydown', esc, true)
      pop()
      prev?.focus?.()
    }
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation()
      e.preventDefault()
      onclose()
    } else if (e.key === 'Tab') {
      const f = [...el.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])')]
      if (!f.length) return
      const first = f[0]
      const last = f[f.length - 1]
      if (e.shiftKey && document.activeElement === first) {
        last.focus()
        e.preventDefault()
      } else if (!e.shiftKey && document.activeElement === last) {
        first.focus()
        e.preventDefault()
      }
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby={id} tabindex="-1" bind:this={el} {onkeydown} style:width>
  <div class="head">
    <h2 {id}>{title}</h2>
    <button class="d2-btn ghost icon close" aria-label={t('common.close')} onclick={onclose}><Icon name="x-lg" /></button>
  </div>
  <div class="body d2-scroll">{@render children()}</div>
  {#if footer}<div class="foot">{@render footer()}</div>{/if}
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: var(--d2-overlay);
  }
  .sheet {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 61;
    max-width: 100vw;
    display: flex;
    flex-direction: column;
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    outline: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    min-height: 56px;
    padding: 0 var(--d2-s3) 0 var(--d2-s5);
    border-bottom: 1px solid var(--d2-border);
  }
  .head h2 {
    flex: 1;
    font-size: var(--d2-fs-lg);
  }
  .body {
    flex: 1;
    padding: var(--d2-s5);
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: var(--d2-s2);
    padding: var(--d2-s3) var(--d2-s5);
    border-top: 1px solid var(--d2-border);
  }
</style>
