#!/usr/bin/env node
// Budget first call: split the language file into "core" (texts of frame + document list + shared
// components/logic) and the rest. The Light UI first loads i18n/core/<locale>.json (small) and the full
// file in the background; every lazily loaded route waits for the full file (src/lib/i18n.ts fullLocale()).
//   node tools/i18n-split.mjs      → i18n/core/*.json (generated, gitignored)
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { findKeys, readCatalog } from './i18n-check.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const SRC = path.join(ROOT, 'src')

/** Sources that belong to the first call (statically reachable from main.ts or the document list). */
export const CORE = [
  'App.svelte',
  'main.ts',
  'lib',
  'stores',
  'api',
  'components/app-frame',
  'components/document-list',
  'components/file-drop',
  ...['bootstrap-icon', 'tag', 'page-header', 'pagination', 'filterable-dropdown', 'dates-dropdown', 'permissions-filter-dropdown', 'custom-fields-query-dropdown', 'clearable-badge', 'ng-select', 'custom-field-display', 'preview-popup', 'toast', 'toasts', 'upload-status-tray', 'datepicker'].map((c) => `components/common/${c}`),
]

/** Core of the second UI (docs/DESIGN2.md): shell + workspace + shared logic → i18n/core-m/ */
export const CORE_MODERN = ['design2', 'lib', 'stores', 'api']
/** lazily loaded parts of the second UI (own areas, wait for the full file: App.svelte for
 *  manage/settings – the user/role management design2/admin is only loaded there –, Overview.svelte for the
 *  home page components design2/home) */
export const MODERN_LAZY = ['design2/manage', 'design2/settings', 'design2/admin', 'design2/home', 'design2/ai', 'design2/automation']

function walk(p, out = []) {
  if (!fs.existsSync(p)) return out
  if (fs.statSync(p).isFile()) {
    if (/\.(svelte|ts)$/.test(p) && !/\.(test|generated)\.ts$/.test(p)) out.push(p)
    return out
  }
  for (const e of fs.readdirSync(p)) walk(path.join(p, e), out)
  return out
}

export function coreKeys(core = CORE, skip = []) {
  const keys = new Set()
  const known = new Set(Object.keys(readCatalog('en-US')))
  const skipped = skip.map((s) => path.join(SRC, s))
  for (const c of core)
    for (const f of walk(path.join(SRC, c))) {
      if (skipped.some((s) => f.startsWith(s))) continue
      for (const k of findKeys(fs.readFileSync(f, 'utf8'), known)) keys.add(k)
    }
  return keys
}

function main() {
  split(coreKeys(), 'core')
  split(coreKeys(CORE_MODERN, MODERN_LAZY), 'core-m')
}

function split(keys, name) {
  const dir = path.join(ROOT, 'i18n')
  const out = path.join(dir, name)
  fs.rmSync(out, { recursive: true, force: true })
  fs.mkdirSync(out, { recursive: true })
  let n = 0
  for (const f of fs.readdirSync(dir)) {
    if (!/^[a-z]{2,3}-[A-Z]{2}\.json$/.test(f)) continue
    const full = JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8'))
    const core = Object.fromEntries(Object.entries(full).filter(([k]) => keys.has(k)))
    fs.writeFileSync(path.join(out, f), JSON.stringify(core))
    n++
  }
  console.log(`[i18n-split] ${keys.size} Kern-Texte, ${n} Sprachen → i18n/${name}/`)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
