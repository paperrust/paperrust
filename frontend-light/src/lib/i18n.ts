// Translations of the Light UI (docs/FRONTEND-LIGHT.md §4): own PaperRust catalog.
// Key = dotted key like `list.sort_title`, texts in i18n/<locale>.json ({$NAME} = placeholder,
// ICU plural/select). Own catalogs for 10 languages (i18n/*.json); every other language gets English.
// Only the active language is loaded (own chunk per language).

export type Params = Record<string, string | number | null | undefined>
export type Part = string | { ph: string }

type Node =
  | { t: 'text'; v: string }
  | { t: 'ph'; name: string }
  | { t: 'icu'; name: string; kind: 'plural' | 'select'; cases: Record<string, Node[]> }

const cache = new Map<string, Node[]>()

/** Splits a message into text, {$PH} and ICU blocks. */
export function parse(msg: string): Node[] {
  let hit = cache.get(msg)
  if (hit) return hit
  let i = 0
  const nodes = parseSeq(false)
  cache.set(msg, nodes)
  return nodes

  function parseSeq(inIcu: boolean): Node[] {
    const out: Node[] = []
    let text = ''
    let literal = 0 // open, non-ICU braces in the text
    const flush = () => {
      if (text) out.push({ t: 'text', v: text })
      text = ''
    }
    while (i < msg.length) {
      const c = msg[i]
      if (c === '}' && literal === 0 && inIcu) break
      if (c === '}' && literal > 0) literal--
      if (c === '{') {
        if (msg[i + 1] === '$') {
          const end = msg.indexOf('}', i)
          flush()
          out.push({ t: 'ph', name: msg.slice(i + 2, end) })
          i = end + 1
          continue
        }
        const m = /^\{\s*([A-Za-z0-9_]+)\s*,\s*(plural|select)\s*,/.exec(msg.slice(i))
        if (m) {
          flush()
          i += m[0].length
          out.push(parseIcu(m[1], m[2] as 'plural' | 'select'))
          continue
        }
        literal++
      }
      text += c
      i++
    }
    flush()
    return out
  }

  function parseIcu(name: string, kind: 'plural' | 'select'): Node {
    const cases: Record<string, Node[]> = {}
    for (;;) {
      while (i < msg.length && /\s/.test(msg[i])) i++
      if (msg[i] === '}') {
        i++
        break
      }
      const sel = /^[^\s{]+/.exec(msg.slice(i))
      if (!sel) throw new Error(`ICU: Fall erwartet in ${JSON.stringify(msg)}`)
      i += sel[0].length
      while (/\s/.test(msg[i])) i++
      if (msg[i] !== '{') throw new Error(`ICU: { erwartet in ${JSON.stringify(msg)}`)
      i++
      cases[sel[0]] = parseSeq(true)
      if (msg[i] !== '}') throw new Error(`ICU: } erwartet in ${JSON.stringify(msg)}`)
      i++
    }
    return { t: 'icu', name, kind, cases }
  }
}

const plurals = new Map<string, Intl.PluralRules>()
function pluralCategory(locale: string, n: number): string {
  let pr = plurals.get(locale)
  if (!pr) plurals.set(locale, (pr = new Intl.PluralRules(locale)))
  return pr.select(n)
}

function render(nodes: Node[], params: Params, locale: string, out: Part[]) {
  for (const n of nodes) {
    if (n.t === 'text') out.push(n.v)
    else if (n.t === 'ph') {
      const v = params[n.name]
      if (v === undefined && /^(START|CLOSE)_/.test(n.name)) out.push({ ph: n.name })
      else out.push(v == null ? '' : String(v))
    } else {
      const raw = params[n.name]
      let branch: Node[] | undefined
      if (n.kind === 'plural') {
        const num = Number(raw ?? 0)
        branch = n.cases[`=${num}`] ?? n.cases[pluralCategory(locale, num)] ?? n.cases.other
      } else branch = n.cases[String(raw)] ?? n.cases.other
      if (branch) render(branch, params, locale, out)
    }
  }
}

/** Formats a message (without dictionary) – for tests and runtime. */
export function format(msg: string, params: Params = {}, locale = 'en-US'): string {
  const parts: Part[] = []
  render(parse(msg), params, locale, parts)
  return parts.map((p) => (typeof p === 'string' ? p : '')).join('')
}

/** Like format, but tag placeholders (START_LINK …) stay as {ph} (rich text). */
export function formatParts(msg: string, params: Params = {}, locale = 'en-US'): Part[] {
  const parts: Part[] = []
  render(parse(msg), params, locale, parts)
  const merged: Part[] = []
  for (const p of parts) {
    if (typeof p === 'string' && typeof merged[merged.length - 1] === 'string') merged[merged.length - 1] += p
    else merged.push(p)
  }
  return merged
}

// ---- active language -----------------------------------------------------------------------

export type Dict = Record<string, string>
let dict: Dict = {}
/** desired language (date/number format) */
let activeLocale = 'en-US'
/** language of the loaded catalog (plural rules of the texts) */
let catalog = 'en-US'

/** Language files as lazy chunks (one per language). */
const loaders = import.meta.glob<Dict>('../../i18n/[a-z][a-z]-[A-Z][A-Z].json', { import: 'default' })

/** Languages with their own catalog (en-US first). */
export const AVAILABLE_LOCALES: string[] = Object.keys(loaders)
  .map((p) => p.replace(/^.*\/([^/]+)\.json$/, '$1'))
  .sort((a, b) => (a === 'en-US' ? -1 : b === 'en-US' ? 1 : a.localeCompare(b)))

/** Catalog for a language: exact, otherwise same base language (de-CH → de-DE), otherwise en-US. */
export function catalogFor(tag: string | null | undefined, available = AVAILABLE_LOCALES): string {
  if (!tag) return 'en-US'
  const norm = tag.replace('_', '-').toLowerCase()
  const exact = available.find((l) => l.toLowerCase() === norm)
  if (exact) return exact
  const lang = norm.split('-')[0]
  return available.find((l) => l.toLowerCase() === lang) ?? available.find((l) => l.toLowerCase().startsWith(lang + '-')) ?? 'en-US'
}

function canonical(want: string | null | undefined): string | undefined {
  if (!want) return undefined
  try {
    return Intl.getCanonicalLocales(want.replace('_', '-'))[0]
  } catch {
    return undefined
  }
}

/**
 * Desired language (for texts and date/number format): UI setting `language`, otherwise the first
 * browser language with its own catalog, otherwise the first browser language, otherwise en-US. The texts come
 * from catalogFor(…) – without own catalog English, the format still follows the desired language.
 */
export function resolveLocale(setting: string | null | undefined, browser: readonly string[], available = AVAILABLE_LOCALES): string {
  const own = canonical(setting)
  if (own) return own
  const tags = browser.map(canonical).filter((x): x is string => !!x)
  const lang = (x: string) => x.split('-')[0].toLowerCase()
  return tags.find((x) => available.some((a) => lang(a) === lang(x))) ?? tags[0] ?? 'en-US'
}

/** Core texts (frame + document list) per language: tools/i18n-split.mjs, own small chunk */
const coreLoaders = import.meta.glob<Dict>('../../i18n/core/[a-z][a-z]*-*.json', { import: 'default' })
let full: Promise<void> = Promise.resolve()

/**
 * Loads the language. If a core dictionary exists, only that is loaded first (budget first call)
 * and the full file is fetched in the background; lazily loaded screens wait for it with fullLocale().
 */
export async function loadLocale(
  locale: string,
  load: (path: string) => Promise<Dict> = (p) => (loaders[p] ?? coreLoaders[p])(),
  /** own core dictionary (second UI, src/design2: i18n/core-m/), otherwise i18n/core/ */
  core?: (locale: string) => Promise<Dict> | null,
): Promise<void> {
  const cat = catalogFor(locale)
  const path = `../../i18n/${cat}.json`
  const corePath = `../../i18n/core/${cat}.json`
  activeLocale = locale
  catalog = cat
  if (!(path in loaders)) {
    dict = {}
    full = Promise.resolve()
    return
  }
  const own = core?.(cat)
  if (own || (!core && corePath in coreLoaders)) {
    dict = await (own ?? load(corePath))
    full = load(path).then((d) => {
      if (catalog === cat) dict = { ...d }
    })
  } else {
    dict = await load(path)
    full = Promise.resolve()
  }
}

/** Promise: full language file loaded (wait for it before rendering lazily loaded routes). */
export const fullLocale = () => full

/** For tests: set the dictionary directly. */
export function setDictionary(locale: string, d: Dict) {
  dict = d
  activeLocale = locale
  catalog = catalogFor(locale)
}

export const locale = () => activeLocale

/** Language of the loaded catalog (de-DE, …; without own catalog en-US). */
export const catalogLocale = () => catalog

/** Translates the key `key`; unknown keys (e.g. names from the API) stay as they are. */
export function t(key: string, params?: Params): string {
  return format(dict[key] ?? key, params, catalog)
}

export function tParts(key: string, params?: Params): Part[] {
  return formatParts(dict[key] ?? key, params, catalog)
}

export interface Segment {
  text: string
  tag?: string // START_xxx placeholder that wraps this text (e.g. link)
}

/** Rich text: text between START_x/CLOSE_x is assigned to tag x. */
export function segments(parts: Part[]): Segment[] {
  const out: Segment[] = []
  let tag: string | undefined
  for (const p of parts) {
    if (typeof p === 'string') out.push(tag ? { text: p, tag } : { text: p })
    else if (p.ph.startsWith('START_')) tag = p.ph
    else if (p.ph.startsWith('CLOSE_')) tag = undefined
  }
  return out
}

export function tRich(key: string, params?: Params): Segment[] {
  return segments(tParts(key, params))
}

/** Marks a key for the i18n check without translating it (for tables/constants). */
export const msg = (key: string) => key
