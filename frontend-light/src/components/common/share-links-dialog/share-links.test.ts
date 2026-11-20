import { describe, expect, it } from 'vitest'
import { daysRemainingLabel, defaultShareVersion, redactionNotice, shareMode, shareUrl, shareVersions } from './share-links'
import { bundleIds, redactedVersionMap } from '../share-link-bundle-dialog/share-link-bundle'

/** Minimal Document with <meta> values like pl-server (frontend/patches/09) */
const docWith = (meta: Record<string, string>) =>
  ({ querySelector: (sel: string) => {
    const name = /name="([^"]+)"/.exec(sel)?.[1] ?? ''
    return name in meta ? { getAttribute: () => meta[name] } : null
  } }) as unknown as Document

describe('share-links (ShareLinksDialogComponent + paperrust.ts)', () => {
  it('Server-Betrieb: kein Modus, URL unverändert', () => {
    const doc = docWith({})
    expect(shareMode(doc)).toBe('')
    expect(shareUrl('http://127.0.0.1:58080/share/abc', doc, 'http://127.0.0.1:58080/')).toBe('http://127.0.0.1:58080/share/abc')
  })

  it("Modus 'local' ändert die URL nicht", () => {
    const doc = docWith({ paperrust_share_mode: 'local', paperrust_share_url: 'http://192.168.1.20:58080' })
    expect(shareMode(doc)).toBe('local')
    expect(shareUrl('http://127.0.0.1:58080/share/abc', doc, 'http://127.0.0.1:58080/')).toBe('http://127.0.0.1:58080/share/abc')
  })

  it("Modus 'lan' ersetzt den Anfang durch die LAN-Adresse", () => {
    const doc = docWith({ paperrust_share_mode: 'lan', paperrust_share_url: 'http://192.168.1.20:58080' })
    expect(shareUrl('http://127.0.0.1:58080/share/abc', doc, 'http://127.0.0.1:58080/')).toBe('http://192.168.1.20:58080/share/abc')
    // invalid address → unchanged
    const bad = docWith({ paperrust_share_mode: 'lan', paperrust_share_url: 'javascript:alert(1)' })
    expect(shareUrl('http://127.0.0.1:58080/share/abc', bad, 'http://127.0.0.1:58080/')).toBe('http://127.0.0.1:58080/share/abc')
  })

  it('verbleibende Tage', () => {
    const now = Date.parse('2026-11-09T12:00:00Z')
    expect(daysRemainingLabel('2026-11-10T12:00:00Z', now)).toBe('1 day')
    expect(daysRemainingLabel('2026-11-16T12:00:00Z', now)).toBe('7 days')
  })
})

describe('Teilen und Schwärzen (pl-pdftools §4.1)', () => {
  // API returns newest first; root 10 = version 1
  const api = [
    { id: 12, version_label: 'Redacted', redacted: true },
    { id: 11, version_label: 'Scan neu', redacted: false },
    { id: 10, is_root: true },
  ]

  it('nummeriert älteste zuerst, Root ist Version 1', () => {
    const l = shareVersions(api, 10)
    expect(l.map((v) => [v.id, v.number, v.redacted, v.isRoot])).toEqual([[10, 1, false, true], [11, 2, false, false], [12, 3, true, false]])
  })

  it('Vorauswahl: neueste geschwärzte Version, sonst Root', () => {
    expect(defaultShareVersion(shareVersions(api, 10), 10)).toBe(12)
    expect(defaultShareVersion(shareVersions([{ id: 11 }, { id: 10 }], 10), 10)).toBe(10)
    expect(defaultShareVersion(shareVersions(undefined, 10), 10)).toBe(10)
  })

  it('Hinweis nennt geschwärzte und lesbare Versionen, ohne Schwärzung keiner', () => {
    expect(redactionNotice(shareVersions(api, 10))).toEqual({ redacted: [3], readable: [1, 2] })
    expect(redactionNotice(shareVersions([{ id: 11 }, { id: 10 }], 10))).toBeNull()
  })

  it('fehlt der Root in der Liste, wird er als Version 1 ergänzt', () => {
    expect(shareVersions([{ id: 12, redacted: true }], 10).map((v) => v.id)).toEqual([10, 12])
  })

  it('Paket: neueste geschwärzte Version statt Root, abschaltbar', () => {
    const m = redactedVersionMap([
      { id: 10, versions: [{ id: 14, redacted: true }, { id: 12, redacted: true }, { id: 10 }] },
      { id: 20, versions: [{ id: 20 }] },
      { id: 30 },
    ])
    expect([...m]).toEqual([[10, 14]])
    expect(bundleIds([10, 20, 30], m, true)).toEqual([14, 20, 30])
    expect(bundleIds([10, 20, 30], m, false)).toEqual([10, 20, 30])
  })
})
