// Port of data/matching-model.ts, data/custom-field.ts (DATA_TYPE_LABELS) and utils/color.ts (randomColor).
import { msg, t } from './i18n'

export const MATCH_NONE = 0
export const MATCH_ANY = 1
export const MATCH_ALL = 2
export const MATCH_LITERAL = 3
export const MATCH_REGEX = 4
export const MATCH_FUZZY = 5
export const MATCH_AUTO = 6
export const DEFAULT_MATCHING_ALGORITHM = MATCH_AUTO

const ALGORITHMS = [
  { id: MATCH_AUTO, shortName: msg('filter.automatic'), name: msg('filter.auto_learn_matching_automatically') },
  { id: MATCH_ANY, shortName: msg('filter.any_word'), name: msg('filter.any_document_contains_any') },
  { id: MATCH_ALL, shortName: msg('filter.all_words'), name: msg('filter.all_document_contains_all') },
  { id: MATCH_LITERAL, shortName: msg('filter.exact_match'), name: msg('filter.exact_document_contains_string') },
  { id: MATCH_REGEX, shortName: msg('filter.regular_expression'), name: msg('filter.regular_expression_document_matches') },
  { id: MATCH_FUZZY, shortName: msg('filter.fuzzy_word'), name: msg('filter.fuzzy_document_contains_word') },
  { id: MATCH_NONE, shortName: msg('common.none'), name: msg('filter.none_disable_matching') },
]

/** MATCHING_ALGORITHMS (translated, order like Angular) */
export function matchingAlgorithms(): { id: number; shortName: string; name: string }[] {
  return ALGORITHMS.map((a) => ({ id: a.id, shortName: t(a.shortName), name: t(a.name) }))
}

export interface MatchingModel {
  id: number
  name?: string
  match?: string
  matching_algorithm?: number
  is_insensitive?: boolean
  document_count?: number
  owner?: number | null
  permissions?: any
  user_can_change?: boolean
  [k: string]: any
}

/** ManagementListComponent.getMatching */
export function getMatching(o: MatchingModel): string {
  if (o.matching_algorithm == MATCH_AUTO) return t('filter.automatic')
  if (o.matching_algorithm == MATCH_NONE) return t('common.none')
  if (o.match && o.match.length > 0) {
    const a = ALGORITHMS.find((x) => x.id == o.matching_algorithm)
    return `${a ? t(a.shortName) : ''}: ${o.match}`
  }
  return '-'
}

/** EditDialogComponent.patternRequired */
export const patternRequired = (algorithm: number | null | undefined) => algorithm !== MATCH_AUTO && algorithm !== MATCH_NONE

const DATA_TYPES = [
  { id: 'boolean', name: msg('common.boolean') },
  { id: 'date', name: msg('common.date') },
  { id: 'integer', name: msg('common.integer') },
  { id: 'float', name: msg('common.number') },
  { id: 'monetary', name: msg('common.monetary') },
  { id: 'string', name: msg('common.text') },
  { id: 'url', name: msg('common.url') },
  { id: 'documentlink', name: msg('common.document_link') },
  { id: 'select', name: msg('common.select') },
  { id: 'longtext', name: msg('common.long_text') },
]

/** DATA_TYPE_LABELS (translated) */
export function dataTypeLabels(): { id: string; name: string }[] {
  return DATA_TYPES.map((d) => ({ id: d.id, name: t(d.name) }))
}

function hue2rgb(p: number, q: number, t: number) {
  if (t < 0) t += 1
  if (t > 1) t -= 1
  if (t < 1 / 6) return p + (q - p) * 6 * t
  if (t < 1 / 2) return q
  if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6
  return p
}

export function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  let r, g, b
  if (s == 0) r = g = b = l
  else {
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s
    const p = 2 * l - q
    r = hue2rgb(p, q, h + 1 / 3)
    g = hue2rgb(p, q, h)
    b = hue2rgb(p, q, h - 1 / 3)
  }
  return [r * 255, g * 255, b * 255]
}

export function componentToHex(c: number) {
  const hex = Math.floor(c).toString(16)
  return hex.length == 1 ? '0' + hex : hex
}

export function randomColor(rnd: () => number = Math.random) {
  const rgb = hslToRgb(rnd(), 0.6, rnd() * 0.4 + 0.4)
  return `#${componentToHex(rgb[0])}${componentToHex(rgb[1])}${componentToHex(rgb[2])}`
}
