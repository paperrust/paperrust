<!-- Overview = modular home page (docs/DESIGN2-ENTWIRREN.md §4): every user chooses components and order
     ("Customize home page", stored in ui_settings `paperrust.home.widgets`). Components are loaded
     individually; "Question to the archive" appears only if AI is switched on. -->
<script lang="ts">
  import type { Component } from 'svelte'
  import { t, fullLocale } from '../../lib/i18n'
  import Icon from '../components/Icon.svelte'
  import { home } from '../home/home.svelte'
  import { WIDE, type WidgetId } from '../home/widgets'

  type Loader = () => Promise<{ default: Component<any> }>
  const LOAD: Record<WidgetId, Loader> = {
    ask: () => import('../home/AskWidget.svelte'),
    deadlines: () => import('../home/DeadlinesWidget.svelte'),
    inbox: () => import('../home/InboxWidget.svelte'),
    recent: () => import('../home/RecentWidget.svelte'),
    views: () => import('../home/ViewsWidget.svelte'),
    year: () => import('../home/YearWidget.svelte'),
    stats: () => import('../home/StatsWidget.svelte'),
    upload: () => import('../home/UploadWidget.svelte'),
    ai: () => import('../home/AiWidget.svelte'),
  }
  const loaded = $state<Partial<Record<WidgetId, Component<any>>>>({})
  const visible = $derived(home.visible)
  $effect(() => {
    // Texts of the components are not in the core dictionary (tools/i18n-split.mjs MODERN_LAZY) → wait for the full language file
    for (const id of visible) if (!loaded[id]) Promise.all([LOAD[id](), fullLocale()]).then(([m]) => (loaded[id] = m.default))
  })
  let customizing = $state(false)
  // Load the customize sheet only on demand (budget first call)
  let Sheet = $state<{ S: Component<any>; C: Component<any> } | null>(null)
  $effect(() => {
    if (customizing && !Sheet)
      Promise.all([import('../components/SideSheet.svelte'), import('../home/HomeCustomize.svelte'), fullLocale()]).then(([s, c]) => (Sheet = { S: s.default, C: c.default }))
  })
</script>

<div class="ov" id="d2-home">
  <header class="d2-pane-head head">
    <h1>{t('common.dashboard')}</h1>
    <span class="grow"></span>
    <button class="d2-btn ghost sm" id="d2-home-customize" onclick={() => (customizing = true)}><Icon name="pencil" />{t('m.home.customize')}</button>
  </header>

  <div class="body d2-scroll">
    {#if visible.length === 0}
      <div class="d2-empty">
        <Icon name="grid-1x2" size="2.5rem" />
        <p>{t('m.home.no_widgets')}</p>
        <button class="d2-btn" onclick={() => (customizing = true)}><Icon name="pencil" />{t('m.home.customize')}</button>
      </div>
    {:else}
      <div class="grid">
        {#each visible as id (id)}
          <div class="cell" class:wide={WIDE.includes(id)}>
            {#if loaded[id]}
              {@const W = loaded[id]}
              <W />
            {:else}
              <div class="ph d2-muted" aria-busy="true">{t('common.loading')}</div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if customizing && Sheet}
  {@const S = Sheet.S}
  {@const C = Sheet.C}
  <S title={t('m.home.customize')} onclose={() => (customizing = false)}>
    <C />
  </S>
{/if}

<style>
  .ov {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--d2-bg);
  }
  .head {
    background: var(--d2-surface);
    flex-wrap: wrap;
  }
  .grow {
    flex: 1;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--d2-s3) var(--d2-s4) var(--d2-s4);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 340px), 1fr));
    gap: var(--d2-s3);
    align-items: stretch;
  }
  .cell {
    min-width: 0;
  }
  .cell.wide {
    grid-column: 1 / -1;
  }
  .ph {
    padding: var(--d2-s4);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  @media (max-width: 767.98px) {
    .body {
      padding: var(--d2-s2);
    }
  }
</style>
