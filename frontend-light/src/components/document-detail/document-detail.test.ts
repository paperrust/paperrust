// Pure logic of the document view (cases adapted from document-detail.component.spec.ts and the input specs).
import { describe, expect, it } from 'vitest'
import {
  changedFields,
  currentZoom,
  decreaseZoom,
  deepEqual,
  fileSize,
  filterRulesFor,
  formFromDocument,
  getRenderType,
  increaseZoom,
  setZoom,
  zoomSettings,
} from './document-detail'
import { relativeDate, titleCase } from './document-history/history'
import { addWithParents, parentChain, removeWithChildren } from '../common/input/tags/tags'
import { withPrivateItems } from '../common/input/select/select'
import { normalizeNumber } from '../common/input/number/number'
import { formatCurrency, localeCurrencyCode, parseCurrencyCode, parseMonetaryValue } from '../common/input/monetary/monetary'
import { fieldSuggestions, mergeSuggestions, noSuggestions, novelSuggestions } from '../common/suggestions-dropdown/suggestions'
import { noteDisplayName } from '../document-notes/notes'
import { FILTER_CORRESPONDENT, FILTER_CREATED_AFTER, FILTER_CREATED_BEFORE, FILTER_HAS_TAGS_ALL } from '../../lib/filter-rule-type'

const doc: any = {
  id: 1, title: 'T', content: 'c', created: '2024-01-15', correspondent: 2, document_type: null, storage_path: null,
  archive_serial_number: 7, tags: [1, 2], owner: 3, permissions: { view: { users: [], groups: [] }, change: { users: [], groups: [] } },
  custom_fields: [{ field: 4, value: 'x' }], notes: [], added: '', page_count: 1,
}

describe('Formular', () => {
  it('formFromDocument + deepEqual', () => {
    const f = formFromDocument(doc)
    expect(f.permissions_form?.owner).toBe(3)
    expect(deepEqual(f, formFromDocument(doc))).toBe(true)
    expect(deepEqual(f, { ...f, tags: [1] })).toBe(false)
    expect(deepEqual({ a: 1, b: undefined }, { a: 1 })).toBe(true)
    expect(deepEqual([1, 2], [2, 1])).toBe(false)
  })
  it('changedFields nur dirty, permissions_form → owner/set_permissions', () => {
    const f = formFromDocument(doc)
    expect(changedFields(1, f, new Set(['title']))).toEqual({ id: 1, title: 'T' })
    const c = changedFields(1, f, new Set(['permissions_form', 'tags']))
    expect(c).toEqual({ id: 1, tags: [1, 2], owner: 3, set_permissions: doc.permissions })
  })
})

describe('Darstellung', () => {
  it('getRenderType', () => {
    expect(getRenderType('application/pdf')).toBe('pdf')
    expect(getRenderType('text/csv')).toBe('text')
    expect(getRenderType('image/tiff')).toBe('tiff')
    expect(getRenderType('image/png')).toBe('image')
    expect(getRenderType('application/msword')).toBe('other')
    expect(getRenderType(undefined)).toBe('unknown')
  })
  it('fileSize wie FileSizePipe', () => {
    expect(fileSize(500)).toBe('500 bytes')
    expect(fileSize(2048)).toBe('2 KB')
    expect(fileSize(1572864)).toBe('1.5 MB')
    expect(fileSize(undefined as any)).toBe('0 bytes')
    expect(fileSize(NaN)).toBe('?')
  })
  it('Zoom', () => {
    let z = setZoom('page-width')
    expect(z).toEqual({ scale: 'page-width', level: '1' })
    z = increaseZoom(z)
    expect(z.level).toBe('1.5')
    z = setZoom('page-fit')
    expect(currentZoom(z)).toBe('page-fit')
    expect(decreaseZoom(z).level).toBe('.5')
    expect(increaseZoom(setZoom('3')).level).toBe('3')
    expect(zoomSettings()[0]).toBe('page-fit')
  })
})

