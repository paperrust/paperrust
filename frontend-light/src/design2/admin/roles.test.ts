import { describe, expect, it } from 'vitest'
import {
  ACTIONS,
  FALLBACK_ALWAYS,
  FALLBACK_TOPICS,
  applyModelSwitch,
  applySwitch,
  bare,
  classify,
  effectivePerms,
  fallbackInfo,
  keepHidden,
  levelOf,
  modelState,
  permsToSwitches,
  pickableRoles,
  qualify,
  representable,
  rolePerms,
  samePerms,
  summarize,
  switchesToPerms,
  userAccess,
  visibleTopics,
  type RolesInfo,
  type Switches,
} from './roles'

const topics = FALLBACK_TOPICS
const always = FALLBACK_ALWAYS

/** all 2^15 switch combinations of the five topics */
function* allSwitches(): Generator<Switches> {
  const n = topics.length * ACTIONS.length
  for (let mask = 0; mask < 1 << n; mask++) {
    const sw: Switches = {}
    topics.forEach((t, ti) => {
      sw[t.key] = { view: false, edit: false, delete: false }
      ACTIONS.forEach((a, ai) => (sw[t.key][a] = !!(mask & (1 << (ti * ACTIONS.length + ai)))))
    })
    yield sw
  }
}

describe('Themen-Schalter ↔ Codenames', () => {
  it('ist für alle Kombinationen verlustfrei (hin und zurück)', () => {
    // without expect() per step (32,768 cases), errors are collected
    let count = 0
    const bad: string[] = []
    for (const sw of allSwitches()) {
      const perms = switchesToPerms(sw, topics, always)
      const back = permsToSwitches(perms, topics)
      const again = switchesToPerms(back as Switches, topics, always)
      if (JSON.stringify(back) !== JSON.stringify(sw) || again.join() !== perms.join() || (count % 97 === 0 && !representable(perms, topics, always))) bad.push(JSON.stringify(sw))
      count++
    }
    expect(bad.slice(0, 3)).toEqual([])
    expect(count).toBe(1 << 15)
  }, 30000)

  it('Bearbeiten = hinzufügen + ändern, Löschen = delete', () => {
    const p = switchesToPerms({ views: { view: false, edit: true, delete: true } }, topics)
    expect(p).toEqual(['add_savedview', 'change_savedview', 'delete_savedview'])
  })

  it('applySwitch ändert nur das eine Thema und lässt fremde Rechte stehen', () => {
    const start = ['view_document', 'view_global_statistics', 'add_processedmail']
    const p = applySwitch(start, topics, 'organize', 'view', true)
    expect(p).toContain('view_global_statistics')
    expect(p).toContain('add_processedmail')
    expect(p).toContain('view_tag')
    expect(applySwitch(p, topics, 'organize', 'view', false).sort()).toEqual([...start].sort())
  })

  it('Teilmengen sind „mixed“ und nicht darstellbar', () => {
    const st = permsToSwitches(['add_document'], topics)
    expect(st.documents.edit).toBe('mixed')
    expect(representable(['add_document'], topics)).toBe(false)
    expect(representable(['view_global_statistics'], topics)).toBe(false)
    expect(levelOf(st.documents)).toBe('partial')
  })

  it('App-Präfix wird ignoriert', () => {
    expect(bare('paperless_mail.view_mailrule')).toBe('view_mailrule')
    expect(permsToSwitches(['documents.view_savedview'], topics).views.view).toBe(true)
  })
})

