<!-- "Customize home page": components on/off and order (dragging, up/down buttons or Alt+arrow up/down in the
     row). Every change is saved immediately (ui_settings `paperrust.home.widgets`, per user).
     Usable standalone, e.g. in Settings → Appearance. -->
<script lang="ts">
  import { tick } from 'svelte'
  import { t, msg } from '../../lib/i18n'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import { home } from './home.svelte'
  import { available, moveWidget, moveWidgetTo, toggleWidget, type WidgetEntry, type WidgetId } from './widgets'

  let { idPrefix = 'd2-hc' }: { idPrefix?: string } = $props()

  const LABEL: Record<WidgetId, [string, string, string]> = {
    ask: [msg('m.home.ask_archive'), msg('m.home.ask_archive_hint'), 'stars'],
    deadlines: [msg('m.home.deadlines'), msg('m.home.deadlines_hint'), 'calendar-event'],
    inbox: [msg('common.inbox'), msg('m.home.inbox_hint'), 'inbox'],
    recent: [msg('m.home.recent'), msg('m.home.recent_hint'), 'clock-history'],
    views: [msg('common.saved_views'), msg('m.home.views_hint'), 'bookmark'],
    year: [msg('m.home.year_bar'), msg('m.home.year_bar_hint'), 'calendar3'],
    stats: [msg('dashboard.statistics'), msg('m.home.stats_hint'), 'grid-1x2'],
    upload: [msg('common.upload'), msg('m.home.upload_hint'), 'cloud-arrow-up'],
    ai: [msg('m.ai.review_title'), msg('m.ai.widget_hint'), 'stars'],
  }

  const list = $derived(home.list)
  const aiOn = $derived(home.aiOn)
  let status = $state('')
  let dragFrom = $state<number | null>(null)
  let dragOver = $state<number | null>(null)
  let root: HTMLElement

  async function save(next: WidgetEntry[] | null) {
    try {
      await home.save(next)
      status = t('m.home.saved')
    } catch (e) {
      status = ''
      toastService.showError(t('common.could_not_save'), e)
    }
  }
  /** move; afterwards focus on `keepId` (keyboard in the row) or on the arrow button of the row */
  async function move(i: number, delta: number, keepId = '') {
    const j = Math.max(0, Math.min(list.length - 1, i + delta))
    if (j === i) return
    const id = list[i].id
    await save(moveWidget(list, i, delta))
    await tick()
    const row = root?.querySelector<HTMLElement>(`[data-w="${id}"]`)
    const keep = (keepId ? document.getElementById(keepId) : null) ?? row?.querySelector<HTMLElement>(`.${delta < 0 ? 'up' : 'down'}:not([disabled])`) ?? row?.querySelector<HTMLElement>('input')
    keep?.focus()
  }
  function onrowkey(e: KeyboardEvent, i: number) {
    if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return
    e.preventDefault()
    move(i, e.key === 'ArrowUp' ? -1 : 1, (e.target as HTMLElement).id)
  }
  function ondrop(e: DragEvent, i: number) {
    e.preventDefault()
    if (dragFrom !== null && dragFrom !== i) save(moveWidgetTo(list, dragFrom, i))
    dragFrom = dragOver = null
  }
</script>

<div class="hc" bind:this={root}>
  <p class="d2-muted intro">{t('m.home.customize_intro')}</p>
  <ul class="items" aria-label={t('m.home.customize')}>
    {#each list as w, i (w.id)}
      {@const [label, hint, icon] = LABEL[w.id]}
      {@const avail = available(w.id, aiOn)}
      <!-- Dragging is only a shortcut; keyboard: up/down buttons or Alt+arrow (event comes from the controls) -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <li
        data-w={w.id}
        class:off={!w.on || !avail}
        class:over={dragOver === i && dragFrom !== i}
        draggable="true"
        ondragstart={(e) => {
          dragFrom = i
          e.dataTransfer?.setData('text/plain', w.id)
          if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
        }}
        ondragover={(e) => {
          if (dragFrom === null) return
          e.preventDefault()
          dragOver = i
        }}
        ondragleave={() => dragOver === i && (dragOver = null)}
        ondrop={(e) => ondrop(e, i)}
        ondragend={() => (dragFrom = dragOver = null)}
        onkeydown={(e) => onrowkey(e, i)}
      >
        <span class="grip" aria-hidden="true" title={t('m.home.drag_to_move')}>⋮⋮</span>
        <label class="lbl" for="{idPrefix}-{w.id}">
          {#if avail}<input type="checkbox" id="{idPrefix}-{w.id}" checked={w.on} onchange={(e) => save(toggleWidget(list, w.id, (e.target as HTMLInputElement).checked))} aria-describedby="{idPrefix}-{w.id}-d" />{:else}<span class="preview-badge" id="{idPrefix}-{w.id}">{t('m.home.preview')}</span>{/if}
          <Icon name={icon} />
          <span class="txt">
            <span class="nm">{t(label)}</span>
            <span class="d2-muted hint" id="{idPrefix}-{w.id}-d">{avail ? t(hint) : t('m.home.ask_needs_ai')}</span>
          </span>
        </label>
        <span class="mv">
          <button class="d2-btn ghost sm icon up" disabled={i === 0} onclick={() => move(i, -1)} aria-label={`${t('m.settings.move_up')}: ${t(label)}`} title={t('m.settings.move_up')}><Icon name="sort-up" /></button>
          <button class="d2-btn ghost sm icon down" disabled={i === list.length - 1} onclick={() => move(i, 1)} aria-label={`${t('m.settings.move_down')}: ${t(label)}`} title={t('m.settings.move_down')}><Icon name="sort-down" /></button>
        </span>
      </li>
    {/each}
  </ul>
  <div class="foot">
    <button class="d2-btn sm" onclick={() => save(null)} id="{idPrefix}-reset"><Icon name="arrow-clockwise" />{t('m.home.restore_default')}</button>
    <span class="d2-muted st" role="status" aria-live="polite">{status}</span>
  </div>
</div>

<style>
  .intro {
    margin: 0 0 var(--d2-s3);
    font-size: var(--d2-fs-sm);
  }
  .items {
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    padding: var(--d2-s2) var(--d2-s2) var(--d2-s2) var(--d2-s1);
  }
  li + li {
    border-top: 1px solid var(--d2-border);
  }
  li.over {
    box-shadow: inset 0 2px 0 var(--d2-accent);
  }
  li.off .nm {
    color: var(--d2-text-2);
  }
  .grip {
    flex: none;
    width: 16px;
    color: var(--d2-text-3);
    letter-spacing: -3px;
    cursor: grab;
    user-select: none;
    text-align: center;
  }
  .lbl {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    cursor: pointer;
  }
  .lbl input {
    flex: none;
    accent-color: var(--d2-accent);
  }
  .lbl :global(.d2-ico) {
    flex: none;
    color: var(--d2-text-2);
  }
  .txt {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .nm {
    font-weight: 600;
  }
  .hint {
    font-size: var(--d2-fs-xs);
  }
  .mv {
    flex: none;
    display: flex;
    gap: 2px;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    margin-top: var(--d2-s3);
  }
  .st {
    font-size: var(--d2-fs-sm);
  }
  .preview-badge { font-size: var(--d2-fs-xs); border: 1px solid var(--d2-border); border-radius: 999px; padding: 0 var(--d2-s2); color: var(--d2-text-3); white-space: nowrap; }
</style>
