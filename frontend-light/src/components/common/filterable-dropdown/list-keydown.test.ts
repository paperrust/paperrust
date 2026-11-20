import { describe, expect, it, vi } from 'vitest'
import { ListKeyboard } from './list-keydown'

const el = (tagName: string, extra: Record<string, unknown> = {}) => ({ tagName, focus: vi.fn(), ...extra })
const ev = (key: string, target: unknown, shiftKey = false) => ({ key, target: target as EventTarget, shiftKey, preventDefault: vi.fn() })

describe('ListKeyboard (upstream listKeyDown)', () => {
  it('ArrowDown im leeren Filterfeld fokussiert den ersten Eintrag', () => {
    const k = new ListKeyboard()
    const input = el('INPUT', { selectionStart: 0 })
    const b = [el('BUTTON'), el('BUTTON')]
    const e = ev('ArrowDown', input)
    k.keyDown(e, input, b, '')
    expect(b[0].focus).toHaveBeenCalled()
    expect(e.preventDefault).toHaveBeenCalled()
    expect(k.index).toBe(0)
  })

  it('ArrowDown im Filterfeld nur mit Cursor am Ende', () => {
    const k = new ListKeyboard()
    const b = [el('BUTTON')]
    const mid = el('INPUT', { selectionStart: 1 })
    k.keyDown(ev('ArrowDown', mid), mid, b, 'abc')
    expect(b[0].focus).not.toHaveBeenCalled()
    const end = el('INPUT', { selectionStart: 3 })
    k.keyDown(ev('ArrowDown', end), end, b, 'abc')
    expect(b[0].focus).toHaveBeenCalled()
  })

  it('ArrowDown/ArrowUp wandern zwischen Einträgen, am Ende bleibt der letzte', () => {
    const k = new ListKeyboard()
    const input = el('INPUT')
    const b = [el('BUTTON'), el('BUTTON'), el('BUTTON')]
    k.keyDown(ev('ArrowDown', b[1]), input, b, '')
    expect(b[2].focus).toHaveBeenCalledTimes(1)
    k.keyDown(ev('ArrowDown', b[2]), input, b, '')
    expect(b[2].focus).toHaveBeenCalledTimes(2)
    k.keyDown(ev('ArrowUp', b[2]), input, b, '')
    expect(b[1].focus).toHaveBeenCalled()
  })

  it('ArrowUp auf dem ersten Eintrag kehrt ins Filterfeld zurück', () => {
    const k = new ListKeyboard()
    const input = el('INPUT')
    const b = [el('BUTTON'), el('BUTTON')]
    const e = ev('ArrowUp', b[0])
    k.keyDown(e, input, b, '')
    expect(input.focus).toHaveBeenCalled()
    expect(e.preventDefault).toHaveBeenCalled()
  })

  it('Tab führt nur den Index mit, Enter/Leertaste bleiben beim Knopf', () => {
    const k = new ListKeyboard()
    const input = el('INPUT')
    const b = [el('BUTTON'), el('BUTTON')]
    k.keyDown(ev('Tab', input), input, b, '')
    expect(k.index).toBe(0)
    k.keyDown(ev('Tab', b[0]), input, b, '')
    expect(k.index).toBe(1)
    k.keyDown(ev('Tab', b[1], true), input, b, '')
    expect(k.index).toBe(0)
    const enter = ev('Enter', b[0])
    k.keyDown(enter, input, b, '')
    k.keyDown(ev(' ', b[0]), input, b, '')
    expect(enter.preventDefault).not.toHaveBeenCalled()
    expect(b.every((x) => x.focus.mock.calls.length === 0)).toBe(true)
  })
})
