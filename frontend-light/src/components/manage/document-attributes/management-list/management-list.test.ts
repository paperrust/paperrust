import { describe, expect, it, vi } from 'vitest'
import { LIST_CONFIGS, ManagementListState, truncatePath, type Loader } from './management-list.svelte'
import { listParams } from '../../../../api/objects'

function make(kind: keyof typeof LIST_CONFIGS, results: any[] = [], count = results.length) {
  const loader = vi.fn<Loader<any>>(async () => ({ count, results }))
  const s = new ManagementListState(LIST_CONFIGS[kind], loader, () => 25)
  return { s, loader }
}

describe('ManagementListState (management-list.component.ts)', () => {
  it('TagList lädt ohne Namensfilter nur Wurzel-Tags, mit Filter alle', async () => {
    const { s, loader } = make('tags')
    await s.reloadData()
    expect(loader).toHaveBeenLastCalledWith(1, 25, undefined, undefined, undefined, true, { is_root: true })
    s.nameFilter = 'Tag'
    await s.reloadData()
    expect(loader).toHaveBeenLastCalledWith(1, 25, undefined, undefined, 'Tag', true, null)
  })

  it('CorrespondentList fordert last_correspondence an', async () => {
    const { s, loader } = make('correspondents')
    await s.reloadData()
    expect(loader).toHaveBeenLastCalledWith(1, 25, undefined, undefined, undefined, true, { last_correspondence: true })
  })

  it('Tags: Kinder ohne/mit Namensfilter ausblenden, wenn Elternteil vorhanden', () => {
    const { s } = make('tags')
    const tags: any[] = [
      { id: 1, name: 'Tag1', parent: null, children: [{ id: 2, name: 'Tag2', parent: 1 }] },
      { id: 2, name: 'Tag2', parent: 1 },
      { id: 3, name: 'Tag3', parent: null },
    ]
    expect(s.filterData(tags).map((t) => t.id)).toEqual([1, 3])
    s.nameFilter = 'Tag2'
    expect(s.filterData(tags).map((t) => t.id)).toEqual([1, 3])
  })

  it('Seite auswählen schließt Kind-Tags ein; Auswahl leeren', () => {
    const { s } = make('tags')
    s.data = [{ id: 10, name: 'Parent', children: [{ id: 11, name: 'Child' }] }] as any
    s.selectPage()
    expect(s.selectedObjects.has(10) && s.selectedObjects.has(11)).toBe(true)
    expect(s.toggleAll).toBe(true)
    s.clearSelection()
    expect(s.selectedObjects.size).toBe(0)
  })

  it('Alle auswählen zählt die Gesamtzahl; Umschalten beendet „alle“', async () => {
    const { s } = make('documenttypes', [{ id: 1 }, { id: 2 }], 30)
    await s.reloadData()
    s.selectAll()
    expect(s.selectedCount).toBe(30)
    s.toggleSelected({ id: 1 } as any)
    expect(s.selectedCount).toBe(1)
    expect(s.hasSelection).toBe(true)
  })

  it('Sortierung rotiert wie SortableDirective', async () => {
    const { s } = make('storagepaths')
    s.rotateSort('name')
    expect([s.sortField, s.sortReverse]).toEqual(['name', false])
    s.rotateSort('name')
    expect([s.sortField, s.sortReverse]).toEqual(['name', true])
    s.rotateSort('name')
    expect(s.sortField).toBeUndefined()
  })

  it('Massenfilter enthalten Zusatzparameter und Namensfilter', async () => {
    const { s } = make('tags')
    s.nameFilter = 'x'
    await s.reloadData()
    expect(s.bulkEditFilters()).toEqual({ name__icontains: 'x' })
  })

  it('Speicherpfad wird auf 49 Zeichen gekürzt', () => {
    expect(truncatePath('a'.repeat(100))).toBe(`${'a'.repeat(49)}...`)
  })

  it('listParams wie HttpParams (null auslassen, Reihenfolge)', () => {
    expect(listParams(1, 25, 'name', true, { is_root: true, full_perms: true, x: null })).toBe('?page=1&page_size=25&ordering=-name&is_root=true&full_perms=true')
  })
})
