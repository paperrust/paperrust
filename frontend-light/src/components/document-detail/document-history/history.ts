// CustomDatePipe 'relative' (pipes/custom-date.pipe.ts) and TitleCasePipe (Angular).

import { msg } from '../../../lib/i18n'

const INTERVALS: [string, string, number][] = [
  [msg('detail.last_year'), msg('detail.n_years_ago'), 31536000],
  [msg('detail.last_month'), msg('detail.n_months_ago'), 2592000],
  [msg('detail.last_week'), msg('detail.n_weeks_ago'), 604800],
  [msg('common.yesterday'), msg('detail.n_days_ago'), 86400],
  [msg('detail.n_hour_ago'), msg('detail.n_hours_ago'), 3600],
  [msg('detail.n_minute_ago'), msg('detail.n_minutes_ago'), 60],
]

/** Key (English source text) + number for %s; Angular replaces %s only after translation. */
export function relativeDate(value: string | Date, now = new Date()): { key: string; n: number } {
  const seconds = Math.floor((+now - +new Date(value)) / 1000)
  if (seconds < 60) return { key: msg('detail.just_now'), n: 0 }
  for (const [one, many, interval] of INTERVALS) {
    const counter = Math.floor(seconds / interval)
    if (counter > 0) return { key: counter > 1 ? many : one, n: counter }
  }
  return { key: msg('detail.just_now'), n: 0 }
}

/** TitleCasePipe: first character of each word uppercase, rest lowercase (words separated by whitespace). */
export function titleCase(value: string | null | undefined): string {
  if (!value) return value ?? ''
  return value.replace(/[^\s]+/g, (w) => w[0].toUpperCase() + w.slice(1).toLowerCase())
}
