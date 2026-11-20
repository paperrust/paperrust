// Logic from common/input/monetary/monetary.component.ts + Angular CurrencyPipe (Intl instead of CLDR tables).

export function parseCurrencyCode(value: string | null | undefined, def: string): string {
  return value?.toString().toUpperCase().match(/^([A-Z]{1,3})/)?.[0] ?? def
}

export function parseMonetaryValue(value: string | null | undefined, fixed = false): string {
  if (!value) return ''
  const val = parseFloat(value.toString().replace(/[^0-9.,-]+/g, ''))
  return fixed ? val.toFixed(2) : val.toString()
}

const EURO = new Set(['AT', 'BE', 'CY', 'DE', 'EE', 'ES', 'FI', 'FR', 'GR', 'HR', 'IE', 'IT', 'LT', 'LU', 'LV', 'MT', 'NL', 'PT', 'SI', 'SK'])
const OTHER: Record<string, string> = {
  US: 'USD', GB: 'GBP', CH: 'CHF', JP: 'JPY', CN: 'CNY', CA: 'CAD', AU: 'AUD', BR: 'BRL', PL: 'PLN', CZ: 'CZK', DK: 'DKK',
  NO: 'NOK', SE: 'SEK', HU: 'HUF', RO: 'RON', BG: 'BGN', TR: 'TRY', UA: 'UAH', RU: 'RUB', IN: 'INR', KR: 'KRW', TW: 'TWD',
  VN: 'VND', ID: 'IDR', IR: 'IRR', ZA: 'ZAR', BY: 'BYN', RS: 'RSD', AR: 'ARS',
}

/** getLocaleCurrencyCode (Angular) for the common regions */
export function localeCurrencyCode(locale: string): string {
  const region = locale.split('-')[1]?.toUpperCase() ?? ''
  return EURO.has(region) ? 'EUR' : (OTHER[region] ?? 'USD')
}

/** CurrencyPipe: {{ value | currency: code }} in the language of the UI */
export function formatCurrency(value: string, currency: string, locale: string): string {
  if (value === '' || value == null || isNaN(+value)) return ''
  try {
    return new Intl.NumberFormat(locale, { style: 'currency', currency }).format(+value)
  } catch {
    return `${currency}${(+value).toFixed(2)}`
  }
}
