// After document-version-dropdown.component.spec.ts + DocumentService (URLs/bodies like Angular).
import { describe, expect, it, vi } from 'vitest'
import { V1Client } from '../../../api/v1'
import { nextVersionLabel, taskIdFrom } from './versions'

function fakeFetch(body: unknown = {}) {
  const calls: { url: string; init: RequestInit }[] = []
  const f = vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, init: init ?? {} })
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } })
  })
  return { f, calls }
}

describe('Versionen', () => {
  it('taskIdFrom: String oder { task_id }', () => {
    expect(taskIdFrom('abc')).toBe('abc')
    expect(taskIdFrom({ task_id: 'x' })).toBe('x')
    expect(taskIdFrom({})).toBeUndefined()
    expect(taskIdFrom(null)).toBeUndefined()
    expect(taskIdFrom('')).toBeUndefined()
  })
  it('nextVersionLabel: getrimmt, leer → null, unverändert → undefined', () => {
    expect(nextVersionLabel(' neu ', 'alt')).toBe('neu')
    expect(nextVersionLabel('  ', 'alt')).toBeNull()
    expect(nextVersionLabel('alt ', 'alt')).toBeUndefined()
    expect(nextVersionLabel('', null)).toBeUndefined()
  })
  it('API: gleiche Endpunkte wie DocumentService', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv', cookie: () => 'csrftoken=t' })
    await c.deleteVersion(3, 7)
    expect(calls[0].url).toBe('http://srv/api/documents/3/versions/7/')
    expect(calls[0].init.method).toBe('DELETE')
    await c.updateVersionLabel(3, 7, null)
    expect(calls[1].init.method).toBe('PATCH')
    expect(JSON.parse(String(calls[1].init.body))).toEqual({ version_label: null })
    await c.getVersions(3)
    expect(calls[2].url).toBe('http://srv/api/documents/3/?fields=id%2Cversions')
    await c.mergeDocumentsAsVersions([3, 9], 3)
    expect(calls[3].url).toBe('http://srv/api/documents/merge_as_versions/')
    expect(JSON.parse(String(calls[3].init.body))).toEqual({ documents: [3, 9], root_document_id: 3 })
    await c.mergeDocumentsAsVersions([3, 9], 3, 'v2')
    expect(JSON.parse(String(calls[4].init.body))).toEqual({ documents: [3, 9], root_document_id: 3, version_label: 'v2' })
    await c.uploadVersion(3, new File(['x'], 'a.pdf'), 'v3')
    expect(calls[5].url).toBe('http://srv/api/documents/3/update_version/')
    const fd = calls[5].init.body as FormData
    expect(fd.get('version_label')).toBe('v3')
    expect((fd.get('document') as File).name).toBe('a.pdf')
    expect((calls[5].init.headers as Record<string, string>)['X-CSRFToken']).toBe('t')
  })
  it('API: edit_pdf und remove_password', async () => {
    const { f, calls } = fakeFetch()
    const c = new V1Client({ fetch: f, base: 'http://srv' })
    await c.editPdfDocuments([5], { operations: [{ page: 1, rotate: 90, doc: 0 }], delete_original: false, update_document: true, include_metadata: true, source_mode: 'explicit_selection' })
    expect(calls[0].url).toBe('http://srv/api/documents/edit_pdf/')
    expect(JSON.parse(String(calls[0].init.body))).toEqual({
      documents: [5],
      operations: [{ page: 1, rotate: 90, doc: 0 }],
      delete_original: false,
      update_document: true,
      include_metadata: true,
      source_mode: 'explicit_selection',
    })
    await c.removePasswordDocuments([5], { password: 'pw', update_document: false, include_metadata: true, delete_original: true, source_mode: 'explicit_selection' })
    expect(calls[1].url).toBe('http://srv/api/documents/remove_password/')
    expect(JSON.parse(String(calls[1].init.body)).password).toBe('pw')
  })
})
