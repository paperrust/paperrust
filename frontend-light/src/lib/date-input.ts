// Date input like Angular: SettingsService.getLocalizedDateInputFormat, LocalizedDateParserFormatter
// (utils/ngb-date-parser-formatter.ts) and ISODateAdapter (utils/ngb-iso-date-adapter.ts).

export interface DateStruct {
  year: number
  month: number
  day: number
}

/** LANGUAGE_OPTIONS (+ ISO_LANGUAGE_OPTION) from services/settings.service.ts: code → dateInputFormat */
const DATE_INPUT_FORMATS: Record<string, string> = {
  'iso-8601': 'yyyy-mm-dd',
  'en-us': 'mm/dd/yyyy', 'af-za': 'yyyy-mm-dd', 'ar-ar': 'yyyy-mm-dd', 'be-by': 'dd.mm.yyyy', 'bg-bg': 'dd.mm.yyyy',
  'ca-es': 'dd/mm/yyyy', 'cs-cz': 'dd.mm.yyyy', 'da-dk': 'dd.mm.yyyy', 'de-de': 'dd.mm.yyyy', 'el-gr': 'dd/mm/yyyy',
  'en-gb': 'dd/mm/yyyy', 'es-es': 'dd/mm/yyyy', 'fi-fi': 'dd.mm.yyyy', 'fr-fr': 'dd/mm/yyyy', 'hu-hu': 'yyyy.mm.dd',
  'id-id': 'dd-mm-yyyy', 'it-it': 'dd/mm/yyyy', 'ja-jp': 'yyyy/mm/dd', 'ko-kr': 'yyyy-mm-dd', 'lb-lu': 'dd.mm.yyyy',
  'nl-nl': 'dd-mm-yyyy', 'no-no': 'dd.mm.yyyy', 'fa-ir': 'yyyy-mm-dd', 'pl-pl': 'dd.mm.yyyy', 'pt-br': 'dd/mm/yyyy',
  'pt-pt': 'dd/mm/yyyy', 'ro-ro': 'dd.mm.yyyy', 'ru-ru': 'dd.mm.yyyy', 'sk-sk': 'dd.mm.yyyy', 'sl-si': 'dd.mm.yyyy',
  'sr-cs': 'dd.mm.yyyy', 'sv-se': 'yyyy-mm-dd', 'tr-tr': 'yyyy-mm-dd', 'uk-ua': 'dd.mm.yyyy', 'vi-vn': 'dd/mm/yyyy',
  'zh-cn': 'yyyy-mm-dd', 'zh-tw': 'yyyy/mm/dd',
}

/**
 * getLocalizedDateInputFormat: DATE_LOCALE || LANGUAGE || LOCALE_ID.toLowerCase(); comparison with the
 * codes exactly like Angular (case matters, e.g. language "de-DE" ≠ "de-de" → ISO format).
 */
export function dateInputFormat(dateLocale: string, language: string, localeId: string): string {
  const code = dateLocale || language || localeId.toLowerCase()
  return DATE_INPUT_FORMATS[code] ?? 'yyyy-mm-dd'
}

const SEPARATOR = /[\.,\/-]+/

/** Add separator, add missing year (1003 → 10.03.<year>) */
export function preformatDateInput(value: string, inputFormat: string, now = new Date()): string {
  const sep = inputFormat.replace(/[dmy]/gi, '').charAt(0)
  if (SEPARATOR.test(value)) {
    const segments = value.split(SEPARATOR)
    if (segments.length == 3 && segments[0].length == 4) {
      return inputFormat.replace('yyyy', segments[0]).replace('mm', segments[1].padStart(2, '0')).replace('dd', segments[2].padStart(2, '0'))
    }
    value = segments.map((s) => s.padStart(2, '0')).join('')
  }
  const yFirst = inputFormat.substring(0, 4) == 'yyyy'
  if (value.length == 4 && !yFirst) return [value.substring(0, 2), value.substring(2, 4), now.getFullYear()].join(sep)
  if (value.length == 4 && yFirst) return [now.getFullYear(), value.substring(0, 2), value.substring(2, 4)].join(sep)
  if (value.length == 6) return [value.substring(0, 2), value.substring(2, 4), value.substring(4, 6)].join(sep)
  if (value.length == 8 && !yFirst) return [value.substring(0, 2), value.substring(2, 4), value.substring(4, 8)].join(sep)
  if (value.length == 8 && yFirst) return [value.substring(0, 4), value.substring(4, 6), value.substring(6, 8)].join(sep)
  return value
}

export function parseDateInput(value: string, inputFormat: string, now = new Date()): DateStruct | null {
  if (value == null) return null
  value = preformatDateInput(value, inputFormat, now)
  const re = new RegExp(
    '^' + inputFormat.replace('dd', '(?<day>[0-9]+)').replace('mm', '(?<month>[0-9]+)').replace('yyyy', '(?<year>[0-9]+)').split('.').join('\\.\\s*') + '$',
  )
  const m = re.exec(value)
  if (!m?.groups) return null
  const currentYear = now.getFullYear()
  const century = currentYear - (currentYear % 100)
  let year = +m.groups.year
  if (year < 100) {
    let four = century + year
    if (four > currentYear + 49) four -= 100
    else if (four <= currentYear - 50) four += 100
    year = four
  }
  return { day: +m.groups.day, month: +m.groups.month, year }
}

export function formatDateInput(d: DateStruct | null, inputFormat: string): string | null {
  if (!d) return null
  return inputFormat
    .replace('dd', String(d.day).padStart(2, '0'))
    .replace('mm', String(d.month).padStart(2, '0'))
    .replace('yyyy', String(d.year).padStart(4, '0'))
}

/** ISODateAdapter.fromModel */
export function isoFromModel(value: string | null | undefined): DateStruct | null {
  if (!value) return null
  if (/\d\d\d\d-\d\d-\d\d/.test(value)) {
    const s = value.split('-')
    return { year: parseInt(s[0]), month: parseInt(s[1]), day: parseInt(s[2]) }
  }
  const d = new Date(value)
  return { day: d.getDate(), month: d.getMonth() + 1, year: d.getFullYear() }
}

/** ISODateAdapter.toModel */
export function isoToModel(d: DateStruct | null): string | null {
  if (!d) return null
  return `${String(d.year).padStart(4, '0')}-${String(d.month).padStart(2, '0')}-${String(d.day).padStart(2, '0')}`
}
