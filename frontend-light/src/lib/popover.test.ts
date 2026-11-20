// Popover autoClose (test log A-017/A-087): a real mouse click on a button in the popover runs
// pointerdown → mousedown → pointerup → mouseup → click. The window may only disappear after the click.
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { Popover } from './popover.svelte'

type L = (e: { target: unknown; key?: string }) => void
class FakeDoc {
  listeners = new Map<string, Set<L>>()
  addEventListener(t: string, f: L) {
    if (!this.listeners.has(t)) this.listeners.set(t, new Set())
    this.listeners.get(t)!.add(f)
  }
  removeEventListener(t: string, f: L) {
    this.listeners.get(t)?.delete(f)
  }
  fire(t: string, target: unknown) {
    for (const f of [...(this.listeners.get(t) ?? [])]) f({ target })
  }
}

const tick = () => new Promise((r) => setTimeout(r, 0))
let doc: FakeDoc
const g = globalThis as unknown as { document?: unknown }
let saved: unknown

beforeEach(() => {
  saved = g.document
  doc = new FakeDoc()
  g.document = doc
})
afterEach(() => {
  g.document = saved
})

const wins = new Map<Popover, HTMLElement>()
const win = (p: Popover) => wins.get(p)

function setup(autoClose: boolean | 'inside' | 'outside' = true) {
  const p = new Popover({ autoClose, triggers: 'manual' })
  const yes = { id: 'yes' }
  const outside = { id: 'outside' }
  const win = { style: {} as Record<string, string>, contains: (n: unknown) => n === yes || n === win } as unknown as HTMLElement
  p.show()
  const action = p.windowEl(win)
  wins.set(p, win)
  return { p, yes, outside, action }
}

/** Order of a real mouse click; the click handler of the button runs only while the window is open. */
function realClick(p: Popover, target: unknown, onclick: () => void) {
  doc.fire('pointerdown', target)
  doc.fire('mousedown', target)
  doc.fire('pointerup', target)
  doc.fire('mouseup', target)
  if (p.open) onclick()
  doc.fire('click', target)
}

describe('Popover autoClose', () => {
  it('Klick auf „Ja“ im Popover erreicht den click-Handler, Fenster schließt danach', async () => {
    const { p, yes, action } = setup(true)
    await tick()
    let clicked = 0
    realClick(p, yes, () => clicked++)
    expect(clicked).toBe(1)
    expect((win(p) as any).style.position).toBe('absolute') // via CSSOM instead of style attribute (CSP)
    await tick()
    expect(p.open).toBe(false)
    action.destroy()
  })

  it('Klick außerhalb schließt (nach dem click)', async () => {
    const { p, outside, action } = setup(true)
    await tick()
    realClick(p, outside, () => {})
    expect(p.open).toBe(true)
    await tick()
    expect(p.open).toBe(false)
    action.destroy()
  })

  it("autoClose 'outside': Klick innen lässt offen", async () => {
    const { p, yes, action } = setup('outside')
    await tick()
    realClick(p, yes, () => {})
    await tick()
    expect(p.open).toBe(true)
    action.destroy()
  })

  it('destroy vor Ablauf des Schließ-Timers wirft nicht und lässt keine Handler zurück', async () => {
    const { p, outside, action } = setup(true)
    await tick()
    realClick(p, outside, () => {})
    action.destroy()
    await tick()
    expect([...doc.listeners.values()].every((s) => s.size === 0)).toBe(true)
    expect(p.open).toBe(true)
  })
})
