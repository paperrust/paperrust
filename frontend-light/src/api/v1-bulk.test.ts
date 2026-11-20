import { describe, expect, it, vi } from 'vitest'
import { V1Client } from './v1'

function fakeFetch(body: unknown = { count: 0, next: null, previous: null, results: [] }) {
  const calls: { url: string; init: RequestInit }[] = []
  const f = vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, init: init ?? {} })
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } })
  })
  return { f, calls }
}
const body = (c: { init: RequestInit }) => JSON.parse(String(c.init.body))

describe('V1Client: Massenbearbeitung wie DocumentService', () => {
  it('bulkEdit/delete/reprocess/rotate mit Auswahl-Abfrage', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    const all = { all: true, filters: { title__icontains: 'x' }, excluded_documents: [4] }
    await c.bulkEdit(all, 'modify_tags', { add_tags: [1], remove_tags: [] })
    expect(calls[0].url).toBe('http://srv/api/documents/bulk_edit/')
    expect(body(calls[0])).toEqual({ ...all, method: 'modify_tags', parameters: { add_tags: [1], remove_tags: [] } })
    await c.deleteDocuments({ documents: [1, 2] })
    expect([calls[1].url, body(calls[1])]).toEqual(['http://srv/api/documents/delete/', { documents: [1, 2] }])
    await c.reprocessSelection({ documents: [1] }, true)
    expect(body(calls[2])).toEqual({ documents: [1], remote_ocr: true })
    await c.rotateDocuments({ documents: [1] }, 270)
    expect([calls[3].url, body(calls[3])]).toEqual(['http://srv/api/documents/rotate/', { documents: [1], degrees: 270, source_mode: 'latest_version' }])
    await c.selectionData({ documents: [1] })
    expect(calls[4].url).toBe('http://srv/api/documents/selection_data/')
  })

  it('Neu verarbeiten mit KI-Texterkennung: ai_ocr nur, wenn gewählt', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.reprocessDocuments([5], false, true)
    expect([calls[0].url, body(calls[0])]).toEqual(['http://srv/api/documents/reprocess/', { documents: [5], remote_ocr: false, ai_ocr: true }])
    await c.reprocessDocuments([5])
    expect(body(calls[1])).toEqual({ documents: [5], remote_ocr: false })
    await c.reprocessSelection({ all: true, filters: {} }, true, true)
    expect(body(calls[2])).toEqual({ all: true, filters: {}, remote_ocr: true, ai_ocr: true })
  })

  it('merge, merge_as_versions, getFew', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.mergeDocuments([3, 1], { metadata_document_id: 1, delete_originals: true })
    expect([calls[0].url, body(calls[0])]).toEqual(['http://srv/api/documents/merge/', { documents: [3, 1], metadata_document_id: 1, delete_originals: true }])
    await c.mergeDocumentsAsVersions([3, 1], 3)
    expect([calls[1].url, body(calls[1])]).toEqual(['http://srv/api/documents/merge_as_versions/', { documents: [3, 1], root_document_id: 3 }])
    await c.getFewDocuments([3, 1])
    expect(calls[2].url).toBe('http://srv/api/documents/?id__in=3%2C1&ordering=-id&page_size=1000')
  })

  it('Freigabe-Links und -Bündel', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.listShareLinks(2, 25, 'document__title', true)
    expect(calls[0].url).toBe('http://srv/api/share_links/?page=2&page_size=25&ordering=-document__title')
    await c.listShareLinkBundles(1, 50, 'created', false)
    expect(calls[1].url).toBe('http://srv/api/share_link_bundles/?page=1&page_size=50&ordering=created')
    await c.createShareLinkBundle({ document_ids: [1], file_version: 'archive', expiration_days: null })
    expect([calls[2].init.method, body(calls[2])]).toEqual(['POST', { document_ids: [1], file_version: 'archive', expiration_days: null }])
    await c.rebuildShareLinkBundle(9)
    expect([calls[3].url, calls[3].init.method]).toEqual(['http://srv/api/share_link_bundles/9/rebuild/', 'POST'])
    await c.deleteShareLinkBundle(9)
    expect([calls[4].url, calls[4].init.method]).toEqual(['http://srv/api/share_link_bundles/9/', 'DELETE'])
  })
})
