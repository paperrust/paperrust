// Port of the specs "should support activating saved view with pre-fetched documents" (upstream #14699,
// frontend/patches/10): activate a view with already loaded documents without reloading; afterwards back to default.
import { describe, expect, it, vi } from 'vitest'

vi.mock('../api/client', () => ({ api: { listDocuments: vi.fn(() => new Promise(() => {})) } }))

const { list } = await import('./listView.svelte')
const { api } = await import('../api/client')

const docs = [1, 2, 3, 4].map((id) => ({ id, title: `d${id}`, tags: [] })) as any[]
const view = { id: 77, name: 'Inbox', filter_rules: [{ rule_type: 22, value: '1' }], sort_field: 'added', sort_reverse: true, page_size: 50 }

describe('ListViewState.activateSavedViewWithDocuments', () => {
  it('übernimmt Ansicht, Dokumente und Seitengröße ohne Neuladen', () => {
    const before = { rules: list.filterRules, sort: list.sortField }
    list.activateSavedViewWithDocuments(view, docs.slice(0, 2), docs.length, 2)
    expect(api.listDocuments).not.toHaveBeenCalled()
    expect(list.activeSavedViewId).toBe(77)
    expect(list.activeSavedViewTitle).toBe('Inbox')
    expect(list.documents.map((d) => d.id)).toEqual([1, 2])
    expect(list.collectionSize).toBe(4)
    expect(list.currentPage).toBe(1)
    expect(list.pageSize).toBe(2)
    expect(list.sortField).toBe('added')
    expect(list.filterRules).toEqual(view.filter_rules)

    list.activateSavedView(null)
    expect(list.activeSavedViewId).toBeNull()
    expect(list.sortField).toBe(before.sort)
    expect(list.filterRules).toEqual(before.rules)
    expect(list.pageSize).not.toBe(2)
  })
})

describe('ListViewState.activateSavedView (Route view/<id>)', () => {
  const full = {
    id: 5,
    name: 'Rechnungen',
    filter_rules: [{ rule_type: 6, value: '3' }],
    sort_field: 'title',
    sort_reverse: false,
    display_mode: 'table' as const,
    page_size: 10,
    display_fields: ['title', 'tag'],
  }

  it('übernimmt Filter, Sortierung, Anzeige, Felder, Seitengröße und Titel; null stellt den Standard wieder her', () => {
    const before = { rules: list.filterRules, sort: list.sortField, rev: list.sortReverse, mode: list.mode, fields: list.displayFields, size: list.pageSize }
    list.selectPage()
    list.activateSavedView(full)
    expect(list.activeSavedViewId).toBe(5)
    expect(list.activeSavedViewTitle).toBe('Rechnungen')
    expect(list.filterRules).toEqual(full.filter_rules)
    expect(list.filterRules).not.toBe(full.filter_rules) // cloned
    expect(list.sortField).toBe('title')
    expect(list.sortReverse).toBe(false)
    expect(list.mode).toBe('table')
    expect(list.displayFields).toEqual(['title', 'tag'])
    expect(list.pageSize).toBe(10)
    expect(list.hasSelection).toBe(false) // own selection per view

    // Display changes in the view do not end up in the stored default state
    list.mode = 'largeCards'
    expect(list.mode).toBe('largeCards')

    list.activateSavedView(null)
    expect(list.activeSavedViewId).toBeNull()
    expect(list.activeSavedViewTitle).toBeNull()
    expect(list.filterRules).toEqual(before.rules)
    expect(list.sortField).toBe(before.sort)
    expect(list.sortReverse).toBe(before.rev)
    expect(list.mode).toBe(before.mode)
    expect(list.displayFields).toEqual(before.fields)
    expect(list.pageSize).toBe(before.size)
  })

  it('Ansicht ohne Anzeigemodus/Felder nutzt die Standardwerte (kleine Karten, Standardfelder)', async () => {
    const { LIST_DEFAULT_DISPLAY_FIELDS } = await import('./listView.svelte')
    list.activateSavedView({ id: 6, name: 'X', filter_rules: [], sort_field: null, sort_reverse: true, display_mode: null, display_fields: null, page_size: null })
    expect(list.mode).toBe('smallCards')
    expect(list.displayFields).toEqual(LIST_DEFAULT_DISPLAY_FIELDS)
    expect(list.sortField).toBe('created')
    // Switching between views keeps the original default state
    list.activateSavedView(full)
    list.activateSavedView(null)
    expect(list.activeSavedViewId).toBeNull()
    expect(list.pageSize).not.toBe(10)
  })

  it('activateSavedViewWithQueryParams liest nur die Seite aus der URL', () => {
    list.activateSavedViewWithQueryParams(full, '?page=3&sort=added')
    expect(list.currentPage).toBe(3)
    expect(list.sortField).toBe('title')
    list.activateSavedViewWithQueryParams(full, '')
    expect(list.currentPage).toBe(1)
    list.activateSavedViewWithQueryParams(full, '?page=abc')
    expect(list.currentPage).toBe(1)
    list.activateSavedView(null)
  })
})

// Port of the specs "should support selecting a range of documents" / "should support selection range reduction"
// (document-list-view.service.spec.ts): shift click selects from the anchor to the document, shrinks/shifts the range.
describe('ListViewState.selectRangeTo', () => {
  it('wählt Bereiche vom Anker aus und nimmt den vorigen Bereich zurück', () => {
    const six = [1, 2, 3, 4, 5, 6].map((id) => ({ id, title: `d${id}`, tags: [] })) as any[]
    list.activateSavedViewWithDocuments(view, six, six.length, 50)
    list.selectNone()
    list.selectRangeTo(six[2]) // without anchor: like toggleSelected
    expect([...list.selected]).toEqual([3])
    list.selectRangeTo(six[5])
    expect([...list.selected].sort()).toEqual([3, 4, 5, 6])
    list.selectRangeTo(six[4]) // shrink range
    expect([...list.selected].sort()).toEqual([3, 4, 5])
    list.selectRangeTo(six[0]) // range to the other side of the anchor
    expect([...list.selected].sort()).toEqual([1, 2, 3])
    list.toggleSelected(six[5]) // new anchor
    list.selectRangeTo(six[3])
    expect([...list.selected].sort()).toEqual([1, 2, 3, 4, 5, 6])
    list.selectAll()
    list.selectRangeTo(six[1]) // "all" is cleared, exceptions emptied
    expect(list.allSelected).toBe(false)
    list.selectNone()
    list.activateSavedView(null)
  })
  it('lastPage aus Anzahl und Seitengröße', () => {
    list.activateSavedViewWithDocuments(view, docs, 101, 50)
    expect(list.lastPage).toBe(3)
    list.activateSavedView(null)
  })
})
