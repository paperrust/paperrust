import { describe, expect, it } from 'vitest'
import { hasSearchHighlights, sanitizeHighlight, searchNoteHighlights, searchScoreClass, shouldShowContentFallback } from './search-hit'

describe('search-hit', () => {
  it('keeps only highlight markup', () => {
    expect(sanitizeHighlight('the <b>quick</b> fox')).toBe('the <b>quick</b> fox')
    expect(sanitizeHighlight('a <span class="match">b</span> c')).toBe('a <span class="match">b</span> c')
    expect(sanitizeHighlight('<img src=x onerror=alert(1)><b>x</b>')).toBe('&lt;img src=x onerror=alert(1)&gt;<b>x</b>')
    expect(sanitizeHighlight('<span onclick="x">y</span>')).toBe('&lt;span onclick="x"&gt;y')
    expect(sanitizeHighlight('<b>open')).toBe('<b>open</b>')
    expect(sanitizeHighlight('x</b></span>')).toBe('x')
    expect(sanitizeHighlight('a &amp; b')).toBe('a &amp; b')
    expect(sanitizeHighlight(null)).toBe('')
  })
  it('score class like searchScoreClass', () => {
    expect(searchScoreClass({})).toBeUndefined()
    expect(searchScoreClass({ __search_hit__: { score: 0.9 } })).toBe('success')
    expect(searchScoreClass({ __search_hit__: { score: 0.5 } })).toBe('warning')
    expect(searchScoreClass({ __search_hit__: { score: 0.1 } })).toBe('danger')
  })
  it('note highlights and content fallback', () => {
    const d = { __search_hit__: { score: 0.5, highlights: '  ', note_highlights: 'no match,a <b>hit</b>' } }
    expect(searchNoteHighlights(d)).toEqual(['a <b>hit</b>'])
    expect(hasSearchHighlights(d)).toBe(false)
    expect(shouldShowContentFallback(d)).toBe(false)
    expect(shouldShowContentFallback({ __search_hit__: { score: 0.5, highlights: '', note_highlights: null } })).toBe(true)
    expect(shouldShowContentFallback({})).toBe(true)
    expect(shouldShowContentFallback({ __search_hit__: { score: 0.5, highlights: '<b>x</b>' } })).toBe(false)
  })
})
