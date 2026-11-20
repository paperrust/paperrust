// Vitest: mapping "Visible to" ↔ owner + set_permissions (lib/visibility.ts).
import { describe, expect, it } from 'vitest'
import { apply, available, bulkParameters, canBulkChangeVisibility, canChangeVisibility, classify, emptyPerms, normPerms, summarize } from './visibility'

const P = (vu: number[], vg: number[], cu: number[], cg: number[]) => ({ view: { users: vu, groups: vg }, change: { users: cu, groups: cg } })

describe('classify', () => {
  it('ohne Besitzer und ohne Rechte = Alle', () => {
    expect(classify(null, emptyPerms(), [1])).toBe('all')
    expect(classify(null, undefined, null)).toBe('all')
  })
  it('ohne Besitzer, aber mit Rechten = Bestimmte Personen (nichts verstecken)', () => {
    expect(classify(null, P([2], [], [], []), null)).toBe('custom')
  })
  it('Besitzer ohne Rechte = Nur ich', () => {
    expect(classify(5, emptyPerms(), [1])).toBe('private')
    expect(classify(5, {}, null)).toBe('private')
  })
  it('Ansehen+Bearbeiten genau für die Gruppen des Besitzers = Mein Team', () => {
    expect(classify(5, P([], [2, 1], [], [1, 2]), [1, 2])).toBe('team')
    // change without view row (older data) also counts
    expect(classify(5, P([], [], [], [1, 2]), [2, 1])).toBe('team')
    // duplicates do not matter
    expect(classify(5, P([], [1, 1], [], [1]), [1])).toBe('team')
  })
  it('alles andere = Bestimmte Personen', () => {
    expect(classify(5, P([], [1], [], []), [1])).toBe('custom') // view only
    expect(classify(5, P([], [1, 2], [], [1, 2]), [1])).toBe('custom') // foreign group
    expect(classify(5, P([], [1], [], [1]), [1, 2])).toBe('custom') // not all groups
    expect(classify(5, P([7], [1], [], [1]), [1])).toBe('custom') // additional user
    expect(classify(5, P([], [1], [], [1]), null)).toBe('custom') // groups unknown
    expect(classify(5, emptyPerms(), [])).toBe('private') // owner without groups
  })
})

describe('apply', () => {
  const ctx = { owner: 5, me: 9, ownerGroups: [3, 1] }
  it('Alle entfernt Besitzer und Rechte', () => {
    expect(apply('all', ctx)).toEqual({ owner: null, set_permissions: emptyPerms() })
  })
  it('Nur ich behält den Besitzer, ohne Besitzer wird der aktuelle Benutzer Besitzer', () => {
    expect(apply('private', ctx)).toEqual({ owner: 5, set_permissions: emptyPerms() })
    expect(apply('private', { ...ctx, owner: null })).toEqual({ owner: 9, set_permissions: emptyPerms() })
  })
  it('Mein Team = Ansehen+Bearbeiten für die Gruppen des Besitzers', () => {
    expect(apply('team', ctx)).toEqual({ owner: 5, set_permissions: P([], [1, 3], [], [1, 3]) })
  })
  it('Mein Team ohne (bekannte) Gruppen ist nicht möglich', () => {
    expect(apply('team', { ...ctx, ownerGroups: [] })).toBeNull()
    expect(apply('team', { ...ctx, ownerGroups: null })).toBeNull()
    expect(available('team', [])).toBe(false)
    expect(available('team', null)).toBe(false)
    expect(available('private', null)).toBe(true)
  })
  it('Bestimmte Personen schreibt nichts', () => {
    expect(apply('custom', ctx)).toBeNull()
  })
  it('Rundweg: angewandte Kurzwahl wird wieder erkannt', () => {
    for (const m of ['all', 'team', 'private'] as const) {
      const v = apply(m, ctx)!
      expect(classify(v.owner, v.set_permissions, ctx.ownerGroups)).toBe(m)
    }
  })
})

describe('Rechte zum Ändern', () => {
  it('Einzeldokument: Superuser, Besitzer oder ohne Besitzer', () => {
    expect(canChangeVisibility(5, { id: 5 })).toBe(true)
    expect(canChangeVisibility(null, { id: 9 })).toBe(true)
    expect(canChangeVisibility(5, { id: 9 })).toBe(false)
    expect(canChangeVisibility(5, { id: 9, is_superuser: true })).toBe(true)
    expect(canChangeVisibility(5, null)).toBe(false)
  })
  it('Massenbearbeitung: Besitzer aller Dokumente (oder ohne Besitzer) oder Superuser', () => {
    expect(canBulkChangeVisibility([9, null, 9], { id: 9 })).toBe(true)
    expect(canBulkChangeVisibility([9, 5], { id: 9 })).toBe(false)
    expect(canBulkChangeVisibility([9, 5], { id: 1, is_superuser: true })).toBe(true)
  })
})

describe('bulkParameters', () => {
  it('sendet owner immer ausdrücklich mit (bulk_edit setzt sonst owner = None)', () => {
    const p = bulkParameters({ owner: 9, set_permissions: emptyPerms() })
    expect(p).toEqual({ set_permissions: emptyPerms(), owner: 9, merge: false })
    expect(Object.keys(p)).toContain('owner')
    expect(bulkParameters({ owner: null, set_permissions: P([2, 2], [], [], []) }, true)).toEqual({ owner: null, merge: true, set_permissions: P([2], [], [], []) })
  })
})

describe('summarize', () => {
  const l = { owner: 'Besitzer', noOwner: 'Kein Besitzer', view: 'Ansehen', change: 'Bearbeiten', nobody: 'niemand' }
  const un = (id: number) => ({ 5: 'alice', 7: 'bob' })[id] ?? '#' + id
  const gn = (id: number) => ({ 1: 'Team A' })[id] ?? '#' + id
  it('listet Besitzer, Ansehen (ohne Doppelte aus Bearbeiten) und Bearbeiten', () => {
    expect(summarize(5, P([7], [1], [], [1]), un, gn, l)).toBe('Besitzer: alice · Ansehen: bob · Bearbeiten: Team A')
  })
  it('ohne Besitzer / ohne Rechte', () => {
    expect(summarize(null, P([7], [], [], []), un, gn, l)).toBe('Kein Besitzer · Ansehen: bob')
    expect(summarize(5, undefined, un, gn, l)).toBe('Besitzer: alice · Ansehen: niemand')
  })
  it('normPerms füllt fehlende Teile', () => {
    expect(normPerms({ view: { users: [3, 1, 3], groups: [] } } as any)).toEqual(P([1, 3], [], [], []))
  })
})
