#!/usr/bin/env node
// Image comparison Angular ↔ Light UI (docs/FRONTEND-LIGHT.md §9, package F0).
//   node tests/visual/compare.mjs [--angular http://127.0.0.1:8330] [--light http://127.0.0.1:4173/rs/app]
//        [--out ../results/e2e/light-f0] [--only documents] [--langs en-US,de-DE] [--themes light,dark]
//        [--viewports desktop,mobile] [--keep-all]
// Prerequisites: local PaperRust test server (tests/server.sh, copy of the reference DB, port 8330) and
// `pnpm preview` of the Light UI (port 4173, proxy to 8330). Playwright/pixelmatch/pngjs from tests/e2e.
// Same state for both: same user (admin), same UI settings (language set via API),
// same list (table view, default sorting), animations off, caret invisible.
// Approval: ≤ 0.5 % differing pixels (pixelmatch threshold 0.1); otherwise diff image + entry in
// accepted.json needed (pattern tests/parity/accepted.json). Exit 1 if a case neither passes nor is accepted.
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '../..')
const REPO = path.resolve(ROOT, '..')
const req = createRequire(path.join(REPO, 'tests/e2e/package.json'))
const { chromium } = req('playwright')
const pixelmatch = (m => m.default ?? m)(req('pixelmatch'))
const { PNG } = req('pngjs')

const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const ANGULAR = opt('--angular', 'http://127.0.0.1:8330')
const LIGHT = opt('--light', 'http://127.0.0.1:4173/rs/app')
const DATE = new Date().toISOString().slice(0, 10)
const OUT = path.resolve(ROOT, opt('--out', `../results/e2e/light-f0/${DATE}`))
const LANGS = opt('--langs', 'en-US,de-DE').split(',')
const THEMES = opt('--themes', 'light,dark').split(',')
const VIEWPORTS = opt('--viewports', 'desktop,mobile').split(',')
const ONLY = opt('--only', null)
const KEEP_ALL = a.includes('--keep-all')
// Images of passed cases are kept only for these samples (space in the repo); --keep-all keeps all
const SAMPLES = new Set(['documents/en-US/light/desktop', 'documents/de-DE/dark/desktop', 'documents/de-DE/light/mobile'])
const LIMIT = 0.005 // 0.5 %
const TOKEN = 'a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001' // reference/README.md: admin (local test values only)
const VP = { desktop: { width: 1440, height: 900 }, mobile: { width: 390, height: 844 } }

