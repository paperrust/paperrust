// Flow tests document view, notes, upload and dashboard (P4, agent B). Runs with tests/flows/flows.mjs against a
// test server WITH processing (F0_ARGS="serve --worker", otherwise the upload stays at "waiting…").
// Same selectors in both UIs (host tags + labels), state afterwards via API v1.
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const ADMIN = 2 // reference/README.md
const base = (page) => page.url().replace(/\/(documents|dashboard).*$/, '')
const DETAIL_READY = 'pngx-document-detail pngx-input-text input'

/** Document from admin with text field (custom field), correspondent changeable; remember the state before the flow */
async function pickDocument(api, ctx) {
  const fields = await api.all('custom_fields')
  const strField = fields.filter((f) => f.data_type === 'string')
  const docs = (await api.get('/api/documents/?page_size=100&owner__id=2&ordering=id')).results
  const doc = docs.find((d) => d.custom_fields.some((cf) => strField.some((f) => f.id === cf.field)) && d.custom_fields.length === 1) ?? docs[0]
  const field = strField.find((f) => doc.custom_fields.some((cf) => cf.field === f.id)) ?? strField[0]
  const corrs = await api.all('correspondents')
  const corr = corrs.find((c) => c.id !== doc.correspondent && (c.owner === ADMIN || c.owner === null))
  const tag = (await api.all('tags')).find((t) => !doc.tags.includes(t.id) && !t.is_inbox_tag && (t.owner === ADMIN || t.owner === null))
  ctx.doc = doc
  ctx.field = field
  ctx.corr = corr
  ctx.newTag = tag
  ctx.assert(doc && field && corr && tag, 'Testdaten gefunden')
}

/** ng-select (both UIs): type search text, accept hit with Enter */
async function ngSelect(page, host, text) {
  const sel = page.locator(host).locator('ng-select').first()
  await sel.locator('input').click() // not in the middle of the container: the × of a tag may be there
  await sel.locator('input').fill(text)
  await page.locator('ng-dropdown-panel .ng-option', { hasText: text }).first().waitFor()
  await sel.locator('input').press('Enter')
  await sel.locator('input').press('Tab') // Escape would discard/close editing in the document view
}