describe('Rollen-Erkennung', () => {
  const info = fallbackInfo()

  it('jede Rolle → Rechtemenge → Erkennung ergibt dieselbe Rolle', () => {
    for (const r of info.roles) {
      const c = classify(rolePerms(r, info.always), info, { superuser: r.admin })
      expect(c.kind).toBe('role')
      if (c.kind === 'role') expect(c.role.id).toBe(r.id)
    }
  })

  it('immer mitgegebene Rechte zählen nicht', () => {
    const read = info.roles.find((r) => r.key === 'read')!
    const withoutUi = rolePerms(read).filter((p) => !p.includes('uisettings'))
    const c = classify(withoutUi, info)
    expect(c.kind === 'role' && c.role.key).toBe('read')
  })

  it('abweichende Menge → Individuell, leer → keine', () => {
    const read = info.roles.find((r) => r.key === 'read')!
    expect(classify([...rolePerms(read), 'add_tag'], info).kind).toBe('individual')
    expect(classify([], info).kind).toBe('none')
    expect(classify(['view_uisettings'], info).kind).toBe('none')
  })

  it('Administrator nur über Superuser, nie über leere Menge', () => {
    expect(classify([], info).kind).toBe('none')
    const c = classify(['view_document'], info, { superuser: true })
    expect(c.kind === 'role' && c.role.key).toBe('admin')
  })

  it('gleiche Mengen: Merkhilfe entscheidet', () => {
    const custom: RolesInfo = {
      ...info,
      mode: 'custom',
      roles: [...info.roles, { id: 9, key: null, name: 'Kopie', description: '', builtin: false, admin: false, permissions: info.roles[0].permissions }],
    }
    const p = rolePerms(info.roles[0])
    expect(classify(p, custom).kind === 'role' && (classify(p, custom) as any).role.id).toBe(info.roles[0].id)
    expect((classify(p, custom, { hint: 9 }) as any).role.id).toBe(9)
  })

  it('Modus klassisch erkennt keine eigenen Rollen', () => {
    const own = { id: 7, key: null, name: 'Buchhaltung', description: '', builtin: false, admin: false, permissions: ['documents.view_tag'] }
    expect(classify(['view_tag'], { ...info, roles: [...info.roles, own], mode: 'classic' }).kind).toBe('individual')
    const c = classify(['view_tag'], { ...info, roles: [...info.roles, own], mode: 'custom' })
    expect(c.kind === 'role' && c.role.id).toBe(7)
  })
})

describe('geerbte Rolle und wirksame Rechte', () => {
  const info = fallbackInfo()
  const edit = info.roles.find((r) => r.key === 'edit')!
  const read = info.roles.find((r) => r.key === 'read')!
  const groups = [
    { id: 1, name: 'Team', permissions: rolePerms(edit) },
    { id: 2, name: 'Leser', permissions: rolePerms(read) },
    { id: 3, name: 'Leer', permissions: [] },
  ]

  it('ohne eigene Rechte: Rolle der Gruppe, „von Gruppe X“', () => {
    const a = userAccess({ id: 5, groups: [3, 1], user_permissions: ['view_uisettings'] }, groups, info)
    expect(a.kind).toBe('role')
    expect(a.kind === 'role' && a.role.key).toBe('edit')
    expect(a.inherited?.name).toBe('Team')
  })

  it('mehrere Gruppen: Vereinigung entscheidet', () => {
    const a = userAccess({ id: 5, groups: [1, 2], user_permissions: [] }, groups, info)
    // Edit ⊇ Read only → union = Edit
    expect(a.kind === 'role' && a.role.key).toBe('edit')
    expect(a.inherited?.name).toBe('Team')
  })

  it('eigene Rechte gehen vor, nicht geerbt', () => {
    const a = userAccess({ id: 5, groups: [1], user_permissions: rolePerms(read) }, groups, info)
    expect(a.kind === 'role' && a.role.key).toBe('read')
    expect(a.inherited).toBeNull()
  })

  it('keine Rechte → none; Superuser → Administrator', () => {
    expect(userAccess({ id: 5, groups: [3] }, groups, info).kind).toBe('none')
    const a = userAccess({ id: 5, is_superuser: true }, groups, info)
    expect(a.kind === 'role' && a.role.admin).toBe(true)
  })

  it('wirksame Rechte = eigene ∪ Gruppen', () => {
    const p = effectivePerms({ id: 5, groups: [2], user_permissions: ['documents.delete_document'] }, groups)
    expect(p).toContain('delete_document')
    expect(p).toContain('view_document')
    const s = summarize(p, topics)
    expect(s.all).toBe(false)
    if (!s.all) {
      expect(s.items.find((i) => i.topic === 'documents')?.level).toBe('partial')
      expect(s.items.find((i) => i.topic === 'organize')?.level).toBe('view')
      expect(s.items.find((i) => i.topic === 'automation')?.level).toBe('none')
    }
    expect(summarize([], topics, true)).toEqual({ all: true, items: [] })
  })
})

