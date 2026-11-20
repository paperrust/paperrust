// Flow tests administration: per screen save via the UI, check the state via API (second P4 agent).
const MODAL = '.modal-dialog'
const SAVE = '.modal-dialog .modal-footer button[type=submit], .modal-dialog .modal-footer .btn-primary'

async function waitGone(page, sel) {
  await page.locator(sel).first().waitFor({ state: 'detached', timeout: 15000 })
}

export default [
  {
    id: 'tags-create',
    path: 'attributes/tags',
    ready: 'pngx-tag-list tbody tr.data-row',
    async run(page, ctx) {
      await page.click('pngx-page-header button:has(pngx-bs-icon[name="plus-circle"])')
      await page.locator(MODAL).waitFor()
      await page.locator(`${MODAL} pngx-input-text input`).first().fill(`Flow-Tag ${ctx.tag}`)
      await page.locator(`${MODAL} pngx-input-color input.form-control`).fill('#123456')
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const t = (await api.all('tags')).find((x) => x.name === `Flow-Tag ${ctx.tag}`)
      ctx.assert(t, 'Tag angelegt')
      ctx.assert(t.color === '#123456', `Farbe ${t.color}`)
      ctx.assert(t.owner === 2, `Eigentümer = admin (Standardrechte), ist ${t.owner}`)
    },
  },
  {
    id: 'correspondents-edit',
    path: 'attributes/correspondents',
    ready: 'pngx-correspondent-list tbody tr.data-row',
    async run(page, ctx) {
      await page.locator('pngx-correspondent-list td.name-cell button').first().click()
      await page.locator(MODAL).waitFor()
      ctx.before = await page.locator(`${MODAL} pngx-input-text input`).first().inputValue()
      await page.locator(`${MODAL} pngx-input-text input`).nth(1).fill(`flowmatch ${ctx.tag}`)
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const c = (await api.all('correspondents')).find((x) => x.name === ctx.before)
      ctx.assert(c?.match === `flowmatch ${ctx.tag}`, `Zuordnung gespeichert (${c?.match})`)
    },
  },
  {
    id: 'settings-save',
    path: 'settings/general',
    ready: 'pngx-settings #dateFormatShort',
    async run(page) {
      await page.check('#dateFormatLong')
      await page.locator('pngx-settings button[type=submit]').click()
      await page.waitForTimeout(1500)
    },
    async check(api, ctx) {
      const s = (await api.get('/api/ui_settings/')).settings
      ctx.assert(s.date_display?.date_format === 'longDate', `Datumsformat ${JSON.stringify(s.date_display)}`)
      s.date_display.date_format = 'mediumDate'
      await api.post('/api/ui_settings/', { settings: s })
    },
  },
  {
    id: 'savedviews-save',
    path: 'savedviews',
    ready: 'pngx-saved-views li.list-group-item.py-3',
    async run(page, ctx) {
      const first = page.locator('pngx-saved-views li.list-group-item.py-3').first()
      ctx.id = +(await first.locator('input[id^=show_in_sidebar_]').getAttribute('id')).split('_').pop()
      await first.locator('pngx-input-text input').fill(`Ansicht ${ctx.tag}`)
      await first.locator('input[id^=show_in_sidebar_]').click()
      ctx.sidebar = await first.locator('input[id^=show_in_sidebar_]').isChecked()
      await first.locator('select.form-select').selectOption('largeCards') // Label "Large cards" (own texts since b2db98e), value stable
      await page.locator('pngx-saved-views button[type=submit]').click()
      await page.waitForTimeout(1500)
    },
    async check(api, ctx) {
      const v = await api.get(`/api/saved_views/${ctx.id}/`)
      ctx.assert(v.name === `Ansicht ${ctx.tag}`, `Name ${v.name}`)
      ctx.assert(v.display_mode === 'largeCards', `Anzeige ${v.display_mode}`)
      const ids = (await api.get('/api/ui_settings/')).settings.saved_views?.sidebar_views_visible_ids ?? []
      ctx.assert(ids.includes(ctx.id) === ctx.sidebar, `Seitenleiste ${JSON.stringify(ids)} / ${ctx.sidebar}`)
    },
  },
  {
    id: 'trash-restore',
    path: 'trash',
    ready: 'pngx-trash tbody tr.data-row',
    async setup(api) {
      // Precondition: at least one document in the trash (other suites empty it); otherwise put one in from admin
      if ((await api.get('/api/trash/?page=1')).count > 0) return
      const doc = (await api.get('/api/documents/?page_size=1&owner__id=2&ordering=-id')).results[0]
      await api.del(`/api/documents/${doc.id}/`)
    },
    async run(page, ctx) {
      const row = page.locator('pngx-trash tbody tr.data-row').first()
      ctx.id = +(await row.locator('input.form-check-input').getAttribute('id'))
      await row.locator('button.btn-outline-secondary').click()
      await page.waitForTimeout(1500)
    },
    async check(api, ctx) {
      const trash = await api.get('/api/trash/?page=1')
      ctx.assert(!trash.results.some((d) => d.id === ctx.id), `Dokument ${ctx.id} nicht mehr im Papierkorb`)
      const doc = await api.get(`/api/documents/${ctx.id}/`)
      ctx.assert(doc.id === ctx.id, 'Dokument wiederhergestellt')
      await api.del(`/api/documents/${ctx.id}/`) // back into the trash (for the other UI)
    },
  },
  {
    id: 'tasks-dismiss',
    path: 'tasks',
    ready: 'pngx-tasks table.tasks-table tbody tr',
    async run(page, ctx) {
      await page.click('#autoRefreshSwitch')
      const cb = page.locator('pngx-tasks table.tasks-table tbody tr input.form-check-input').first()
      ctx.id = +(await cb.getAttribute('id')).replace('task', '')
      await page.locator('pngx-tasks table.tasks-table tbody tr').first().locator('button.btn-outline-secondary').click()
      await page.waitForTimeout(1500)
    },
    async check(api, ctx) {
      const r = await api.get('/api/tasks/?page=1&page_size=100&acknowledged=false')
      ctx.assert(!r.results.some((x) => x.id === ctx.id), `Aufgabe ${ctx.id} bestätigt`)
    },
  },
  {
    id: 'logs-switch',
    path: 'logs',
    ready: 'pngx-logs ul.nav-tabs li',
    async run(page, ctx) {
      await page.click('#autoRefreshSwitch')
      const [req] = await Promise.all([page.waitForRequest((r) => /\/api\/logs\/mail\/\?limit=5000/.test(r.url())), page.click('pngx-logs ul.nav-tabs li:nth-child(2) a')])
      ctx.url = req.url()
      await page.waitForTimeout(500)
      ctx.active = await page.locator('pngx-logs ul.nav-tabs a.active').innerText()
    },
    async check(api, ctx) {
      ctx.assert(ctx.active.trim() === 'mail.log', `aktiver Reiter ${ctx.active}`)
      ctx.assert(Array.isArray(await api.get('/api/logs/mail/?limit=5000')), 'mail.log über API lesbar')
    },
  },
  {
    id: 'config-save',
    path: 'config',
    ready: 'pngx-config .card .card-title',
    async run(page, ctx) {
      await page.locator('pngx-config pngx-input-text input').first().fill(`Titel ${ctx.tag}`)
      await page.locator('pngx-config button[type=submit]').click()
      await page.waitForTimeout(1500)
    },
    async check(api, ctx) {
      const c = (await api.get('/api/config/'))[0]
      ctx.assert(c.app_title === `Titel ${ctx.tag}`, `app_title ${c.app_title}`)
      await api.patch(`/api/config/${c.id}/`, { id: c.id, app_title: null })
    },
  },
  {
    id: 'users-create',
    path: 'usersgroups',
    ready: 'pngx-users-groups li.list-group-item button.btn-link',
    async run(page, ctx) {
      await page.locator('pngx-users-groups h4 button').first().click()
      await page.locator(MODAL).waitFor()
      const inputs = page.locator(`${MODAL} pngx-input-text input`)
      await inputs.nth(0).fill(`flow${ctx.tag.toLowerCase()}`)
      await inputs.nth(2).fill('Flow')
      await page.locator(`${MODAL} pngx-input-password input`).fill('Flow-Passwort-1') // without password the API rejects (like upstream)
      await page.locator(`${MODAL} input#Tag_View`).check()
      await page.locator(`${MODAL} input#is_staff`).uncheck()
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const u = (await api.all('users')).find((x) => x.username === `flow${ctx.tag.toLowerCase()}`)
      ctx.assert(u, 'Benutzer angelegt')
      ctx.assert(u.first_name === 'Flow' && u.is_staff === false, `Felder ${u.first_name}/${u.is_staff}`)
      ctx.assert(u.user_permissions.includes('view_tag'), `Rechte ${JSON.stringify(u.user_permissions)}`)
    },
  },
  {
    id: 'groups-create',
    path: 'usersgroups',
    ready: 'pngx-users-groups li.list-group-item button.btn-link',
    async run(page, ctx) {
      await page.locator('pngx-users-groups h4 button').nth(1).click()
      await page.locator(MODAL).waitFor()
      await page.locator(`${MODAL} pngx-input-text input`).first().fill(`Gruppe ${ctx.tag}`)
      await page.locator(`${MODAL} input#Correspondent_all`).check()
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const g = (await api.all('groups')).find((x) => x.name === `Gruppe ${ctx.tag}`)
      ctx.assert(g, 'Gruppe angelegt')
      const want = ['add_correspondent', 'view_correspondent', 'change_correspondent', 'delete_correspondent']
      ctx.assert(want.every((p) => g.permissions.includes(p)), `Rechte ${JSON.stringify(g.permissions)}`)
    },
  },
  {
    id: 'mail-account-create',
    path: 'mail',
    ready: 'pngx-mail .row.fade.show',
    async run(page, ctx) {
      await page.locator('pngx-mail h4 button').first().click()
      await page.locator(MODAL).waitFor()
      const inputs = page.locator(`${MODAL} pngx-input-text input`)
      await inputs.nth(0).fill(`Konto ${ctx.tag}`)
      await inputs.nth(1).fill('imap.flow.invalid')
      await inputs.nth(2).fill('143')
      await inputs.nth(3).fill('flow@example.invalid')
      await page.locator(`${MODAL} pngx-input-password input`).fill('geheim')
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const a = (await api.all('mail_accounts')).find((x) => x.name === `Konto ${ctx.tag}`)
      ctx.assert(a, 'Konto angelegt')
      ctx.assert(a.imap_server === 'imap.flow.invalid' && +a.imap_port === 143 && a.imap_security === 2 && a.character_set === 'UTF-8', JSON.stringify(a).slice(0, 200))
    },
  },
  {
    id: 'mail-rule-toggle',
    path: 'mail',
    ready: 'pngx-mail .row.fade.show',
    async run(page, ctx) {
      const sw = page.locator('pngx-mail ul.list-group').nth(1).locator('input.form-check-input').first()
      ctx.id = +(await sw.getAttribute('id')).split('_')[0]
      ctx.before = await sw.isChecked()
      await sw.click()
      await page.waitForTimeout(1200)
    },
    async check(api, ctx) {
      const r = await api.get(`/api/mail_rules/${ctx.id}/`)
      ctx.assert(r.enabled === !ctx.before, `enabled ${r.enabled}`)
      await api.patch(`/api/mail_rules/${ctx.id}/`, { id: ctx.id, enabled: ctx.before })
    },
  },
  {
    id: 'workflows-create',
    path: 'workflows',
    ready: 'pngx-workflows .row.fade.show',
    async run(page, ctx) {
      await page.click('pngx-page-header button:has(pngx-bs-icon[name="plus-circle"])')
      await page.locator(MODAL).waitFor()
      await page.locator(`${MODAL} pngx-input-text input`).first().fill(`Ablauf ${ctx.tag}`)
      await page.locator(`${MODAL} pngx-input-number input`).first().fill('7')
      await page.locator(`${MODAL} .accordion-button`).nth(0).click() // Triggers
      await page.locator(`${MODAL} button:has(pngx-bs-icon[name="plus-circle"])`).first().click()
      await page.locator(`${MODAL} .accordion .accordion .accordion-button`).first().click()
      await page.locator(`${MODAL} .accordion .accordion pngx-input-text input`).first().fill('*flow*')
      await page.locator(`${MODAL} .btn-lg.accordion-button`).click() // Actions
      await page.locator(`${MODAL} button:has(pngx-bs-icon[name="plus-circle"])`).last().click() // Add action
      await page.locator(SAVE).first().click()
      await waitGone(page, MODAL)
    },
    async check(api, ctx) {
      const w = (await api.all('workflows')).find((x) => x.name === `Ablauf ${ctx.tag}`)
      ctx.assert(w, 'Arbeitsablauf angelegt')
      ctx.assert(w.order === 7 && w.triggers.length === 1 && w.actions.length === 1, `order ${w.order}, ${w.triggers.length}/${w.actions.length}`)
      ctx.assert(w.triggers[0].type === 1 && w.triggers[0].filter_filename === '*flow*', JSON.stringify(w.triggers[0]).slice(0, 200))
      ctx.assert(w.actions[0].type === 1, `Aktion ${w.actions[0].type}`)
    },
  },
]
