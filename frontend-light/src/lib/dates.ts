// Date display like pipes/custom-date.pipe.ts (Angular DatePipe) – with Intl instead of a library.
// Angular formats shortDate/mediumDate/longDate correspond to Intl dateStyle short/medium/long
// (both from CLDR); 'iso-8601' → y-MM-dd.

const STYLE: Record<string, 'short' | 'medium' | 'long'> = {
  shortDate: 'short',
  mediumDate: 'medium',
  longDate: 'long',
}

/** Angular parses "YYYY-MM-DD" as a local date (not UTC). */
export function parseDate(value: string | Date | null | undefined): Date | null {
  if (value == null || value === '') return null
  if (value instanceof Date) return value
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (m) return new Date(+m[1], +m[2] - 1, +m[3])
  const d = new Date(value)
  return isNaN(+d) ? null : d
}

const fmtCache = new Map<string, Intl.DateTimeFormat>()

export function formatDate(value: string | Date | null | undefined, format = 'mediumDate', locale = 'en-US'): string {
  const d = parseDate(value)
  if (!d) return ''
  if (locale === 'iso-8601') {
    const p = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
  }
  // Angular 'short'/'medium' = date + time, 'shortTime' = time only
  const opts: Intl.DateTimeFormatOptions =
    format === 'short' || format === 'medium'
      ? { dateStyle: format, timeStyle: format }
      : format === 'shortTime'
        ? { timeStyle: 'short' }
        : { dateStyle: STYLE[format] ?? 'medium' }
  const k = `${locale}|${JSON.stringify(opts)}`
  let f = fmtCache.get(k)
  if (!f) fmtCache.set(k, (f = new Intl.DateTimeFormat(locale, opts)))
  return f.format(d)
}