describe('Hilfen', () => {
  const info = fallbackInfo()
  it('qualify ergänzt das App-Präfix', () => {
    expect(qualify(['view_mailrule', 'view_user', 'change_uisettings', 'unknown_x'], info)).toEqual([
      'auth.view_user',
      'documents.change_uisettings',
      'paperless_mail.view_mailrule',
      'unknown_x',
    ])
  })
  it('System-Thema nur für Administratoren; versteckte Schalter bleiben erhalten', () => {
    expect(visibleTopics(topics, false).map((t) => t.key)).not.toContain('system')
    expect(visibleTopics(topics, true).map((t) => t.key)).toContain('system')
    const hidden = topics.filter((t) => t.key === 'system')
    expect(keepHidden(['view_document'], ['view_user', 'view_tag'], hidden)).toEqual(['view_document', 'view_user'])
  })
  it('Auswahl: klassisch nur feste Rollen, Gruppen ohne Administrator', () => {
    const custom = { id: 8, key: null, name: 'Eigene', description: '', builtin: false, admin: false, permissions: [] }
    const i2: RolesInfo = { ...info, roles: [...info.roles, custom] }
    expect(pickableRoles(i2).map((r) => r.key)).toEqual(['read', 'edit', 'manage', 'admin'])
    expect(pickableRoles({ ...i2, mode: 'custom' }).map((r) => r.id)).toContain(8)
    expect(pickableRoles(i2, true).some((r) => r.admin)).toBe(false)
    expect(pickableRoles(i2, false, false).some((r) => r.admin)).toBe(false)
  })
  it('samePerms vergleicht ohne Präfix und ohne immer mitgegebene Rechte', () => {
    expect(samePerms(['documents.view_tag', 'view_uisettings'], ['view_tag'], always)).toBe(true)
    expect(samePerms(['view_tag'], ['view_tag', 'add_tag'], always)).toBe(false)
  })
})

describe('Rollen-API-Client (Attrappe)', async () => {
  const api = await import('../api/roles')
  const json = (status: number, body: unknown, ct = 'application/json') => new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { 'Content-Type': ct } })

  it('normalisiert GET /roles', async () => {
    api.setRolesFetch(async () =>
      json(200, {
        mode: 'custom',
        roles: [
          { id: 4, key: 'admin', name: 'Administrator', description: '', builtin: true, admin: true, permissions: [] },
          { id: 1, key: 'read', name: 'Nur lesen', description: 'x', builtin: true, admin: false, permissions: ['documents.view_document'] },
        ],
        topics: [{ key: 'documents', models: ['documents.document'] }],
        always: ['documents.view_uisettings'],
      }),
    )
    const info = await api.loadRoles()
    expect(info.mode).toBe('custom')
    expect(info.fallback).toBeUndefined()
    expect(info.roles.map((r) => r.key)).toEqual(['read', 'admin'])
    expect(info.topics).toHaveLength(1)
  })

  it('404, HTML oder 403 → lokale Vorgabe', async () => {
    for (const r of [json(404, {}), json(200, '<html>', 'text/html'), json(403, { detail: 'nope' })]) {
      api.setRolesFetch(async () => r)
      const info = await api.loadRoles()
      expect(info.fallback).toBe(true)
      expect(info.roles).toHaveLength(4)
    }
  })

  it('400 mit Fehlertext (Selbst-Herabstufung)', async () => {
    api.setRolesFetch(async () => json(400, { error: { message: 'Sie können sich nicht selbst herabstufen.' } }))
    await expect(api.assignRole(2, { users: [1] })).rejects.toMatchObject({ status: 400, message: 'Sie können sich nicht selbst herabstufen.' })
    api.setRolesFetch(async () => json(400, { users: ['Fehler A'] }))
    await expect(api.assignRole(2, { users: [1] })).rejects.toMatchObject({ message: 'Fehler A' })
  })

  it('sendet assign und resolve richtig', async () => {
    const calls: [string, RequestInit | undefined][] = []
    api.setRolesFetch(async (u, init) => {
      calls.push([u, init])
      return u.includes('resolve') ? json(200, { users: { 3: 1 }, groups: { 2: 'individual' } }) : new Response(null, { status: 204 })
    })
    await api.assignRole(2, { groups: [5] })
    expect(calls[0][0]).toBe('/rs/api/v2/roles/2/assign')
    expect(JSON.parse(String(calls[0][1]?.body))).toEqual({ users: [], groups: [5] })
    const r = await api.resolveRoles([3], [2])
    expect(r.users['3']).toBe(1)
    expect(calls[1][0]).toBe('/rs/api/v2/roles/resolve?users=3&groups=2')
    api.setRolesFetch(null)
  })
})

