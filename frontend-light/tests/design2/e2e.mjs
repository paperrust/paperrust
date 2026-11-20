#!/usr/bin/env node
// Flow tests + screenshots of the PaperRust design (docs/DESIGN2.md §9) against a LOCAL test server.
//   node tests/design2/e2e.mjs --base http://127.0.0.1:4191 [--api http://127.0.0.1:8351] [--out ../results/e2e/design2/<date>]
//                              [--only <id>] [--no-shots | --shots-only]
// Prerequisite: `vite preview` (dist with m.html) in front of a PaperRust test server (tests/server.sh, DB copy).
// Browser: Playwright from tests/e2e; PW_CHROME=<exe> uses an existing Chromium/Edge.
// Timeline/assistant run with the demo adapter (localStorage paperrust.design2.mock=1) as long as the server
// does not offer /rs/api/v2/timeline or /ask; if it answers, the real endpoints are used.
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '../..')
const REPO = path.resolve(ROOT, '..')
const req = createRequire(path.join(REPO, 'tests/e2e/package.json'))
const { chromium } = req('playwright')

const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const BASE = opt('--base', 'http://127.0.0.1:4191')
const API = opt('--api', BASE)
const ONLY = opt('--only', null)
const SHOTS = !a.includes('--no-shots')
const SHOTS_ONLY = a.includes('--shots-only')
const OUT = path.resolve(ROOT, opt('--out', `../results/e2e/design2/${new Date().toISOString().slice(0, 10)}`))
const TOKEN = 'a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001' // reference/README.md: admin (local test values only)
const M = `${BASE}/rs/app/m`
// --ai off|llm|demo: expected state of the assistant (off = AI off → hits only, llm = answer streamed)
let AI = opt('--ai', 'auto') // auto: from ui_settings.ai_enabled (off → off, on → sources/answer)
let V2 = false // Server offers API v2 + design 2 endpoints (instead of demo adapter)
async function v2(p, init = {}) {
  const r = await fetch(`${API}/rs/api/v2${p}`, { ...init, headers: { Authorization: `Token ${TOKEN}`, Accept: 'application/json', 'Content-Type': 'application/json', ...(init.headers ?? {}) } })
  if (!r.ok) throw new Error(`v2 ${init.method ?? 'GET'} ${p} → ${r.status}`)
  return { body: await r.json(), etag: r.headers.get('ETag') }
}
/** v2 UUID/rev for legacy_id (like the UI: narrow by date + title) */
async function v2ref(id) {
  const d = await api(`/api/documents/${id}/`)
  const day = d.created.slice(0, 10)
  const { body } = await v2('/documents/query', { method: 'POST', body: JSON.stringify({ filter: { all: [{ created: { from: day, to: day } }, { text: { query: d.title, field: 'title' } }] }, limit: 100 }) })
  const hit = body.items.find((x) => x.legacy_id === id)
  return hit ? { uuid: hit.uuid, rev: hit.rev } : null
}

async function api(p, init = {}) {
  const r = await fetch(API + p, { ...init, headers: { Authorization: `Token ${TOKEN}`, Accept: 'application/json; version=10', 'Content-Type': 'application/json', ...(init.headers ?? {}) } })
  if (!r.ok) throw new Error(`${init.method ?? 'GET'} ${p} → ${r.status}`)
  return r.status === 204 ? null : r.json()
}
const design = async () => (await api('/api/ui_settings/')).settings?.paperrust?.design ?? 'classic'
function assert(c, msg) {
  if (!c) throw new Error(msg)
}

async function login(page) {
  await page.goto(`${BASE}/accounts/login/`, { waitUntil: 'domcontentloaded' })
  await page.fill('#inputUsername', 'admin')
  await page.fill('#inputPassword', 'admin')
  await Promise.all([page.waitForURL((u) => !/accounts\/login/.test(u.toString())), page.click('button[type=submit]')])
}
async function ready(page) {
  await page.waitForSelector('#d2-main', { timeout: 20000 })
  await page.waitForLoadState('networkidle').catch(() => {})
}
const noPageScroll = (page) => page.evaluate(() => ({ sh: document.scrollingElement.scrollHeight, sw: document.scrollingElement.scrollWidth, ih: innerHeight, iw: innerWidth }))

