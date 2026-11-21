// Vitest: temporary selection of single documents (`id__in`, PaperRust rule type FILTER_ID_IN)
import { describe, expect, it } from 'vitest'
import { FILTER_ID_IN } from '../../lib/filter-rule-type'
import { queryParamsFromFilterRules } from '../../lib/query-params'
import { clearIdSelection, getIdSelection, idSelectionQuery, otherRules, queryFromState, stateFromQuery } from './filters'
import { activeFilterCount, remainingRules } from './filterbar'

describe('Auswahl aus dem Assistenten (id__in)', () => {
  it('URL ↔ Regeln ↔ API-Parameter', () => {
    const q = idSelectionQuery([12, 5, 12, 7])
    expect(q).toBe('?id__in=12%2C5%2C7&sort=created&reverse=1')
    const s = stateFromQuery(q)
    expect(s.rules).toEqual([
      { rule_type: FILTER_ID_IN, value: '12' },
      { rule_type: FILTER_ID_IN, value: '5' },
      { rule_type: FILTER_ID_IN, value: '7' },
    ])
    expect(getIdSelection(s.rules)).toEqual([12, 5, 7])
    expect(queryParamsFromFilterRules(s.rules)).toEqual({ id__in: '12,5,7' })
    expect(queryFromState(s)).toBe(q)
  })
  it('eigener Chip (nicht "weitere Regeln"), entfernbar, zählt als Filter', () => {
    const rules = [...stateFromQuery('?id__in=1,2').rules, { rule_type: 6, value: '3' }]
    expect(otherRules(rules)).toEqual([])
    expect(remainingRules(rules)).toEqual([])
    expect(activeFilterCount(rules, 1)).toBe(2)
    expect(clearIdSelection(rules)).toEqual([{ rule_type: 6, value: '3' }])
  })
  it('ungültige Werte werden ignoriert', () => {
    expect(idSelectionQuery([0, -1, 1.5])).toBe('?sort=created&reverse=1')
    expect(getIdSelection(stateFromQuery('?id__in=abc,4').rules)).toEqual([4])
  })
})
