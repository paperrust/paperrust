// After document-detail/document-barcodes/document-barcodes.component.spec.ts
import { describe, expect, it } from 'vitest'
import { isLink } from './barcodes'

describe('DocumentBarcodes', () => {
  it('isLink: nur http(s) mit Host', () => {
    expect(isLink('https://example.com/a')).toBe(true)
    expect(isLink(' http://example.com ')).toBe(true)
    expect(isLink('ASN00012')).toBe(false)
    expect(isLink('javascript:alert(1)')).toBe(false)
    expect(isLink('mailto:a@b.c')).toBe(false)
  })
})
