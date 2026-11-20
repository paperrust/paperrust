// Relative date values of the custom field query (custom-fields-query-dropdown.component.ts: parseRelativeDate,
// relativeDateUnits) – format that upstream sends to the API.
import { describe, expect, it } from 'vitest'
import { CustomFieldQueryOperator } from './custom-field-query'
import { CustomFieldQueryAtom, CustomFieldsQueryDropdownState, RELATIVE_DATE_UNITS, parseRelativeDate } from './filter-editor/custom-field-queries'

describe('relative custom field dates', () => {
  it('parses every unit and rejects malformed amounts', () => {
    expect(parseRelativeDate('today', 'today')).toEqual({ amount: null, unit: 'today' })
    expect(parseRelativeDate('3 weeks ago', '{n} weeks ago')).toEqual({ amount: 3, unit: '{n} weeks ago' })
    expect(parseRelativeDate('in 0 years', 'in {n} years')).toEqual({ amount: 0, unit: 'in {n} years' })
    expect(parseRelativeDate('-1 days ago', '{n} days ago')).toBeUndefined()
    expect(parseRelativeDate('1.5 days ago', '{n} days ago')).toBeUndefined()
    expect(parseRelativeDate(' days ago', '{n} days ago')).toBeUndefined()
    expect(parseRelativeDate('2026-01-01', '{n} days ago')).toBeUndefined()
    expect(RELATIVE_DATE_UNITS.map((u) => u.id)).toEqual([
      'today', '{n} days ago', '{n} weeks ago', '{n} months ago', '{n} years ago',
      'in {n} days', 'in {n} weeks', 'in {n} months', 'in {n} years',
    ])
  })

  it('keeps a missing amount as null value and builds the query value from amount + unit', () => {
    const st = new CustomFieldsQueryDropdownState()
    st.customFields = [{ id: 7, name: 'Due', data_type: 'date' } as any]
    const atom = new CustomFieldQueryAtom([7, CustomFieldQueryOperator.LessThan, null])
    st.selectionModel.addAtom(atom)
    st.toggleRelativeDate(atom)
    expect(atom.serialize()).toEqual([7, 'lt', null])
    st.updateRelativeDate(atom, { unit: 'in {n} days', amount: 14 })
    expect(atom.serialize()).toEqual([7, 'lt', 'in 14 days'])
    // a fixed date restored from a saved view is not taken as relative
    expect(st.getRelativeDate(new CustomFieldQueryAtom([7, CustomFieldQueryOperator.Exact, '2026-11-13']))).toBeUndefined()
  })
})
