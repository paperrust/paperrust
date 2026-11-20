// Esc in the side sheet after an open selection list (Menu/ObjectPicker): focus rules from lib/focus.ts.
import { describe, expect, it } from 'vitest'
import { focusLost, menuFocusBack, SheetStack, sheetCatchesEscape } from './focus'

const body = { isConnected: true, tag: 'body' }
const node = (connected = true) => ({ isConnected: connected })
const menuOf = (...inside: object[]) => ({ contains: (x: unknown) => inside.includes(x as object) })

describe('menuFocusBack', () => {
  const trigger = node()
  it('Fokus auf <body> nach Entfernen des Menüs (Svelte 5: DOM vor Abbau weg) → zurück zum Auslöser', () => {
    // exactly the failure case: search field of the selection list had focus, menu is already removed
    expect(menuFocusBack(body, body, menuOf(), trigger)).toBe(trigger)
    expect(menuFocusBack(null, body, menuOf(), trigger)).toBe(trigger)
  })
  it('Fokus auf einem schon entfernten Element → zurück zum Auslöser', () => {
    expect(menuFocusBack(node(false), body, menuOf(), trigger)).toBe(trigger)
  })
  it('Fokus noch im Menü → zurück zum Auslöser', () => {
    const input = node()
    expect(menuFocusBack(input, body, menuOf(input), trigger)).toBe(trigger)
  })
  it('Nutzer hat woanders hingeklickt (Fokus auf anderem Element) → Fokus nicht anfassen', () => {
    expect(menuFocusBack(node(), body, menuOf(), trigger)).toBeNull()
  })
  it('Auslöser fehlt oder ist schon entfernt → nichts', () => {
    expect(menuFocusBack(body, body, menuOf(), null)).toBeNull()
    expect(menuFocusBack(body, body, menuOf(), node(false))).toBeNull()
  })
})

describe('Seitenblatt fängt Esc ohne Fokus ab', () => {
  it('nur das oberste Blatt, nur Esc, nur wenn der Fokus verloren ist', () => {
    const st = new SheetStack<object>()
    const a = {}
    const b = {}
    const popA = st.push(a)
    const popB = st.push(b)
    expect(st.isTop(b)).toBe(true)
    expect(st.isTop(a)).toBe(false)
    expect(sheetCatchesEscape('Escape', body, body, st.isTop(b))).toBe(true)
    expect(sheetCatchesEscape('Escape', body, body, st.isTop(a))).toBe(false)
    expect(sheetCatchesEscape('Enter', body, body, true)).toBe(false)
    // Focus in the sheet: the sheet itself (onkeydown) or the menu handles Esc, not the catcher
    expect(sheetCatchesEscape('Escape', node(), body, true)).toBe(false)
    popB()
    expect(st.isTop(a)).toBe(true)
    popA()
    expect(st.isTop(a)).toBe(false)
  })
  it('focusLost', () => {
    expect(focusLost(undefined, body)).toBe(true)
    expect(focusLost(body, body)).toBe(true)
    expect(focusLost(node(false), body)).toBe(true)
    expect(focusLost(node(), body)).toBe(false)
  })
})
