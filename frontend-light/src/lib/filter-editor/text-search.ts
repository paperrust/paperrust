// Port of src-ui/src/app/utils/text-search.ts (+ pipes/filter.pipe.ts).
//
// Difference to Angular: Angular folds diacritics with the `normalize-diacritics`
// lookup table. Here we use Unicode NFD + stripping `\p{Diacritic}` plus a small
// supplement table for Latin letters that do not decompose (ø, æ, œ, ß, ł, đ, ð, þ, ħ, ı,
// ŀ, ŧ, ligatures ﬀ–ﬄ) with exactly the replacements the library uses. Remaining
// differences: exotic letters only covered by the library table (e.g. ƀ, ɓ, circled
// and full-width letters) are not folded, and NFD additionally strips combining marks
// of non-Latin scripts (e.g. Greek tonos, Hebrew points) – applied
// symmetrically to value and query.

export type SearchTextValue = string | number | boolean | bigint | null | undefined

const NON_ASCII = /[^\x00-\x7F]/
const SEPARATORS = /[^\p{L}\p{N}]+/u
const DIACRITIC = /\p{Diacritic}/gu

const SUPPLEMENT: Record<string, string> = {
  Æ: 'AE', æ: 'ae', Ð: 'Dh', ð: 'dh', Ø: 'O', ø: 'o', Þ: 'Th', þ: 'th', ß: 'ss', ẞ: 'S',
  Đ: 'D', đ: 'd', Ħ: 'H', ħ: 'h', ı: 'i', Ŀ: 'L', ŀ: 'l', Ł: 'L', ł: 'l', Œ: 'OE', œ: 'oe',
  Ŧ: 'T', ŧ: 't', ﬀ: 'ff', ﬁ: 'fi', ﬂ: 'fl', ﬃ: 'ffi', ﬄ: 'ffl',
}
const SUPPLEMENT_RE = new RegExp(`[${Object.keys(SUPPLEMENT).join('')}]`, 'g')

/** Diacritic folding (stand-in for `normalize-diacritics`). */
export function foldDiacritics(text: string): string {
  return text
    .replace(SUPPLEMENT_RE, (c) => SUPPLEMENT[c])
    .normalize('NFD')
    .replace(DIACRITIC, '')
    .normalize('NFC')
}

export function normalizeSearchText(value: SearchTextValue): string {
  const text = String(value ?? '')
  // Nothing in the table matches ASCII, so skip normalisation
  if (!NON_ASCII.test(text)) return text.toLocaleLowerCase()
  return foldDiacritics(text).toLocaleLowerCase()
}

export function matchesSearchText(value: SearchTextValue, searchText: SearchTextValue): boolean {
  const query = normalizeSearchText(searchText)
  const terms = query.split(SEPARATORS).filter(Boolean)

  // Empty or punctuation-only query, nothing to split into terms
  if (terms.length === 0) {
    return normalizeSearchText(value).includes(query.trim())
  }

  const words = normalizeSearchText(value).split(SEPARATORS).filter(Boolean)
  const claimed = new Array<boolean>(words.length).fill(false)

  // Each term takes a word of its own, longest first, so that "another tag th"
  // doesn't match "Another Tag" by finding the "th" inside "another"
  return terms
    .sort((a, b) => b.length - a.length)
    .every((term) => {
      for (let i = 0; i < words.length; i++) {
        if (!claimed[i] && words[i].includes(term)) {
          claimed[i] = true
          return true
        }
      }
      return false
    })
}

/** Port of `FilterPipe.transform(items, searchText, key?)`. */
export function filterPipe<T extends Record<string, any>>(items: T[], searchText: string, key?: string): T[] {
  if (!items) return []
  if (!searchText) return items

  return items.filter((item) => {
    const keys = key
      ? [key]
      : Object.keys(item).filter((k) => typeof item[k] === 'string' || typeof item[k] === 'number')
    return keys.some((k) => matchesSearchText(item[k], searchText))
  })
}
