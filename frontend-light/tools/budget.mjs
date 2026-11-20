#!/usr/bin/env node
// Budget check (docs/FRONTEND-LIGHT.md §1): first call (frame + document list, one language)
// ≤ 120 KB JS + ≤ 30 KB CSS (brotli). Comparison with the Angular baseline measurement (frontend/dist).
//   node tools/budget.mjs [--lang de-DE] [--out ../results/bench/light/f0.md]   Exit 1 when exceeded
import fs from 'node:fs'
import path from 'node:path'
import zlib from 'node:zlib'
import { fileURLToPath } from 'node:url'
import { brotli, gzip } from './compress.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const REPO = path.resolve(ROOT, '..')
// --dist <folder> (relative to frontend-light/) for parallel builds, default dist
const DIST_ARG = process.argv.includes('--dist') ? process.argv[process.argv.indexOf('--dist') + 1] : 'dist'
const DIST = path.resolve(ROOT, DIST_ARG)
const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const LANG = opt('--lang', 'de-DE')
const OUT = opt('--out', null)
export const BUDGET = { js: 120 * 1024, css: 30 * 1024 }

const sizes = (buf) => ({ raw: buf.length, gzip: gzip(buf).length, br: brotli(buf).length })
const kb = (n) => `${(n / 1024).toFixed(1)} KB`

function initialFiles() {
  const html = fs.readFileSync(path.join(DIST, 'index.html'), 'utf8')
  const refs = [...html.matchAll(/(?:src|href)="\/rs\/app\/([^"]+)"/g)].map((m) => m[1])
  const js = refs.filter((r) => r.endsWith('.js'))
  const css = refs.filter((r) => r.endsWith('.css'))
  // static imports of the entry chunks recursively (Vite appends modulepreload links, to be safe)
  const seen = new Set(js)
  const queue = [...js]
  while (queue.length) {
    const f = queue.shift()
    const src = fs.readFileSync(path.join(DIST, f), 'utf8')
    for (const m of src.matchAll(/(?:^|[;\s])import\s*(?:[^'"()]*?from\s*)?["']\.\/([^"']+\.js)["']/g)) {
      const dep = path.posix.join(path.posix.dirname(f), m[1])
      if (!seen.has(dep)) {
        seen.add(dep)
        queue.push(dep)
      }
    }
  }
  // Route of the document list (lazy, but part of the first call "frame + document list"): from the Vite manifest
  const manifest = JSON.parse(fs.readFileSync(path.join(DIST, '.vite/manifest.json'), 'utf8'))
  const cssSet = new Set(css)
  const addChunk = (key) => {
    const c = manifest[key]
    if (!c) return
    if (!seen.has(c.file)) seen.add(c.file)
    for (const f of c.css ?? []) cssSet.add(f)
    for (const i of c.imports ?? []) addChunk(i)
  }
  for (const key of ROUTE_CHUNKS) {
    if (!manifest[key]) throw new Error(`Manifest ohne ${key}`)
    addChunk(key)
  }
  // Language chunk (lazy, but part of the first call for a non-English language)
  const assets = fs.readdirSync(path.join(DIST, 'assets'))
  // Core dictionary (smallest file of the language; the full file is loaded in the background)
  const lang = assets.filter((f) => f.startsWith(`${LANG}-`) && f.endsWith('.js')).sort((a, b) => fs.statSync(path.join(DIST, 'assets', a)).size - fs.statSync(path.join(DIST, 'assets', b)).size)[0]
  return { js: [...seen], css: [...cssSet], lang: lang ? `assets/${lang}` : null, manifest }
}

/** First call = frame + these route chunks */
const ROUTE_CHUNKS = ['src/components/document-list/DocumentList.svelte']
/** every further route ≤ 40 KB brotli (JS + CSS, without already loaded chunks) */
export const ROUTE_BUDGET = 40 * 1024

function angularBaseline() {
  const dir = path.join(REPO, 'frontend/dist/frontend/en-US')
  if (!fs.existsSync(path.join(dir, 'main.js'))) return null
  const row = (f) => ({ file: f, ...sizes(fs.readFileSync(path.join(dir, f))) })
  return [row('main.js'), row('polyfills.js'), row('styles.css')]
}

function main() {
  const { js, css, lang } = initialFiles()
  const rows = []
  for (const f of js) rows.push({ kind: 'js', file: f, ...sizes(fs.readFileSync(path.join(DIST, f))) })
  if (lang) rows.push({ kind: 'js', file: `${lang} (Sprache ${LANG})`, ...sizes(fs.readFileSync(path.join(DIST, lang))) })
  for (const f of css) rows.push({ kind: 'css', file: f, ...sizes(fs.readFileSync(path.join(DIST, f))) })
  const sum = (k, field) => rows.filter((r) => r.kind === k).reduce((s, r) => s + r[field], 0)
  const jsBr = sum('js', 'br')
  const cssBr = sum('css', 'br')
  const ng = angularBaseline()
  const ngTotal = ng ? ng.reduce((s, r) => s + r.br, 0) : null
  const okJs = jsBr <= BUDGET.js
  const okCss = cssBr <= BUDGET.css
  const lines = []
  lines.push(`| Datei | roh | gzip | brotli |`, `|---|---:|---:|---:|`)
  for (const r of rows) lines.push(`| ${r.kind} \`${r.file}\` | ${r.raw} B | ${r.gzip} B | ${r.br} B |`)
  lines.push(`| **JS gesamt** | ${sum('js', 'raw')} B | ${sum('js', 'gzip')} B | **${jsBr} B (${kb(jsBr)})** – Budget ${kb(BUDGET.js)} ${okJs ? '✅' : '❌'} |`)
  lines.push(`| **CSS gesamt** | ${sum('css', 'raw')} B | ${sum('css', 'gzip')} B | **${cssBr} B (${kb(cssBr)})** – Budget ${kb(BUDGET.css)} ${okCss ? '✅' : '❌'} |`)
  if (ng) {
    lines.push('', '**Angular (frontend/dist/frontend/en-US, erster Aufruf):**', '', `| Datei | roh | gzip | brotli |`, `|---|---:|---:|---:|`)
    for (const r of ng) lines.push(`| \`${r.file}\` | ${r.raw} B | ${r.gzip} B | ${r.br} B |`)
    lines.push(`| **gesamt** | | | **${ngTotal} B (${kb(ngTotal)})** |`)
    lines.push('', `Light-UI erster Aufruf: **${kb(jsBr + cssBr)}** brotli = **${(((jsBr + cssBr) / ngTotal) * 100).toFixed(1)} %** von Angular (Faktor ${(ngTotal / (jsBr + cssBr)).toFixed(1)}×).`)
  }
  const md = lines.join('\n')
  console.log(md)
  if (OUT) {
    fs.mkdirSync(path.dirname(path.resolve(ROOT, OUT)), { recursive: true })
    fs.writeFileSync(path.resolve(ROOT, OUT.replace(/\.md$/, '.json')), JSON.stringify({ lang: LANG, budget: BUDGET, rows, jsBr, cssBr, angular: ng, angularBr: ngTotal, ok: okJs && okCss }, null, 2) + '\n')
  }
  if (!okJs || !okCss) {
    console.error('[budget] Budget überschritten')
    process.exit(1)
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
