// Logic of ng-bootstrap 21 NgbDatepicker (datepicker-tools.ts buildMonth/getFirstViewDate, NgbCalendarGregorian,
// NgbDatepickerI18nDefault) for the calendar popup (Datepicker.svelte): one month, 6 weeks, navigation "select",
// days outside the month visible, weekdays "narrow". First weekday like PngxDatePickerConfig
// (Intl.Locale(LOCALE_ID).getWeekInfo().firstDay, otherwise 1 = Monday).
import type { DateStruct } from '../../../lib/date-input'

export interface DpDay {
  date: DateStruct
  ariaLabel: string
  today: boolean
  selected: boolean
  focused: boolean
  /** NgbDatepickerDayView.isMuted: not selected and different month */
  muted: boolean
  tabindex: 0 | -1
}

export interface DpMonth {
  year: number
  month: number
  weekdays: string[]
  weeks: DpDay[][]
}

const toJs = (d: DateStruct) => {
  const js = new Date(d.year, d.month - 1, d.day, 12)
  if (!isNaN(js.getTime())) js.setFullYear(d.year)
  return js
}
const fromJs = (js: Date): DateStruct => ({ year: js.getFullYear(), month: js.getMonth() + 1, day: js.getDate() })
export const sameDate = (a: DateStruct | null | undefined, b: DateStruct | null | undefined) => !!a && !!b && a.year === b.year && a.month === b.month && a.day === b.day
export const before = (a: DateStruct, b: DateStruct) => a.year !== b.year ? a.year < b.year : a.month !== b.month ? a.month < b.month : a.day < b.day

/** NgbCalendarGregorian.getNext ('d' | 'm' | 'y'; month end like ngb) */
export function getNext(date: DateStruct, period: 'd' | 'm' | 'y' = 'd', n = 1): DateStruct {
  const js = toJs(date)
  let checkMonth = true
  let expected = js.getMonth()
  if (period === 'y') js.setFullYear(js.getFullYear() + n)
  else if (period === 'm') {
    expected += n
    js.setMonth(expected)
    expected = expected % 12
    if (expected < 0) expected += 12
  } else {
    js.setDate(js.getDate() + n)
    checkMonth = false
  }
  if (checkMonth && js.getMonth() !== expected) js.setDate(0)
  return fromJs(js)
}
export const getPrev = (date: DateStruct, period: 'd' | 'm' | 'y' = 'd', n = 1) => getNext(date, period, -n)

/** NgbCalendarGregorian.getWeekday: 1 = Monday … 7 = Sunday */
export function weekday(d: DateStruct): number {
  const w = toJs(d).getDay()
  return w === 0 ? 7 : w
}

export function today(now = new Date()): DateStruct {
  return fromJs(now)
}

/** PngxDatePickerConfig.firstDayOfWeek */
export function firstDayOfWeek(locale: string): number {
  try {
    const l = new Intl.Locale(locale) as any
    const first = l.getWeekInfo?.().firstDay ?? l.weekInfo?.firstDay
    if (first !== undefined) return first
  } catch {}
  return 1
}

/** NgbDatepickerI18nDefault */
export function monthNames(locale: string, width: 'short' | 'long'): string[] {
  const f = Intl.DateTimeFormat(locale, { month: width, timeZone: 'UTC' })
  return [...Array(12).keys()].map((m) => f.format(Date.UTC(2000, m)))
}
export function weekdayLabel(locale: string, wd: number, width: 'narrow' | 'short' = 'narrow'): string {
  // 1 May 2000 is a Monday
  return Intl.DateTimeFormat(locale, { weekday: width, timeZone: 'UTC' }).format(Date.UTC(2000, 4, wd))
}
/** getDayAriaLabel (Angular formatDate 'fullDate') */
export function dayAriaLabel(locale: string, d: DateStruct): string {
  return Intl.DateTimeFormat(locale, { dateStyle: 'full' }).format(toJs(d))
}

/** getFirstViewDate */
export function firstViewDate(year: number, month: number, firstDay: number): DateStruct {
  const first = { year, month, day: 1 }
  const dow = weekday(first) % 7
  return getPrev(first, 'd', (7 + dow - firstDay) % 7)
}

/** buildMonth + _patchContexts (displayMonths 1, outsideDays 'visible') */
export function buildMonth(
  year: number,
  month: number,
  o: { locale: string; firstDay: number; today: DateStruct; selected: DateStruct | null; focus: DateStruct | null; focusVisible: boolean },
): DpMonth {
  let date = firstViewDate(year, month, o.firstDay)
  const weekdays: string[] = []
  const weeks: DpDay[][] = []
  for (let w = 0; w < 6; w++) {
    const days: DpDay[] = []
    for (let i = 0; i < 7; i++) {
      if (w === 0) weekdays.push(weekdayLabel(o.locale, weekday(date)))
      const selected = sameDate(o.selected, date)
      days.push({
        date,
        ariaLabel: dayAriaLabel(o.locale, date),
        today: sameDate(o.today, date),
        selected,
        focused: o.focusVisible && sameDate(o.focus, date),
        muted: !selected && date.month !== month,
        tabindex: o.focus && sameDate(o.focus, date) && o.focus.month === month ? 0 : -1,
      })
      date = getNext(date)
    }
    weeks.push(days)
  }
  return { year, month, weekdays, weeks }
}

/** generateSelectBoxYears (without min/max: ±10 years) */
export const selectYears = (year: number) => Array.from({ length: 21 }, (_, i) => year - 10 + i)

/** NgbDatepickerKeyboardService.processKey: new focus date (or null = not handled) */
export function keyTarget(key: string, shift: boolean, focus: DateStruct, view: { first: DateStruct; last: DateStruct }): DateStruct | null {
  switch (key) {
    case 'PageUp':
      return getPrev(focus, shift ? 'y' : 'm', 1)
    case 'PageDown':
      return getNext(focus, shift ? 'y' : 'm', 1)
    case 'End':
      return view.last
    case 'Home':
      return view.first
    case 'ArrowLeft':
      return getPrev(focus, 'd', 1)
    case 'ArrowUp':
      return getPrev(focus, 'd', 7)
    case 'ArrowRight':
      return getNext(focus, 'd', 1)
    case 'ArrowDown':
      return getNext(focus, 'd', 7)
  }
  return null
}
