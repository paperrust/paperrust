// Old Angular addresses (app-routing.module.ts) land on the same route in the own UI.
import { describe, expect, it } from 'vitest'
import { stripBase } from './router.svelte'

describe('stripBase: Angular-Weiterleitungen', () => {
  const s = (p: string) => stripBase(p, '/rs/app')
  it('Wurzel und Verwaltungs-Kurzpfade', () => {
    expect(s('/rs/app/')).toBe('dashboard')
    expect(s('/rs/app')).toBe('dashboard')
    expect(s('/rs/app/tags')).toBe('attributes/tags')
    expect(s('/rs/app/customfields')).toBe('attributes/customfields')
    expect(s('/rs/app/settings/mail')).toBe('mail')
    expect(s('/rs/app/settings/savedviews')).toBe('savedviews')
  })
  it('documentproperties (ältere Angular-Fassung)', () => {
    expect(s('/rs/app/documentproperties')).toBe('attributes')
    expect(s('/rs/app/documentproperties/storagepaths')).toBe('attributes/storagepaths')
  })
  it('übrige Pfade bleiben', () => {
    expect(s('/rs/app/documents/12/details/')).toBe('documents/12/details')
    expect(s('/rs/app/view/3')).toBe('view/3')
    expect(s('/rs/app/asn/42')).toBe('asn/42')
  })
})
