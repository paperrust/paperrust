// Vitest: icons of the PaperRust design (docs/DESIGN2-ENTWIRREN.md §7 – "no wrong icons").
import { describe, expect, it } from 'vitest'
import { ico, iconNames, SYMBOL } from './icons'
import { applyDensity, densityOf } from './theme'
import manageHost from '../manage/ManageHost.svelte?raw'
import sections from '../manage/sections.ts?raw'
import settingsHost from '../settings/SettingsHost.svelte?raw'

const SRC: Record<string, string> = { 'manage/ManageHost.svelte': manageHost, 'manage/sections.ts': sections, 'settings/SettingsHost.svelte': settingsHost }
const src = (rel: string) => SRC[rel]

describe('Symbole', () => {
  it('jede Bedeutung hat ein vorhandenes Symbol', () => {
    const names = new Set(iconNames())
    const missing = Object.entries(SYMBOL).filter(([, n]) => !names.has(n))
    expect(missing).toEqual([])
    for (const n of Object.values(SYMBOL)) expect(ico(n)).toContain('<svg aria-hidden="true"')
  })
  it('kein Symbol steht für zwei Bedeutungen', () => {
    const seen = new Map<string, string>()
    const dup: string[] = []
    for (const [k, n] of Object.entries(SYMBOL)) {
      if (seen.has(n)) dup.push(`${n}: ${seen.get(n)} + ${k}`)
      seen.set(n, k)
    }
    expect(dup).toEqual([])
  })
  it('feste Zuordnungen laut Bauplan', () => {
    expect(SYMBOL.storagePath).toBe('folder')
    expect(SYMBOL.correspondent).toBe('person')
    expect(SYMBOL.workflow).toBe('diagram-3')
    expect(SYMBOL.mail).toBe('envelope')
    expect(SYMBOL.trash).toBe('trash')
    expect(SYMBOL.documentType).not.toBe(SYMBOL.asnLabels)
    expect([SYMBOL.documentType, SYMBOL.asnLabels]).not.toContain('hash')
  })
  it('Verwaltung und Einstellungen: nur bekannte Symbole, kein „#“, kein KI-Glitzer', () => {
    const names = new Set(iconNames())
    for (const f of ['manage/ManageHost.svelte', 'manage/sections.ts', 'settings/SettingsHost.svelte']) {
      const s = src(f)
      const used = [...s.matchAll(/(?:name=|icon: ?)['"]([a-z0-9-]+)['"]/g)].map((m) => m[1]).filter((n) => !n.startsWith('d2-'))
      expect(used.filter((n) => !names.has(n)), f).toEqual([])
      expect(used, f).not.toContain('hash')
      expect(used, f).not.toContain('stars')
    }
  })
})

describe('Dichte', () => {
  it('Standard normal, nur „compact“ ist kompakt', () => {
    expect(densityOf(undefined)).toBe('normal')
    expect(densityOf({})).toBe('normal')
    expect(densityOf({ paperrust: { density: 'compact' } })).toBe('compact')
    expect(densityOf({ paperrust: { density: 'riesig' } })).toBe('normal')
  })
  it('setzt data-density am <html>', () => {
    const attrs: Record<string, string> = {}
    const doc = { documentElement: { setAttribute: (k: string, v: string) => (attrs[k] = v) } } as unknown as Document
    applyDensity('compact', doc)
    expect(attrs['data-density']).toBe('compact')
  })
})
