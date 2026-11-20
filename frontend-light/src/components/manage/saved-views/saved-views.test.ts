import { describe, expect, it, vi } from 'vitest'
vi.mock('./saved-view-icons', () => ({ DEFAULT_SAVED_VIEW_ICON: 'funnel', SAVED_VIEW_ICONS: [] }))
const { computeSave, formFromView, isDirtyForms } = await import('./saved-views.svelte')

const view = (id: number, extra: any = {}) => ({ id, name: `V${id}`, icon: null, sort_field: null, sort_reverse: false, filter_rules: [], page_size: null, display_mode: null, display_fields: null, ...extra })

describe('SavedViewsComponent (saved-views.component.ts)', () => {
  it('Formular: Standardsymbol, Sichtbarkeit aus den UI-Settings', () => {
    const f = formFromView(view(1), [1], [])
    expect(f).toMatchObject({ icon: 'funnel', show_on_dashboard: true, show_in_sidebar: false })
  })
  it('save: nur geänderte, editierbare Ansichten (ohne show_*), Sichtbarkeit getrennt', () => {
    const store = { 1: formFromView(view(1), [1], [1]), 2: formFromView(view(2), [], []), 3: formFromView(view(3), [], []) }
    const forms = [{ ...store[1], name: 'Neu' }, { ...store[2], show_in_sidebar: true }, { ...store[3], page_size: 50 }]
    expect(isDirtyForms(forms, store)).toBe(true)
    const r = computeSave(forms, store, (id) => id !== 3)
    expect(r.changed).toEqual([{ id: 1, name: 'Neu', icon: 'funnel', page_size: null, display_mode: null, display_fields: null }])
    expect(r.visibilityChanged).toBe(true)
    expect(r.dashboard).toEqual([1])
    expect(r.sidebar).toEqual([1, 2])
  })
  it('nichts geändert → nichts zu tun', () => {
    const store = { 1: formFromView(view(1, { display_fields: ['title'] }), [], []) }
    const forms = [{ ...store[1], display_fields: ['title'] }]
    expect(isDirtyForms(forms, store)).toBe(false)
    const r = computeSave(forms, store, () => true)
    expect(r.changed).toEqual([])
    expect(r.visibilityChanged).toBe(false)
  })
})
