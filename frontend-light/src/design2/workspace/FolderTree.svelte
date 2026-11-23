<!-- Folder view (PaperRust only, results/ideas/2026-11-22-papermerge-ordneransicht.md stage 1): folder tree on the left of
     the workspace. On top the virtual "Unsorted (n)", below the own folders with their document count. New / rename /
     delete act on the selected folder; deleting never deletes documents (contents go to the parent folder or
     "Unsorted"). Data: GET/POST/PATCH/DELETE /rs/api/v2/folders.
     Stage 2: every row is a drop target for dragged documents (move-documents) and folders (re-parent; "Unsorted" =
     top level); folder rows can be dragged (mouse only). Rules: ../lib/folder-dnd.ts, events: ./folder-dnd.ts. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import { folders } from '../stores/folders.svelte'
  import { createFolder, deleteFolder, folderRoute, renameFolder, treeRows, type FolderKey } from '../api/folders'
  import { ui } from '../stores/ui.svelte'
  import { dragLeave, dragOver, drop, endDrag, startFolderDrag } from './folder-dnd'

  let { current }: { current: FolderKey } = $props()

  const rows = $derived(treeRows(folders.items, folders.collapsed))
  const sel = $derived(current === 'unsorted' ? undefined : folders.byId(current))
  let editing = $state<'new' | 'rename' | null>(null)
  let name = $state('')
  let busy = $state(false)
  let confirmDelete = $state(false)
  let contents = $state<'parent' | 'unsorted'>('parent')

  $effect(() => folders.ensure())

  // focus the name field when "New folder"/"Rename" opens it (the autofocus attribute did not fire reliably)
  let nameInput = $state<HTMLInputElement>()
  $effect(() => {
    if (!editing || !nameInput) return
    nameInput.focus()
    if (editing === 'rename') nameInput.select()
  })
  const canDragFolders = $derived(folders.canOrganize && !ui.mobile)

  function startNew() {
    editing = 'new'
    name = ''
  }
  function startRename() {
    if (!sel) return
    editing = 'rename'
    name = sel.name
  }
  async function save(e: SubmitEvent) {
    e.preventDefault()
    const n = name.trim()
    if (!n || busy) return
    busy = true
    try {
      if (editing === 'new') {
        const f = await createFolder(n, sel?.id ?? null)
        toastService.showSuccess(t('m.folders.created', { PH: f.name }))
        await folders.refresh()
        router.navigate(folderRoute(f.id))
      } else if (editing === 'rename' && sel) {
        await renameFolder(sel.id, n)
        await folders.refresh()
      }
      editing = null
    } catch (err) {
      toastService.showError(t('m.folders.could_not_save'), err)
    } finally {
      busy = false
    }
  }
  async function remove() {
    if (!sel) return
    const parent = sel.parent
    busy = true
    try {
      const r = await deleteFolder(sel.id, contents)
      confirmDelete = false
      toastService.showSuccess(t('m.folders.deleted', { VAR_PLURAL: r.moved_documents, PH: r.moved_documents }))
      await folders.refresh()
      router.navigate(folderRoute(contents === 'parent' && parent !== null ? parent : 'unsorted'))
    } catch (err) {
      toastService.showError(t('m.folders.could_not_delete'), err)
    } finally {
      busy = false
    }
  }
</script>

