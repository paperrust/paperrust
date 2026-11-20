import { afterEach, describe, expect, it, vi } from 'vitest'
import { handleHotkeyKeydown, matchesKeys, onHotkeysChange, registerHotkey, registeredHotkeys } from './hotkeys-registry'
import { modals } from './modal.svelte'

const ev = (key: string, mods: Partial<Record<'altKey' | 'metaKey' | 'ctrlKey' | 'shiftKey', boolean>> = {}) =>
  ({ key, altKey: false, metaKey: false, ctrlKey: false, shiftKey: false, target: null, preventDefault: vi.fn(), ...mods }) as any

const offs: (() => void)[] = []
afterEach(() => {
  offs.splice(0).forEach((f) => f())
  modals.stack.splice(0)
})

describe('hotkeys-registry', () => {
  it('matches keys like Angular keydown.<keys> (exact modifiers, control also meta)', () => {
    expect(matchesKeys(ev('t'), 't')).toBe(true)
    expect(matchesKeys(ev('T', { shiftKey: true }), 't')).toBe(false)
    expect(matchesKeys(ev('t', { ctrlKey: true }), 't')).toBe(false)
    expect(matchesKeys(ev('s', { ctrlKey: true }), 'control.s')).toBe(true)
    expect(matchesKeys(ev('s', { metaKey: true }), 'control.s')).toBe(true)
    expect(matchesKeys(ev('S', { ctrlKey: true, shiftKey: true }), 'control.shift.s')).toBe(true)
    expect(matchesKeys(ev('s'), 'control.s')).toBe(false)
  })

  it('runs the handler, prevents default, skips disabled entries', () => {
    const a = vi.fn()
    const b = vi.fn()
    let enabled = false
    offs.push(registerHotkey({ keys: 't', description: 'Open Tags filter', handler: a, enabled: () => enabled }))
    offs.push(registerHotkey({ keys: 't', description: 'Open Tags filter (bulk)', handler: b }))
    const e = ev('t')
    handleHotkeyKeydown(e)
    expect(a).not.toHaveBeenCalled()
    expect(b).toHaveBeenCalledTimes(1)
    expect(e.preventDefault).toHaveBeenCalled()
    expect(registeredHotkeys()).toEqual([['t', 'Open Tags filter (bulk)']])
    enabled = true
    handleHotkeyKeydown(ev('t'))
    expect(a).toHaveBeenCalledTimes(1)
    expect(registeredHotkeys()).toEqual([['t', 'Open Tags filter']])
  })

  it('ignores events while a modal is open; unregister removes the entry and notifies', () => {
    const h = vi.fn()
    const changed = vi.fn()
    offs.push(onHotkeysChange(changed))
    const off = registerHotkey({ keys: 'y', description: 'Open Correspondent filter', handler: h })
    expect(changed).toHaveBeenCalledTimes(1)
    modals.stack.push({} as any)
    handleHotkeyKeydown(ev('y'))
    expect(h).not.toHaveBeenCalled()
    modals.stack.splice(0)
    handleHotkeyKeydown(ev('y'))
    expect(h).toHaveBeenCalledTimes(1)
    off()
    expect(changed).toHaveBeenCalledTimes(2)
    expect(registeredHotkeys()).toEqual([])
    handleHotkeyKeydown(ev('y'))
    expect(h).toHaveBeenCalledTimes(1)
  })
})
