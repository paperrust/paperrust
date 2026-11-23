<!-- Compact document list (concept A, middle). Continuous loading on scrolling in the pane, keyboard:
     ↑/↓ or j/k move, Enter/Space open, x selects. Click opens, ctrl/cmd-click toggles, shift-click selects a range.
     Folder view: rows can be dragged onto a folder (the whole selection travels along). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { data } from '../../stores/data.svelte'
  import { ws } from '../stores/workspace.svelte'
  import Icon from '../components/Icon.svelte'
  import type { Document } from '../../api/v1'
  import { ui } from '../stores/ui.svelte'
  import { folders } from '../stores/folders.svelte'
  import { endDrag, startDocDrag } from './folder-dnd'

  let listEl: HTMLUListElement
  let sentinel: HTMLLIElement | undefined = $state()
  const fmt = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  const date = (s: string) => {
    try {
      return fmt.format(new Date(s.length === 10 ? s + 'T00:00:00' : s))
    } catch {
      return s
    }
  }
  const corr = (d: Document) => (d.correspondent != null ? data.correspondents.get(d.correspondent)?.name : null)
  const type = (d: Document) => (d.document_type != null ? data.documentTypes.get(d.document_type)?.name : null)

  // drag & drop only in the folder view, with the mouse (touch: bulk bar "Move to folder")
  const canDrag = $derived(ws.ctx.kind === 'folder' && folders.canOrganize && !ui.mobile)

  $effect(() => {
    if (!sentinel) return
    const io = new IntersectionObserver((e) => e[0]?.isIntersecting && ws.loadMore(), { root: listEl, rootMargin: '300px' })
    io.observe(sentinel)
    return () => io.disconnect()
  })

  export function focusRow(i: number) {
    const n = Math.max(0, Math.min(ws.docs.length - 1, i))
    ws.focusIndex = n
    const el = listEl?.querySelector<HTMLElement>(`[data-i="${n}"]`)
    el?.focus()
    el?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(e: KeyboardEvent, i: number, d: Document) {
    if (e.key === 'ArrowDown') {
      focusRow(i + 1)
      e.preventDefault()
    } else if (e.key === 'ArrowUp') {
      focusRow(i - 1)
      e.preventDefault()
    } else if (e.key === 'Enter' || e.key === ' ') {
      ws.open(d.id)
      e.preventDefault()
    } else if (e.key === 'Home') {
      focusRow(0)
      e.preventDefault()
    } else if (e.key === 'End') {
      focusRow(ws.docs.length - 1)
      e.preventDefault()
    }
  }
</script>

<ul class="list d2-scroll" bind:this={listEl} role="listbox" aria-label={t('common.documents_2')} aria-multiselectable="true" id="d2-doclist" aria-busy={ws.loading}>
  {#each ws.docs as d, i (d.id)}
    {@const active = ws.ctx.activeId === d.id}
    {@const sel = ws.selected.includes(d.id)}
    <li
      class="row"
      class:active
      class:sel
      role="option"
      aria-selected={active}
      tabindex={i === ws.focusIndex ? 0 : -1}
      data-i={i}
      data-id={d.id}
      class:dragging={folders.drag?.kind === 'docs' && folders.drag.ids.includes(d.id)}
      draggable={canDrag ? 'true' : undefined}
      ondragstart={(e) => canDrag && startDocDrag(e, d.id)}
      ondragend={endDrag}
      onclick={(e) => {
        ws.click(d.id, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey })
        ws.focusIndex = i
      }}
      onkeydown={(e) => onkeydown(e, i, d)}
    >
      <span class="cb" title={t('common.select')}>
        <input type="checkbox" checked={sel} onclick={(e) => e.stopPropagation()} onchange={() => ws.toggle(d.id)} aria-label={t('m.workspace.select', { PH: d.title })} tabindex="-1" />
      </span>
      <img class="thumb" src={api.thumbUrl(d.id)} alt="" loading="lazy" decoding="async" width="36" height="46" />
      <div class="txt">
        <div class="t1">
          <span class="title">{d.title}</span>
          {#if d.notes?.length}<span class="meta-ico" title={t('common.notes')}><Icon name="chat-square-text" /></span>{/if}
        </div>
        <div class="t2">
          <span>{date(d.created)}</span>
          {#if corr(d)}<span class="dot">·</span><span class="ell">{corr(d)}</span>{/if}
          {#if type(d)}<span class="dot">·</span><span class="ell">{type(d)}</span>{/if}
        </div>
        {#if d.tags?.length}
          <div class="tags">
            {#each d.tags.slice(0, 3) as tid (tid)}
              {@const tag = data.tags.get(tid)}
              {#if tag}<span class="tag" style:--c={tag.color}>{tag.name}</span>{/if}
            {/each}
            {#if d.tags.length > 3}<span class="tag more">+{d.tags.length - 3}</span>{/if}
          </div>
        {/if}
      </div>
    </li>
  {/each}
  {#if ws.hasMore}
    <li class="more" bind:this={sentinel} aria-hidden="true">{t('common.loading')}</li>
  {/if}
  {#if !ws.loading && !ws.error && ws.docs.length === 0}
    <li class="d2-empty" role="presentation"><Icon name="files" size="2rem" /><p>{t('common.no_documents_found')}</p></li>
  {/if}
  {#if ws.error}
    <li class="d2-empty err" role="alert"><Icon name="exclamation-triangle" size="2rem" /><p>{ws.error}</p><button class="d2-btn" onclick={() => ws.reload()}>{t('common.retry')}</button></li>
  {/if}
</ul>

<style>
  .list {
    flex: 1;
    list-style: none;
    margin: 0;
    padding: var(--d2-s1) 0;
  }
  .row {
    display: grid;
    grid-template-columns: 28px 36px minmax(0, 1fr);
    align-items: start;
    gap: var(--d2-s3);
    padding: var(--d2-row-pad-y) var(--d2-s3) var(--d2-row-pad-y) var(--d2-s2);
    border-left: 3px solid transparent;
    cursor: pointer;
    outline: none;
  }
  .row:hover {
    background: var(--d2-surface-2);
  }
  .row:focus-visible {
    box-shadow: inset 0 0 0 2px var(--d2-accent);
  }
  .row.sel {
    background: var(--d2-surface-3);
  }
  .row.dragging {
    opacity: 0.55;
  }
  .row.active {
    border-left-color: var(--d2-accent);
    background: var(--d2-accent-soft);
  }
  .cb {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 46px;
    cursor: pointer;
  }
  .cb input {
    width: 16px;
    height: 16px;
    accent-color: var(--d2-accent);
  }
  .thumb {
    width: 36px;
    height: 46px;
    border: 1px solid var(--d2-border);
    border-radius: 4px;
    background: var(--d2-surface-2);
    object-fit: cover;
    object-position: top;
  }
  :global([data-theme='dark']) .thumb {
    filter: brightness(0.85);
  }
  .txt {
    min-width: 0;
  }
  .t1 {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .meta-ico {
    color: var(--d2-text-3);
  }
  .t2 {
    display: flex;
    gap: 4px;
    min-width: 0;
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
    white-space: nowrap;
  }
  .ell {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dot {
    color: var(--d2-text-3);
  }
  .tags {
    display: flex;
    gap: 4px;
    margin-top: 4px;
    overflow: hidden;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 18px;
    padding: 0 6px;
    border-radius: var(--d2-r-pill);
    background: var(--d2-surface-3);
    color: var(--d2-text);
    font-size: 11px;
    font-weight: 500;
    white-space: nowrap;
  }
  .tag::before {
    content: '';
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--c, var(--d2-text-3));
  }
  .tag.more::before {
    display: none;
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
</style>
