// Vitest: sidebar collapsed by the user (lib/navrail.ts)
import { describe, expect, it } from 'vitest'
import { isRail, NAV_COLLAPSED_KEY, readNavCollapsed, writeNavCollapsed } from './navrail'

function mem() {
  const m = new Map<string, string>()
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
    m,
  }
}
const fail = () => {
  throw new Error('denied')
}

describe('Seitenleiste einklappen', () => {
  it('pro Browser gespeichert, Standard ausgeklappt', () => {
    const s = mem()
    expect(readNavCollapsed(s)).toBe(false)
    writeNavCollapsed(true, s)
    expect(s.m.get(NAV_COLLAPSED_KEY)).toBe('1')
    expect(readNavCollapsed(s)).toBe(true)
    writeNavCollapsed(false, s)
    expect(s.m.has(NAV_COLLAPSED_KEY)).toBe(false)
    expect(readNavCollapsed(s)).toBe(false)
  })
  it('Speicher fehlt oder wirft: kein Fehler, ausgeklappt', () => {
    const bad = { getItem: fail, setItem: fail, removeItem: fail }
    expect(readNavCollapsed(bad)).toBe(false)
    expect(() => writeNavCollapsed(true, bad)).not.toThrow()
    expect(readNavCollapsed(null)).toBe(false)
  })
  it('Leiste: Tablet-Breite oder eingeklappt, nie auf dem Telefon', () => {
    expect(isRail(false, false, false)).toBe(false)
    expect(isRail(false, false, true)).toBe(true)
    expect(isRail(false, true, false)).toBe(true)
    expect(isRail(true, true, true)).toBe(false)
  })
})