<nav class="ft" aria-label={t('m.folders.title')} id="d2-folder-tree">
  <div class="head">
    <h2>{t('m.folders.title')}</h2>
    {#if folders.canOrganize}
      <button class="d2-btn ghost sm icon" onclick={startNew} title={sel ? t('m.folders.new_subfolder') : t('m.folders.new_folder')} aria-label={sel ? t('m.folders.new_subfolder') : t('m.folders.new_folder')} id="d2-folder-new"><Icon name="folder-plus" /></button>
      <button class="d2-btn ghost sm icon" onclick={startRename} disabled={!sel?.user_can_change} title={t('m.folders.rename')} aria-label={t('m.folders.rename')} id="d2-folder-rename"><Icon name="pencil" /></button>
      <button class="d2-btn ghost sm icon" onclick={() => ((contents = 'parent'), (confirmDelete = true))} disabled={!sel?.user_can_delete} title={t('m.folders.delete')} aria-label={t('m.folders.delete')} id="d2-folder-delete"><Icon name="trash" /></button>
    {/if}
  </div>
  {#if editing}
    <form class="edit" onsubmit={save}>
      <input class="d2-input" bind:this={nameInput} bind:value={name} maxlength="128" aria-label={t('m.folders.name')} placeholder={editing === 'new' ? (sel ? t('m.folders.new_subfolder') : t('m.folders.new_folder')) : t('m.folders.name')} id="d2-folder-name"
        onkeydown={(e) => e.key === 'Escape' && (e.stopPropagation(), (editing = null))} />
      <button class="d2-btn primary sm" type="submit" disabled={busy || !name.trim()}>{t('common.save')}</button>
      <button class="d2-btn ghost sm" type="button" onclick={() => (editing = null)}>{t('common.cancel')}</button>
    </form>
  {/if}
  <ul class="d2-scroll" role="tree">
    <li role="treeitem" aria-selected={current === 'unsorted'}>
      <a class="row" class:active={current === 'unsorted'} class:drop={folders.dropOver === 'unsorted'} href={router.href(folderRoute('unsorted'))} id="d2-folder-unsorted"
        ondragover={(e) => dragOver(e, 'unsorted')} ondragleave={(e) => dragLeave(e, 'unsorted')} ondrop={(e) => drop(e, 'unsorted')}>
        <span class="tw"></span><Icon name="inbox" /><span class="nm">{t('m.folders.unsorted')}</span>
        {#if folders.unsorted !== null}<span class="cnt">{folders.unsorted}</span>{/if}
      </a>
    </li>
    {#each rows as r (r.folder.id)}
      <li role="treeitem" aria-selected={current === r.folder.id} aria-expanded={r.hasChildren ? !folders.collapsed.has(r.folder.id) : undefined}>
        <div class="row" class:active={current === r.folder.id} class:drop={folders.dropOver === r.folder.id} class:dragging={folders.drag?.kind === 'folder' && folders.drag.id === r.folder.id}
          style:padding-left={`${0.25 + r.depth * 1}rem`} data-folder={r.folder.id}
          draggable={canDragFolders && r.folder.user_can_change ? 'true' : undefined}
          ondragstart={(e) => canDragFolders && startFolderDrag(e, r.folder.id, r.folder.name)} ondragend={endDrag}
          ondragover={(e) => dragOver(e, r.folder.id)} ondragleave={(e) => dragLeave(e, r.folder.id)} ondrop={(e) => drop(e, r.folder.id)}
          role="presentation">
          {#if r.hasChildren}
            <button class="tw" type="button" onclick={() => folders.toggle(r.folder.id)} aria-label={folders.collapsed.has(r.folder.id) ? t('m.folders.expand') : t('m.folders.collapse')}>
              <Icon name={folders.collapsed.has(r.folder.id) ? 'chevron-right' : 'chevron-down'} size="0.8rem" />
            </button>
          {:else}<span class="tw"></span>{/if}
          <a class="lnk" href={router.href(folderRoute(r.folder.id))}>
            <Icon name={current === r.folder.id ? 'folder2-open' : 'folder'} /><span class="nm">{r.folder.name}</span>
            {#if r.folder.document_count !== undefined}<span class="cnt">{r.folder.document_count}</span>{/if}
          </a>
        </div>
      </li>
    {/each}
  </ul>
  {#if folders.loaded && !folders.items.length && !folders.error}
    <p class="d2-muted hint">{t('m.folders.no_folders_yet')}</p>
  {/if}
  {#if folders.error}<p class="d2-muted hint" role="alert">{t('m.folders.could_not_load')}</p>{/if}
</nav>

{#if confirmDelete && sel}
  <SideSheet title={t('m.folders.delete_title', { PH: sel.name })} onclose={() => (confirmDelete = false)} width="440px">
    <p>{t('m.folders.delete_hint')}</p>
    <fieldset class="choices">
      <label class="d2-check"><input type="radio" name="d2-folder-contents" value="parent" bind:group={contents} id="d2-folder-contents-parent" />{sel.parent !== null ? t('m.folders.contents_to_parent') : t('m.folders.contents_to_top')}</label>
      <label class="d2-check"><input type="radio" name="d2-folder-contents" value="unsorted" bind:group={contents} id="d2-folder-contents-unsorted" />{t('m.folders.contents_to_unsorted')}</label>
    </fieldset>
    {#snippet footer()}
      <button class="d2-btn" onclick={() => (confirmDelete = false)}>{t('common.cancel')}</button>
      <button class="d2-btn primary" id="d2-folder-delete-confirm" disabled={busy} onclick={remove}>{t('common.delete')}</button>
    {/snippet}
  </SideSheet>
{/if}

<style>
  .ft {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--d2-border);
    background: var(--d2-bg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: var(--d2-s3) var(--d2-s2) var(--d2-s2) var(--d2-s3);
  }
  .head h2 {
    flex: 1;
    margin: 0;
    font-size: var(--d2-fs-sm);
    font-weight: 600;
  }
  .edit {
    display: flex;
    gap: 4px;
    padding: 0 var(--d2-s2) var(--d2-s2);
  }
  .edit input {
    flex: 1;
    min-width: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0 var(--d2-s2) var(--d2-s2);
    flex: 1;
    min-height: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: var(--d2-r1);
    color: var(--d2-text);
    text-decoration: none;
    min-height: 30px;
    padding-right: var(--d2-s2);
  }
  a.row {
    padding-left: 0.25rem;
  }
  .row:hover {
    background: var(--d2-surface-2);
  }
  .row.active {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text, var(--d2-text));
  }
  /* drop highlight while dragging documents/folders over a row */
  .row.drop {
    background: var(--d2-accent-soft);
    box-shadow: inset 0 0 0 2px var(--d2-accent);
  }
  .row.dragging {
    opacity: 0.5;
  }
  .lnk {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
    color: inherit;
    text-decoration: none;
  }
  a.row :global(svg) {
    margin-right: 2px;
  }
  .tw {
    width: 1.1rem;
    flex: none;
    display: inline-flex;
    justify-content: center;
    background: none;
    border: 0;
    padding: 0;
    color: var(--d2-text-3);
    cursor: pointer;
  }
  .nm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cnt {
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
  }
  .hint {
    padding: 0 var(--d2-s3);
    font-size: var(--d2-fs-sm);
  }
  .choices {
    border: 0;
    padding: 0;
    margin: var(--d2-s3) 0 0;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
  }
</style>
