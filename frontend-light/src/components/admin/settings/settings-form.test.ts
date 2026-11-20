import { beforeEach, describe, expect, it, vi } from 'vitest'

const stored: any[] = []
vi.mock('../../../api/objects', () => ({
  storeUiSettings: vi.fn(async (s: any) => {
    stored.push(JSON.parse(JSON.stringify(s)))
    return { success: true }
  }),
}))

const { session, SETTINGS_KEYS } = await import('../../../stores/settings.svelte')
const { EXTRA_KEYS, setting } = await import('../../../stores/settingsSave.svelte')
const { currentSettings, dateLocaleOptions, displayLanguageOptions, FIELDS, isDirty, languageOptions, saveSettings } = await import('./settings-form.svelte')

function login(settings: Record<string, any> = {}) {
  session.data = { user: { id: 7, username: 'admin', is_superuser: true }, settings, permissions: [] } as any
}

describe('settings-form (settings.component.ts)', () => {
  beforeEach(() => {
    stored.length = 0
    login({ language: 'de-de', dark_mode: { use_system: false, enabled: 'true' } })
    ;(globalThis as any).document = { cookie: '', querySelector: () => null }
  })

  it('Sprachen nach übersetztem Namen sortiert, Anzeige-/Datumslisten mit den Sondereinträgen vorne', () => {
    const l = languageOptions()
    const names = l.map((x) => x.name)
    expect([...names].sort((a, b) => (a < b ? -1 : 1))).toEqual(names)
    expect(displayLanguageOptions()[0]).toEqual({ code: '', name: 'Same as the system' })
    expect(dateLocaleOptions().slice(0, 2).map((x) => x.code)).toEqual(['', 'iso-8601'])
    // Display language: only the 10 languages with their own catalog (+ saved); date format: all languages
    expect(displayLanguageOptions('').slice(1).map((x) => x.code).sort()).toEqual(['cs-cz', 'de-de', 'en-us', 'es-es', 'fr-fr', 'it-it', 'nl-nl', 'pl-pl', 'pt-br', 'sv-se'])
    expect(displayLanguageOptions('ja-jp').map((x) => x.code)).toContain('ja-jp')
    expect(dateLocaleOptions().length - 2).toBe(l.length)
  })

  it('getCurrentSettings liest UI-Settings inkl. Standardwerten und Standard-Eigentümer = aktueller Nutzer', () => {
    const v = currentSettings()
    expect(v.displayLanguage).toBe('de-de')
    expect(v.darkModeUseSystem).toBe(false)
    expect(v.notificationsConsumerSuccess).toBe(true)
    expect(v.defaultPermsOwner).toBe(7)
    expect(v.searchLink).toBe('title-content')
    expect(Object.keys(v)).toHaveLength(FIELDS.length + 1)
  })

  it('dirtyCheck: Schlüsselreihenfolge und undefined/null egal, Wertänderung zählt', () => {
    const a = currentSettings()
    const b = Object.fromEntries(Object.entries(a).reverse())
    expect(isDirty(a, b)).toBe(false)
    expect(isDirty({ ...a, defaultPermsOwner: undefined }, { ...a, defaultPermsOwner: null })).toBe(false)
    expect(isDirty({ ...a, documentListItemPerPage: 100 }, a)).toBe(true)
  })

  it('saveSettings schreibt alle Felder (Dunkelmodus als Zeichenkette), Sprache, Cookie und meldet Neuladen bei Sprachwechsel', async () => {
    const store = currentSettings()
    const form = { ...store, darkModeEnabled: false, documentListItemPerPage: 100, displayLanguage: 'en-gb' }
    const reload = await saveSettings(form, store)
    expect(reload).toBe(true)
    expect(stored).toHaveLength(1)
    expect(stored[0].language).toBe('en-gb')
    expect(stored[0].dark_mode.enabled).toBe('false')
    expect(session.get(SETTINGS_KEYS.DOCUMENT_LIST_SIZE)).toBe(100)
    expect(setting(EXTRA_KEYS.NOTIFICATIONS_CONSUMER_FAILED)).toBe(true)
    expect((globalThis as any).document.cookie).toBe('django_language=en-gb; path=/')
  })

  it('saveSettings ohne Sprachwechsel: kein Neuladen; Update-Prüfung einschalten erzwingt Neuladen', async () => {
    const store = currentSettings()
    expect(await saveSettings({ ...store, notesEnabled: false }, store)).toBe(false)
    const s2 = currentSettings()
    expect(await saveSettings({ ...s2, updateCheckingEnabled: true }, { ...s2, updateCheckingEnabled: false })).toBe(true)
  })
})
