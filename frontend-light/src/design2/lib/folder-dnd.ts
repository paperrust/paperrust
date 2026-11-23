// Folder view stage 2 (results/ideas/2026-11-22-papermerge-ordneransicht.md): pure helpers for drag & drop
// (documents onto folders, folders within the tree), selection with ctrl/shift-click (list and tiles) and the
// "Undo" of a move. No DOM, no stores – unit-tested in folder-dnd.test.ts.
import type { Folder, FolderKey } from '../api/folders'

/** DataTransfer types of internal drags (external file drops use 'Files' and go to the upload) */
export const DND_DOCS = 'application/x-paperrust-documents'
export const DND_FOLDER = 'application/x-paperrust-folder'

/** What is being dragged right now (kept in a store: dragover cannot read the DataTransfer data) */
export type DragItem = { kind: 'docs'; ids: number[] } | { kind: 'folder'; id: number }

/** Documents that travel with a drag: the whole selection if the dragged one is part of it, else only that one */
export function dragDocIds(selected: readonly number[], dragged: number): number[] {
  return selected.includes(dragged) ? [...selected] : [dragged]
}

export function encodeIds(ids: readonly number[]): string {
  return ids.join(',')
}

/** Positive integer ids from a DataTransfer string (anything else is dropped) */
export function decodeIds(raw: string | null | undefined): number[] {
  if (!raw) return []
  const out: number[] = []
  for (const p of raw.split(',')) {
    const n = Number(p.trim())
    if (Number.isInteger(n) && n > 0 && !out.includes(n)) out.push(n)
  }
  return out
}

/** Ids of all (visible) descendants of `id`, without `id` itself */
export function descendants(items: readonly Folder[], id: number): Set<number> {
  const kids = new Map<number, number[]>()
  for (const f of items) {
    if (f.parent === null) continue
    const l = kids.get(f.parent) ?? []
    l.push(f.id)
    kids.set(f.parent, l)
  }
  const out = new Set<number>()
  const stack = [...(kids.get(id) ?? [])]
  while (stack.length) {
    const n = stack.pop() as number
    if (out.has(n) || n === id) continue
    out.add(n)
    stack.push(...(kids.get(n) ?? []))
  }
  return out
}

/** Drop target of the tree/tiles: a folder or "Unsorted" (= top level for folders) */
export type DropKey = FolderKey

/**
 * May `item` be dropped on `target`? Documents: any changeable folder or "Unsorted". Folders: a changeable
 * folder that is neither the folder itself nor one of its subfolders (the server refuses that too), and not the
 * current parent (no-op); "Unsorted" moves the folder to the top level.
 */
export function canDrop(items: readonly Folder[], item: DragItem | null, target: DropKey): boolean {
  if (!item) return false
  const t = target === 'unsorted' ? null : items.find((f) => f.id === target)
  if (target !== 'unsorted' && (!t || !t.user_can_change)) return false
  if (item.kind === 'docs') return item.ids.length > 0
  const f = items.find((x) => x.id === item.id)
  if (!f || !f.user_can_change) return false
  const parent = target === 'unsorted' ? null : target
  if (parent === f.parent) return false
  if (parent === null) return true
  return parent !== f.id && !descendants(items, f.id).has(parent)
}

export interface Selection {
  selected: number[]
  /** last plainly clicked/toggled id: start of a shift range */
  anchor: number | null
}

/**
 * Click on a document in the list or the tiles. Plain click opens the document (selection unchanged, anchor set);
 * ctrl/cmd toggles; shift selects the range anchor…id (in list order) in addition to the current selection, or
 * toggles when there is no usable anchor. Returns the new selection and whether to open the document.
 */
export function clickSelect(
  s: Selection,
  order: readonly number[],
  id: number,
  mods: { shift?: boolean; ctrl?: boolean },
): Selection & { open: boolean } {
  if (mods.shift && s.anchor !== null) {
    const a = order.indexOf(s.anchor)
    const b = order.indexOf(id)
    if (a >= 0 && b >= 0) {
      const range = order.slice(Math.min(a, b), Math.max(a, b) + 1)
      const selected = [...s.selected, ...range.filter((x) => !s.selected.includes(x))]
      return { selected, anchor: s.anchor, open: false }
    }
  }
  if (mods.shift || mods.ctrl) {
    const selected = s.selected.includes(id) ? s.selected.filter((x) => x !== id) : [...s.selected, id]
    return { selected, anchor: id, open: false }
  }
  return { selected: s.selected, anchor: id, open: true }
}

/**
 * Groups for "Undo" of a move: previous folder → document ids. `prev` gives the folder before the move (`null` =
 * Unsorted, `undefined` = unknown). Any unknown → no undo (null). Documents that were already in the target are left
 * out; nothing to undo → empty list.
 */
export function undoGroups(
  ids: readonly number[],
  target: number | null,
  prev: (id: number) => number | null | undefined,
): { folder: number | null; ids: number[] }[] | null {
  const groups = new Map<number | null, number[]>()
  for (const id of ids) {
    const p = prev(id)
    if (p === undefined) return null
    if (p === target) continue
    const l = groups.get(p) ?? []
    l.push(id)
    groups.set(p, l)
  }
  return [...groups.entries()].map(([folder, list]) => ({ folder, ids: list }))
}

/** Subfolders shown as tiles: children of the open folder; at "Unsorted" (the folder root) the top-level folders */
export function childFolders(items: readonly Folder[], current: FolderKey): Folder[] {
  const ids = new Set(items.map((f) => f.id))
  if (current === 'unsorted') return items.filter((f) => f.parent === null || !ids.has(f.parent))
  return items.filter((f) => f.parent === current)
}

/** Next index in a grid of `cols` columns for an arrow key (clamped); null for other keys */
export function gridStep(key: string, index: number, count: number, cols: number): number | null {
  if (count <= 0) return null
  const c = Math.max(1, cols)
  const d = key === 'ArrowRight' ? 1 : key === 'ArrowLeft' ? -1 : key === 'ArrowDown' ? c : key === 'ArrowUp' ? -c : key === 'Home' ? -index : key === 'End' ? count - 1 - index : null
  if (d === null) return null
  return Math.max(0, Math.min(count - 1, index + d))
}

/** View mode of the folder view (per user, ui_settings.paperrust.folder_view), default list */
export type FolderViewMode = 'list' | 'tiles'
export const FOLDER_VIEW_KEY = 'paperrust:folder_view'
export function folderViewMode(settings: Record<string, any> | null | undefined): FolderViewMode {
  return settings?.paperrust?.folder_view === 'tiles' ? 'tiles' : 'list'
}
