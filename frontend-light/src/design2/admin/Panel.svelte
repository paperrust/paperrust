<!-- Sidebar on the right for user/group/role: not modal (list stays visible), Esc = close (with confirmation
     on unsaved changes, the confirmation is in the panel). -->
<script lang="ts">
  import { onMount, type Snippet } from 'svelte'
  import { t } from '../../lib/i18n'
  import Icon from '../components/Icon.svelte'

  let {
    title,
    dirty,
    onclose,
    children,
    footer,
    confirm = undefined,
  }: { title: string; dirty: boolean; onclose: () => void; children: Snippet; footer: Snippet; confirm?: Snippet } = $props()
  const id = `ad-p-${Math.random().toString(36).slice(2, 8)}`
  let el: HTMLElement
  let asking = $state(false)
  let after: (() => void) | null = null

  /** Request close; with unsaved changes ask in the panel first. `then` runs after closing. */
  export function requestClose(then?: () => void) {
    if (!dirty) {
      onclose()
      then?.()
      return
    }
    after = then ?? null
    asking = true
    queueMicrotask(() => el?.querySelector<HTMLElement>('.ad-ask .keep')?.focus())
  }
  function discard() {
    asking = false
    onclose()
    after?.()
    after = null
  }
  function keep() {
    asking = false
    after = null
  }
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null
    ;(el.querySelector<HTMLElement>('[data-autofocus]') ?? el.querySelector<HTMLElement>('h2'))?.focus({ preventScroll: true })
    return () => {
      if (prev && document.contains(prev)) prev.focus({ preventScroll: true })
    }
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented) return
    e.preventDefault()
    e.stopPropagation()
    if (asking) keep()
    else requestClose()
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside class="ad-panel" aria-labelledby={id} bind:this={el} {onkeydown}>
  <div class="ad-ph">
    <h2 {id} tabindex="-1">{title}</h2>
    <button type="button" class="d2-btn ghost icon" aria-label={t('common.close')} onclick={() => requestClose()}><Icon name="x-lg" /></button>
  </div>
  <div class="ad-pb d2-scroll">{@render children()}</div>
  {#if asking}
    <div class="ad-confirm ad-ask" role="group" aria-label={t('m.admin.unsaved')}>
      <p>{t('m.admin.unsaved')}</p>
      <div class="ad-line">
        <button type="button" class="d2-btn keep" onclick={keep}>{t('m.admin.keep_editing')}</button>
        <button type="button" class="d2-btn" onclick={discard}>{t('common.discard')}</button>
      </div>
    </div>
  {:else if confirm}
    {@render confirm()}
  {/if}
  <div class="ad-pf">{@render footer()}</div>
</aside>