/** Small PDF with text layer (unique content so that the consumer does not reject it as a duplicate) */
function makePdf(file, text) {
  const objs = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
    null,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ]
  const stream = `BT /F1 24 Tf 72 760 Td (${text}) Tj ET`
  objs[3] = `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`
  let out = '%PDF-1.4\n'
  const offs = []
  objs.forEach((o, i) => {
    offs.push(out.length)
    out += `${i + 1} 0 obj\n${o}\nendobj\n`
  })
  const xref = out.length
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offs.map((o) => String(o).padStart(10, '0') + ' 00000 n \n').join('')}`
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
  fs.writeFileSync(file, out, 'latin1')
}

export default [
  {
    id: 'detail-edit',
    path: 'dashboard',
    ready: 'pngx-upload-file-widget',
    async run(page, ctx) {
      await pickDocument(ctx.api, ctx)
      await page.goto(`${base(page)}/documents/${ctx.doc.id}/details`, { waitUntil: 'networkidle' })
      await page.locator(DETAIL_READY).first().waitFor()
      await page.locator('pngx-input-text', { hasText: 'Title' }).locator('input').first().fill(`Flow ${ctx.tag}`)
      await ngSelect(page, 'pngx-input-select:has(label:text-is("Correspondent"))', ctx.corr.name)
      await ngSelect(page, 'pngx-input-tags', ctx.newTag.name)
      await page.locator('pngx-input-text', { hasText: ctx.field.name }).locator('input').first().fill(`Wert ${ctx.tag}`)
      await Promise.all([
        page.waitForResponse((r) => r.url().includes(`/api/documents/${ctx.doc.id}/`) && ['PUT', 'PATCH'].includes(r.request().method()), { timeout: 15000 }),
        page.locator('pngx-document-detail button[type=submit]').first().click(),
      ])
      await page.waitForTimeout(500)
    },
    async check(api, ctx) {
      const d = await api.get(`/api/documents/${ctx.doc.id}/`)
      ctx.assert(d.title === `Flow ${ctx.tag}`, `Titel ${d.title}`)
      ctx.assert(d.correspondent === ctx.corr.id, `Korrespondent ${d.correspondent} ≠ ${ctx.corr.id}`)
      ctx.assert(d.tags.includes(ctx.newTag.id) && ctx.doc.tags.every((t) => d.tags.includes(t)), `Tags ${d.tags}`)
      const cf = d.custom_fields.find((c) => c.field === ctx.field.id)
      ctx.assert(cf?.value === `Wert ${ctx.tag}`, `Feld ${JSON.stringify(cf)}`)
    },
  },
  {
    id: 'notes-add-delete',
    path: 'dashboard',
    ready: 'pngx-upload-file-widget',
    async run(page, ctx) {
      const doc = (await ctx.api.get('/api/documents/?page_size=1&owner__id=2&ordering=id')).results[0]
      ctx.doc = doc
      ctx.before = (await ctx.api.get(`/api/documents/${doc.id}/notes/`)).length
      await page.goto(`${base(page)}/documents/${doc.id}/notes`, { waitUntil: 'networkidle' })
      await page.locator('pngx-document-notes textarea').waitFor()
      // Create notes A and B, then delete A
      for (const n of ['A', 'B']) {
        await page.locator('pngx-document-notes textarea').fill(`Notiz ${n} ${ctx.tag}`)
        await Promise.all([
          page.waitForResponse((r) => r.url().includes(`/api/documents/${doc.id}/notes/`) && r.request().method() === 'POST'),
          page.locator('pngx-document-notes button.btn-primary').click(),
        ])
        await page.locator('pngx-document-notes .card', { hasText: `Notiz ${n} ${ctx.tag}` }).waitFor()
      }
      await Promise.all([
        page.waitForResponse((r) => r.url().includes(`/api/documents/${doc.id}/notes/`) && r.request().method() === 'DELETE'),
        page.locator('pngx-document-notes .card', { hasText: `Notiz A ${ctx.tag}` }).locator('button[title="Delete note"]').click({ force: true }),
      ])
      await page.locator('pngx-document-notes .card', { hasText: `Notiz A ${ctx.tag}` }).waitFor({ state: 'detached' })
    },
    async check(api, ctx) {
      const notes = await api.get(`/api/documents/${ctx.doc.id}/notes/`)
      ctx.assert(notes.length === ctx.before + 1, `Anzahl Notizen ${notes.length} (vorher ${ctx.before})`)
      ctx.assert(notes.some((n) => n.note === `Notiz B ${ctx.tag}`) && !notes.some((n) => n.note === `Notiz A ${ctx.tag}`), 'B da, A gelöscht')
      ctx.assert(notes.find((n) => n.note === `Notiz B ${ctx.tag}`)?.user?.id === ADMIN, 'Verfasser admin')
    },
  },
  {
    id: 'upload-consume',
    path: 'dashboard',
    ready: 'pngx-upload-file-widget',
    async run(page, ctx) {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pngx-flow-'))
      ctx.file = `flow-${ctx.tag}.pdf`
      const file = path.join(dir, ctx.file)
      makePdf(file, `PaperRust Flow Upload ${ctx.tag} - sample document with a text layer`) // > 50 characters: no OCR needed
      await page.locator('#navbar-file-upload').setInputFiles(file)
      // Processing finished: upload list shows "Open document", bell contains "<file> is now in PaperRust." (common.document_added_paperrust)
      await page.locator('pngx-upload-status-tray ngb-alert', { hasText: ctx.file }).locator('.btn-open').waitFor({ timeout: 120000 })
      await page.locator('#notificationsDropdown').click()
      await page.locator('pngx-toasts-dropdown, .dropdown-menu', { hasText: `${ctx.file} is now in PaperRust.` }).first().waitFor({ timeout: 10000 })
      fs.rmSync(dir, { recursive: true, force: true })
    },
    async check(api, ctx) {
      const r = await api.get(`/api/documents/?original_filename__icontains=${encodeURIComponent(ctx.file)}`)
      ctx.assert(r.count === 1, `Dokument angelegt (${r.count})`)
      const d = r.results[0]
      ctx.assert(d.owner === ADMIN, `Eigentümer ${d.owner}`)
      ctx.assert(d.content.includes(`Flow Upload ${ctx.tag}`), `Inhalt ${JSON.stringify(d.content.slice(0, 80))}`)
    },
  },
  {
    id: 'dashboard-reorder',
    path: 'dashboard',
    ready: 'pngx-upload-file-widget',
    async run(page, ctx) {
      const s = (await ctx.api.get('/api/ui_settings/')).settings
      s.saved_views = { ...(s.saved_views || {}), dashboard_views_visible_ids: [1, 2, 4], dashboard_views_sort_order: [1, 2, 4] }
      await ctx.api.post('/api/ui_settings/', { settings: s })
      await page.reload({ waitUntil: 'networkidle' })
      await page.locator('pngx-saved-view-widget table tbody tr').first().waitFor()
      // drag the first widget by the handle below the second (cdkDrag: pointer events, threshold 5 px)
      const h = await page.locator('pngx-saved-view-widget .card-header pngx-bs-icon[name="grip-vertical"]').first().boundingBox()
      const second = await page.locator('pngx-saved-view-widget .card').nth(1).boundingBox()
      const x = h.x + h.width / 2
      await page.mouse.move(x, h.y + h.height / 2)
      await page.mouse.down()
      const target = second.y + second.height * 0.75
      for (let i = 1; i <= 20; i++) await page.mouse.move(x, h.y + ((target - h.y) * i) / 20)
      await page.waitForTimeout(300)
      await Promise.all([page.waitForResponse((r) => r.url().includes('/api/ui_settings/') && r.request().method() === 'POST'), page.mouse.up()])
      await page.locator('ngb-toast', { hasText: 'Dashboard saved' }).first().waitFor({ timeout: 10000 })
      ctx.order = await page.$$eval('pngx-saved-view-widget h6.card-title', (e) => e.map((x) => x.textContent.trim()))
    },
    async check(api, ctx) {
      const s = (await api.get('/api/ui_settings/')).settings
      const order = s.saved_views?.dashboard_views_sort_order
      ctx.assert(JSON.stringify(order) === '[2,1,4]', `Reihenfolge ${JSON.stringify(order)}`)
      const names = new Map((await api.all('saved_views')).map((v) => [v.id, v.name]))
      ctx.assert(JSON.stringify(ctx.order) === JSON.stringify(order.map((id) => names.get(id))), `Anzeige ${ctx.order}`)
      s.saved_views.dashboard_views_visible_ids = []
      s.saved_views.dashboard_views_sort_order = []
      await api.post('/api/ui_settings/', { settings: s })
    },
  },
]
