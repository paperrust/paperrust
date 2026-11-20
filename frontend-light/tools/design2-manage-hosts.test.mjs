// Every area in SECTIONS (v('<host>', …)) needs a display branch `view.host === '<host>'` – otherwise the
// page stays empty without an error message (desktop test 2026-11-18c: scan/scanners/protection/share links empty).
import { describe, expect, it } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const src = fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), '../src/design2/manage/ManageHost.svelte'), 'utf8')

describe('ManageHost', () => {
  it('jeder Bereich hat einen Anzeige-Zweig', () => {
    const hosts = [...src.matchAll(/\bv\('([a-z-]+)'/g)].map((m) => m[1])
    expect(hosts.length).toBeGreaterThan(10)
    const missing = hosts.filter((h) => !src.includes(`view.host === '${h}'`) && !/(users-admin|roles-admin)/.test(h))
    expect(missing).toEqual([])
  })
})
