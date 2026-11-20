#!/usr/bin/env node
// Flow tests Light UI (docs/FRONTEND-LIGHT.md §9 "behavior parity"): one flow per screen that saves via the UI
// (API v1) and then checks the state via API. Runs against a DB copy (tests/server.sh), never against "paperless".
//   node tests/flows/flows.mjs [--light http://127.0.0.1:4173/rs/app] [--angular http://127.0.0.1:8330] [--ui light|angular|both]
//        [--only <prefix>] [--out ../results/e2e/light-flows/<date>]
// Flows are in tests/flows/flows-*.mjs: export default [{ id, path, ready, setup?(api, ctx), run(page, ctx), check(api, ctx) }].
// setup runs before opening the page (create the precondition via API, e.g. a document in the trash).
// ctx.tag = unique short code per run and UI ("L1a2b" / "A1a2b"), so that both UIs can run one after the other on
// the same DB. Same selectors in both UIs; --ui both checks that Angular produces the same state.
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '../..')
const REPO = path.resolve(ROOT, '..')
const req = createRequire(path.join(REPO, 'tests/e2e/package.json'))
const { chromium } = req('playwright')

const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const LIGHT = opt('--light', 'http://127.0.0.1:4173/rs/app')
const ANGULAR = opt('--angular', 'http://127.0.0.1:8330')
const UIS = { light: ['light'], angular: ['angular'], both: ['angular', 'light'] }[opt('--ui', 'light')]
const ONLY = opt('--only', null)
const OUT = opt('--out', null)
const TOKEN = 'a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001' // reference/README.md: admin (local test values only)
const API = new URL(ANGULAR).origin

const FLOWS = []
for (const f of fs.readdirSync(HERE).filter((n) => /^flows-[\w-]+\.mjs$/.test(n)).sort()) {
  FLOWS.push(...(await import(pathToFileURL(path.join(HERE, f)).href)).default)
}

/** API v1 like the UI (Accept: version=10), token of the test user admin */
const api = {
  async req(method, p, body) {
    const r = await fetch(API + p, {
      method,
      headers: { Authorization: `Token ${TOKEN}`, Accept: 'application/json; version=10', 'Content-Type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    })
    const text = await r.text()
    if (!r.ok) throw new Error(`${method} ${p} → ${r.status} ${text.slice(0, 200)}`)
    return text ? JSON.parse(text) : null
  },
  get: (p) => api.req('GET', p),
  post: (p, b) => api.req('POST', p, b),
  patch: (p, b) => api.req('PATCH', p, b),
  del: (p) => api.req('DELETE', p),
  async all(ep) {
    return (await api.get(`/api/${ep}/?page_size=100000`)).results
  },
}

function assert(cond, msg) {
  if (!cond) throw new Error(`Prüfung: ${msg}`)
}

async function login(page, base) {
  const origin = new URL(base).origin
  await page.goto(`${origin}/accounts/login/`, { waitUntil: 'domcontentloaded' })
  await page.fill('#inputUsername', 'admin')
  await page.fill('#inputPassword', 'admin')
  await Promise.all([page.waitForURL((u) => !/accounts\/login/.test(u.toString())), page.click('button[type=submit]')])
}

/**
 * Light UI: ensure the classic design for admin (default is the PaperRust design, docs/DESIGN2.md §6; then
 * /dashboard redirects to /m/). Other suites (tests/design2/e2e.mjs) switch on the same DB.
 */
async function ensureClassic() {
  const s = (await api.get('/api/ui_settings/')).settings
  if (s.paperrust?.design === 'classic') return
  s.paperrust = { ...(s.paperrust || {}), design: 'classic' }
  await api.post('/api/ui_settings/', { settings: s })
}

async function main() {
  const browser = await chromium.launch(process.env.PW_CHROME ? { executablePath: process.env.PW_CHROME } : {})
  const results = []
  const run = Date.now().toString(36).slice(-4)
  for (const ui of UIS) {
    const base = ui === 'angular' ? ANGULAR : LIGHT
    for (const flow of FLOWS) {
      if (ONLY && !ONLY.split(',').some((p) => flow.id.startsWith(p))) continue // --only a,b: several prefixes
      const ctx = { tag: `${ui === 'angular' ? 'A' : 'L'}${run}`, ui, assert, api }
      const ctxB = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US', reducedMotion: 'reduce' })
      const page = await ctxB.newPage()
      const errors = []
      page.on('pageerror', (e) => errors.push(String(e)))
      const res = { flow: flow.id, ui }
      const t0 = Date.now()
      try {
        if (ui === 'light') await ensureClassic()
        if (flow.setup) await flow.setup(api, ctx)
        await login(page, base)
        await page.goto(`${base}/${flow.path}`, { waitUntil: 'networkidle' })
        if (flow.ready) await page.locator(flow.ready).first().waitFor({ timeout: 30000 })
        await flow.run(page, ctx)
        await page.waitForLoadState('networkidle')
        await flow.check(api, ctx)
        res.status = errors.length ? 'FAIL' : 'ok'
        if (errors.length) res.error = 'Seitenfehler: ' + errors.slice(0, 3).join(' | ')
      } catch (e) {
        res.status = 'FAIL'
        res.error = String(e?.message ?? e).split('\n')[0].slice(0, 400)
        if (OUT) {
          fs.mkdirSync(path.resolve(ROOT, OUT), { recursive: true })
          await page.screenshot({ path: path.resolve(ROOT, OUT, `${flow.id}.${ui}.png`) }).catch(() => {})
        }
      }
      res.ms = Date.now() - t0
      await ctxB.close()
      results.push(res)
      console.log(`${res.status.padEnd(5)} ${ui.padEnd(8)} ${flow.id.padEnd(36)} ${res.ms} ms${res.error ? ' ' + res.error : ''}`)
    }
  }
  await browser.close()
  const failed = results.filter((r) => r.status !== 'ok').length
  if (OUT) {
    fs.mkdirSync(path.resolve(ROOT, OUT), { recursive: true })
    fs.writeFileSync(path.resolve(ROOT, OUT, 'flows.json'), JSON.stringify({ date: new Date().toISOString(), light: LIGHT, angular: ANGULAR, results }, null, 2) + '\n')
  }
  console.log(`\n${results.length - failed} ok, ${failed} fehlgeschlagen`)
  process.exit(failed ? 1 : 0)
}

main().catch((e) => {
  console.error(e)
  process.exit(2)
})
