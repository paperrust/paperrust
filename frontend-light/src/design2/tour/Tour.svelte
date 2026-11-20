<!-- Tour (tour.svelte.ts): highlights the element of the step (rest dimmed) and shows a card next to it with
     next/back/finish. Keyboard: → / Enter next, ← back, Esc finishes. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { STEPS, tour, visible } from './state.svelte'

  const step = $derived(STEPS[tour.index])
  // visible steps for "x of y"
  const total = STEPS.length
  let rect = $state<DOMRect | null>(null)
  let card = $state<HTMLElement>()
  let pos = $state<{ top: number; left: number }>({ top: 0, left: 0 })

  async function measure() {
    const s = STEPS[tour.index]
    if (!s?.anchor) {
      rect = null
      return
    }
    if (!(await visible(s.anchor))) {
      rect = null
      return
    }
    const el = document.querySelector<HTMLElement>(s.anchor)
    el?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
    rect = el ? el.getBoundingClientRect() : null
    place()
  }

  function place() {
    const w = card?.offsetWidth ?? 320
    const h = card?.offsetHeight ?? 160
    if (!rect) {
      pos = { top: Math.max(16, innerHeight / 2 - h / 2), left: Math.max(16, innerWidth / 2 - w / 2) }
      return
    }
    const gap = 12
    let top: number
    let left: number
    if (rect.right + gap + w < innerWidth && rect.height < innerHeight * 0.6 && rect.width < innerWidth * 0.4) {
      // to the right next to it (navigation)
      left = rect.right + gap
      top = rect.top + rect.height / 2 - h / 2
    } else if (rect.bottom + gap + h < innerHeight) {
      top = rect.bottom + gap
      left = rect.left + rect.width / 2 - w / 2
    } else if (rect.top - gap - h > 0) {
      top = rect.top - gap - h
      left = rect.left + rect.width / 2 - w / 2
    } else {
      // large element (list/home page): card at the bottom inside
      top = Math.min(innerHeight - h - 16, rect.top + 24)
      left = rect.left + rect.width / 2 - w / 2
    }
    pos = { top: Math.max(8, Math.min(top, innerHeight - h - 8)), left: Math.max(8, Math.min(left, innerWidth - w - 8)) }
  }

  $effect(() => {
    void tour.index
    void router.path
    void measure()
  })

  onMount(() => {
    const re = () => void measure()
    addEventListener('resize', re)
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') tour.end(false)
      else if (e.key === 'ArrowRight' || e.key === 'Enter') void tour.step(1)
      else if (e.key === 'ArrowLeft') void tour.step(-1)
      else return
      e.preventDefault()
      e.stopPropagation()
    }
    addEventListener('keydown', key, true)
    return () => {
      removeEventListener('resize', re)
      removeEventListener('keydown', key, true)
    }
  })
</script>

<div class="tour" role="dialog" aria-modal="true" aria-labelledby="d2-tour-title" aria-describedby="d2-tour-text" id="d2-tour">
  {#if rect}
    <div class="spot" style:top="{rect.top - 6}px" style:left="{rect.left - 6}px" style:width="{rect.width + 12}px" style:height="{rect.height + 12}px"></div>
  {:else}
    <div class="dim"></div>
  {/if}
  <div class="card" bind:this={card} style:top="{pos.top}px" style:left="{pos.left}px">
    <p class="count">{t('m.tour.step', { PH: tour.index + 1, PH_1: total })}</p>
    <h2 id="d2-tour-title">{t(`m.tour.${step.k}_title`)}</h2>
    <p id="d2-tour-text">{t(`m.tour.${step.k}_text`)}</p>
    <div class="actions">
      <button class="d2-btn ghost sm" onclick={() => tour.end(false)} id="d2-tour-end">{t('m.tour.end')}</button>
      <span class="grow"></span>
      {#if tour.index > 0}<button class="d2-btn sm" onclick={() => tour.step(-1)} id="d2-tour-prev">{t('m.tour.back')}</button>{/if}
      <button class="d2-btn primary sm" onclick={() => tour.step(1)} id="d2-tour-next">{tour.index === total - 1 ? t('m.tour.done') : t('m.tour.next')}</button>
    </div>
  </div>
</div>

<style>
  .tour {
    position: fixed;
    inset: 0;
    z-index: 2000;
  }
  .dim {
    position: absolute;
    inset: 0;
    background: rgb(0 0 0 / 0.45);
  }
  .spot {
    position: fixed;
    border-radius: var(--d2-r2, 8px);
    box-shadow: 0 0 0 9999px rgb(0 0 0 / 0.45);
    outline: 2px solid var(--d2-accent, #b7410e);
    transition: all 0.2s ease;
    pointer-events: none;
  }
  .card {
    position: fixed;
    width: min(340px, calc(100vw - 16px));
    background: var(--d2-surface);
    color: var(--d2-text);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2, 8px);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.25);
    padding: var(--d2-s4, 16px);
  }
  .count {
    margin: 0 0 4px;
    font-size: var(--d2-fs-sm, 0.8rem);
    color: var(--d2-text-2);
  }
  h2 {
    margin: 0 0 6px;
    font-size: 1.05rem;
  }
  #d2-tour-text {
    margin: 0 0 var(--d2-s4, 16px);
    line-height: 1.45;
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .grow {
    flex: 1;
  }
  @media (prefers-reduced-motion: reduce) {
    .spot {
      transition: none;
    }
  }
</style>
