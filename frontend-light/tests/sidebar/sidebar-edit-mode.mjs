#!/usr/bin/env node
// Regression test sidebar "on/off switches" (bug report 2026-11-10): dashboard, documentation and the other
// hideable entries (HideableSidebarItemID) may show switches ONLY in edit mode. Upstream v3.4.2:
// SettingsService.organizingSidebarItems() is true exactly when Settings → tab "General" is open
// (SettingsComponent sets sidebarHiddenItemsEditing, ngOnDestroy/another tab sets null). In the mode
// hidden entries stay visible (opacity-50), switches and checkboxes "Sidebar items to show" are coupled.
//   node tests/sidebar/sidebar-edit-mode.mjs [--angular http://127.0.0.1:8330] [--light http://127.0.0.1:4173/rs/app]
//        [--upstream http://127.0.0.1:8000] [--ui angular,light[,upstream]] [--out ../results/e2e/sidebar-fix]
// Read-only except for unsaved form changes (discarded with "Cancel") – DB copy (tests/server.sh),
// --upstream (reference Django with unchanged Angular) read-only.
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '../..')
const REPO = path.resolve(ROOT, '..')
const { chromium } = createRequire(path.join(REPO, 'tests/e2e/package.json'))('playwright')

const a = process.argv.slice(2)
const opt = (n, d) => (a.includes(n) ? a[a.indexOf(n) + 1] : d)
const BASES = {
  angular: opt('--angular', 'http://127.0.0.1:8330'),
  light: opt('--light', 'http://127.0.0.1:4173/rs/app'),
  upstream: opt('--upstream', 'http://127.0.0.1:8000'),
}
const UIS = opt('--ui', 'angular,light').split(',')
const OUT = opt('--out', null)
const CHANNEL = opt('--channel', 'msedge')

/** Order and titles like upstream app-frame.component.html (i18n-title of the pngx-input-switch) */
const EXPECTED = ['Dashboard', 'Saved Views', 'Share Links', 'Workflows', 'Mail', 'Documentation']

/** State of the sidebar: visible switches (aria-label), classes of the hideable entries */
async function sidebarState(page) {
  return page.evaluate(() => {
    const nav = document.querySelector('#sidebarMenu')
    const vis = (e) => !!e && e.offsetParent !== null
    const switches = [...nav.querySelectorAll('.form-switch input[type=checkbox]')].filter(vis)
    const item = (label) =>
      [...nav.querySelectorAll('li.position-relative')].find((li) => li.querySelector('a')?.textContent.trim() === label)
    const info = (label) => {
      const li = item(label)
      const link = li?.querySelector('a')
      return li && { visible: vis(li), faded: link.classList.contains('opacity-50'), padded: link.classList.contains('pe-5'), toggle: !!li.querySelector('.form-switch') }
    }
    return {
      switches: switches.map((s) => s.getAttribute('aria-label')),
      checked: switches.map((s) => s.checked),
      dashboard: info('Dashboard'),
      documentation: info('Documentation'),
    }
  })
}

async function login(page, base) {
  await page.goto(`${new URL(base).origin}/accounts/login/`, { waitUntil: 'domcontentloaded' })
  await page.fill('#inputUsername', 'admin')
  await page.fill('#inputPassword', 'admin')
  await Promise.all([page.waitForURL((u) => !/accounts\/login/.test(u.toString())), page.click('button[type=submit]')])
}

const settle = (page) => page.waitForLoadState('networkidle').then(() => page.waitForTimeout(400))

