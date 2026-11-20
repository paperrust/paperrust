import { describe, expect, it } from 'vitest'
import { isLocalDesktopWindow } from './desktop-local'

const docWith = (mode?: string) =>
  ({ querySelector: () => (mode === undefined ? null : { getAttribute: () => mode }) }) as unknown as Document

describe('isLocalDesktopWindow (Abmelden ausblenden)', () => {
  it('Desktop-Betrieb auf Loopback: lokales Fenster', () => {
    expect(isLocalDesktopWindow(docWith('local'), '127.0.0.1')).toBe(true)
    expect(isLocalDesktopWindow(docWith('lan'), 'localhost')).toBe(true)
  })
  it('LAN-Zugriff von einem anderen Gerät und Server-Betrieb behalten Abmelden', () => {
    expect(isLocalDesktopWindow(docWith('lan'), '192.168.1.20')).toBe(false)
    expect(isLocalDesktopWindow(docWith(undefined), '127.0.0.1')).toBe(false)
    expect(isLocalDesktopWindow(docWith(''), '127.0.0.1')).toBe(false)
  })
})