describe('filterDocuments', () => {
  it('Datum → created after/before ±1 Tag, Objekte → Regeltyp', () => {
    expect(filterRulesFor([{ year: 2024, month: 1, day: 15 }])).toEqual([
      { rule_type: FILTER_CREATED_AFTER, value: '2024-01-14' },
      { rule_type: FILTER_CREATED_BEFORE, value: '2024-01-16' },
    ])
    expect(filterRulesFor([{ id: 5 }], 'correspondent')).toEqual([{ rule_type: FILTER_CORRESPONDENT, value: '5' }])
    expect(filterRulesFor([{ id: 1 }, { id: 2 }], 'tag')).toEqual([
      { rule_type: FILTER_HAS_TAGS_ALL, value: '1' },
      { rule_type: FILTER_HAS_TAGS_ALL, value: '2' },
    ])
  })
})

describe('Verlauf', () => {
  it('relativeDate/titleCase', () => {
    const now = new Date('2026-01-10T12:00:00Z')
    expect(relativeDate('2026-01-10T11:59:30Z', now).key).toBe('detail.just_now')
    expect(relativeDate('2026-01-10T10:00:00Z', now)).toEqual({ key: 'detail.n_hours_ago', n: 2 })
    expect(relativeDate('2026-01-09T12:00:00Z', now)).toEqual({ key: 'common.yesterday', n: 1 })
    expect(relativeDate('2024-01-01T00:00:00Z', now)).toEqual({ key: 'detail.n_years_ago', n: 2 })
    expect(titleCase('archive_serial_number')).toBe('Archive_serial_number')
    expect(titleCase('UPDATE now')).toBe('Update Now')
  })
})

describe('Eingaben', () => {
  const tags: any[] = [
    { id: 1, name: 'a', parent: null, children: [{ id: 2, children: [{ id: 3 }] }] },
    { id: 2, name: 'b', parent: 1, children: [{ id: 3 }] },
    { id: 3, name: 'c', parent: 2 },
  ]
  it('Schlagwort-Hierarchie', () => {
    expect(parentChain(3, tags).map((t) => t.id)).toEqual([1, 2])
    expect(addWithParents([], 3, tags)).toEqual([3, 2, 1])
    expect(removeWithChildren([1, 2, 3, 9], 1, tags)).toEqual([9])
  })
  it('Private Einträge, Zahlen, Währung', () => {
    expect(withPrivateItems([{ id: 1 }], 5, 'Private')).toEqual([{ id: 1 }, { id: 5, name: 'Private', private: true }])
    expect(withPrivateItems([{ id: 1 }], 1, 'Private')).toEqual([{ id: 1 }])
    expect(normalizeNumber('12', 1)).toBe(12)
    expect(normalizeNumber(null, 1)).toBe(null)
    expect(normalizeNumber('1.5', 0.01)).toBe('1.50')
    expect(parseCurrencyCode('eur12.00', 'USD')).toBe('EUR')
    expect(parseCurrencyCode('12.00', 'USD')).toBe('USD')
    expect(parseMonetaryValue('EUR12.5', true)).toBe('12.50')
    expect(localeCurrencyCode('de-DE')).toBe('EUR')
    expect(localeCurrencyCode('en-US')).toBe('USD')
    expect(formatCurrency('12.5', 'USD', 'en-US')).toBe('$12.50')
  })
  it('Notiz-Anzeigename', () => {
    const users = new Map<number, any>([[1, { id: 1, username: 'u', first_name: 'A', last_name: 'B' }], [2, { id: 2, username: 'x' }]])
    expect(noteDisplayName({ id: 1, note: '', created: '', user: 1 }, users)).toBe('A B (u)')
    expect(noteDisplayName({ id: 1, note: '', created: '', user: { id: 2, username: 'x' } }, users)).toBe('x')
    expect(noteDisplayName({ id: 1, note: '', created: '' }, users)).toBe('')
  })
})

describe('Vorschläge', () => {
  it('Zählung und Zusammenführen', () => {
    const s = { title: 'x', tags: [1, 2], suggested_tags: ['n'], correspondents: [5], dates: ['2024-01-15'] }
    expect(novelSuggestions(s)).toBe(1)
    expect(fieldSuggestions(s, { tags: [1], correspondent: 5, title: 'x', created: '2024-01-15' })).toBe(1)
    expect(fieldSuggestions(s, {})).toBe(5)
    expect(noSuggestions({}, false)).toBe(true)
    expect(noSuggestions(null, false)).toBe(false)
    expect(mergeSuggestions({ tags: [1] }, { tags: [1, 2], title: 'b' })).toMatchObject({ tags: [1, 2], title: 'b' })
  })
})
