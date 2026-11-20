import { describe, expect, it } from 'vitest'
import { buildMonth, firstDayOfWeek, firstViewDate, getNext, getPrev, keyTarget, monthNames, selectYears, weekday } from './datepicker'

const d = (year: number, month: number, day: number) => ({ year, month, day })

describe('datepicker (NgbCalendarGregorian / buildMonth)', () => {
  it('getNext/getPrev wie ngb (Monatsende)', () => {
    expect(getNext(d(2026, 1, 31), 'm', 1)).toEqual(d(2026, 2, 28))
    expect(getNext(d(2026, 12, 15), 'm', 1)).toEqual(d(2027, 1, 15))
    expect(getPrev(d(2026, 3, 1), 'd', 1)).toEqual(d(2026, 2, 28))
    expect(getNext(d(2024, 2, 29), 'y', 1)).toEqual(d(2025, 2, 28))
  })

  it('Wochentag 1 = Montag … 7 = Sonntag', () => {
    expect(weekday(d(2026, 11, 9))).toBe(1)
    expect(weekday(d(2026, 11, 8))).toBe(7)
  })

  it('erster Wochentag aus Intl.Locale (PngxDatePickerConfig)', () => {
    expect(firstDayOfWeek('de-DE')).toBe(1)
    expect([1, 7]).toContain(firstDayOfWeek('en-US'))
  })

  it('erste sichtbare Woche beginnt am ersten Wochentag', () => {
    expect(firstViewDate(2026, 11, 1)).toEqual(d(2026, 10, 26))
    expect(firstViewDate(2026, 11, 7)).toEqual(d(2026, 11, 1))
  })

  it('baut 6 Wochen, markiert Auswahl, heute, Fokus und fremde Monate', () => {
    const m = buildMonth(2026, 11, { locale: 'de-DE', firstDay: 1, today: d(2026, 11, 9), selected: d(2026, 11, 3), focus: d(2026, 11, 3), focusVisible: true })
    expect(m.weeks).toHaveLength(6)
    expect(m.weekdays).toEqual(['M', 'D', 'M', 'D', 'F', 'S', 'S'])
    const days = m.weeks.flat()
    expect(days[0].date).toEqual(d(2026, 10, 26))
    expect(days[0].muted).toBe(true)
    const sel = days.find((x) => x.selected)!
    expect(sel.date).toEqual(d(2026, 11, 3))
    expect(sel.tabindex).toBe(0)
    expect(sel.focused).toBe(true)
    expect(sel.muted).toBe(false)
    expect(days.filter((x) => x.tabindex === 0)).toHaveLength(1)
    expect(days.find((x) => x.today)!.date).toEqual(d(2026, 11, 9))
  })

  it('Monatsnamen und Jahresauswahl (±10)', () => {
    expect(monthNames('en-US', 'short')[0]).toBe('Jan')
    expect(monthNames('de-DE', 'long')[2]).toBe('März')
    const y = selectYears(2026)
    expect(y[0]).toBe(2016)
    expect(y[y.length - 1]).toBe(2036)
  })

  it('Tastatur wie NgbDatepickerKeyboardService', () => {
    const view = { first: d(2026, 11, 1), last: d(2026, 11, 30) }
    expect(keyTarget('ArrowDown', false, d(2026, 11, 9), view)).toEqual(d(2026, 11, 16))
    expect(keyTarget('PageUp', true, d(2026, 11, 9), view)).toEqual(d(2025, 11, 9))
    expect(keyTarget('Home', false, d(2026, 11, 9), view)).toEqual(d(2026, 11, 1))
    expect(keyTarget('x', false, d(2026, 11, 9), view)).toBeNull()
  })
})
