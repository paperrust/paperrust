#!/usr/bin/env node
// i18n check (docs/FRONTEND-LIGHT.md §4), runs in the build:
//  - every key used in src/ (t('…'), tRich('…'), tParts('…'), msg('…') and key literals in tables)
//    is in i18n/en-US.json,
//  - every language file in i18n/ is one of the supported languages and has exactly the keys of en-US.json,
//    with the same placeholders/ICU variables/tags.
// On an error: abort (exit 1).
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const I18N = path.join(ROOT, 'i18n')

/** Languages with their own catalog (product decision, docs/FRONTEND-LIGHT.md §4). */
export const SUPPORTED_LOCALES = ['en-US', 'de-DE', 'fr-FR', 'it-IT', 'es-ES', 'nl-NL', 'pt-BR', 'pl-PL', 'cs-CZ', 'sv-SE']

/** Shape of a key: area.name (lowercase letters, digits, _). */
export const KEY_RE = /^[a-z0-9_]+(\.[a-z0-9_]+)+$/

/**
 * Used keys of a source file: first string literal of t/tRich/tParts/msg; with `known` additionally
 * every other string literal that is exactly one known key (tables like RELATIVE_DATE_LABELS).
 */
export function findKeys(source, known) {
  const out = []
  for (const m of source.matchAll(/\b(?:t|tRich|tParts|msg)\(\s*(['"])((?:\\.|(?!\1).)*)\1/g)) {
    out.push(m[2].replace(/\\(['"\\])/g, '$1'))
  }
  if (known) for (const m of source.matchAll(/(['"])([a-z0-9_]+(?:\.[a-z0-9_]+)+)\1/g)) if (known.has(m[2])) out.push(m[2])
  return out
}

export function readCatalog(locale) {
  return JSON.parse(fs.readFileSync(path.join(I18N, `${locale}.json`), 'utf8'))
}

/** Placeholders, ICU variables, HTML tags and %s of a text – must be the same in every language. */
export function signature(text) {
  const ph = [...new Set([...text.matchAll(/\{\$([A-Z0-9_]+)\}/g)].map((m) => m[1]))].sort().join(',')
  const icu = [...text.matchAll(/\{(\w+), (plural|select),/g)].map((m) => `${m[1]}:${m[2]}`).join(',')
  const tags = [...text.matchAll(/<\/?([a-z]+)/g)].map((m) => m[1]).sort().join(',')
  const pct = (text.match(/%s/g) || []).length
  return `${ph}|${icu}|${tags}|${pct}`
}

export function walk(dir, files = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) walk(p, files)
    else if (/\.(svelte|ts)$/.test(e.name) && !/\.(test|generated)\.ts$/.test(e.name)) files.push(p)
  }
  return files
}

/** Returns a list of errors (empty = all good). */
export function check() {
  const errors = []
  const en = readCatalog('en-US')
  const known = new Set(Object.keys(en))
  for (const k of known) if (!KEY_RE.test(k)) errors.push(`en-US: Schlüssel hat nicht die Form bereich.name: ${JSON.stringify(k)}`)
  // Language files: only supported, complete, same placeholders
  const files = fs.readdirSync(I18N).filter((f) => f.endsWith('.json'))
  for (const f of files) if (!SUPPORTED_LOCALES.includes(f.replace(/\.json$/, ''))) errors.push(`i18n/${f}: keine unterstützte Sprache`)
  for (const loc of SUPPORTED_LOCALES) {
    if (!files.includes(`${loc}.json`)) { errors.push(`i18n/${loc}.json fehlt`); continue }
    const tr = readCatalog(loc)
    for (const k of known) {
      if (typeof tr[k] !== 'string' || !tr[k].trim()) errors.push(`${loc}: ${k} fehlt`)
      else if (signature(tr[k]) !== signature(en[k])) errors.push(`${loc}: ${k} hat andere Platzhalter als en-US`)
    }
    for (const k of Object.keys(tr)) if (!known.has(k)) errors.push(`${loc}: unbekannter Schlüssel ${k}`)
  }
  // used keys
  for (const f of walk(path.join(ROOT, 'src'))) {
    for (const k of findKeys(fs.readFileSync(f, 'utf8'))) {
      if (!known.has(k)) errors.push(`${path.relative(ROOT, f)}: ${JSON.stringify(k)} steht nicht in i18n/en-US.json`)
    }
  }
  return errors
}

function main() {
  const errors = check()
  if (errors.length) {
    console.error(`[i18n-check] ${errors.length} Fehler:\n  ${errors.slice(0, 80).join('\n  ')}`)
    process.exit(1)
  }
  console.log(`[i18n-check] ${SUPPORTED_LOCALES.length} Sprachen vollständig, alle benutzten Schlüssel bekannt`)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
