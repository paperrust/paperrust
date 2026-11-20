// Icons that routes register via ?raw import must exist (as tools/icons.mjs checks) in the npm package bootstrap-icons.
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const SRC = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src')
const ICONS = path.resolve(SRC, '../node_modules/bootstrap-icons/icons')
const files = new Set(fs.readdirSync(ICONS).filter((f) => f.endsWith('.svg')).map((f) => f.slice(0, -4)))

function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) {
      if (e.name !== 'design2') walk(p, out) // second UI: own icon list (design2/lib/icons.ts)
    }
    else if (/\.(ts|svelte)$/.test(e.name) && !e.name.endsWith('.test.ts')) out.push(p)
  }
  return out
}

describe('route-icons', () => {
  it('alle ?raw-Symbole gibt es in bootstrap-icons, ricon-Namen sind registriert', () => {
    const raw = new Set()
    const used = new Set()
    for (const f of walk(SRC)) {
      const s = fs.readFileSync(f, 'utf8')
      for (const m of s.matchAll(/bootstrap-icons\/icons\/([a-z0-9-]+)\.svg\?raw/g)) raw.add(m[1])
      for (const m of s.matchAll(/\bricon\(\s*'([a-z0-9-]+)'/g)) used.add(m[1])
    }
    expect([...raw].filter((n) => !files.has(n))).toEqual([])
    // Core + lazily loaded icons (tools/icons.mjs shares the list for the budget of the first call)
    const generated = ['icons.generated.ts', 'icons-extra.generated.ts']
      .map((f) => fs.readFileSync(path.join(SRC, 'components/common/bootstrap-icon', f), 'utf8'))
      .join('\n')
    const regKeys = new Set()
    for (const f of walk(SRC)) {
      const s = fs.readFileSync(f, 'utf8')
      const m = /registerIcons\(\{([\s\S]*?)\}\)/.exec(s)
      if (m) for (const k of m[1].matchAll(/"([a-z0-9-]+)"|'([a-z0-9-]+)'|^\s*([a-z0-9]+)[:,]/gm)) regKeys.add(k[1] ?? k[2] ?? k[3])
    }
    expect([...used].filter((n) => !regKeys.has(n) && !generated.includes(`"${n}":`))).toEqual([])
  })
})
