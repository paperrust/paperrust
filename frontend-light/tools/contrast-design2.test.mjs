// Contrast of the design tokens of the PaperRust design (WCAG 2.x, text ≥ 4.5:1, large/bold controls ≥ 3:1) – light and dark,
// read directly from styles/tokens.css (docs/DESIGN2.md §2).
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const css = fs.readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src/design2/styles/tokens.css'), 'utf8')
function block(sel) {
  const i = css.indexOf(sel)
  const body = css.slice(css.indexOf('{', i) + 1, css.indexOf('}', i))
  return Object.fromEntries([...body.matchAll(/--(d2-[\w-]+):\s*(#[0-9a-f]{6})/gi)].map((m) => [m[1], m[2]]))
}
const light = block(':root {')
const dark = { ...light, ...block(":root[data-theme='dark'] {") }

const lum = (hex) => {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
export const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p)
  return (x + 0.05) / (y + 0.05)
}

/** [foreground, background, minimum value] */
const PAIRS = [
  ['d2-text', 'd2-bg', 4.5],
  ['d2-text', 'd2-surface', 4.5],
  ['d2-text', 'd2-surface-3', 4.5],
  ['d2-text-2', 'd2-surface', 4.5],
  ['d2-text-2', 'd2-bg', 4.5],
  ['d2-text-2', 'd2-surface-2', 4.5],
  ['d2-text-3', 'd2-surface', 4.5],
  ['d2-text-3', 'd2-bg', 4.5],
  ['d2-text-3', 'd2-surface-2', 4.5],
  ['d2-accent-text', 'd2-surface', 4.5],
  ['d2-accent-text', 'd2-accent-soft', 4.5],
  ['d2-accent-text', 'd2-bg', 4.5],
  ['d2-on-accent', 'd2-accent', 4.5],
  ['d2-on-accent', 'd2-accent-hover', 4.5],
  ['d2-danger', 'd2-surface', 4.5],
  ['d2-ok', 'd2-surface', 4.5],
  ['d2-warn', 'd2-warn-soft', 4.5],
  ['d2-warn', 'd2-surface', 4.5],
  ['d2-info', 'd2-surface', 4.5],
  ['d2-accent', 'd2-surface', 3], // Focus rings, brands, active lines
  ['d2-border-strong', 'd2-surface', 1.9], // Input field border (plus label/focus)
]

describe('Kontrast Design-Tokens', () => {
  for (const [name, t] of [
    ['hell', light],
    ['dunkel', dark],
  ])
    it(name, () => {
      const bad = PAIRS.filter(([f, b, min]) => ratio(t[f], t[b]) < min).map(([f, b, min]) => `${f} auf ${b}: ${ratio(t[f], t[b]).toFixed(2)} < ${min}`)
      expect(bad).toEqual([])
    })
  it('Akzent ist Rost #B7410E', () => {
    expect(light['d2-accent'].toLowerCase()).toBe('#b7410e')
    expect(dark['d2-accent'].toLowerCase()).toBe('#b7410e')
  })
})
