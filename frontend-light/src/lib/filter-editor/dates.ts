// Port of the pure parts of DatesDropdownComponent
// (src-ui/src/app/components/common/dates-dropdown/dates-dropdown.component.ts).
// `datesSet` output → Emitter (`onChange(fn)`); the 400 ms debounce of manual date
// input uses setTimeout (cancel with `destroy()`).

import { Emitter } from './emitter'

export interface DateSelection {
  createdTo?: string
  createdFrom?: string
  createdRelativeDateID?: number
  addedTo?: string
  addedFrom?: string
  addedRelativeDateID?: number
}

export const RelativeDate = {
  WITHIN_1_WEEK: 1,
  WITHIN_1_MONTH: 2,
  WITHIN_3_MONTHS: 3,
  WITHIN_1_YEAR: 4,
  THIS_YEAR: 5,
  THIS_MONTH: 6,
  TODAY: 7,
  YESTERDAY: 8,
  PREVIOUS_WEEK: 9,
  PREVIOUS_MONTH: 10,
  PREVIOUS_QUARTER: 11,
  PREVIOUS_YEAR: 12,
} as const
export type RelativeDate = (typeof RelativeDate)[keyof typeof RelativeDate]

export interface RelativeDateOption {
  id: RelativeDate
  /** i18n key (English source); translate with `t(option.name)`. */
  name: string
  /** Timestamp (ms) for "within" options. */
  dateTilNow?: number
  /** Start (Date or timestamp, as in Angular). */
  date?: Date | number
  /** End date. */
  dateEnd?: Date
}

/** i18n keys of the relative date options, in display order. */
export const RELATIVE_DATE_LABELS: Record<RelativeDate, string> = {
  [RelativeDate.WITHIN_1_WEEK]: 'filter.within_1_week',
  [RelativeDate.WITHIN_1_MONTH]: 'filter.within_1_month',
  [RelativeDate.WITHIN_3_MONTHS]: 'filter.within_3_months',
  [RelativeDate.WITHIN_1_YEAR]: 'filter.within_1_year',
  [RelativeDate.THIS_YEAR]: 'filter.year',
  [RelativeDate.THIS_MONTH]: 'filter.month',
  [RelativeDate.TODAY]: 'common.today',
  [RelativeDate.YESTERDAY]: 'common.yesterday',
  [RelativeDate.PREVIOUS_WEEK]: 'filter.previous_week',
  [RelativeDate.PREVIOUS_MONTH]: 'common.previous_month',
  [RelativeDate.PREVIOUS_QUARTER]: 'filter.previous_quarter',
  [RelativeDate.PREVIOUS_YEAR]: 'common.previous_year',
}

/**
 * The `relativeDates` array of the component (with the dates shown next to each option),
 * computed for `now` (Angular computes it once at component creation).
 */
export function getRelativeDates(now: Date = new Date()): RelativeDateOption[] {
  const d = () => new Date(now.getTime())
  const y = now.getFullYear()
  const m = now.getMonth()
  const day = now.getDate()
  const wd = now.getDay()
  const n = (id: RelativeDate) => RELATIVE_DATE_LABELS[id]
  const at = (fn: (x: Date) => number) => {
    const x = d()
    return fn(x)
  }
  return [
    { id: RelativeDate.WITHIN_1_WEEK, name: n(1), dateTilNow: at((x) => x.setDate(day - 7)) },
    { id: RelativeDate.WITHIN_1_MONTH, name: n(2), dateTilNow: at((x) => x.setMonth(m - 1)) },
    { id: RelativeDate.WITHIN_3_MONTHS, name: n(3), dateTilNow: at((x) => x.setMonth(m - 3)) },
    { id: RelativeDate.WITHIN_1_YEAR, name: n(4), dateTilNow: at((x) => x.setFullYear(y - 1)) },
    { id: RelativeDate.THIS_YEAR, name: n(5), date: new Date(y, 0, 1), dateEnd: new Date(y, 11, 31) },
    { id: RelativeDate.THIS_MONTH, name: n(6), date: at((x) => x.setDate(1)), dateEnd: new Date(y, m + 1, 0) },
    { id: RelativeDate.TODAY, name: n(7), date: at((x) => x.setHours(0, 0, 0, 0)) },
    { id: RelativeDate.YESTERDAY, name: n(8), date: at((x) => x.setDate(day - 1)) },
    {
      id: RelativeDate.PREVIOUS_WEEK,
      name: n(9),
      date: new Date(y, m, day - wd - 6),
      dateEnd: new Date(y, m, day - wd),
    },
    { id: RelativeDate.PREVIOUS_MONTH, name: n(10), date: new Date(y, m - 1, 1), dateEnd: new Date(y, m, 0) },
    {
      id: RelativeDate.PREVIOUS_QUARTER,
      name: n(11),
      date: new Date(y, Math.floor(m / 3) * 3 - 3, 1),
      dateEnd: new Date(y, Math.floor(m / 3) * 3, 0),
    },
    { id: RelativeDate.PREVIOUS_YEAR, name: n(12), date: new Date(y - 1, 0, 1), dateEnd: new Date(y - 1, 11, 31) },
  ]
}

