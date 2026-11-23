<!-- Folder view stage 2 (results/ideas/2026-11-22-papermerge-ordneransicht.md): tiles instead of the list. First the
     subfolders of the open folder (at "Unsorted" = folder root: the top-level folders) – double-click or Enter opens,
     documents/folders can be dropped on them – then the documents as thumbnails with title. Selection like the list:
     click opens, ctrl/cmd-click toggles, shift-click selects a range; selected tiles can be dragged onto a folder.
     Keyboard: arrow keys move in the grid, Enter opens, x/Space selects (Space like the checkbox). -->
<script lang="ts">
  import { untrack } from 'svelte'
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { router } from '../../router.svelte'
  import { ws } from '../stores/workspace.svelte'
  import { ui } from '../stores/ui.svelte'
  import { folders } from '../stores/folders.svelte'
  import { folderRoute } from '../api/folders'
  import { childFolders, gridStep } from '../lib/folder-dnd'
  import { dragLeave, dragOver, drop, endDrag, startDocDrag, startFolderDrag } from './folder-dnd'
  import Icon from '../components/Icon.svelte'

  let gridEl: HTMLDivElement
  let sentinel: HTMLDivElement | undefined = $state()
  const subfolders = $derived(ws.ctx.kind === 'folder' ? childFolders(folders.items, ws.ctx.folder ?? 'unsorted') : [])
  const canDrag = $derived(ws.ctx.kind === 'folder' && folders.canOrganize && !ui.mobile)
  /** focus index over folder tiles followed by document tiles */
  let focus = $state(0)
  const count = $derived(subfolders.length + ws.docs.length)
  /** tabbable tile (roving tabindex), back to the first one after the folder changed */
  const tabFocus = $derived(focus < count ? focus : 0)

  $effect(() => {
    if (!sentinel) return
    const io = new IntersectionObserver((e) => e[0]?.isIntersecting && ws.loadMore(), { root: gridEl, rootMargin: '300px' })
    io.observe(sentinel)
    return () => io.disconnect()
  })
  // follow the shared focus index when j/k moves it (only reacts to ws.focusIndex, not to own focus changes)
  $effect(() => {
    const i = ws.focusIndex + subfolders.length
    untrack(() => {
      if (i < count && focus >= subfolders.length) focus = i
    })
  })

  function columns(): number {
    const tiles = gridEl?.querySelectorAll<HTMLElement>('.tile')
    if (!tiles?.length) return 1
    const top = tiles[0].offsetTop
    let n = 0
    for (const el of tiles) {
      if (el.offsetTop !== top) break
      n++
    }
    return Math.max(1, n)
  }

  function focusTile(i: number) {
    focus = i
    if (i >= subfolders.length) ws.focusIndex = i - subfolders.length
    const el = gridEl?.querySelector<HTMLElement>(`[data-t="${i}"]`)
    el?.focus()
    el?.scrollIntoView({ block: 'nearest' })
    if (i >= count - 5) ws.loadMore()
  }

  function onkeydown(e: KeyboardEvent, i: number) {
    const n = gridStep(e.key, i, count, columns())
    if (n !== null) {
      focusTile(n)
      e.preventDefault()
      return
    }
    if (e.key === 'Enter') {
      if (i < subfolders.length) router.navigate(folderRoute(subfolders[i].id))
      else ws.open(ws.docs[i - subfolders.length].id)
      e.preventDefault()
    } else if (e.key === ' ' && i >= subfolders.length) {
      ws.toggle(ws.docs[i - subfolders.length].id)
      e.preventDefault()
    }
  }
</script>

