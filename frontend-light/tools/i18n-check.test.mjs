// Own translation catalog (docs/FRONTEND-LIGHT.md §4):
//  - every key used in the code is in en-US and in all 10 languages (same placeholders),
//  - i18n/ contains only our 10 language files, none adopted from paperless-ngx.
// The proof "not from ngx" runs via checksums (ngx-i18n-fingerprints.json): SHA-256 (10 characters) of all
// longer sentences (>= 8 words; shorter standard sentences like "%s days ago" coincide in every translation) of the former ngx catalog, English and per language – without our own additions.
// No own text may equal such a sentence.
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { check, findKeys, KEY_RE, readCatalog, SUPPORTED_LOCALES } from './i18n-check.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const norm = (s) => s.replace(/\{\$[A-Z0-9_]+\}/g, '#').replace(/\s+/g, ' ').trim().toLowerCase()
/** Words of the visible text (without ICU syntax) */
const words = (s) => s.replace(/\{\$[A-Z0-9_]+\}/g, ' ').replace(/\{\w+, (plural|select),/g, ' ').replace(/(=\d+|zero|one|two|few|many|other)\s*\{/g, ' ').replace(/[{}]/g, ' ').trim().split(/\s+/).length
const hash = (s) => crypto.createHash('sha256').update(norm(s)).digest('hex').slice(0, 10)

describe('eigener Übersetzungskatalog', () => {
  it('alle benutzten Schlüssel stehen in en-US und in allen 10 Sprachen, gleiche Platzhalter', () => {
    expect(check()).toEqual([])
  })

  it('genau die 10 unterstützten Sprachen, keine ngx-Reste', () => {
    const files = fs.readdirSync(path.join(ROOT, 'i18n')).filter((f) => f.endsWith('.json')).sort()
    expect(files).toEqual(SUPPORTED_LOCALES.map((l) => `${l}.json`).sort())
    expect(fs.existsSync(path.join(ROOT, 'i18n/_keys.json'))).toBe(false)
    expect(fs.existsSync(path.join(ROOT, 'i18n-extra.json'))).toBe(false)
    expect(fs.existsSync(path.join(ROOT, 'tools/xlf2json.mjs'))).toBe(false)
  })

  it('Schlüssel sind eigene Punkt-Schlüssel, kein englischer Quelltext (ngx-Katalog war nach Text verschlüsselt)', () => {
    for (const loc of SUPPORTED_LOCALES) for (const k of Object.keys(readCatalog(loc))) expect(k).toMatch(KEY_RE)
  })

  it('kein Satz gleicht einem Satz aus dem früheren ngx-Katalog (englisch und je Sprache)', () => {
    const fp = JSON.parse(fs.readFileSync(path.join(ROOT, 'tools/ngx-i18n-fingerprints.json'), 'utf8'))
    const ngxEn = new Set(fp.en)
    const hits = []
    for (const loc of SUPPORTED_LOCALES) {
      const own = new Set(loc === 'en-US' ? fp.en : fp[loc] ?? [])
      for (const [k, v] of Object.entries(readCatalog(loc))) {
        if (words(v) < 8) continue
        const h = hash(v)
        if (own.has(h) || (loc !== 'en-US' && ngxEn.has(h))) hits.push(`${loc}: ${k}`)
      }
    }
    expect(hits).toEqual([])
  })

  it('findKeys erkennt Aufrufe und Schlüssel-Literale in Tabellen', () => {
    const known = new Set(['filter.year', 'common.save'])
    expect(findKeys("t('common.save') + msg(\"a.b\")")).toEqual(['common.save', 'a.b'])
    expect(findKeys("const X = { y: 'filter.year', z: 'other.key' }", known)).toEqual(['filter.year'])
  })
})