/** `today` of the component: local date as YYYY-MM-DD. */
export function todayISO(now: Date = new Date()): string {
  return now.toLocaleDateString('en-CA')
}

/** `onKeyPress`: allow only digits and separators (returns false → preventDefault). */
export function isAllowedDateInputKey(key: string): boolean {
  return 'Enter' === key || /[0-9,\.\/-]+/.test(key)
}

export const DATES_DEBOUNCE_MS = 400

/** Date-range model + logic of `<pngx-dates-dropdown>`. */
export class DatesModel {
  /** Angular `datesSet` output (also covers the individual `…Change` outputs). */
  readonly datesSet = new Emitter<DateSelection>()

  createdDateTo: string = null
  createdDateFrom: string = null
  createdRelativeDate: RelativeDate = null
  addedDateTo: string = null
  addedDateFrom: string = null
  addedRelativeDate: RelativeDate = null

  title: string = null
  disabled = false

  private debounceTimer: ReturnType<typeof setTimeout> = null

  /** Subscribe to `datesSet`. Returns unsubscribe. */
  onChange(fn: (selection: DateSelection) => void): () => void {
    return this.datesSet.subscribe(fn)
  }

  get isActive(): boolean {
    return (
      this.createdRelativeDate !== null ||
      this.createdDateFrom?.length > 0 ||
      this.createdDateTo?.length > 0 ||
      this.addedRelativeDate !== null ||
      this.addedDateFrom?.length > 0 ||
      this.addedDateTo?.length > 0
    )
  }

  /** Toggle button `btn-primary` (Angular only checks the created range here; the badge uses isActive). */
  get toggleHighlighted(): boolean {
    return !!(this.createdDateTo || this.createdDateFrom)
  }

  get selection(): DateSelection {
    return {
      createdFrom: this.createdDateFrom,
      createdTo: this.createdDateTo,
      createdRelativeDateID: this.createdRelativeDate,
      addedFrom: this.addedDateFrom,
      addedTo: this.addedDateTo,
      addedRelativeDateID: this.addedRelativeDate,
    }
  }

  reset(): void {
    this.createdDateTo = null
    this.createdDateFrom = null
    this.createdRelativeDate = null
    this.addedDateTo = null
    this.addedDateFrom = null
    this.addedRelativeDate = null
    this.onChangeEmit()
  }

  /** Relative-date select changed (Angular sets the value via ngModel first). */
  onSetCreatedRelativeDate(id: RelativeDate | null = this.createdRelativeDate): void {
    this.createdRelativeDate = id
    this.createdDateTo = null
    this.createdDateFrom = null
    this.onChangeEmit()
  }

  onSetAddedRelativeDate(id: RelativeDate | null = this.addedRelativeDate): void {
    this.addedRelativeDate = id
    this.addedDateTo = null
    this.addedDateFrom = null
    this.onChangeEmit()
  }

  /** Angular `onChange()` – emits `datesSet` immediately. */
  onChangeEmit(): void {
    this.datesSet.next(this.selection)
  }

  /** Manual date input / date picker: clears relative dates, emits after 400 ms. */
  onChangeDebounce(): void {
    this.createdRelativeDate = null
    this.addedRelativeDate = null
    if (this.debounceTimer) clearTimeout(this.debounceTimer)
    this.debounceTimer = setTimeout(() => {
      this.debounceTimer = null
      this.onChangeEmit()
    }, DATES_DEBOUNCE_MS)
  }

  clearCreatedTo(): void {
    this.createdDateTo = null
    this.onChangeEmit()
  }

  clearCreatedFrom(): void {
    this.createdDateFrom = null
    this.onChangeEmit()
  }

  clearCreatedRelativeDate(): void {
    this.createdRelativeDate = null
    this.onChangeEmit()
  }

  clearAddedTo(): void {
    this.addedDateTo = null
    this.onChangeEmit()
  }

  clearAddedFrom(): void {
    this.addedDateFrom = null
    this.onChangeEmit()
  }

  clearAddedRelativeDate(): void {
    this.addedRelativeDate = null
    this.onChangeEmit()
  }

  /** Cancel a pending debounced emission. */
  destroy(): void {
    if (this.debounceTimer) clearTimeout(this.debounceTimer)
    this.debounceTimer = null
  }
}