async function runUi(browser, ui) {
  const base = BASES[ui]
  const results = []
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US', reducedMotion: 'reduce' })
  const page = await ctx.newPage()
  const errors = []
  page.on('pageerror', (e) => errors.push(String(e)))
  const shot = async (name) => {
    if (!OUT) return
    fs.mkdirSync(path.resolve(ROOT, OUT), { recursive: true })
    await page.screenshot({ path: path.resolve(ROOT, OUT, `${ui}-${name}.png`) })
  }
  const check = async (name, fn) => {
    try {
      await fn()
      results.push({ ui, name, status: 'ok' })
    } catch (e) {
      results.push({ ui, name, status: 'FAIL', error: String(e?.message ?? e).split('\n')[0] })
      await shot(`FAIL-${name}`).catch(() => {})
    }
  }
  const expectNoToggles = async (where) => {
    const s = await sidebarState(page)
    if (s.switches.length) throw new Error(`${where}: ${s.switches.length} Schalter in der Seitenleiste (${s.switches.join(', ')})`)
    if (s.dashboard?.toggle || s.documentation?.toggle) throw new Error(`${where}: Dashboard/Dokumentation als Schalter`)
    if (s.dashboard?.padded || s.documentation?.padded) throw new Error(`${where}: pe-5 ohne Bearbeiten-Modus`)
  }
  const expectEditMode = async (where) => {
    await page.locator('#sidebarMenu .form-switch').first().waitFor({ timeout: 15000 })
    const s = await sidebarState(page)
    if (JSON.stringify(s.switches) !== JSON.stringify(EXPECTED)) throw new Error(`${where}: Schalter ${JSON.stringify(s.switches)} ≠ ${JSON.stringify(EXPECTED)}`)
    if (!s.checked.every(Boolean)) throw new Error(`${where}: nicht alle Schalter an ${JSON.stringify(s.checked)}`)
    if (!s.dashboard.padded || !s.documentation.padded) throw new Error(`${where}: pe-5 fehlt im Bearbeiten-Modus`)
  }

  await login(page, base)

  // 1. Outside edit mode never switches (also tabs Documents/Permissions, Saved views)
  for (const p of ['dashboard', 'documents', 'settings/documents', 'settings/permissions', 'settings/notifications', 'savedviews', 'tasks']) {
    await check(`keine-schalter:${p}`, async () => {
      await page.goto(`${base}/${p}`, { waitUntil: 'networkidle' })
      await settle(page)
      await expectNoToggles(p)
      if (p === 'dashboard') await shot('dashboard')
    })
  }

  // 2. Edit mode: Settings → General, six switches like upstream
  await check('bearbeiten:settings', async () => {
    await page.goto(`${base}/settings`, { waitUntil: 'networkidle' })
    await expectEditMode('settings')
    await shot('settings-general')
  })

  // 3. Switch "Dashboard" off: entry stays visible (opacity-50), checkbox in the form follows; Cancel restores
  await check('bearbeiten:umschalten-und-verwerfen', async () => {
    await page.locator('#sidebarMenu .form-switch input[aria-label="Dashboard"]').click()
    await page.waitForTimeout(300)
    const s = await sidebarState(page)
    if (!s.dashboard.visible || !s.dashboard.faded) throw new Error(`Dashboard nach Ausschalten ${JSON.stringify(s.dashboard)}`)
    const formBox = page.locator('#sidebar-item-setting-dashboard')
    if (await formBox.isChecked()) throw new Error('Kontrollkästchen „Dashboard“ im Formular nicht mitgeschaltet')
    await shot('settings-general-dashboard-off')
    await page.getByRole('button', { name: 'Cancel' }).click()
    await page.waitForTimeout(300)
    const r = await sidebarState(page)
    if (r.dashboard.faded || !r.checked.every(Boolean)) throw new Error(`Cancel stellt nicht her ${JSON.stringify(r)}`)
    if (!(await formBox.isChecked())) throw new Error('Kontrollkästchen nach Cancel nicht zurückgesetzt')
  })

  // 4. Checkbox in the form → switch in the sidebar
  await check('bearbeiten:formular-koppelt-schalter', async () => {
    await page.locator('#sidebar-item-setting-documentation').click()
    await page.waitForTimeout(300)
    const s = await sidebarState(page)
    if (s.checked[EXPECTED.indexOf('Documentation')] || !s.documentation.faded) throw new Error(`Dokumentation ${JSON.stringify(s)}`)
    await page.getByRole('button', { name: 'Cancel' }).click()
    await page.waitForTimeout(300)
  })

  // 5. Leaving ends the mode: tab change, click in the sidebar, back
  await check('verlassen:reiter-dokumente', async () => {
    await page.locator('.nav-tabs a', { hasText: /^\s*Documents\s*$/ }).first().click()
    await page.waitForURL(/settings\/documents/)
    await settle(page)
    await expectNoToggles('settings/documents (Reiterwechsel)')
  })
  await check('verlassen:seitenleiste', async () => {
    await page.goto(`${base}/settings`, { waitUntil: 'networkidle' })
    await expectEditMode('settings (erneut)')
    await page.locator('#sidebarMenu a.nav-link', { hasText: 'Dashboard' }).click()
    await page.waitForURL(/dashboard/)
    await settle(page)
    await expectNoToggles('dashboard nach Einstellungen')
    await shot('dashboard-after-settings')
  })
  await check('verlassen:zurueck', async () => {
    await page.goto(`${base}/settings`, { waitUntil: 'networkidle' })
    await expectEditMode('settings (vor Zurück)')
    await page.goBack({ waitUntil: 'networkidle' })
    await settle(page)
    await expectNoToggles(`zurück (${page.url()})`)
  })

  // 6. Narrow view (menu expanded): outside never, in the mode like wide
  await check('schmal:dashboard-und-settings', async () => {
    await page.setViewportSize({ width: 600, height: 900 })
    await page.goto(`${base}/dashboard`, { waitUntil: 'networkidle' })
    await page.locator('button.navbar-toggler').click()
    await settle(page)
    await expectNoToggles('dashboard schmal')
    await shot('narrow-dashboard')
    await page.goto(`${base}/settings`, { waitUntil: 'networkidle' })
    await page.locator('button.navbar-toggler').click()
    await expectEditMode('settings schmal')
    await page.setViewportSize({ width: 1440, height: 900 })
  })

  if (errors.length) results.push({ ui, name: 'seitenfehler', status: 'FAIL', error: errors.slice(0, 3).join(' | ') })
  await ctx.close()
  return results
}

const browser = await chromium.launch(CHANNEL === 'none' ? {} : { channel: CHANNEL })
const all = []
for (const ui of UIS) {
  const r = await runUi(browser, ui)
  for (const x of r) console.log(`${x.status.padEnd(5)} ${x.ui.padEnd(9)} ${x.name}${x.error ? ' – ' + x.error : ''}`)
  all.push(...r)
}
await browser.close()
const failed = all.filter((r) => r.status !== 'ok').length
if (OUT) fs.writeFileSync(path.resolve(ROOT, OUT, 'sidebar-edit-mode.json'), JSON.stringify({ date: new Date().toISOString(), bases: BASES, results: all }, null, 2) + '\n')
console.log(`\n${all.length - failed} ok, ${failed} fehlgeschlagen`)
process.exit(failed ? 1 : 0)
