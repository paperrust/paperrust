import { describe, expect, it } from 'vitest'
import { getMatching, patternRequired, randomColor, MATCH_ANY, MATCH_AUTO, MATCH_NONE, MATCH_REGEX, matchingAlgorithms } from './matching'
import { setSettingValue } from './settings-write'
import { bsPlacementClass, resolvePlacements } from './popover.svelte'

describe('matching (ManagementListComponent.getMatching)', () => {
  it('liefert Automatisch/Keine/Kurzname: Muster/-', () => {
    expect(getMatching({ id: 1, matching_algorithm: MATCH_AUTO })).toBe('Automatic')
    expect(getMatching({ id: 1, matching_algorithm: MATCH_NONE })).toBe('None')
    expect(getMatching({ id: 1, matching_algorithm: MATCH_ANY, match: 'foo bar' })).toBe('Any word: foo bar')
    expect(getMatching({ id: 1, matching_algorithm: MATCH_REGEX, match: '' })).toBe('-')
  })
  it('patternRequired nur für Wort-/Regex-Algorithmen', () => {
    expect(patternRequired(MATCH_AUTO)).toBe(false)
    expect(patternRequired(MATCH_NONE)).toBe(false)
    expect(patternRequired(MATCH_ANY)).toBe(true)
  })
  it('Reihenfolge der Algorithmen wie Angular', () => {
    expect(matchingAlgorithms().map((a) => a.id)).toEqual([6, 1, 2, 3, 4, 5, 0])
  })
  it('randomColor liefert #rrggbb', () => {
    expect(randomColor(() => 0.5)).toMatch(/^#[0-9a-f]{6}$/)
  })
})

describe('SettingsService.set', () => {
  it('legt verschachtelte Schlüssel an (general-settings: entfällt, - → _)', () => {
    const s: any = { dark_mode: { use_system: true } }
    setSettingValue(s, 'general-settings:dark-mode:enabled', 'true')
    setSettingValue(s, 'general-settings:object-list-sizes', { tags: 50 })
    setSettingValue(s, 'language', 'de-de')
    expect(s).toEqual({ dark_mode: { use_system: true, enabled: 'true' }, object_list_sizes: { tags: 50 }, language: 'de-de' })
  })
  it('ignoriert unsichere Schlüssel', () => {
    const s: any = {}
    setSettingValue(s, '__proto__:x', 1)
    expect(({} as any).x).toBeUndefined()
  })
})

describe('NgbPopover-Platzierung', () => {
  it("löst 'auto' wie getPopperOptions auf", () => {
    expect(resolvePlacements('auto').slice(0, 4)).toEqual(['top', 'bottom', 'left', 'right'])
    expect(resolvePlacements('bottom')).toEqual(['bottom'])
  })
  it('setzt bs-popover-Klassen', () => {
    expect(bsPlacementClass('bs-popover', 'bottom')).toBe('bs-popover-bottom')
    expect(bsPlacementClass('bs-popover', 'left-start')).toBe('bs-popover-start bs-popover-start-top')
  })
})
