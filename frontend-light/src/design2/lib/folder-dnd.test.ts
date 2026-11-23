// Vitest: folder view stage 2 – drag & drop rules, ctrl/shift selection, undo groups, tiles helpers
import { describe, expect, it } from 'vitest'
import type { Folder } from '../api/folders'
import { canDrop, childFolders, clickSelect, decodeIds, descendants, dragDocIds, encodeIds, folderViewMode, gridStep, undoGroups } from './folder-dnd'

const f = (id: number, name: string, parent: number | null, extra: Partial<Folder> = {}): Folder => ({
  id,
  uuid: `u${id}`,
  rev: 1,
  name,
  parent,
  owner: null,
  color: null,
  sort: 0,
  user_can_change: true,
  user_can_delete: true,
  ...extra,
})

// 1 Invoices ─ 2 2026 ─ 3 Q1 ; 4 Insurance ; 5 read-only
const items = [f(1, 'Invoices', null), f(2, '2026', 1), f(3, 'Q1', 2), f(4, 'Insurance', null), f(5, 'Shared', null, { user_can_change: false })]

describe('dragged documents', () => {
  it('whole selection when the dragged one is selected, else only that one', () => {
    expect(dragDocIds([3, 7, 9], 7)).toEqual([3, 7, 9])
    expect(dragDocIds([3, 7, 9], 4)).toEqual([4])
    expect(dragDocIds([], 4)).toEqual([4])
  })
  it('DataTransfer encoding round trip, junk dropped', () => {
    expect(decodeIds(encodeIds([3, 7]))).toEqual([3, 7])
    expect(decodeIds('1, x,-2,0,1.5,4,4')).toEqual([1, 4])
    expect(decodeIds('')).toEqual([])
    expect(decodeIds(null)).toEqual([])
  })
})

describe('drop rules', () => {
  const docs = { kind: 'docs' as const, ids: [10, 11] }
  it('documents: changeable folders and Unsorted only', () => {
    expect(canDrop(items, docs, 1)).toBe(true)
    expect(canDrop(items, docs, 'unsorted')).toBe(true)
    expect(canDrop(items, docs, 5)).toBe(false)
    expect(canDrop(items, docs, 99)).toBe(false)
    expect(canDrop(items, { kind: 'docs', ids: [] }, 1)).toBe(false)
    expect(canDrop(items, null, 1)).toBe(false)
  })
  it('descendants', () => {
    expect([...descendants(items, 1)].sort()).toEqual([2, 3])
    expect([...descendants(items, 3)]).toEqual([])
    // cycle in the data does not hang
    expect([...descendants([f(1, 'a', 2), f(2, 'b', 1)], 1)].sort()).toEqual([2])
  })
  it('folders: not into itself or its subtree, not onto the current parent, Unsorted = top level', () => {
    const drag = (id: number) => ({ kind: 'folder' as const, id })
    expect(canDrop(items, drag(1), 1)).toBe(false)
    expect(canDrop(items, drag(1), 2)).toBe(false)
    expect(canDrop(items, drag(1), 3)).toBe(false)
    expect(canDrop(items, drag(1), 4)).toBe(true)
    expect(canDrop(items, drag(1), 'unsorted')).toBe(false) // already top level
    expect(canDrop(items, drag(3), 'unsorted')).toBe(true)
    expect(canDrop(items, drag(3), 2)).toBe(false) // current parent
    expect(canDrop(items, drag(3), 1)).toBe(true)
    expect(canDrop(items, drag(2), 5)).toBe(false) // target not changeable
    expect(canDrop(items, drag(5), 4)).toBe(false) // folder not changeable
  })
})

describe('click selection (list and tiles)', () => {
  const order = [1, 2, 3, 4, 5]
  it('plain click opens and sets the anchor, selection unchanged', () => {
    expect(clickSelect({ selected: [4], anchor: null }, order, 2, {})).toEqual({ selected: [4], anchor: 2, open: true })
  })
  it('ctrl toggles', () => {
    expect(clickSelect({ selected: [4], anchor: null }, order, 2, { ctrl: true })).toEqual({ selected: [4, 2], anchor: 2, open: false })
    expect(clickSelect({ selected: [4, 2], anchor: 2 }, order, 4, { ctrl: true })).toEqual({ selected: [2], anchor: 4, open: false })
  })
  it('shift selects the range from the anchor (both directions), keeps the anchor', () => {
    expect(clickSelect({ selected: [], anchor: 2 }, order, 4, { shift: true })).toEqual({ selected: [2, 3, 4], anchor: 2, open: false })
    expect(clickSelect({ selected: [5], anchor: 4 }, order, 2, { shift: true })).toEqual({ selected: [5, 2, 3, 4], anchor: 4, open: false })
  })
  it('shift without usable anchor toggles', () => {
    expect(clickSelect({ selected: [], anchor: null }, order, 3, { shift: true })).toEqual({ selected: [3], anchor: 3, open: false })
    expect(clickSelect({ selected: [], anchor: 99 }, order, 3, { shift: true })).toEqual({ selected: [3], anchor: 3, open: false })
  })
})

describe('undo of a move', () => {
  it('groups by previous folder, skips documents already in the target', () => {
    const prev = new Map<number, number | null>([
      [1, 4],
      [2, null],
      [3, 4],
      [5, 7],
    ])
    expect(undoGroups([1, 2, 3, 5], 7, (id) => prev.get(id))).toEqual([
      { folder: 4, ids: [1, 3] },
      { folder: null, ids: [2] },
    ])
  })
  it('unknown previous folder → no undo', () => {
    expect(undoGroups([1, 2], 7, (id) => (id === 1 ? 4 : undefined))).toBeNull()
  })
  it('nothing changed → empty', () => {
    expect(undoGroups([1], null, () => null)).toEqual([])
  })
})

describe('tiles', () => {
  it('subfolders of the open folder; at Unsorted the top level (incl. orphans)', () => {
    expect(childFolders(items, 1).map((x) => x.id)).toEqual([2])
    expect(childFolders(items, 3)).toEqual([])
    expect(childFolders(items, 'unsorted').map((x) => x.id)).toEqual([1, 4, 5])
    expect(childFolders([f(9, 'orphan', 42)], 'unsorted').map((x) => x.id)).toEqual([9])
  })
  it('arrow keys in the grid, clamped', () => {
    expect(gridStep('ArrowRight', 0, 10, 4)).toBe(1)
    expect(gridStep('ArrowLeft', 0, 10, 4)).toBe(0)
    expect(gridStep('ArrowDown', 1, 10, 4)).toBe(5)
    expect(gridStep('ArrowDown', 8, 10, 4)).toBe(9)
    expect(gridStep('ArrowUp', 5, 10, 4)).toBe(1)
    expect(gridStep('Home', 7, 10, 4)).toBe(0)
    expect(gridStep('End', 2, 10, 4)).toBe(9)
    expect(gridStep('Enter', 2, 10, 4)).toBeNull()
    expect(gridStep('ArrowRight', 0, 0, 4)).toBeNull()
  })
  it('view mode from ui_settings, default list', () => {
    expect(folderViewMode(null)).toBe('list')
    expect(folderViewMode({ paperrust: { folder_view: 'tiles' } })).toBe('tiles')
    expect(folderViewMode({ paperrust: { folder_view: 'bogus' } })).toBe('list')
  })
})
