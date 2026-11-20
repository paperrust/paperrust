import { afterEach, describe, expect, it, vi } from 'vitest'
import { asnTarget, findDocumentIdsByAsn } from './document-asn'

afterEach(() => vi.unstubAllGlobals())

describe('DocumentAsn', () => {
  it('navigates to the single match, else 404', () => {
    expect(asnTarget([7])).toBe('documents/7')
    expect(asnTarget([])).toBe('404')
    expect(asnTarget([1, 2])).toBe('404')
  })
  it('queries like listAllFilteredIds([FILTER_ASN])', async () => {
    const fetchMock = vi.fn(async (..._args: any[]) => new Response(JSON.stringify({ count: 1, results: [{ id: 42 }] }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    expect(await findDocumentIdsByAsn('123')).toEqual([42])
    expect(fetchMock.mock.calls[0][0]).toBe('/api/documents/?page=1&page_size=100000&fields=id&archive_serial_number=123')
  })
})
