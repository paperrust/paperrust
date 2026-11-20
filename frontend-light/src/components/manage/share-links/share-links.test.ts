import { afterEach, describe, expect, it, vi } from 'vitest'
import { initialTab, nextSort, pageAfterDelete, sortClass } from './share-links'
import { bundlePayload, docTitle, fileVersionLabel, isExpired, replaceBundle, slugShareUrl, statusLabel } from '../../common/share-link-bundle-dialog/share-link-bundle'
import type { ShareLinkBundle } from '../../../api/v1'

describe('manage/share-links', () => {
  it('getInitialNavID: Bündel nur mit Recht, sonst Dokument-Links', () => {
    expect(initialTab('bundles', true, true)).toBe('bundles')
    expect(initialTab('bundles', true, false)).toBe('links')
    expect(initialTab(null, true, true)).toBe('links')
    expect(initialTab(null, false, true)).toBe('bundles')
  })

  it('SortableDirective + onSort: aufsteigend, absteigend, zurück auf created absteigend', () => {
    expect(nextSort('created', true, 'expiration')).toEqual({ sortField: 'expiration', sortReverse: false })
    expect(nextSort('expiration', false, 'expiration')).toEqual({ sortField: 'expiration', sortReverse: true })
    expect(nextSort('expiration', true, 'expiration')).toEqual({ sortField: 'created', sortReverse: true })
    expect(sortClass('created', true, 'created')).toBe('des')
    expect(sortClass('created', false, 'created')).toBe('asc')
    expect(sortClass('created', false, 'status')).toBe('')
  })

  it('Seite nach dem Löschen', () => {
    expect(pageAfterDelete(1, 3)).toBe(2)
    expect(pageAfterDelete(1, 1)).toBe(1)
    expect(pageAfterDelete(4, 3)).toBe(3)
  })
})

describe('share-link-bundle (Dialog + Liste)', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('Nutzlast wie ShareLinkBundleDialogComponent.submit', () => {
    expect(bundlePayload([1, 2], true, 7)).toEqual({ document_ids: [1, 2], file_version: 'archive', expiration_days: 7 })
    expect(bundlePayload([3], false, null)).toEqual({ document_ids: [3], file_version: 'original', expiration_days: null })
  })

  it('Beschriftungen, Ablauf, Titel', () => {
    expect(statusLabel('ready')).toBe('Ready')
    expect(statusLabel('weird')).toBe('weird')
    expect(fileVersionLabel('original')).toBe('Original')
    expect(isExpired(null)).toBe(false)
    expect(isExpired('2020-01-01T00:00:00Z', Date.parse('2021-01-01'))).toBe(true)
    expect(isExpired('2030-01-01T00:00:00Z', Date.parse('2021-01-01'))).toBe(false)
    expect(docTitle('')).toBe('(untitled)')
  })

  it('Link: Server-Wurzel + /share/<slug>', () => {
    vi.stubGlobal('document', { querySelector: () => null })
    vi.stubGlobal('location', { origin: 'http://srv:8000' })
    expect(slugShareUrl('abc', 'http://srv:8000')).toBe('http://srv:8000/share/abc')
  })

  it('replaceBundle ersetzt an gleicher Stelle oder stellt voran', () => {
    const b = (id: number, status: ShareLinkBundle['status'] = 'pending') => ({ id, status }) as ShareLinkBundle
    expect(replaceBundle([b(1), b(2)], b(2, 'ready')).map((x) => [x.id, x.status])).toEqual([
      [1, 'pending'],
      [2, 'ready'],
    ])
    expect(replaceBundle([b(1)], b(5)).map((x) => x.id)).toEqual([5, 1])
  })
})
