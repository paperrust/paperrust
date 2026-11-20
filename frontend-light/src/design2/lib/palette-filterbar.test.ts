// Search + question (palette.ts) and additional filters of the filter bar (filterbar.ts), docs/DESIGN2-ENTWIRREN.md §5–6.
import { describe, expect, it } from 'vitest'
import { askPath, textActions } from './palette'
import { activeFilterCount, getFieldFilter, getFields, getOwnerMode, remainingRules, setFields, setOwnerMode, OWNER_MODES } from './filterbar'
import { clearObjects, getObjects, otherRules, queryFromState, stateFromQuery, toggleObject, QUICK_FILTERS } from './filters'
import {
  FILTER_CUSTOM_FIELDS_QUERY,
  FILTER_HAS_CUSTOM_FIELDS_ALL,
  FILTER_HAS_TAGS_ALL,
  FILTER_IS_IN_INBOX,
  FILTER_OWNER,
  FILTER_OWNER_DOES_NOT_INCLUDE,
  FILTER_OWNER_ISNULL,
  FILTER_SHARED_BY_USER,
  FILTER_SIMPLE_TEXT,
} from '../../lib/filter-rule-type'

describe('Suche mit Frage', () => {
  it('leer → nichts; KI aus → nur Volltext; KI an → Volltext und Frage', () => {
    expect(textActions('  ', true)).toEqual([])
    expect(textActions(' Rechnung ', false)).toEqual([{ kind: 'fulltext', query: 'Rechnung' }])
    expect(textActions('Wann endet der Vertrag?', true).map((a) => a.kind)).toEqual(['fulltext', 'ask'])
  })
  it('Antwortansicht mit kodierter Frage', () => {
    expect(askPath(' a&b? ')).toBe('assistant?q=a%26b%3F')
  })
})

describe('Filterleiste: Felder', () => {
  it('setzen, lesen, ersetzen, URL (custom_field_query wie die klassische Oberfläche)', () => {
    let r = setFields([{ rule_type: FILTER_SIMPLE_TEXT, value: 'x' }], [3, 5])
    expect(getFields(r)).toEqual([3, 5])
    expect(r[1]).toEqual({ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '["AND",[[3,"exists",true],[5,"exists",true]]]' })
    r = setFields(r, [5])
    expect(getFields(r)).toEqual([5])
    expect(setFields(r, [])).toEqual([{ rule_type: FILTER_SIMPLE_TEXT, value: 'x' }])
    const back = stateFromQuery(queryFromState({ rules: setFields([], [3, 5]), sortField: 'created', sortReverse: true })).rules
    expect(getFields(back)).toEqual([3, 5])
  })
  it('alte „hat Feld“-Regeln aus URLs werden übernommen', () => {
    const back = stateFromQuery('?custom_fields__id__all=4').rules
    expect(getFields(back)).toEqual([4])
  })
  it('andere Abfragen = „query“ (nur zurücksetzbar)', () => {
    expect(getFieldFilter([{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '["AND",[[3,"exact","x"]]]' }]).kind).toBe('query')
    expect(getFieldFilter([{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: 'kaputt' }]).kind).toBe('query')
    expect(getFieldFilter([{ rule_type: FILTER_HAS_CUSTOM_FIELDS_ALL, value: '1' }]).kind).toBe('none')
  })
})

describe('Filterleiste: Rechte', () => {
  const me = 7
  it('jede Wahl ergibt beim Lesen wieder dieselbe Wahl', () => {
    for (const m of OWNER_MODES) expect(getOwnerMode(setOwnerMode([], m, me), me)).toBe(m)
  })
  it('Regeltypen wie die klassische Oberfläche', () => {
    expect(setOwnerMode([], 'mine', me)).toEqual([{ rule_type: FILTER_OWNER, value: '7' }])
    expect(setOwnerMode([], 'shared_by_me', me)).toEqual([{ rule_type: FILTER_SHARED_BY_USER, value: '7' }])
    expect(setOwnerMode([], 'unowned', me)).toEqual([{ rule_type: FILTER_OWNER_ISNULL, value: 'true' }])
    expect(setOwnerMode([], 'shared_with_me', me)).toEqual([
      { rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '7' },
      { rule_type: FILTER_OWNER_ISNULL, value: 'false' },
    ])
  })
  it('Wechsel ersetzt, andere Regeln bleiben; fremde Kombination = custom', () => {
    const tag = { rule_type: FILTER_HAS_TAGS_ALL, value: '1' }
    const r = setOwnerMode(setOwnerMode([tag], 'mine', me), 'unowned', me)
    expect(r).toEqual([tag, { rule_type: FILTER_OWNER_ISNULL, value: 'true' }])
    expect(setOwnerMode(r, 'all', me)).toEqual([tag])
    expect(getOwnerMode([{ rule_type: FILTER_OWNER, value: '9' }], me)).toBe('custom')
    expect(getOwnerMode([{ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '7' }], me)).toBe('shared_with_me')
  })
  it('ohne bekannten Benutzer keine benutzerbezogenen Regeln', () => {
    expect(setOwnerMode([], 'mine', null)).toEqual([])
  })
})

describe('Filterleiste: Zähler und übrige Regeln', () => {
  it('Felder/Rechte zählen nicht als „weitere Regeln“', () => {
    const r = [...setFields([], [1]), ...setOwnerMode([], 'mine', 7), { rule_type: FILTER_IS_IN_INBOX, value: 'true' }]
    expect(remainingRules(r)).toEqual([{ rule_type: FILTER_IS_IN_INBOX, value: 'true' }])
    expect(activeFilterCount(r, 7)).toBe(3)
    expect(activeFilterCount([{ rule_type: FILTER_SIMPLE_TEXT, value: 'x' }], 7)).toBe(0)
  })
})

describe('Filterleiste: Tags „nicht zugewiesen“ (= früherer Schnellfilter „Ohne Tags“)', () => {
  it('gleiche Regel und Query wie der Schnellfilter der Befehlspalette', () => {
    const quick = QUICK_FILTERS.find((x) => x.id === 'untagged')!.rules('2026-11-16')
    const none = toggleObject([], 'tags', null)
    expect(none).toEqual(quick)
    const q = queryFromState({ rules: none, sortField: 'created', sortReverse: true })
    expect(q).toBe(queryFromState({ rules: quick, sortField: 'created', sortReverse: true }))
    expect(q).toContain('is_tagged=0')
    // back from the URL (is_tagged=0 → 'false') is recognized as "not assigned" again
    expect(getObjects(stateFromQuery(q).rules, 'tags')).toEqual([null])
    expect(otherRules(none)).toEqual([])
    expect(remainingRules(none)).toEqual([])
    expect(activeFilterCount(none, 1)).toBe(1)
  })
  it('schließt konkrete Tags aus und umgekehrt; „hat Tags“ (is_tagged=1) bleibt eine übrige Regel', () => {
    let r = toggleObject([], 'tags', 4)
    r = toggleObject(r, 'tags', null)
    expect(getObjects(r, 'tags')).toEqual([null])
    expect(r.some((x) => x.rule_type === FILTER_HAS_TAGS_ALL)).toBe(false)
    r = toggleObject(r, 'tags', 5)
    expect(getObjects(r, 'tags')).toEqual([5])
    expect(clearObjects(toggleObject([], 'tags', null), 'tags')).toEqual([])
    const tagged = [{ rule_type: 7, value: 'true' }]
    expect(getObjects(tagged, 'tags')).toEqual([])
    expect(otherRules(tagged)).toEqual(tagged)
  })
})
