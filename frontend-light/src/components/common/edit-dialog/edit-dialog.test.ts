import { beforeEach, describe, expect, it, vi } from 'vitest'

const calls: any[] = []
let fail: any = null
vi.mock('../../../api/objects', () => ({
  createObject: vi.fn(async (ep: string, o: any) => {
    calls.push(['POST', ep, JSON.parse(JSON.stringify(o))])
    if (fail) throw fail
    return { ...o, id: 99 }
  }),
  updateObject: vi.fn(async (ep: string, o: any) => {
    calls.push(['PUT', ep, JSON.parse(JSON.stringify(o))])
    if (fail) throw fail
    return o
  }),
}))

const { session } = await import('../../../stores/settings.svelte')
const { EditDialogState, patchForm, defaultPermissionsForm } = await import('./edit-dialog.svelte')

const modal = () => ({ close: vi.fn() }) as any
const DEFAULTS = { name: '', match: '', matching_algorithm: 6, is_insensitive: true, permissions_form: null }

describe('EditDialogState (edit-dialog.component.ts)', () => {
  beforeEach(() => {
    calls.length = 0
    fail = null
    session.data = {
      user: { id: 2, username: 'bob', is_superuser: false },
      settings: { permissions: { default_owner: null, default_view_users: [3], default_edit_groups: [1] } },
      permissions: [],
    } as any
  })

  it('patchForm übernimmt nur bekannte Schlüssel', () => {
    const f: any = { a: 1, b: 2 }
    patchForm(f, { b: 5, c: 9 })
    expect(f).toEqual({ a: 1, b: 5 })
  })

  it('Anlegen: Standardrechte aus den UI-Settings, POST mit owner/set_permissions statt permissions_form', async () => {
    expect(defaultPermissionsForm().set_permissions.view.users).toEqual([3])
    const props = { modal: modal(), dialogMode: 'create' as const, onsucceeded: vi.fn() }
    const d = new EditDialogState('tags', props, DEFAULTS)
    d.form.name = 'Neu'
    await d.save()
    expect(calls[0][0]).toBe('POST')
    expect(calls[0][2]).toMatchObject({ name: 'Neu', owner: null, set_permissions: { view: { users: [3], groups: [] }, change: { users: [], groups: [1] } } })
    expect(calls[0][2]).not.toHaveProperty('permissions_form')
    expect(props.modal.close).toHaveBeenCalled()
    expect(props.onsucceeded).toHaveBeenCalledWith(expect.objectContaining({ id: 99 }))
  })

  it('Bearbeiten fremder Objekte: keine Rechte senden; Fehler je Feld bleiben im Dialog', async () => {
    const object = { id: 5, name: 'Alt', owner: 3, permissions: { view: { users: [], groups: [] }, change: { users: [], groups: [] } }, user_can_change: true }
    const props = { modal: modal(), dialogMode: 'edit' as const, object, onfailed: vi.fn() }
    const d = new EditDialogState('correspondents', props, DEFAULTS)
    expect(d.form.name).toBe('Alt')
    expect(d.form.permissions_form).toEqual({ owner: 3, set_permissions: object.permissions })
    expect(d.userIsOwner).toBe(false)
    fail = { status: 400, body: { name: ['schon vorhanden'] } }
    await d.save()
    expect(calls[0][0]).toBe('PUT')
    expect(calls[0][2]).not.toHaveProperty('set_permissions')
    expect(d.error).toEqual({ name: ['schon vorhanden'] })
    expect(d.networkActive).toBe(false)
    expect(props.modal.close).not.toHaveBeenCalled()
    expect(props.onfailed).toHaveBeenCalled()
  })
})
