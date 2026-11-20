#!/usr/bin/env node
// Budget PaperRust design (docs/DESIGN2.md §8): first call = m.html entry (frame + workspace, static)
// + start area overview (lazy, but loaded immediately) + core dictionary of the language ≤ 150 KB brotli (JS + CSS).
// Every further area (assistant, administration, settings, palette …) is reported individually.
//   node tools/budget-design2.mjs [--dist dist] [--lang de-DE] [--out ../results/bench/design2/budget.md]
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { brotli } from './compress.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const DIST = path.resolve(ROOT, opt('--dist', 'dist'))
const LANG = opt('--lang', 'de-DE')
const OUT = opt('--out', null)
export const BUDGET = 150 * 1024
const START = 'src/design2/timeline/Overview.svelte'
const LAZY = {
  Assistent: 'src/design2/assistant/Assistant.svelte',
  Verwaltung: 'src/design2/manage/ManageHost.svelte',
  Einstellungen: 'src/design2/settings/SettingsHost.svelte',
  Befehlspalette: 'src/design2/components/CommandPalette.svelte',
  'Upload-Liste': 'src/design2/components/UploadTray.svelte',
  Automatik: 'src/design2/automation/AutomationHost.svelte',
}
const br = (f) => brotli(fs.readFileSync(path.join(DIST, f))).length
const kb = (n) => `${(n / 1024).toFixed(1)} KB`

const manifest = JSON.parse(fs.readFileSync(path.join(DIST, '.vite/manifest.json'), 'utf8'))
/** Chunk + static imports (recursive) → files */
function closure(key, seen = new Set(), css = new Set()) {
  const c = manifest[key]
  if (!c || seen.has(c.file)) return { js: seen, css }
  seen.add(c.file)
  for (const f of c.css ?? []) css.add(f)
  for (const i of c.imports ?? []) closure(i, seen, css)
  return { js: seen, css }
}
const entry = closure('m.html')
const first = { js: new Set(entry.js), css: new Set(entry.css) }
closure(START, first.js, first.css)
const assets = fs.readdirSync(path.join(DIST, 'assets'))
// Core dictionary of the modern UI (i18n/core-m → smallest chunk with this language prefix that is not a full dictionary)
const langFiles = assets.filter((f) => f.startsWith(`${LANG}-`) && f.endsWith('.js')).sort((x, y) => fs.statSync(path.join(DIST, 'assets', x)).size - fs.statSync(path.join(DIST, 'assets', y)).size)
const coreM = langFiles.find((f) => manifest['i18n/core-m/' + LANG + '.json']?.file === 'assets/' + f) ?? null
const lang = coreM ? `assets/${coreM}` : langFiles[0] ? `assets/${langFiles[0]}` : null

const rows = []
for (const f of first.js) rows.push(['js', f, br(f)])
for (const f of first.css) rows.push(['css', f, br(f)])
if (lang) rows.push(['js', `${lang} (Sprache ${LANG})`, br(lang)])
const total = rows.reduce((s, r) => s + r[2], 0)
const ok = total <= BUDGET
const lines = ['| Art | Datei | brotli |', '|---|---|---:|', ...rows.map((r) => `| ${r[0]} | \`${r[1]}\` | ${r[2]} B |`), `| **gesamt** | erster Aufruf (Rahmen + Arbeitsplatz + Übersicht + Sprache) | **${kb(total)}** – Budget ${kb(BUDGET)} ${ok ? '✅' : '❌'} |`, '', '| Bereich (nachgeladen) | zusätzlich brotli |', '|---|---:|']
for (const [name, key] of Object.entries(LAZY)) {
  const c = closure(key)
  let sum = 0
  for (const f of c.js) if (!first.js.has(f)) sum += br(f)
  for (const f of c.css) if (!first.css.has(f)) sum += br(f)
  lines.push(`| ${name} | ${kb(sum)} |`)
}
const md = lines.join('\n')
console.log(md)
if (OUT) {
  const out = path.resolve(ROOT, OUT)
  fs.mkdirSync(path.dirname(out), { recursive: true })
  fs.writeFileSync(out, `# Budget PaperRust-Design (${new Date().toISOString().slice(0, 10)})\n\n${md}\n`)
}
if (!ok) {
  console.error('[budget-design2] Budget überschritten')
  process.exit(1)
}
