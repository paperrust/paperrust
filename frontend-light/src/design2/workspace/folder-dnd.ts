// Folder view stage 2: DOM side of drag & drop (HTML5 drag events, mouse). Documents from the list/tiles onto a
// folder in the tree, a folder tile or "Unsorted"; folders within the tree/onto tiles to re-parent. Rules and
// selection logic are pure in ../lib/folder-dnd.ts; the moves themselves in the folder store. Touch devices keep
// the bulk bar "Move to folder" (HTML5 drag events do not fire for touch in most mobile browsers).
import { t } from '../../lib/i18n'
import { canDrop, dragDocIds, DND_DOCS, DND_FOLDER, encodeIds, type DropKey } from '../lib/folder-dnd'
import { folders } from '../stores/folders.svelte'
import { ws } from '../stores/workspace.svelte'

/** Small "n documents" badge as drag image (removed right after the browser took the snapshot) */
function dragBadge(e: DragEvent, text: string) {
  if (!e.dataTransfer || typeof document === 'undefined') return
  const el = document.createElement('div')
  el.className = 'd2-dragbadge'
  el.textContent = text
  document.body.appendChild(el)
  try {
    e.dataTransfer.setDragImage(el, -10, -10)
  } catch {}
  setTimeout(() => el.remove(), 0)
}

export function startDocDrag(e: DragEvent, id: number) {
  const ids = dragDocIds(ws.selected, id)
  folders.drag = { kind: 'docs', ids }
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData(DND_DOCS, encodeIds(ids))
  }
  dragBadge(e, t('m.folders.drag_documents', { PH: ids.length }))
}

export function startFolderDrag(e: DragEvent, id: number, name: string) {
  folders.drag = { kind: 'folder', id }
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData(DND_FOLDER, String(id))
  }
  dragBadge(e, name)
}

export function endDrag() {
  folders.drag = null
  folders.dropOver = null
}

export function dragOver(e: DragEvent, key: DropKey) {
  if (!folders.drag || !canDrop(folders.items, folders.drag, key)) return
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'
  folders.dropOver = key
}

export function dragLeave(e: DragEvent, key: DropKey) {
  const el = e.currentTarget as HTMLElement | null
  if (el && e.relatedTarget instanceof Node && el.contains(e.relatedTarget)) return
  if (folders.dropOver === key) folders.dropOver = null
}

export async function drop(e: DragEvent, key: DropKey) {
  const item = folders.drag
  folders.dropOver = null
  folders.drag = null
  if (!item || !canDrop(folders.items, item, key)) return
  e.preventDefault()
  const target = key === 'unsorted' ? null : key
  if (item.kind === 'docs') await folders.moveDocs(item.ids, target)
  else await folders.reparent(item.id, target)
}
