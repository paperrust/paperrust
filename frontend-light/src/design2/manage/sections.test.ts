// Vitest: visibility of the administration areas (docs/DESIGN2-ENTWIRREN.md §6) and tabs of the settings.
import { describe, expect, it } from 'vitest'
import { allowedSections, fetchRolesMode, rolesModeOf, sectionFor, visibleSections, type SectionDef, type Viewer } from './sections'
import { settingsTab } from '../settings/tabs'

const S: SectionDef[] = [
  { path: 'attributes/tags', also: ['attributes'], label: 'tags', icon: 'tags', area: 'order', perm: 'tag' },
  { path: 'savedviews', label: 'views', icon: 'bookmark', area: 'order', perm: 'savedview' },
  { path: 'usersgroups', label: 'users', icon: 'people', area: 'system', perm: 'user', adminOnly: true },
  { path: 'roles', label: 'roles', icon: 'person-badge', area: 'system', perm: 'user', adminOnly: true, customRolesOnly: true },
  { path: 'workflows', label: 'workflows', icon: 'diagram-3', area: 'system', perm: 'workflow' },
  { path: 'mail', label: 'mail', icon: 'envelope', area: 'system', perm: 'mailaccount' },
  { path: 'logs', label: 'logs', icon: 'journal-text', area: 'system', perm: null, adminOnly: true },
  { path: 'trash', label: 'trash', icon: 'trash', area: 'system', perm: 'document', permAction: 'delete' },
  { path: 'config', also: ['settings/system'], label: 'config', icon: 'sliders', area: 'system', perm: 'applicationconfiguration', adminOnly: true },
]
const viewer = (perms: string[], admin: boolean, rolesMode: Viewer['rolesMode'] = 'classic'): Viewer => ({ can: (p, a) => perms.includes(a === 'view' ? p : `${a}:${p}`), admin, rolesMode })
const paths = (v: Viewer) => visibleSections(S, v).map((s) => s.path)
const routes = (v: Viewer) => allowedSections(S, v).map((s) => s.path)

describe('Verwalten: Sichtbarkeit', () => {
  it('Benutzer ohne Admin: Benutzer, Rollen, Protokolle, Konfiguration bleiben verborgen, auch mit Recht', () => {
    expect(paths(viewer(['tag', 'user', 'applicationconfiguration'], false))).toEqual(['attributes/tags'])
    expect(routes(viewer(['tag', 'user', 'applicationconfiguration'], false, 'custom'))).toEqual(['attributes/tags'])
  })
  it('Rolle „Verwalten“ ohne Admin sieht Workflows, Mail und Papierkorb', () => {
    expect(paths(viewer(['tag', 'savedview', 'workflow', 'mailaccount', 'document', 'delete:document'], false))).toEqual(['attributes/tags', 'savedviews', 'workflows', 'mail', 'trash'])
    // Trash like upstream only with "Delete document" (role "Read only" does not see it)
    expect(paths(viewer(['document'], false))).toEqual([])
    expect(paths(viewer(['document', 'delete:document'], false))).toEqual(['trash'])
  })
  it('ohne Rechte nichts', () => {
    expect(paths(viewer([], false))).toEqual([])
  })
  it('Administrator sieht „System“, aber nur Bereiche mit Recht; Rollen in der Navigation nur im Modus benutzerdefiniert', () => {
    expect(paths(viewer(['tag', 'savedview', 'user'], true))).toEqual(['attributes/tags', 'savedviews', 'usersgroups', 'logs'])
    expect(paths(viewer(['user', 'applicationconfiguration'], true, 'custom'))).toEqual(['usersgroups', 'roles', 'logs', 'config'])
  })
  it('Rollen-Route ist für Administratoren in beiden Modi erreichbar', () => {
    expect(routes(viewer(['user'], true, 'classic'))).toContain('roles')
    expect(routes(viewer(['user'], true, 'custom'))).toContain('roles')
    expect(sectionFor(allowedSections(S, viewer(['user'], true)), 'roles')?.path).toBe('roles')
    expect(routes(viewer(['user'], false, 'custom'))).not.toContain('roles')
  })
  it('Route → Bereich (exakt, Präfix, also)', () => {
    expect(sectionFor(S, 'attributes')?.path).toBe('attributes/tags')
    expect(sectionFor(S, 'attributes/tags')?.path).toBe('attributes/tags')
    expect(sectionFor(S, 'settings/system/mail')?.path).toBe('config')
    expect(sectionFor(S, 'settings/systemx')).toBeUndefined()
    expect(sectionFor(S, 'documents')).toBeUndefined()
  })
})

describe('Rollenmodus', () => {
  it('nur ausdrücklich „custom“ ist benutzerdefiniert; 404/Fehler = klassisch', () => {
    expect(rolesModeOf(200, { mode: 'custom' })).toBe('custom')
    expect(rolesModeOf(200, { mode: 'classic' })).toBe('classic')
    expect(rolesModeOf(200, null)).toBe('classic')
    expect(rolesModeOf(404, { mode: 'custom' })).toBe('classic')
    expect(rolesModeOf(403, null)).toBe('classic')
  })
  it('fetchRolesMode: Netzfehler und 404 → klassisch', async () => {
    const ok = (async () => new Response(JSON.stringify({ mode: 'custom' }), { status: 200 })) as unknown as typeof fetch
    const nf = (async () => new Response('not found', { status: 404 })) as unknown as typeof fetch
    const bad = (async () => {
      throw new Error('offline')
    }) as unknown as typeof fetch
    expect(await fetchRolesMode(ok)).toBe('custom')
    expect(await fetchRolesMode(nf)).toBe('classic')
    expect(await fetchRolesMode(bad)).toBe('classic')
  })
})

describe('Einstellungen: Reiter', () => {
  it('vier Reiter, klassische Abschnitte unter „Dokumente“', () => {
    expect(settingsTab('')).toBe('display')
    expect(settingsTab('interface')).toBe('display')
    expect(settingsTab('documents')).toBe('documents')
    expect(settingsTab('permissions')).toBe('documents')
    expect(settingsTab('general')).toBe('documents')
    expect(settingsTab('notifications')).toBe('notifications')
    expect(settingsTab('account')).toBe('account')
    expect(settingsTab('unbekannt')).toBe('display')
  })
})
