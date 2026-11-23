import { afterEach, describe, expect, it } from 'vitest'
import {
  folderOptions,
  folderParams,
  folderPath,
  folderRoute,
  foldersEnabled,
  moveDocuments,
  parseFolderRoute,
  setFoldersFetch,
  treeRows,
  type Folder,
} from './folders'
import { ctxPath, parseRoute } from '../stores/workspace.svelte'

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

// server order: sort, name
const ITEMS = [f(1, 'Invoices', null), f(3, '2025', 1), f(2, '2026', 1), f(4, 'Taxes', null), f(5, 'Shared', 99)]

describe('folder tree', () => {
  it('rows depth-first, invisible parent → root', () => {
    expect(treeRows(ITEMS).map((r) => [r.folder.id, r.depth, r.hasChildren])).toEqual([
      [1, 0, true],
      [3, 1, false],
      [2, 1, false],
      [4, 0, false],
      [5, 0, false],
    ])
  })
  it('collapsed nodes hide their subtree', () => {
    expect(treeRows(ITEMS, new Set([1])).map((r) => r.folder.id)).toEqual([1, 4, 5])
  })
  it('path from the root', () => {
    expect(folderPath(ITEMS, 2).map((x) => x.name)).toEqual(['Invoices', '2026'])
    expect(folderPath(ITEMS, 5).map((x) => x.id)).toEqual([5])
    expect(folderPath(ITEMS, 42)).toEqual([])
  })
  it('picker options with path, filter', () => {
    const o = folderOptions(ITEMS, (x) => x.id !== 4)
    expect(o.map((x) => x.name)).toEqual(['Invoices', 'Invoices › 2025', 'Invoices › 2026', 'Shared'])
  })
  it('survives a cycle in broken data', () => {
    const bad = [f(1, 'a', 2), f(2, 'b', 1)]
    expect(treeRows(bad)).toEqual([])
    expect(folderPath(bad, 1).length).toBe(2)
  })
})

describe('folder routes and parameters', () => {
  it('parses folders routes', () => {
    expect(parseFolderRoute('folders')).toEqual({ key: 'unsorted', activeId: null })
    expect(parseFolderRoute('folders/unsorted/12')).toEqual({ key: 'unsorted', activeId: 12 })
    expect(parseFolderRoute('folders/7')).toEqual({ key: 7, activeId: null })
    expect(parseFolderRoute('folders/7/3')).toEqual({ key: 7, activeId: 3 })
    expect(parseFolderRoute('folders/x')).toBeNull()
    expect(folderRoute(7, 3)).toBe('folders/7/3')
    expect(folderRoute('unsorted')).toBe('folders/unsorted')
  })
  it('workspace context round trip', () => {
    const c = parseRoute('folders/7/3')
    expect(c).toEqual({ kind: 'folder', viewId: null, activeId: 3, folder: 7 })
    expect(ctxPath(c!, 9)).toBe('folders/7/9')
    expect(ctxPath(c!, null)).toBe('folders/7')
  })
  it('v1 list parameters', () => {
    expect(folderParams('unsorted')).toEqual({ folder__isnull: '1' })
    expect(folderParams(7)).toEqual({ folder__id: '7' })
  })
  it('switch default off', () => {
    expect(foldersEnabled(null)).toBe(false)
    expect(foldersEnabled({ paperrust: { folders: 'yes' } })).toBe(false)
    expect(foldersEnabled({ paperrust: { folders: true } })).toBe(true)
  })
})

describe('folder api', () => {
  afterEach(() => setFoldersFetch(null))
  it('move-documents body and error text', async () => {
    const calls: [string, RequestInit | undefined][] = []
    setFoldersFetch(async (u, init) => {
      calls.push([u, init])
      return new Response(JSON.stringify({ moved: 2, folder: 7 }), { status: 200, headers: { 'Content-Type': 'application/json' } })
    })
    expect(await moveDocuments([1, 2], 7)).toEqual({ moved: 2, folder: 7 })
    expect(calls[0][0]).toBe('/rs/api/v2/folders/move-documents')
    expect(calls[0][1]?.method).toBe('POST')
    expect(JSON.parse(String(calls[0][1]?.body))).toEqual({ documents: [1, 2], folder: 7 })

    setFoldersFetch(async () => new Response(JSON.stringify({ error: { code: 'permission_denied', message: 'Insufficient permissions.' } }), { status: 403 }))
    await expect(moveDocuments([1], null)).rejects.toMatchObject({ status: 403, message: 'Insufficient permissions.' })
  })
})
