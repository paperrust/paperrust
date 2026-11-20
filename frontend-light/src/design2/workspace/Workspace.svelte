<!-- Workspace (concept A): list (middle) + document (right) on one screen, fixed panes. -->
<script lang="ts">
  import { untrack } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { ws } from '../stores/workspace.svelte'
  import { ui } from '../stores/ui.svelte'
  import { onCommand } from '../lib/bus'
  import FilterBar from './FilterBar.svelte'
  import BulkBar from './BulkBar.svelte'
  import DocList from './DocList.svelte'
  import DocDetail from './DocDetail.svelte'
  import Icon from '../components/Icon.svelte'
  import { classicDialogs } from './classic-dialogs.svelte'

  $effect(() => {
    // saved views may arrive only after the first render
    const p = router.path
    const q = router.query
    void data.savedViews
    untrack(() => ws.sync(p, q))
  })
  const heading = $derived(ws.ctx.kind === 'inbox' ? t('common.inbox') : ws.ctx.kind === 'view' ? ws.title() : t('common.all_documents'))
  const showList = $derived(!ui.mobile || !ws.ctx.activeId)
  const showDetail = $derived(!ui.mobile || !!ws.ctx.activeId)

  function move(d: number) {
    if (!ws.docs.length) return
    const cur = ws.ctx.activeId ? ws.docs.findIndex((x) => x.id === ws.ctx.activeId) : ws.focusIndex - (d > 0 ? 1 : 0)
    const n = Math.max(0, Math.min(ws.docs.length - 1, cur + d))
    ws.focusIndex = n
    ws.open(ws.docs[n].id, true)
    if (n >= ws.docs.length - 5) ws.loadMore()
    queueMicrotask(() => document.querySelector(`#d2-doclist [data-i="${n}"]`)?.scrollIntoView({ block: 'nearest' }))
  }
  $effect(() =>
    onCommand((c) => {
      switch (c) {
        case 'next':
          move(1)
          return true
        case 'prev':
          move(-1)
          return true
        case 'toggle-select': {
          const id = ws.ctx.activeId ?? ws.docs[ws.focusIndex]?.id
          if (id) ws.toggle(id)
          return true
        }
        case 'select-all':
          ws.selectAll()
          return true
        case 'escape':
          if (ws.selected.length) {
            ws.clearSelection()
            return true
          }
          return false
        case 'search':
          document.getElementById('d2-filter-text')?.focus()
          return true
      }
      return false
    }),
  )
</script>

<div class="ws" class:mobile={ui.mobile}>
  {#if showList}
    <section class="d2-pane listpane" aria-label={t('m.workspace.document_list')}>
      <header class="d2-pane-head">
        <h1>{heading}</h1>
        <span class="d2-muted count" aria-live="polite">{ws.total !== null ? t('common.documents', { PH: ws.total }) : ''}</span>
      </header>
      <FilterBar />
      {#if ws.selected.length}<BulkBar />{/if}
      <DocList />
    </section>
  {/if}
  {#if showDetail}
    <div class="detailpane">
      {#if ws.ctx.activeId}
        {#key ws.ctx.activeId}<DocDetail id={ws.ctx.activeId} />{/key}
      {:else}
        <div class="d2-empty hint">
          <Icon name="file-earmark" size="2.5rem" />
          <p>{t('m.workspace.select_document_see_preview')}</p>
          <p class="keys"><span class="d2-kbd">j</span> <span class="d2-kbd">k</span> {t('m.workspace.move')}, <span class="d2-kbd">Ctrl</span>+<span class="d2-kbd">K</span> {t('m.workspace.search')}</p>
        </div>
      {/if}
    </div>
  {/if}
</div>
{#if classicDialogs.host}
  {@const Host = classicDialogs.host}
  <Host />
{/if}

<style>
  .ws {
    display: grid;
    grid-template-columns: minmax(300px, var(--d2-list-w)) minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  .ws.mobile {
    grid-template-columns: minmax(0, 1fr);
  }
  .listpane {
    min-height: 0;
    border-right: 1px solid var(--d2-border);
  }
  .count {
    margin-left: auto;
    font-size: var(--d2-fs-sm);
    white-space: nowrap;
  }
  .detailpane {
    min-width: 0;
    min-height: 0;
    background: var(--d2-surface);
  }
  .hint {
    background: var(--d2-bg);
  }
  .keys {
    font-size: var(--d2-fs-sm);
  }
</style>
