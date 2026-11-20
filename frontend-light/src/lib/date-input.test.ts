// Test cases from utils/ngb-date-parser-formatter.spec.ts / ngb-iso-date-adapter.spec.ts (adapted)
import { describe, expect, it } from 'vitest'
import { dateInputFormat, formatDateInput, isoFromModel, isoToModel, parseDateInput, preformatDateInput } from './date-input'

const now = new Date(2026, 10, 9)

describe('dateInputFormat', () => {
  it('nimmt date-locale, sonst Sprache (exakter Code), sonst LOCALE_ID klein', () => {
    expect(dateInputFormat('de-de', 'en-US', 'en-US')).toBe('dd.mm.yyyy')
    expect(dateInputFormat('', 'en-us', 'de-DE')).toBe('mm/dd/yyyy')
    expect(dateInputFormat('', 'de-DE', 'de-DE')).toBe('yyyy-mm-dd') // as Angular: "de-DE" ≠ "de-de"
    expect(dateInputFormat('', '', 'de-DE')).toBe('dd.mm.yyyy')
    expect(dateInputFormat('iso-8601', '', 'en-US')).toBe('yyyy-mm-dd')
  })
})

describe('LocalizedDateParserFormatter', () => {
  it('parst und formatiert de', () => {
    const f = 'dd.mm.yyyy'
    expect(parseDateInput('10.03.2022', f, now)).toEqual({ day: 10, month: 3, year: 2022 })
    expect(parseDateInput('1003', f, now)).toEqual({ day: 10, month: 3, year: 2026 })
    expect(parseDateInput('100322', f, now)).toEqual({ day: 10, month: 3, year: 2022 })
    expect(parseDateInput('10032022', f, now)).toEqual({ day: 10, month: 3, year: 2022 })
    expect(parseDateInput('1.3.99', f, now)).toEqual({ day: 1, month: 3, year: 1999 })
    expect(parseDateInput('2022-03-10', f, now)).toEqual({ day: 10, month: 3, year: 2022 })
    expect(parseDateInput('abc', f, now)).toBeNull()
    expect(formatDateInput({ day: 1, month: 3, year: 2022 }, f)).toBe('01.03.2022')
  })
  it('en-us und iso', () => {
    expect(parseDateInput('03/10/2022', 'mm/dd/yyyy', now)).toEqual({ day: 10, month: 3, year: 2022 })
    expect(preformatDateInput('0310', 'yyyy-mm-dd', now)).toBe('2026-03-10')
    expect(formatDateInput({ day: 9, month: 11, year: 2026 }, 'yyyy-mm-dd')).toBe('2026-11-09')
    expect(formatDateInput(null, 'yyyy-mm-dd')).toBeNull()
  })
})

describe('ISODateAdapter', () => {
  it('fromModel/toModel', () => {
    expect(isoFromModel('2022-03-10')).toEqual({ year: 2022, month: 3, day: 10 })
    expect(isoFromModel(null)).toBeNull()
    expect(isoToModel({ year: 22, month: 3, day: 1 })).toBe('0022-03-01')
    expect(isoToModel(null)).toBeNull()
  })
})