<div class="grid d2-scroll" bind:this={gridEl} role="listbox" aria-label={t('common.documents_2')} aria-multiselectable="true" id="d2-doclist" aria-busy={ws.loading}>
  {#each subfolders as f, i (`f${f.id}`)}
    <div
      class="tile folder"
      class:drop={folders.dropOver === f.id}
      class:dragging={folders.drag?.kind === 'folder' && folders.drag.id === f.id}
      role="option"
      aria-selected="false"
      tabindex={i === tabFocus ? 0 : -1}
      data-t={i}
      data-folder={f.id}
      title={t('m.folders.open_hint')}
      draggable={canDrag && f.user_can_change ? 'true' : undefined}
      ondragstart={(e) => canDrag && startFolderDrag(e, f.id, f.name)}
      ondragend={endDrag}
      ondragover={(e) => dragOver(e, f.id)}
      ondragleave={(e) => dragLeave(e, f.id)}
      ondrop={(e) => drop(e, f.id)}
      onclick={() => (focus = i)}
      ondblclick={() => router.navigate(folderRoute(f.id))}
      onkeydown={(e) => onkeydown(e, i)}
    >
      <span class="ficon"><Icon name="folder2" size="2.6rem" /></span>
      <span class="name">{f.name}</span>
      {#if f.document_count !== undefined}<span class="cnt">{t('common.documents', { PH: f.document_count })}</span>{/if}
      <!-- touch/one tap: an explicit open button (double-click is awkward on phones) -->
      {#if ui.mobile}<a class="open" href={router.href(folderRoute(f.id))} aria-label={t('m.folders.open', { PH: f.name })}><Icon name="chevron-right" /></a>{/if}
    </div>
  {/each}
  {#each ws.docs as d, j (d.id)}
    {@const i = subfolders.length + j}
    {@const active = ws.ctx.activeId === d.id}
    {@const sel = ws.selected.includes(d.id)}
    <div
      class="tile doc"
      class:active
      class:sel
      class:dragging={folders.drag?.kind === 'docs' && folders.drag.ids.includes(d.id)}
      role="option"
      aria-selected={sel}
      tabindex={i === tabFocus ? 0 : -1}
      data-t={i}
      data-i={j}
      data-id={d.id}
      draggable={canDrag ? 'true' : undefined}
      ondragstart={(e) => canDrag && startDocDrag(e, d.id)}
      ondragend={endDrag}
      onclick={(e) => {
        ws.click(d.id, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey })
        focus = i
        ws.focusIndex = j
      }}
      onkeydown={(e) => onkeydown(e, i)}
    >
      <span class="thumbbox">
        <img class="thumb" src={api.thumbUrl(d.id)} alt="" loading="lazy" decoding="async" draggable="false" />
        <span class="cb">
          <input type="checkbox" checked={sel} onclick={(e) => e.stopPropagation()} onchange={() => ws.toggle(d.id)} aria-label={t('m.workspace.select', { PH: d.title })} tabindex="-1" />
        </span>
      </span>
      <span class="name" title={d.title}>{d.title}</span>
    </div>
  {/each}
  {#if ws.hasMore}
    <div class="more" bind:this={sentinel} aria-hidden="true">{t('common.loading')}</div>
  {/if}
  {#if !ws.loading && !ws.error && count === 0}
    <div class="d2-empty wide" role="presentation"><Icon name="files" size="2rem" /><p>{t('common.no_documents_found')}</p></div>
  {/if}
  {#if ws.error}
    <div class="d2-empty wide err" role="alert"><Icon name="exclamation-triangle" size="2rem" /><p>{ws.error}</p><button class="d2-btn" onclick={() => ws.reload()}>{t('common.retry')}</button></div>
  {/if}
</div>

<style>
  .grid {
    flex: 1;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
    grid-auto-rows: max-content;
    align-content: start;
    gap: var(--d2-s3);
    padding: var(--d2-s3);
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: var(--d2-s2);
    border: 1px solid transparent;
    border-radius: var(--d2-r2, 8px);
    cursor: pointer;
    outline: none;
    user-select: none;
  }
  .tile:hover {
    background: var(--d2-surface-2);
  }
  .tile:focus-visible {
    box-shadow: 0 0 0 2px var(--d2-accent);
  }
  .tile.sel {
    background: var(--d2-surface-3);
    border-color: var(--d2-border);
  }
  .tile.active {
    background: var(--d2-accent-soft);
    border-color: var(--d2-accent);
  }
  .tile.drop {
    background: var(--d2-accent-soft);
    box-shadow: inset 0 0 0 2px var(--d2-accent);
  }
  .tile.dragging {
    opacity: 0.5;
  }
  .ficon {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 96px;
    color: var(--d2-accent);
  }
  .thumbbox {
    position: relative;
    width: 100%;
    max-width: 120px;
    aspect-ratio: 3 / 4;
  }
  .thumb {
    width: 100%;
    height: 100%;
    border: 1px solid var(--d2-border);
    border-radius: 4px;
    background: var(--d2-surface-2);
    object-fit: cover;
    object-position: top;
  }
  :global([data-theme='dark']) .thumb {
    filter: brightness(0.85);
  }
  .cb {
    position: absolute;
    top: 4px;
    left: 4px;
    display: flex;
    padding: 2px;
    border-radius: 4px;
    background: var(--d2-surface);
    opacity: 0;
  }
  .tile:hover .cb,
  .tile.sel .cb,
  .tile:focus-visible .cb {
    opacity: 1;
  }
  .cb input {
    width: 16px;
    height: 16px;
    margin: 0;
    accent-color: var(--d2-accent);
  }
  .name {
    width: 100%;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    text-align: center;
    font-size: var(--d2-fs-sm);
    line-height: 1.3;
    word-break: break-word;
  }
  .folder .name {
    font-weight: 600;
  }
  .cnt {
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
  }
  .open {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: flex-start;
    justify-content: flex-end;
    padding: 6px;
    color: var(--d2-text-3);
  }
  .more,
  .wide {
    grid-column: 1 / -1;
  }
  .more {
    padding: var(--d2-s3);
    color: var(--d2-text-3);
    text-align: center;
    font-size: var(--d2-fs-sm);
  }
  .err {
    color: var(--d2-danger);
  }
  /* phones: two or three columns, smaller folder icons */
  @media (max-width: 767.98px) {
    .grid {
      grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
      gap: var(--d2-s2);
      padding: var(--d2-s2);
    }
    .ficon {
      height: 72px;
    }
    .cb {
      opacity: 1;
    }
  }
</style>
