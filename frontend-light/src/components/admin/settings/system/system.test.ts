import { describe, expect, it } from 'vitest'
import type { SettingItem } from './api'
import { setDictionary } from '../../../../lib/i18n'
import { collectChanges, cronText, displayKey, displayName, editable, fileName, formerNames, fromForm, groupOf, helpText, matches, toForm } from './system'

function item(over: Partial<SettingItem>): SettingItem {
  return {
    key: 'PAPERLESS_EMAIL_HOST',
    name: 'PAPERRUST_EMAIL_HOST',
    aliases: ['PAPERLESS_EMAIL_HOST'],
    alias_of: null,
    category: 'email',
    class: 'A',
    effect: 'live',
    reindex: false,
    secret: false,
    type: { kind: 'str' },
    default: 'localhost',
    value: 'localhost',
    is_set: false,
    unreadable: false,
    source: 'default',
    source_detail: null,
    shadowed: null,
    locked: false,
    lock_reason: null,
    restart_pending: false,
    django_consumer: true,
    appconfig_field: null,
    in_db: false,
    updated_at: null,
    updated_by: null,
    help: 'SMTP server',
    doc: '',
    ...over,
  }
}

const F = { q: '', changed: false, restart: false, readOnly: false }

describe('system settings tab', () => {
  it('groups and filters', () => {
    expect(groupOf(item({ category: 'webhooks' }))).toBe('network')
    expect(groupOf(item({ category: 'paths' }))).toBe('storage')
    expect(matches(item({}), { ...F, q: 'smtp' })).toBe(true)
    expect(matches(item({}), { ...F, q: 'paperrust_email' })).toBe(true)
    expect(matches(item({}), { ...F, q: 'nope' })).toBe(false)
    expect(matches(item({ class: 'C', locked: true }), F)).toBe(false)
    expect(matches(item({ class: 'C', locked: true }), { ...F, readOnly: true })).toBe(true)
    // locked from environment, but A: visible
    expect(matches(item({ locked: true, source: 'env' }), F)).toBe(true)
    expect(matches(item({ source: 'db' }), { ...F, changed: true })).toBe(true)
    expect(matches(item({}), { ...F, changed: true })).toBe(false)
    expect(matches(item({ restart_pending: true }), { ...F, restart: true })).toBe(true)
    expect(editable(item({ locked: true }))).toBe(false)
  })

  it('shows the help text in the UI language', () => {
    const i = item({ help: 'SMTP-Server', help_en: 'SMTP server' })
    expect(helpText(i, 'de-DE')).toBe('SMTP-Server')
    expect(helpText(i, 'de-CH')).toBe('SMTP-Server')
    setDictionary('en-US', {})
    expect(helpText(i, 'en-US')).toBe('SMTP server')
    // other languages: English text as dictionary key
    setDictionary('fr-FR', { 'SMTP server': 'Serveur SMTP' })
    expect(helpText(i, 'fr-FR')).toBe('Serveur SMTP')
    setDictionary('en-US', {})
    // older server without help_en
    expect(helpText(item({ help: 'SMTP-Server' }), 'en-US')).toBe('SMTP-Server')
    // Search finds both languages
    expect(matches(i, { ...F, q: 'smtp-server' })).toBe(true)
    expect(matches(i, { ...F, q: 'smtp server' })).toBe(true)
  })

  it('converts form values', () => {
    const port = item({ type: { kind: 'int', min: 1, max: 65535 }, value: 25 })
    expect(toForm(port)).toBe('25')
    expect(fromForm(port, '587')).toEqual({ ok: true, value: 587 })
    expect(fromForm(port, '0').ok).toBe(false)
    expect(fromForm(port, 'x').ok).toBe(false)
    const b = item({ type: { kind: 'bool' }, value: true })
    expect(toForm(b)).toBe(true)
    expect(fromForm(b, false)).toEqual({ ok: true, value: false })
    const l = item({ type: { kind: 'list' }, value: ['a', 'b'] })
    expect(toForm(l)).toBe('a, b')
    expect(fromForm(l, 'a, ,c')).toEqual({ ok: true, value: ['a', 'c'] })
    const j = item({ type: { kind: 'json', shape: 'object' }, value: { a: 1 } })
    expect(fromForm(j, '[1]').ok).toBe(false)
    expect(fromForm(j, '{"b":2}')).toEqual({ ok: true, value: { b: 2 } })
    const c = item({ type: { kind: 'choice', choices: ['x', 'y'] } })
    expect(fromForm(c, 'z').ok).toBe(false)
    // Secret: empty = unchanged, never prefilled
    const s = item({ secret: true, value: null, is_set: true })
    expect(toForm(s)).toBe('')
    expect(fromForm(s, '')).toEqual({ ok: true, skip: true })
    expect(fromForm(s, 'pw')).toEqual({ ok: true, value: 'pw' })
  })

  it('collects only changed editable values', () => {
    const items = [item({}), item({ key: 'PAPERLESS_EMAIL_PORT', type: { kind: 'int', min: 1, max: 65535 }, value: 25 }), item({ key: 'L', locked: true })]
    const initial = { PAPERLESS_EMAIL_HOST: 'localhost', PAPERLESS_EMAIL_PORT: '25', L: 'x' }
    const r = collectChanges(items, { PAPERLESS_EMAIL_HOST: 'smtp', PAPERLESS_EMAIL_PORT: 'abc', L: 'y' }, initial)
    expect(r.values).toEqual({ PAPERLESS_EMAIL_HOST: 'smtp' })
    expect(Object.keys(r.errors)).toEqual(['PAPERLESS_EMAIL_PORT'])
  })

  it('describes crontabs', () => {
    expect(cronText('0 1 * * *')).toEqual({ text: 'admin.settings.every_day', params: { PH: '01:00' } })
    expect(cronText('30 0 * * sun')).toEqual({ text: 'admin.settings.on_at', params: { PH: 'sun', PH_1: '00:30' } })
    expect(cronText('*/10 * * * *')?.params).toEqual({ PH: '10' })
    expect(cronText('5 */1 * * *')?.text).toBe('admin.settings.every_hour_minute')
    expect(cronText('disable')?.text).toBe('admin.settings.disabled')
    expect(cronText('1-5 2 * * *')).toBe(null)
  })

  it('shows PAPERRUST_ names, the old name only as former name', () => {
    const i = item({})
    expect(displayName(i)).toBe('PAPERRUST_EMAIL_HOST')
    expect(formerNames(i)).toEqual(['PAPERLESS_EMAIL_HOST'])
    // older server without name
    expect(displayName(item({ name: undefined, aliases: ['PAPERRUST_EMAIL_HOST'] }))).toBe('PAPERRUST_EMAIL_HOST')
    expect(displayKey('PAPERLESS_OCR_MODE')).toBe('PAPERRUST_OCR_MODE')
    expect(displayKey('PAPERLESS_RS_LOG')).toBe('PAPERRUST_LOG')
    expect(displayKey('desktop.server.port')).toBe('desktop.server.port')
    expect(displayKey('AC:output_type')).toBe('AC:output_type')
    // Search finds both names
    expect(matches(i, { ...F, q: 'paperrust_email' })).toBe(true)
    expect(matches(i, { ...F, q: 'paperless_email' })).toBe(true)
  })

  it('names the configuration file that was read', () => {
    expect(fileName('/etc/paperrust.conf')).toBe('paperrust.conf')
    expect(fileName('C:\\data\\paperless.conf')).toBe('paperless.conf')
    expect(fileName(null)).toBe('paperrust.conf')
  })
})
