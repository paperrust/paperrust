import { describe, expect, it, vi } from 'vitest'
import { ApiError, V1Client, getCookie, queryString } from './v1'

function fakeFetch(status = 200, body: unknown = { count: 0, next: null, previous: null, results: [] }) {
  const calls: { url: string; init: RequestInit }[] = []
  const f = vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, init: init ?? {} })
    return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
  })
  return { f, calls }
}

describe('Hilfsfunktionen', () => {
  it('queryString: feste Reihenfolge, leere Werte weg, kodiert', () => {
    expect(queryString({ page: 1, page_size: 50, ordering: '-created', q: '', x: null, y: undefined })).toBe('?page=1&page_size=50&ordering=-created')
    expect(queryString({ title_content: 'ÄÖÜ & Co' })).toBe('?title_content=%C3%84%C3%96%C3%9C%20%26%20Co')
    expect(queryString({})).toBe('')
  })
  it('getCookie', () => {
    expect(getCookie('csrftoken', 'a=1; csrftoken=abc%3D; b=2')).toBe('abc=')
    expect(getCookie('missing', 'a=1')).toBeNull()
  })
})

describe('V1Client', () => {
  it('manueller Workflow-Lauf: Liste gefiltert, bulk_edit run_workflow', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.listManualWorkflows()
    expect(calls[0].url).toBe('http://srv/api/workflows/?page=1&page_size=100000&trigger_type=5&enabled=true')
    await c.runWorkflow([3, 4], 5, false)
    expect(calls[1].url).toBe('http://srv/api/documents/bulk_edit/')
    expect(JSON.parse(String(calls[1].init.body))).toEqual({ documents: [3, 4], method: 'run_workflow', parameters: { workflow: 5, filter_documents: false } })
  })
  it('Dokumentliste: gleiche Parameter wie Angular (listFiltered)', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.listDocuments({ page: 1, pageSize: 50, sortField: 'created', sortReverse: true })
    expect(calls[0].url).toBe('http://srv/api/documents/?page=1&page_size=50&ordering=-created&truncate_content=true&include_selection_data=true')
    await c.listDocuments({ page: 2, pageSize: 25, sortField: 'title', sortReverse: false, extra: { title_content: 'rent' } })
    expect(calls[1].url).toBe('http://srv/api/documents/?page=2&page_size=25&ordering=title&truncate_content=true&include_selection_data=true&title_content=rent')
  })
  it('listAll wie AbstractPaperlessService (page_size=100000), Accept mit API-Version 10, Session-Cookies', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f })
    await c.listAll('tags')
    expect(calls[0].url).toBe('/api/tags/?page=1&page_size=100000')
    const h = calls[0].init.headers as Record<string, string>
    expect(h.Accept).toBe('application/json; version=10')
    expect(calls[0].init.credentials).toBe('same-origin')
    expect(h['X-CSRFToken']).toBeUndefined()
  })
  it('CSRF-Header nur bei schreibenden Methoden', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, cookie: () => 'csrftoken=tok123' })
    await c.request('POST', '/api/ui_settings/', { settings: {} })
    const h = calls[0].init.headers as Record<string, string>
    expect(h['X-CSRFToken']).toBe('tok123')
    expect(h['Content-Type']).toBe('application/json')
    expect(calls[0].init.body).toBe('{"settings":{}}')
  })
  it('Fehler: ApiError mit Status/Detail, 401/403 melden', async () => {
    const { f } = fakeFetch(403, { detail: 'Authentication credentials were not provided.' })
    const seen: number[] = []
    const c = new V1Client({ fetch: f, onUnauthorized: (s) => seen.push(s) })
    await expect(c.uiSettings()).rejects.toBeInstanceOf(ApiError)
    await c.uiSettings().catch((e: ApiError) => {
      expect(e.status).toBe(403)
      expect(e.body).toEqual({ detail: 'Authentication credentials were not provided.' })
    })
    expect(seen).toEqual([403, 403])
  })
  it('Aufgaben-Zähler und Vorschau-URL', async () => {
    const { f, calls } = fakeFetch(200, { all: 10, needs_attention: 3, in_progress: 3, completed: 4 })
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    expect((await c.taskStatusCounts()).needs_attention).toBe(3)
    expect(calls[0].url).toBe('http://srv/api/tasks/status_counts/?acknowledged=false')
    expect(c.previewUrl(192)).toBe('http://srv/api/documents/192/preview/')
  })
  it('204 ohne Inhalt', async () => {
    const { f } = fakeFetch(204)
    expect(await new V1Client({ fetch: f }).request('DELETE', '/api/x/1/')).toBeUndefined()
  })
})
