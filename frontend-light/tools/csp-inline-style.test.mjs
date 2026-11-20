// The CSP of the Light UI is `default-src 'self'` without 'unsafe-inline' (pl-server light_ui.rs): static
// style="…" attributes end up in the Svelte template (HTML parser) and are blocked (test log A-331/D-029).
// Allowed are runtime styles via CSSOM: style={"…"}, style="…{expression}…" (set_style → style.cssText) and style:prop.
import { describe, expect, it } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

function walk(d, out = []) {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (p.endsWith('.svelte')) out.push(p)
  }
  return out
}

describe('CSP: keine statischen style-Attribute', () => {
  it('src/**/*.svelte', () => {
    const bad = []
    for (const f of walk(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src'))) {
      const markup = fs.readFileSync(f, 'utf8').replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>|<!--[\s\S]*?-->/g, '')
      for (const m of markup.matchAll(/\sstyle=("[^"{}]*"|'[^'{}]*')/g)) bad.push(`${path.basename(f)}: ${m[0].trim()}`)
    }
    expect(bad).toEqual([])
  })
})
