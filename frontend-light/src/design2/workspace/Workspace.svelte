<!-- Workspace (concept A): list (middle) + document (right) on one screen, fixed panes.
     Folder view: tree on the left; stage 2 adds the list/tiles switch (tiles use the whole width until a document is
     opened) and uploads while a folder is open land in that folder. -->
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
  import FolderTree from './FolderTree.svelte'
  import DocTiles from './DocTiles.svelte'
  import { folders } from '../stores/folders.svelte'
  import { folderPath, folderRoute } from '../api/folders'
  import { consumerStatus } from '../../stores/uploads.svelte'

  $effect(() => {
    // saved views may arrive only after the first render
    const p = router.path
    const q = router.query
    void data.savedViews
    untrack(() => ws.sync(p, q))
  })
  const heading = $derived(ws.ctx.kind === 'inbox' ? t('common.inbox') : ws.ctx.kind === 'view' ? ws.title() : t('common.all_documents'))
  // folder view: breadcrumbs "Folders › A › B" (or "Unsorted")
  const inFolders = $derived(ws.ctx.kind === 'folder')
  const crumbs = $derived(inFolders && typeof ws.ctx.folder === 'number' ? folderPath(folders.items, ws.ctx.folder) : [])
  const tiles = $derived(inFolders && folders.view === 'tiles')
  const showList = $derived(!ui.mobile || !ws.ctx.activeId)
  // tiles without an open document: no empty preview pane, the tiles take the width
  const showDetail = $derived(ui.mobile ? !!ws.ctx.activeId : !tiles || !!ws.ctx.activeId)

  // uploads while a (changeable) folder is open go into that folder (upload field `folder`, PaperRust only)
  $effect(() => {
    const key = ws.ctx.folder
    const f = inFolders && typeof key === 'number' ? folders.byId(key) : undefined
    consumerStatus.folder = f && f.user_can_change && folders.canOrganize ? { id: f.id, name: f.name } : null
    return () => {
      consumerStatus.folder = null
    }
  })
  // new documents of an upload show up in the open folder (and its count)
  $effect(() =>
    consumerStatus.onDocumentConsumptionFinished(() => {
      if (ws.ctx.kind !== 'folder') return
      void folders.refresh()
      void ws.reload()
    }),
  )

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

<div class="ws" class:mobile={ui.mobile} class:folders={inFolders && showList} class:wide={tiles && !showDetail}>
  {#if inFolders && showList}
    <FolderTree current={ws.ctx.folder ?? 'unsorted'} />
  {/if}
  {#if showList}
    <section class="d2-pane listpane" aria-label={t('m.workspace.document_list')}>
      <header class="d2-pane-head">
        {#if inFolders}
          <h1 class="crumbs">
            <a href={router.href(folderRoute('unsorted'))}>{t('m.folders.title')}</a>
            {#if ws.ctx.folder === 'unsorted'}
              <span aria-hidden="true">›</span><span>{t('m.folders.unsorted')}</span>
            {:else}
              {#each crumbs as c, i (c.id)}
                <span aria-hidden="true">›</span>
                {#if i < crumbs.length - 1}<a href={router.href(folderRoute(c.id))}>{c.name}</a>{:else}<span aria-current="page">{c.name}</span>{/if}
              {/each}
            {/if}
          </h1>
        {:else}
          <h1>{heading}</h1>
        {/if}
        <span class="d2-muted count" aria-live="polite">{ws.total !== null ? t('common.documents', { PH: ws.total }) : ''}</span>
        {#if inFolders}
          <div class="viewmode" role="group" aria-label={t('m.folders.view_mode')}>
            <button class="d2-btn ghost sm icon" class:on={!tiles} aria-pressed={!tiles} title={t('m.folders.view_list')} aria-label={t('m.folders.view_list')} id="d2-folder-view-list" onclick={() => folders.setView('list')}><Icon name="view-list" /></button>
            <button class="d2-btn ghost sm icon" class:on={tiles} aria-pressed={tiles} title={t('m.folders.view_tiles')} aria-label={t('m.folders.view_tiles')} id="d2-folder-view-tiles" onclick={() => folders.setView('tiles')}><Icon name="grid-3x3-gap" /></button>
          </div>
        {/if}
      </header>
      <FilterBar />
      {#if ws.selected.length}<BulkBar />{/if}
      {#if tiles}<DocTiles />{:else}<DocList />{/if}
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
  .ws.folders {
    grid-template-columns: minmax(180px, 240px) minmax(300px, var(--d2-list-w)) minmax(0, 1fr);
  }
  .ws.folders.wide {
    grid-template-columns: minmax(180px, 240px) minmax(0, 1fr);
  }
  .ws.folders.mobile {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 35%) minmax(0, 1fr);
  }
  .crumbs {
    display: flex;
    flex-wrap: wrap;
    gap: 0 6px;
    align-items: baseline;
    min-width: 0;
  }
  .crumbs a {
    color: var(--d2-text-2);
    text-decoration: none;
  }
  .crumbs a:hover {
    text-decoration: underline;
  }
  .listpane {
    min-height: 0;
    border-right: 1px solid var(--d2-border);
  }
  .viewmode {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .viewmode .on {
    background: var(--d2-surface-3);
    color: var(--d2-text);
  }
  /* drag image of documents/folders (folder-dnd.ts), briefly in the DOM during dragstart */
  :global(.d2-dragbadge) {
    position: fixed;
    top: -100px;
    left: -100px;
    padding: 4px 10px;
    border-radius: var(--d2-r-pill);
    background: var(--d2-accent);
    color: #fff;
    font-size: var(--d2-fs-sm);
    font-weight: 600;
    white-space: nowrap;
    pointer-events: none;
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