// Screens: path relative to the respective base, display mode of the list (localStorage like Angular),
// wait condition, actions (clicks on the same selectors in both UIs) for open states.
const T = 'table tbody tr'
const D = 'pngx-document-detail .thumb-preview.fade'
const SCREENS = [
  { id: 'documents', path: 'documents', mode: 'table', ready: T },
  { id: 'documents-full', path: 'documents', mode: 'table', ready: T, fullPage: true },
  { id: 'documents-small', path: 'documents', mode: 'smallCards', ready: 'pngx-document-card-small .card-text' },
  { id: 'documents-large', path: 'documents', mode: 'largeCards', ready: 'pngx-document-card-large .card-title' },
  { id: 'doc-show-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: '#dropdownDisplayFields' }] },
  { id: 'doc-sort-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: '#dropdownBasic1' }] },
  { id: 'doc-views-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-page-header button.dropdown-toggle.flex-fill' }] },
  { id: 'doc-target-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor .input-group .dropdown-toggle' }] },
  { id: 'doc-tags-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-filterable-dropdown:nth-of-type(1) .dropdown-toggle' }] },
  { id: 'doc-corr-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-filterable-dropdown:nth-of-type(2) .dropdown-toggle' }] },
  { id: 'doc-dates-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-dates-dropdown .dropdown-toggle' }] },
  { id: 'doc-perm-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-permissions-filter-dropdown .dropdown-toggle' }] },
  { id: 'doc-cf-open', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-custom-fields-query-dropdown .dropdown-toggle' }] },
  { id: 'doc-tag-filter', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-filterable-dropdown:nth-of-type(1) .dropdown-toggle' }, { click: 'pngx-filter-editor pngx-filterable-dropdown button[role=menuitem]:has-text("Finance")' }, { wait: 1200 }] },
  { id: 'doc-selected', path: 'documents', mode: 'table', ready: T, actions: [{ click: '#docCheck192' }, { click: '#docCheck191' }] },
  // S5 document view (preview loaded = thumbnail hidden; afterwards wait for pdf.js page layout)
  { id: 'detail', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-content', path: 'documents/191/content', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-metadata', path: 'documents/191/metadata', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-notes', path: 'documents/191/notes', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-history', path: 'documents/191/history', ready: D, readyState: 'attached', actions: [{ wait: 2000 }] },
  { id: 'detail-permissions', path: 'documents/191/permissions', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-preview', path: 'documents/191/preview', ready: D, readyState: 'attached', actions: [{ wait: 1500 }] },
  { id: 'detail-actions-open', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [{ wait: 1000 }, { click: '#actionsDropdown' }] },
]
// Further cases from tests/visual/cases-*.mjs (export default [...screens], optional export const samples = [...]),
// so that agents working in parallel can add their own cases without conflict in this file.
for (const f of fs.readdirSync(HERE).filter((n) => /^cases-[\w-]+\.mjs$/.test(n)).sort()) {
  const m = await import(pathToFileURL(path.join(HERE, f)).href)
  SCREENS.push(...m.default)
  for (const k of m.samples ?? []) SAMPLES.add(k)
}
// Only cases with "only" or all (--only prefix)

const accepted = JSON.parse(fs.readFileSync(path.join(HERE, 'accepted.json'), 'utf8'))
const isAccepted = (key) => accepted.cases.find((c) => c.case === key || (c.case.endsWith('*') && key.startsWith(c.case.slice(0, -1))))

// Hide dynamic content: animations, transitions, caret
const STABILIZE = `*,*::before,*::after{transition:none!important;animation:none!important;caret-color:transparent!important}`

async function setLanguage(request, lang) {
  const h = { Authorization: `Token ${TOKEN}`, Accept: 'application/json' }
  const r = await request.get(`${ANGULAR}/api/ui_settings/`, { headers: h })
  const s = (await r.json()).settings || {}
  s.language = lang
  const w = await request.post(`${ANGULAR}/api/ui_settings/`, { headers: h, data: { settings: s } })
  if (!w.ok()) throw new Error(`ui_settings POST ${w.status()}`)
}

async function login(page, base) {
  const origin = new URL(base).origin
  await page.goto(`${origin}/accounts/login/`, { waitUntil: 'domcontentloaded' })
  await page.fill('#inputUsername', 'admin')
  await page.fill('#inputPassword', 'admin')
  await Promise.all([page.waitForURL((u) => !/accounts\/login/.test(u.toString())), page.click('button[type=submit]')])
}

async function shoot(browser, ui, screen, lang, theme, vp) {
  const base = ui === 'angular' ? ANGULAR : LIGHT
  const ctx = await browser.newContext({ viewport: VP[vp], locale: lang, colorScheme: theme, reducedMotion: 'reduce', deviceScaleFactor: 1 })
  // same list state in both UIs (same localStorage key as Angular)
  await ctx.addInitScript((mode) => {
    try {
      if (!sessionStorage.getItem('pngx-visual-init')) {
        localStorage.setItem('document-list-service:currentViewConfig', JSON.stringify({ displayMode: mode }))
        sessionStorage.setItem('pngx-visual-init', '1')
      }
    } catch {}
  }, screen.mode ?? 'table')
  const page = await ctx.newPage()
  const errors = []
  page.on('pageerror', (e) => errors.push(String(e)))
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))
  await login(page, base)
  const t0 = Date.now()
  await page.goto(`${base}/${screen.path}`, { waitUntil: 'networkidle' })
  await page.locator(screen.ready).first().waitFor({ timeout: 30000, state: screen.readyState ?? 'visible' })
  const readyMs = Date.now() - t0
  await page.addStyleTag({ content: STABILIZE })
  for (const a of screen.actions ?? []) {
    if (a.click) await page.locator(a.click).first().click()
    if (a.fill) await page.locator(a.fill[0]).first().fill(a.fill[1])
    if (a.hover) await page.locator(a.hover).first().hover()
    if (a.eval) await page.evaluate(a.eval)
    if (a.reload) {
      await page.reload({ waitUntil: 'networkidle' })
      await page.addStyleTag({ content: STABILIZE })
    }
    if (a.upload) await page.locator(a.upload[0]).first().setInputFiles(typeof a.upload[1] === 'function' ? a.upload[1]() : a.upload[1])
    // Drag without releasing (cdkDrag): press the mouse on the element and move by [dx, dy] in steps ("is being dragged" state)
    if (a.dragHold) {
      const b = await page.locator(a.dragHold[0]).first().boundingBox()
      const [x, y] = [b.x + b.width / 2, b.y + b.height / 2]
      await page.mouse.move(x, y)
      await page.mouse.down()
      for (let i = 1; i <= 20; i++) await page.mouse.move(x + (a.dragHold[1] * i) / 20, y + (a.dragHold[2] * i) / 20)
    }
    if (a.waitFor) await page.locator(a.waitFor).first().waitFor({ timeout: 15000 })
    await page.waitForTimeout(a.wait ?? 400)
  }
  await page.waitForLoadState('networkidle')
  await page.evaluate(() => document.fonts?.ready)
  await page.evaluate(() => Promise.race([new Promise((r) => setTimeout(r, 5000)), Promise.all([...document.images].filter((i) => i.loading !== 'lazy' || i.getBoundingClientRect().top < innerHeight).map((i) => (i.complete ? null : new Promise((r) => { i.onload = i.onerror = r }))))]))
  if (!screen.keepMouse) await page.mouse.move(0, VP[vp].height - 1)
  await page.waitForTimeout(300)
  const buf = await page.screenshot({ fullPage: !!screen.fullPage, animations: 'disabled', caret: 'hide' })
  await ctx.close()
  return { buf, readyMs, errors }
}

