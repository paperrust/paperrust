// Reordering (cdk moveItemInArray, cases from @angular/cdk drag-utils.spec) and saved view widget queue
// (saved-view-widget.component.spec.ts, upstream #14699: „should hand its documents to the list service“, „page size“).
import { describe, expect, it } from 'vitest'
import { moveItemInArray } from '../../lib/cdk-drag'
import { activateViewWithDocuments, dashboardPageSize, dashboardViews, DEFAULT_DASHBOARD_VIEW_PAGE_SIZE, type DashboardView } from './dashboard'

describe('moveItemInArray', () => {
  it('verschiebt nach vorn und hinten', () => {
    const a = [0, 1, 2, 3]
    moveItemInArray(a, 1, 3)
    expect(a).toEqual([0, 2, 3, 1])
    moveItemInArray(a, 3, 0)
    expect(a).toEqual([1, 0, 2, 3])
  })
  it('begrenzt die Indizes', () => {
    const a = [0, 1, 2]
    moveItemInArray(a, -1, 9)
    expect(a).toEqual([1, 2, 0])
    moveItemInArray(a, 1, 1)
    expect(a).toEqual([1, 2, 0])
  })
  it('ergibt die gespeicherte Reihenfolge wie onDrop', () => {
    const views = [{ id: 1 }, { id: 2 }, { id: 4 }]
    const shown = dashboardViews(views, [1, 2, 4], [])
    const next = [...shown]
    moveItemInArray(next, 0, 2)
    const order = [...new Set(next.map((v) => v.id))]
    expect(order).toEqual([2, 4, 1])
    expect(dashboardViews(views, [1, 2, 4], order).map((v) => v.id)).toEqual([2, 4, 1])
  })
})

describe('Saved-View-Widget Warteschlange', () => {
  const view = { id: 7, name: 'v', page_size: 20 } as unknown as DashboardView
  it('nimmt die Seitengröße der Ansicht, sonst den Standard', () => {
    expect(dashboardPageSize(view)).toBe(20)
    expect(dashboardPageSize({ ...view, page_size: undefined })).toBe(DEFAULT_DASHBOARD_VIEW_PAGE_SIZE)
    expect(dashboardPageSize(null)).toBe(DEFAULT_DASHBOARD_VIEW_PAGE_SIZE)
  })
  it('gibt Dokumente, Anzahl und Seitengröße an die Liste', () => {
    const calls: unknown[][] = []
    const docs = [{ id: 2 }, { id: 3 }]
    activateViewWithDocuments({ activateSavedViewWithDocuments: (...a) => void calls.push(a) }, view, docs, 25, dashboardPageSize(view))
    expect(calls).toEqual([[view, docs, 25, 20]])
    expect(() => activateViewWithDocuments({}, view, docs, 25, 20)).not.toThrow()
  })
})