describe('Rollen der API (Teilmengen je Modell)', () => {
  const info = fallbackInfo()
  const manage = info.roles.find((r) => r.key === 'manage')!
  const edit = info.roles.find((r) => r.key === 'edit')!

  it('Verwalten/Bearbeiten sind je Thema teilweise, je Modell darstellbar', () => {
    const st = permsToSwitches(rolePerms(manage), topics)
    expect(st.automation.edit).toBe('mixed') // Tasks view only
    expect(modelState(rolePerms(manage), 'documents.paperlesstask')).toEqual({ view: true, edit: false, delete: false })
    expect(modelState(rolePerms(edit), 'documents.sharelink').edit).toBe('mixed') // only add_sharelink
  })

  it('Schalter je Modell ändert nur dieses Modell; zurückschalten ergibt wieder die Rolle', () => {
    const p0 = rolePerms(manage, info.always)
    const p1 = applyModelSwitch(p0, 'documents.paperlesstask', 'delete', true)
    expect(p1).toContain('delete_paperlesstask')
    expect(classify(p1, info).kind).toBe('individual')
    const p2 = applyModelSwitch(p1, 'documents.paperlesstask', 'delete', false)
    const c = classify(p2, info)
    expect(c.kind === 'role' && c.role.key).toBe('manage')
  })

  it('jedes *_uisettings zählt nie zur Rolle', () => {
    const c = classify([...rolePerms(edit), 'delete_uisettings', 'documents.view_uisettings'], info)
    expect(c.kind === 'role' && c.role.key).toBe('edit')
  })

  it('Ergebnis von resolve hat Vorrang (z. B. ältere DB ohne einzelne Rechte)', () => {
    const missing = rolePerms(edit).filter((p) => !p.includes('sharelinkbundle'))
    expect(classify(missing, info).kind).toBe('individual')
    const c = classify(missing, info, { resolved: edit.id })
    expect(c.kind === 'role' && c.role.key).toBe('edit')
    expect(classify(rolePerms(edit), info, { resolved: 'individual' }).kind).toBe('individual')
  })

  it('geerbte Rolle über resolve der Gruppe', () => {
    const groups = [{ id: 9, name: 'Alt', permissions: rolePerms(edit).filter((p) => !p.includes('sharelinkbundle')) }]
    const a = userAccess({ id: 1, groups: [9], user_permissions: [] }, groups, info, null, { 9: edit.id })
    expect(a.kind === 'role' && a.role.key).toBe('edit')
    expect(a.inherited?.name).toBe('Alt')
  })
})