function compare(bufA, bufB, diffFile) {
  const A = PNG.sync.read(bufA)
  const B = PNG.sync.read(bufB)
  const w = Math.max(A.width, B.width)
  const h = Math.max(A.height, B.height)
  const pad = (img) => {
    if (img.width === w && img.height === h) return img
    const p = new PNG({ width: w, height: h })
    p.data.fill(0)
    PNG.bitblt(img, p, 0, 0, img.width, img.height, 0, 0)
    return p
  }
  const pa = pad(A)
  const pb = pad(B)
  const diff = new PNG({ width: w, height: h })
  const n = pixelmatch(pa.data, pb.data, diff.data, w, h, { threshold: 0.1 })
  fs.writeFileSync(diffFile, PNG.sync.write(diff))
  return { diffPixels: n, total: w * h, ratio: n / (w * h), sizeA: [A.width, A.height], sizeB: [B.width, B.height] }
}

async function main() {
  fs.mkdirSync(OUT, { recursive: true })
  const browser = await chromium.launch()
  const reqCtx = await (await browser.newContext()).request
  const results = []
  for (const lang of LANGS) {
    await setLanguage(reqCtx, lang)
    for (const theme of THEMES) {
      for (const vp of VIEWPORTS) {
        for (const screen of SCREENS) {
          if (ONLY && !screen.id.startsWith(ONLY)) continue
          if (screen.viewports && !screen.viewports.includes(vp)) continue // Case only for certain viewports (e.g. buttons visible on mobile only)
          const key = `${screen.id}/${lang}/${theme}/${vp}`
          const name = key.replaceAll('/', '_')
          const res = { case: key }
          try {
            const ng = await shoot(browser, 'angular', screen, lang, theme, vp)
            const li = await shoot(browser, 'light', screen, lang, theme, vp)
            fs.writeFileSync(path.join(OUT, `${name}.angular.png`), ng.buf)
            fs.writeFileSync(path.join(OUT, `${name}.light.png`), li.buf)
            Object.assign(res, compare(ng.buf, li.buf, path.join(OUT, `${name}.diff.png`)))
            res.readyMs = { angular: ng.readyMs, light: li.readyMs }
            res.consoleErrors = { angular: ng.errors.slice(0, 5), light: li.errors.slice(0, 5) }
            res.pass = res.ratio <= LIMIT
            const acc = !res.pass && isAccepted(key)
            res.accepted = acc ? acc.reason : undefined
            res.status = res.pass ? 'ok' : acc ? 'accepted' : 'FAIL'
            if (res.pass && !KEEP_ALL && !SAMPLES.has(key)) for (const k of ['angular', 'light', 'diff']) fs.rmSync(path.join(OUT, `${name}.${k}.png`))
            else res.images = ['angular', 'light', 'diff'].map((k) => `${name}.${k}.png`)
          } catch (e) {
            res.status = 'ERROR'
            res.error = String(e).slice(0, 500)
          }
          results.push(res)
          const pct = res.ratio != null ? (res.ratio * 100).toFixed(3) + ' %' : '-'
          console.log(`${res.status.padEnd(8)} ${key.padEnd(40)} ${pct}${res.error ? ' ' + res.error : ''}`)
        }
      }
    }
  }
  await browser.close()
  const summary = {
    date: new Date().toISOString(),
    angular: ANGULAR,
    light: LIGHT,
    limit: LIMIT,
    threshold: 0.1,
    total: results.length,
    ok: results.filter((r) => r.status === 'ok').length,
    accepted: results.filter((r) => r.status === 'accepted').length,
    failed: results.filter((r) => r.status === 'FAIL' || r.status === 'ERROR').length,
    results,
  }
  fs.writeFileSync(path.join(OUT, 'results.json'), JSON.stringify(summary, null, 2) + '\n')
  const md = [
    `# Bildvergleich Angular ↔ Light-UI (F0) – ${summary.date}`,
    '',
    `Angular \`${ANGULAR}\`, Light \`${LIGHT}\`, Grenze ${LIMIT * 100} % (pixelmatch threshold 0.1).`,
    `Ergebnis: ${summary.ok} ok, ${summary.accepted} akzeptiert, ${summary.failed} fehlgeschlagen (von ${summary.total}).`,
    '',
    '| Fall | abweichende Pixel | Anteil | Status | bereit Angular / Light |',
    '|---|---:|---:|---|---:|',
    ...results.map(
      (r) =>
        `| ${r.case} | ${r.diffPixels ?? '-'} | ${r.ratio != null ? (r.ratio * 100).toFixed(3) + ' %' : '-'} | ${r.status}${r.accepted ? ` (${r.accepted})` : ''}${r.error ? ` ${r.error}` : ''} | ${r.readyMs ? `${r.readyMs.angular} / ${r.readyMs.light} ms` : '-'} |`,
    ),
  ].join('\n')
  fs.writeFileSync(path.join(OUT, 'README.md'), md + '\n')
  console.log(`\n${summary.ok} ok, ${summary.accepted} akzeptiert, ${summary.failed} fehlgeschlagen → ${path.relative(REPO, OUT)}`)
  process.exit(summary.failed ? 1 : 0)
}

main().catch((e) => {
  console.error(e)
  process.exit(2)
})
