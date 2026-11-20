import { describe, expect, it } from 'vitest'
import { generateFilterName, savedViewIsModified, visibilityAfterCreate } from './save-view'

import en from '../../../../i18n/en-US.json'

const EN = en as Record<string, string>
const t = (key: string, p?: Record<string, unknown>) => (EN[key] ?? key).replace('{$PH}', String(p?.PH ?? ''))
const names = {
  correspondent: (id: number) => (id === 1 ? { name: 'ACME' } : undefined),
  documentType: (id: number) => (id === 2 ? { name: 'Rechnung' } : undefined),
  storagePath: (id: number) => (id === 3 ? { name: 'Archiv' } : undefined),
  tag: (id: number) => (id === 4 ? { name: 'Steuer' } : undefined),
}

describe('generateFilterName (FilterEditorComponent)', () => {
  it('nennt eine einzelne Regel', () => {
    expect(generateFilterName([{ rule_type: 26, value: '1' }], names, t)).toBe('Correspondent: ACME')
    expect(generateFilterName([{ rule_type: 3, value: null }], names, t)).toBe('No correspondent')
    expect(generateFilterName([{ rule_type: 28, value: '2' }], names, t)).toBe('Document type: Rechnung')
    expect(generateFilterName([{ rule_type: 30, value: '3' }], names, t)).toBe('Storage path: Archiv')
    expect(generateFilterName([{ rule_type: 6, value: '4' }], names, t)).toBe('Tag: Steuer')
    expect(generateFilterName([{ rule_type: 7, value: 'false' }], names, t)).toBe('No tags')
    expect(generateFilterName([{ rule_type: 49, value: 'foo' }], names, t)).toBe('Title and content: foo')
    expect(generateFilterName([{ rule_type: 2, value: '12' }], names, t)).toBe('ASN: 12')
  })
  it('kein Vorschlag bei keiner oder mehreren Regeln oder unbekanntem Typ', () => {
    expect(generateFilterName([], names, t)).toBe('')
    expect(generateFilterName([{ rule_type: 6, value: '4' }, { rule_type: 26, value: '1' }], names, t)).toBe('')
    expect(generateFilterName([{ rule_type: 20, value: 'x' }], names, t)).toBe('')
  })
})

describe('savedViewIsModified (DocumentListComponent)', () => {
  const defaults = ['title', 'created']
  const view = { filter_rules: [{ rule_type: 6, value: '4' }], sort_field: 'created', sort_reverse: true, page_size: null, display_mode: null, display_fields: null }
  const list = { filterRules: [{ rule_type: 6, value: '4' }], sortField: 'created', sortReverse: true, pageSize: 50, displayMode: 'smallCards', displayFields: ['title', 'created'] }
  it('unverändert', () => {
    expect(savedViewIsModified(view, list, defaults)).toBe(false)
  })
  it('Ansicht ohne Sortierfeld gilt als „created“ (frisch geöffnet nicht geändert)', () => {
    expect(savedViewIsModified({ ...view, sort_field: null }, list, defaults)).toBe(false)
    expect(savedViewIsModified({ ...view, sort_field: null }, { ...list, sortField: 'title' }, defaults)).toBe(true)
  })
  it('erkennt Änderungen an Sortierung, Filter, Anzeige, Feldern, Seitengröße', () => {
    expect(savedViewIsModified(view, { ...list, sortReverse: false }, defaults)).toBe(true)
    expect(savedViewIsModified(view, { ...list, filterRules: [] }, defaults)).toBe(true)
    expect(savedViewIsModified(view, { ...list, displayMode: 'table' }, defaults)).toBe(true)
    expect(savedViewIsModified(view, { ...list, displayFields: ['title'] }, defaults)).toBe(true)
    expect(savedViewIsModified({ ...view, page_size: 25 }, list, defaults)).toBe(true)
    expect(savedViewIsModified({ ...view, display_mode: 'table' }, { ...list, displayMode: 'table' }, defaults)).toBe(false)
    expect(savedViewIsModified({ ...view, display_fields: ['asn'] }, { ...list, displayFields: ['asn'] }, defaults)).toBe(false)
  })
})

describe('visibilityAfterCreate (saveCreatedViewVisibility)', () => {
  it('hängt die neue Ansicht nur bei gesetztem Haken an', () => {
    expect(visibilityAfterCreate([1], [2], 9, true, false)).toEqual({ dashboard: [1, 9], sidebar: [2] })
    expect(visibilityAfterCreate([], [], 9, false, true)).toEqual({ dashboard: [], sidebar: [9] })
  })
})
