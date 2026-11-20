import { describe, expect, it } from 'vitest'
import { activeHotkeys, formatKey, ignoreHotkeyEvent, matchAppHotkey, matchListHotkey } from './hotkeys'

const ev = (key: string, mods: Partial<Record<'altKey' | 'metaKey' | 'ctrlKey' | 'shiftKey', boolean>> = {}) => ({
  key,
  altKey: false,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  target: null,
  ...mods,
})

describe('hotkeys', () => {
  it('formatKey like HotkeyDialogComponent', () => {
    expect(formatKey('control.arrowright')).toBe('⌃ →')
    expect(formatKey('control.arrowright', true)).toBe('⌘ →')
    expect(formatKey('control.shift.s')).toBe('⌃ ⇧ s')
    expect(formatKey('escape')).toBe('escape')
  })
  it('matches app shortcuts without modifiers', () => {
    expect(matchAppHotkey(ev('?', { shiftKey: true }))).toBe('help')
    expect(matchAppHotkey(ev('h'))).toBe('dashboard')
    expect(matchAppHotkey(ev('d'))).toBe('documents')
    expect(matchAppHotkey(ev('s'))).toBe('settings')
    expect(matchAppHotkey(ev('s', { ctrlKey: true }))).toBeNull()
    expect(matchAppHotkey(ev('S', { shiftKey: true }))).toBeNull()
    expect(matchAppHotkey(ev('x'))).toBeNull()
  })
  it('does not ignore events outside inputs', () => {
    expect(ignoreHotkeyEvent(ev('h'))).toBe(false)
  })
  it('lists active shortcuts by route and permission', () => {
    const all = { viewDocuments: true, changeSettings: true }
    expect(activeHotkeys('dashboard', all).map((k) => k[0])).toEqual(['h', 'd', 's'])
    expect(activeHotkeys('dashboard', { viewDocuments: false, changeSettings: false }).map((k) => k[0])).toEqual(['h'])
    expect(activeHotkeys('documents/12/details', all).map((k) => k[0])).toEqual([
      'h', 'd', 's', 'control.arrowright', 'control.arrowleft', 'escape', 'control.s', 'control.shift.s',
    ])
    const list = ['h', 'd', 's', 'escape', 'a', 'p', 'o', 'control.arrowleft', 'control.arrowright']
    expect(activeHotkeys('documents', all).map((k) => k[0])).toEqual(list)
    expect(activeHotkeys('view/3', all).map((k) => k[0])).toEqual(list)
    expect(activeHotkeys('view/3/x', all)).toHaveLength(3)
  })
  it('matches document list shortcuts', () => {
    expect(matchListHotkey(ev('Escape'))).toBe('reset')
    expect(matchListHotkey(ev('a'))).toBe('selectAll')
    expect(matchListHotkey(ev('p'))).toBe('selectPage')
    expect(matchListHotkey(ev('o'))).toBe('open')
    expect(matchListHotkey(ev('ArrowLeft', { ctrlKey: true }))).toBe('previousPage')
    expect(matchListHotkey(ev('ArrowRight', { metaKey: true }))).toBe('nextPage')
    expect(matchListHotkey(ev('ArrowRight'))).toBeNull()
    expect(matchListHotkey(ev('a', { ctrlKey: true }))).toBeNull()
    expect(matchListHotkey(ev('A', { shiftKey: true }))).toBeNull()
    expect(matchListHotkey(ev('Escape', { shiftKey: true }))).toBeNull()
  })
})
