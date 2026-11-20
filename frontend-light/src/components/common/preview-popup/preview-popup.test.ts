import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { HoverPreview, isPdf, previewKind, SHOW_DELAY_MS, CLOSE_DELAY_MS } from './preview-popup'

describe('preview-popup: Darstellung', () => {
  it('isPdf: Archivdatei oder PDF-MIME', () => {
    expect(isPdf({ id: 1, mime_type: 'application/pdf' })).toBe(true)
    expect(isPdf({ id: 1, mime_type: 'image/png', archived_file_name: 'a.pdf' })).toBe(true)
    expect(isPdf({ id: 1, mime_type: 'image/png', archived_file_name: null })).toBe(false)
    expect(isPdf(undefined)).toBe(false)
  })
  it('renderAsObject wie upstream', () => {
    expect(previewKind({ id: 1, mime_type: 'application/pdf' }, false)).toBe('pdfjs')
    expect(previewKind({ id: 1, mime_type: 'application/pdf' }, true)).toBe('object')
    expect(previewKind({ id: 1, mime_type: 'image/jpeg' }, false)).toBe('object')
    expect(previewKind({ id: 1, mime_type: 'text/plain' }, false)).toBe('text')
    expect(previewKind({ id: 1 }, false)).toBe('object')
  })
})

describe('preview-popup: Hover', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  function setup() {
    const s = { open: false, hidden: true }
    const h = new HoverPreview({
      isOpen: () => s.open,
      open: () => ((s.open = true), (s.hidden = true)),
      reveal: () => (s.hidden = false),
      close: () => (s.open = false),
    })
    return { s, h }
  }

  it('öffnet versteckt und zeigt nach der Verzögerung', () => {
    const { s, h } = setup()
    h.enter()
    expect(s).toEqual({ open: true, hidden: true })
    vi.advanceTimersByTime(SHOW_DELAY_MS)
    expect(s).toEqual({ open: true, hidden: false })
  })

  it('kurzes Überfahren schließt wieder, ohne sichtbar zu werden', () => {
    const { s, h } = setup()
    h.enter()
    h.leave()
    vi.advanceTimersByTime(SHOW_DELAY_MS)
    expect(s.open).toBe(false)
    expect(s.hidden).toBe(true)
  })

  it('bleibt offen beim Wechsel vom Link ins Popover, schließt beim Verlassen', () => {
    const { s, h } = setup()
    h.enter()
    vi.advanceTimersByTime(SHOW_DELAY_MS)
    h.leave() // leave link
    h.enter() // enter popover
    vi.advanceTimersByTime(CLOSE_DELAY_MS)
    expect(s.open).toBe(true)
    h.leave() // leave popover
    vi.advanceTimersByTime(CLOSE_DELAY_MS)
    expect(s.open).toBe(false)
  })

  it('dispose verwirft offene Zeitgeber', () => {
    const { s, h } = setup()
    h.enter()
    h.dispose()
    vi.advanceTimersByTime(SHOW_DELAY_MS)
    expect(s.hidden).toBe(true)
  })
})