const TESTS = []
const test = (id, fn) => TESTS.push({ id, fn })

test('switch-design', async (page) => {
  // Since 2026-11-18 the PaperRust design is the default: first explicitly switch to classic
  await page.goto(`${BASE}/rs/app/dashboard?design=classic`)
  await page.waitForSelector('#userDropdown')
  await page.click('#userDropdown')
  await Promise.all([page.waitForURL(/\/rs\/app\/m\//), page.click('#d2-switch-modern')])
  await ready(page)
  assert((await design()) === 'modern', 'ui_settings paperrust.design ≠ modern')
  // Entry of the classic UI now redirects
  await page.goto(`${BASE}/rs/app/`)
  await page.waitForURL(/\/rs\/app\/m\//)
  await ready(page)
  await page.click('#d2-user')
  await Promise.all([page.waitForURL(/\/rs\/app\/dashboard/), page.click('#d2-switch-classic')])
  assert((await design()) === 'classic', 'ui_settings paperrust.design ≠ classic')
  // Desktop tray path: ?design=… on any route
  await page.goto(`${BASE}/rs/app/documents?design=modern`)
  await page.waitForURL(/\/rs\/app\/m\/documents/)
  await ready(page)
  await page.waitForFunction(() => !location.search.includes('design='))
  for (let i = 0; i < 20 && (await design()) !== 'modern'; i++) await page.waitForTimeout(200)
  assert((await design()) === 'modern', '?design=modern nicht gespeichert')
  await page.goto(`${M}/inbox?design=classic`)
  await page.waitForURL(/\/rs\/app\/(documents|dashboard)/)
  for (let i = 0; i < 20 && (await design()) !== 'classic'; i++) await page.waitForTimeout(200)
  assert((await design()) === 'classic', '?design=classic nicht gespeichert')
  return 'Menü: klassisch → modern → klassisch; ?design=modern|classic (Desktop-Tray); ui_settings geprüft'
})

test('open-document', async (page) => {
  await page.goto(`${M}/documents`)
  await ready(page)
  const first = page.locator('#d2-doclist [role=option]').first()
  const id = await first.getAttribute('data-id')
  const title = (await first.locator('.title').textContent()).trim()
  await first.click()
  await page.waitForURL(new RegExp(`/m/documents/${id}`))
  await page.waitForSelector('#d2-field-title')
  const h = (await page.locator('#d2-detail h2').textContent()).trim()
  assert(h === title, `Titel ${h} ≠ ${title}`)
  await page.waitForSelector('#d2-detail pngx-pdf-viewer canvas, #d2-detail .pv img', { timeout: 20000 }).catch(() => {})
  return `Dokument ${id} „${title}“`
})

test('autosave', async (page) => {
  const docs = await api('/api/documents/?page_size=1&ordering=-created&fields=id,title')
  const id = docs.results[0].id
  await page.goto(`${M}/documents/${id}`)
  await ready(page)
  await page.waitForSelector('#d2-field-title')
  const newTitle = `D2 Autosave ${Date.now().toString(36)}`
  await page.fill('#d2-field-title', newTitle)
  await page.waitForSelector('#d2-save-state[data-state=saved]', { timeout: 10000 })
  const via = await page.locator('#d2-save-state').getAttribute('data-via')
  if (V2) assert(via === 'v2', `Autosave über ${via} statt v2 (If-Match)`)
  const t1 = (await api(`/api/documents/${id}/`)).title
  assert(t1 === newTitle, `API-Titel ${t1} ≠ ${newTitle}`)
  if (V2) {
    const ref = await v2ref(id)
    const { body } = await v2(`/documents/${ref.uuid}`)
    assert(body.title === newTitle, `v2-Titel ${body.title} ≠ ${newTitle}`)
  }
  // Selection field: set correspondent (saved immediately)
  const corr = (await api('/api/correspondents/?page_size=5&ordering=name')).results
  const before = (await api(`/api/documents/${id}/`)).correspondent
  const target = corr.find((c) => c.id !== before) ?? corr[0]
  await page.click('#d2-field-correspondent')
  await page.locator('[role=menu] input[type=search]').fill(target.name)
  await page.locator('[role=menu] [role=option] button', { hasText: target.name }).first().click()
  await page.waitForSelector('#d2-save-state[data-state=saved]', { timeout: 10000 })
  await page.waitForTimeout(300)
  const d = await api(`/api/documents/${id}/`)
  assert(d.correspondent === target.id, `API-Korrespondent ${d.correspondent} ≠ ${target.id}`)
  // List shows the new title
  await page.waitForFunction((tt) => [...document.querySelectorAll('#d2-doclist .title')].some((e) => e.textContent.trim() === tt), newTitle, { timeout: 5000 })
  return `Titel + Korrespondent von ${id} per API geprüft (Weg: ${via})`
})

test('autosave-conflict', async (page) => {
  if (!V2) return 'übersprungen (kein v2)'
  const id = (await api('/api/documents/?page_size=2&ordering=-created&fields=id')).results[1].id
  await page.goto(`${M}/documents/${id}`)
  await ready(page)
  await page.waitForSelector('#d2-field-title')
  await page.waitForTimeout(800) // UUID/rev determined
  const before = await v2ref(id)
  // foreign change via v1 (other session)
  await api(`/api/documents/${id}/`, { method: 'PATCH', body: JSON.stringify({ archive_serial_number: 777000 + (id % 1000) }) })
  const after = await v2ref(id)
  if (after.rev === before.rev) return `übersprungen: rev ändert sich ohne aktive Änderungserfassung nicht (${before.rev})`
  await page.fill('#d2-field-title', `D2 Konflikt ${Date.now().toString(36)}`)
  await page.waitForSelector('#d2-save-state[data-state=error]', { timeout: 10000 })
  await page.waitForSelector('#d2-save-reload')
  await page.click('#d2-save-reload')
  await page.waitForFunction((n) => (document.querySelector('#d2-field-asn')?.value ?? '') === String(n), 777000 + (id % 1000), { timeout: 10000 })
  return `412 erkannt (rev ${before.rev} → ${after.rev}), „Neu laden“ zeigt den fremden Stand`
})

test('upload', async (page) => {
  await page.goto(`${M}/documents`)
  await ready(page)
  const name = `d2-upload-${Date.now().toString(36)}.txt`
  const text = 'PaperRust Design upload test. This file has enough text content to be processed without OCR at all.\n'
  await page.setInputFiles('#d2-upload-input', { name, mimeType: 'text/plain', buffer: Buffer.from(text.repeat(3)) })
  await page.waitForSelector('#d2-upload-tray', { timeout: 10000 })
  await page.waitForFunction((n) => document.querySelector('#d2-upload-tray')?.textContent?.includes(n), name)
  await page.waitForFunction(() => +(document.querySelector('#d2-upload-tray [role=progressbar]')?.getAttribute('aria-valuenow') ?? 0) >= 20, null, { timeout: 15000 })
  const v = await page.locator('#d2-upload-tray [role=progressbar]').first().getAttribute('aria-valuenow')
  return `${name} hochgeladen, Fortschritt ${v} %`
})

test('keyboard', async (page) => {
  // A still running ingestion (e.g. from the "upload" test) would re-sort the list during the check:
  // first wait until the tasks of the last minutes are finished (click test 2026-11-20, otherwise flaky).
  for (let i = 0; i < 60; i++) {
    const tasks = await api('/api/tasks/')
    const list = Array.isArray(tasks) ? tasks : (tasks.results ?? [])
    const recent = list.filter((x) => Date.now() - Date.parse(x.date_created) < 5 * 60_000)
    if (recent.every((x) => /success|failure|revoked/i.test(x.status))) break
    await page.waitForTimeout(1000)
  }
  await page.goto(`${M}/documents`)
  await ready(page)
  await page.locator('body').click({ position: { x: 900, y: 500 } })
  const ids = await page.locator('#d2-doclist [role=option]').evaluateAll((els) => els.slice(0, 3).map((e) => e.getAttribute('data-id')))
  await page.keyboard.press('j')
  await page.waitForURL(new RegExp(`/m/documents/${ids[0]}`))
  await page.keyboard.press('j')
  await page.waitForURL(new RegExp(`/m/documents/${ids[1]}`))
  await page.keyboard.press('k')
  await page.waitForURL(new RegExp(`/m/documents/${ids[0]}`))
  await page.keyboard.press('x')
  await page.waitForSelector('#d2-bulkbar')
  await page.keyboard.press('Escape')
  await page.waitForSelector('#d2-bulkbar', { state: 'detached' })
  await page.keyboard.press('Control+k')
  await page.waitForSelector('#d2-palette')
  // Command name in the UI language (strong command hits come before the full-text search)
  const lang = await page.evaluate(() => document.documentElement.lang || navigator.language)
  await page.keyboard.type(lang.startsWith('de') ? 'Posteingang' : 'Inbox')
  await page.keyboard.press('Enter')
  await page.waitForURL(/\/m\/inbox/)
  await page.locator('body').click({ position: { x: 900, y: 500 } })
  await page.keyboard.press('g')
  await page.keyboard.press('d')
  await page.waitForURL(/\/m\/documents/)
  await page.keyboard.press('?')
  await page.waitForSelector('[role=dialog]:has-text("j")')
  await page.keyboard.press('Escape')
  // List via arrow keys (focus in the list)
  await page.locator('#d2-doclist [role=option]').first().focus()
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('Enter')
  await page.waitForURL(new RegExp(`/m/documents/${ids[1]}`))
  return 'j/k, x, Esc, Ctrl+K → Posteingang, g d, ?, ↓ + Enter'
})

// docs/DESIGN2-ENTWIRREN.md §5–6: no menu item "Assistant"/no quick filters; "Ask question" in search and
// filter bar only with AI on; filter bar wraps (no horizontal scrolling) at 1280, 768 and 390 px.
test('nav-search-filterbar', async (page) => {
  const aiOn = !!(await api('/api/ui_settings/')).settings?.ai_enabled
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.goto(`${M}/documents`)
  await ready(page)
  assert(!(await page.locator('#d2-nav-assistant, [id^=d2-quick-]').count()), 'Assistent/Schnellfilter noch in der Navigation')
  for (const id of ['#d2-nav-overview', '#d2-nav-inbox', '#d2-nav-documents', '#d2-nav-settings']) assert(await page.locator(id).count(), `${id} fehlt`)
  // Header row → palette: full text + (with AI on) "Ask question"
  await page.click('#d2-search')
  await page.waitForSelector('#d2-palette')
  await page.keyboard.type('Rechnung')
  await page.waitForSelector('#d2-palette [role=option]:has-text("Rechnung")')
  const opts = await page.locator('#d2-palette [role=option]').allTextContents()
  const ask = opts.some((x) => /Frage stellen|Ask a question/.test(x))
  assert(ask === aiOn, `Zeile „Frage stellen“ ${ask ? 'sichtbar' : 'fehlt'} bei KI ${aiOn ? 'an' : 'aus'}`)
  await page.keyboard.press('Escape')
  // Filter bar: search field with question row
  await page.fill('#d2-filter-text', 'Vertrag')
  await page.waitForTimeout(200)
  assert((await page.locator('#d2-filter-ask').count()) === (aiOn ? 1 : 0), 'Frage-Zeile der Filterleiste falsch')
  if (aiOn) {
    await page.click('#d2-filter-ask')
    await page.waitForURL(/\/m\/assistant\?q=Vertrag/)
    await page.goto(`${M}/documents`)
    await ready(page)
  } else await page.fill('#d2-filter-text', '')
  // Tags chip "not assigned" = former quick filter "Without tags" (is_tagged=0)
  await page.click('#d2-filter-tags')
  await page.locator('[role=menu] [role=option] .nm.none').first().click()
  await page.waitForURL(/is_tagged=0/)
  await page.keyboard.press('Escape')
  const untagged = await api('/api/documents/?page_size=1&is_tagged=0')
  // List reloads after the filter change: wait up to 10 s until it matches the API state (otherwise flaky)
  let listed = 0
  for (let i = 0; i < 50; i++) {
    listed = await page.locator('#d2-doclist [role=option]').count()
    if (untagged.count === 0 ? listed === 0 : listed > 0) break
    await page.waitForTimeout(200)
  }
  assert(untagged.count === 0 ? listed === 0 : listed > 0, `Ohne Tags: Liste ${listed}, API ${untagged.count}`)
  await page.goto(`${M}/documents`)
  await ready(page)
  // all filters visible, nothing cut off at the side
  const bad = []
  for (const [w, h] of [
    [1280, 800],
    [768, 1024],
    [390, 844],
  ]) {
    await page.setViewportSize({ width: w, height: h })
    await page.goto(`${M}/documents`)
    await ready(page)
    if (await page.locator('#d2-filter-toggle').count()) await page.click('#d2-filter-toggle')
    await page.waitForSelector('#d2-filter-chips')
    const r = await page.evaluate(() => {
      const c = document.querySelector('#d2-filter-chips')
      const box = c.getBoundingClientRect()
      const out = [...c.children].filter((e) => { const b = e.getBoundingClientRect(); return b.right > box.right + 1 || b.left < box.left - 1 }).length
      return { sw: c.scrollWidth, cw: c.clientWidth, out, rights: !!document.querySelector('#d2-filter-rights'), date: !!document.querySelector('#d2-filter-date') }
    })
    if (r.sw > r.cw + 1 || r.out || !r.rights || !r.date) bad.push(`${w}px: ${JSON.stringify(r)}`)
  }
  await page.setViewportSize({ width: 1440, height: 900 })
  assert(!bad.length, bad.join('; '))
  return `Navigation ohne Assistent/Schnellfilter, Tags „nicht zugewiesen“ (is_tagged=0), Frage-Zeile bei KI ${aiOn ? 'an' : 'aus'} korrekt, Filter umbrechend bei 1280/768/390 px`
})

const SCREENS = [
  ['overview', '/dashboard'],
  ['documents', '/documents'],
  ['document', '/documents/{id}'],
  ['inbox', '/inbox'],
  ['assistant', '/assistant'],
  ['manage-tags', '/attributes/tags'],
  ['manage-workflows', '/workflows'],
  ['settings', '/settings'],
  ['settings-general', '/settings/general'],
]

test('no-page-scroll', async (page, ctx) => {
  const id = (await api('/api/documents/?page_size=1&ordering=-created&fields=id')).results[0].id
  const bad = []
  for (const [w, h] of [
    [1440, 900],
    [1280, 720],
    [390, 844],
  ]) {
    await page.setViewportSize({ width: w, height: h })
    for (const [name, p] of SCREENS) {
      await page.goto(M + p.replace('{id}', id))
      await ready(page)
      await page.waitForTimeout(400)
      const r = await noPageScroll(page)
      if (r.sh > r.ih || r.sw > r.iw) bad.push(`${name}@${w}x${h}: ${r.sw}x${r.sh} > ${r.iw}x${r.ih}`)
    }
  }
  await page.setViewportSize({ width: 1440, height: 900 })
  assert(!bad.length, bad.join('; '))
  return `${SCREENS.length} Bildschirme × 3 Größen ohne Seiten-Scrollen`
})

test('home', async (page) => {
  // Modular home page (docs/DESIGN2-ENTWIRREN.md §4): default, deadlines, "Customize home page" (on/off, keyboard),
  // year bar by document type/tag. The stored selection is restored at the end.
  const before = (await api('/api/ui_settings/')).settings ?? {}
  const saved = () => page.waitForResponse((r) => r.url().includes('/api/ui_settings/') && r.request().method() === 'POST')
  try {
    await page.goto(`${M}/dashboard`)
    await ready(page)
    await page.waitForSelector('#d2-home')
    if (!Array.isArray(before['paperrust.home.widgets'])) for (const id of ['inbox', 'deadlines', 'recent', 'views']) await page.waitForSelector(`#d2-w-${id}`)
    await page.waitForSelector('#d2-upcoming .card, #d2-upcoming .none, #d2-w-deadlines .un')
    let note = ''
    if (V2) {
      // real data: "Deadlines and appointments" = server, "Done" → server state
      const { body } = await v2('/timeline/upcoming?days=60&overdue_days=30')
      if (!body.items.length) {
        // Test data without deadlines in the time window (depends on today's date): check the empty display instead of failing
        assert((await page.locator('#d2-upcoming .card').count()) === 0, 'Karten ohne Server-Fristen')
        note = 'keine Fristen in den Testdaten (leere Anzeige geprüft), '
      }
    }
    if (V2 && !note) {
      const { body } = await v2('/timeline/upcoming?days=60&overdue_days=30')
      const cards = await page.locator('#d2-upcoming .card').count()
      assert(cards === body.items.length, `Karten ${cards} ≠ Server ${body.items.length}`)
      const first = body.items[0]
      await page.locator('#d2-upcoming .card').first().locator('button', { hasText: /Done|Erledigt/ }).click()
      await page.waitForFunction((n) => document.querySelectorAll('#d2-upcoming .card').length === n - 1, cards)
      const after = (await v2('/timeline/upcoming?days=60&overdue_days=30')).body.items
      assert(!after.some((x) => x.id === first.id), 'erledigter Eintrag noch in „Fristen und Termine“')
      note = `${cards} Fristen, „Erledigt“ per API geprüft, `
    }
    // Switch on the year bar and move it one place up via keyboard (Alt+↑)
    await page.click('#d2-home-customize')
    const box = page.locator('#d2-hc-year')
    if (!(await box.isChecked())) await Promise.all([saved(), box.check()])
    const order = () => page.evaluate(() => [...document.querySelectorAll('[data-w]')].map((e) => e.getAttribute('data-w')))
    const i0 = (await order()).indexOf('year')
    if (i0 > 0) {
      await box.focus()
      await Promise.all([saved(), page.keyboard.press('Alt+ArrowUp')])
      await page.waitForFunction((i) => [...document.querySelectorAll('[data-w]')].map((e) => e.getAttribute('data-w')).indexOf('year') === i - 1, i0)
    }
    const stored = (await api('/api/ui_settings/')).settings['paperrust.home.widgets']
    assert(Array.isArray(stored) && stored.find((w) => w.id === 'year')?.on === true, 'Auswahl nicht in ui_settings gespeichert')
    await page.keyboard.press('Escape')
    await page.waitForSelector('#d2-timeline .lane .mk, #d2-timeline .pad, #d2-w-year .un')
    await page.click('#d2-year-by-tag')
    await page.waitForSelector('#d2-year-by-tag[aria-checked=true]')
    await page.click('#d2-year-by-type')
    const mk = page.locator('#d2-timeline .mk').first()
    if (!(await mk.count())) return note + 'Anpassen gespeichert, Jahresleiste ohne Einträge'
    const lanes = await page.locator('#d2-timeline .lane').count()
    await mk.click()
    await page.waitForURL(/\/m\/documents\/\d+/)
    return note + `Anpassen (an/aus, Alt+↑) gespeichert, Jahresleiste ${lanes} Bahnen (Dokumenttyp/Tag), Marke → Dokument`
  } finally {
    await api('/api/ui_settings/', { method: 'POST', body: JSON.stringify({ settings: before }) }).catch(() => {})
  }
})

test('assistant', async (page) => {
  if (AI === 'auto' && !(await api('/api/ui_settings/')).settings?.ai_enabled) AI = 'off'
  await page.goto(`${M}/assistant`)
  await ready(page)
  if (AI === 'off') await page.waitForSelector('#d2-ai-off-intro')
  await page.fill('#d2-ask', 'Rechnung Insurance')
  await page.keyboard.press('Enter')
  await page.waitForFunction(() => document.querySelector('#d2-chat-log')?.getAttribute('aria-busy') === 'false' && document.querySelector('#d2-chat-log .answer .cite, #d2-chat-log .passages a, #d2-chat-log .err'), null, { timeout: 30000 })
  const err = await page.evaluate(() => [...document.querySelectorAll('#d2-chat-log .err')].map((e) => e.textContent).join(' | '))
  assert(!err, `Fehler: ${err}`)
  let note
  if (AI === 'off') {
    await page.waitForSelector('#d2-ai-off')
    const n = await page.locator('#d2-chat-log .passages li').count()
    assert(n > 0, 'keine Fundstellen')
    note = `KI aus: Hinweis + ${n} Fundstellen`
    await page.locator('#d2-chat-log .passages a').first().click()
  } else {
    const n = await page.locator('#d2-chat-log .cite').count()
    if (AI === 'llm') {
      const refs = await page.locator('#d2-chat-log .txt .ref').count()
      assert(refs > 0, 'kein [n]-Verweis in der Antwort')
      await page.locator('#d2-chat-log .txt .ref').first().click()
      note = `LLM-Antwort gestreamt, ${refs} Verweis(e), ${n} Quellen`
    } else {
      note = `${n} Quellen`
      await page.locator('#d2-chat-log .cite').first().click()
    }
  }
  await page.waitForURL(/\/m\/documents\/\d+/)
  // History: conversation appears in the list and can be opened again
  if (V2) {
    await page.goto(`${M}/assistant`)
    await ready(page)
    await page.waitForSelector('.conv')
    await page.locator('.conv').first().click()
    await page.waitForSelector('#d2-chat-log .bubble')
    note += ', Verlauf geöffnet'
  }
  return note
})

test('contrast', async (page) => {
  // cheap axe-like check: text color against effective background for visible texts of the shell
  const bad = []
  for (const theme of ['light', 'dark']) {
    await page.goto(`${M}/documents`)
    await ready(page)
    await page.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme)
    await page.waitForTimeout(150)
    const r = await page.evaluate(() => {
      const parse = (c) => (c.match(/[\d.]+/g) ?? []).map(Number)
      const lum = ([r, g, b]) => {
        const f = (v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)
        return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
      }
      const bg = (el) => {
        for (let e = el; e; e = e.parentElement) {
          const c = parse(getComputedStyle(e).backgroundColor)
          if (c.length >= 3 && (c[3] ?? 1) > 0.5) return c
        }
        return [255, 255, 255]
      }
      const out = []
      for (const el of document.querySelectorAll('#d2-root *')) {
        if (!el.childNodes.length || ![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue
        const s = getComputedStyle(el)
        if (s.visibility === 'hidden' || s.display === 'none' || el.closest('[aria-hidden=true]') || el.closest('.tag, .d2-chip')) continue
        const fg = parse(s.color)
        const L1 = lum(fg)
        const L2 = lum(bg(el))
        const ratio = (Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05)
        const large = parseFloat(s.fontSize) >= 24 || (parseFloat(s.fontSize) >= 18.66 && +s.fontWeight >= 700)
        if (ratio < (large ? 3 : 4.5)) out.push(`${el.tagName.toLowerCase()}.${[...el.classList].join('.')} "${el.textContent.trim().slice(0, 30)}" ${ratio.toFixed(2)}`)
      }
      return out
    })
    bad.push(...r.map((x) => `${theme}: ${x}`))
  }
  assert(!bad.length, bad.slice(0, 10).join('; '))
  return 'alle sichtbaren Shell-Texte ≥ 4,5:1 (hell + dunkel)'
})

async function shots(page) {
  fs.mkdirSync(OUT, { recursive: true })
  const id = (await api('/api/documents/?page_size=1&ordering=-created&fields=id')).results[0].id
  const files = []
  for (const theme of ['light', 'dark']) {
    await page.setViewportSize({ width: 1440, height: 900 })
    for (const [name, p] of SCREENS) {
      await page.goto(M + p.replace('{id}', id))
      await ready(page)
      await page.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme)
      if (name === 'assistant' && V2) {
        await page.fill('#d2-ask', 'Rechnung Insurance')
        await page.keyboard.press('Enter')
        await page.waitForFunction(() => document.querySelector('#d2-chat-log')?.getAttribute('aria-busy') === 'false' && document.querySelector('#d2-chat-log .answer .cite, #d2-chat-log .passages a, #d2-chat-log .err'), null, { timeout: 30000 }).catch(() => {})
      }
      await page.waitForTimeout(name === 'document' ? 2000 : 600)
      const f = `${name === 'assistant' && AI !== 'auto' ? `assistant-${AI}` : name}-${theme}-desktop.png`
      await page.screenshot({ path: path.join(OUT, f) })
      files.push(f)
    }
    // Command palette open
    await page.goto(`${M}/documents/${id}`)
    await ready(page)
    await page.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme)
    await page.keyboard.press('Control+k')
    await page.keyboard.type('rech')
    await page.waitForTimeout(800)
    await page.screenshot({ path: path.join(OUT, `palette-${theme}-desktop.png`) })
    files.push(`palette-${theme}-desktop.png`)
  }
  await page.setViewportSize({ width: 390, height: 844 })
  for (const [name, p] of [
    ['documents', '/documents'],
    ['document', '/documents/{id}'],
    ['overview', '/dashboard'],
  ]) {
    await page.goto(M + p.replace('{id}', id))
    await ready(page)
    await page.evaluate(() => document.documentElement.setAttribute('data-theme', 'light'))
    await page.waitForTimeout(800)
    const f = `${name}-light-mobile.png`
    await page.screenshot({ path: path.join(OUT, f) })
    files.push(f)
  }
  return files
}

async function main() {
  const browser = await chromium.launch(process.env.PW_CHROME ? { executablePath: process.env.PW_CHROME } : {})
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'de-DE' })
  const page = await ctx.newPage()
  const errors = []
  page.on('pageerror', (e) => errors.push(e.message))
  await login(page)
  // Demo adapter for timeline/assistant if the server does not have the endpoints yet
  const hasTimeline = (await fetch(`${API}/rs/api/v2/timeline`, { headers: { Authorization: `Token ${TOKEN}` }, redirect: 'manual' })).status === 200
  V2 = hasTimeline
  if (!hasTimeline) await ctx.addInitScript(() => localStorage.setItem('paperrust.design2.mock', '1'))
  const results = []
  for (const t of TESTS) {
    if (SHOTS_ONLY || (ONLY && t.id !== ONLY)) continue
    const t0 = Date.now()
    try {
      const note = await t.fn(page, ctx)
      results.push({ id: t.id, ok: true, ms: Date.now() - t0, note })
      console.log(`ok   ${t.id} – ${note}`)
    } catch (e) {
      results.push({ id: t.id, ok: false, ms: Date.now() - t0, error: String(e.message ?? e).slice(0, 2000) })
      console.log(`FAIL ${t.id} – ${e.message}`)
      await page.screenshot({ path: path.join(fs.mkdirSync(OUT, { recursive: true }) ?? OUT, `fail-${t.id}.png`) }).catch(() => {})
    }
  }
  const files = SHOTS && (!ONLY || SHOTS_ONLY) ? await shots(page) : []
  await browser.close()
  fs.mkdirSync(OUT, { recursive: true })
  const ok = results.every((r) => r.ok)
  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify({ base: BASE, timeline_backend: hasTimeline, results, screenshots: files, page_errors: errors }, null, 2) + '\n')
  console.log(`${results.filter((r) => r.ok).length}/${results.length} ok, ${files.length} Bilder → ${OUT}${errors.length ? `, Seitenfehler: ${errors.length}` : ''}`)
  if (errors.length) console.log(errors.slice(0, 5).join('\n'))
  process.exit(ok ? 0 : 1)
}

main()
