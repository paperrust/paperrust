// Core and lazily loaded icons (tools/icons.mjs shares the list for the budget of the first call).
import { describe, expect, it } from 'vitest'
import { icon } from './icon'
import { icons } from './icons.generated'
import { extraIcons } from './icons-extra.generated'
import { extraIconState, loadExtraIcons } from './icon-extra.svelte'

describe('icon', () => {
  it('Kern und Nachlade-Liste sind getrennt und nicht leer', () => {
    const core = Object.keys(icons)
    const extra = Object.keys(extraIcons)
    expect(core.length).toBeGreaterThan(0)
    expect(extra.length).toBeGreaterThan(0)
    expect(extra.filter((n) => core.includes(n))).toEqual([])
  })

  it('Kernsymbol sofort, nachgeladenes Symbol nach dem Laden (mit Größe), unbekanntes leer', async () => {
    const coreName = Object.keys(icons)[0]
    expect(icon(coreName)).toContain('<svg')
    const extraName = Object.keys(extraIcons)[0]
    const before = extraIconState.version
    if (!extraIconState.loaded) expect(icon(extraName)).toBe('')
    await loadExtraIcons()
    expect(extraIconState.loaded).toBe(true)
    expect(extraIconState.version).toBeGreaterThan(before - 1)
    const svg = icon(extraName, '1em', '2em')
    expect(svg).toContain(`bi-${extraName}`)
    expect(svg).toContain(' width="1em"')
    expect(svg).toContain(' height="2em"')
    expect(icon('gibt-es-nicht')).toBe('')
  })
})
