import { describe, expect, it } from 'vitest'
import { noteFormat, renderNoteMarkdown } from './notes'

describe('noteFormat', () => {
  it('only an explicit "markdown" counts as Markdown', () => {
    expect(noteFormat({ format: 'markdown' })).toBe('markdown')
    expect(noteFormat({ format: 'plain' })).toBe('plain')
    // older servers (ngx 3.4.2) send no format
    expect(noteFormat({})).toBe('plain')
    expect(noteFormat({ format: 'html' as never })).toBe('plain')
  })
})

describe('renderNoteMarkdown', () => {
  it('renders Markdown', () => {
    expect(renderNoteMarkdown('**Due** 2026-12-01\n- pay')).toBe('<p><strong>Due</strong> 2026-12-01</p>\n<ul><li>pay</li></ul>')
    expect(renderNoteMarkdown('')).toBe('')
    expect(renderNoteMarkdown(undefined)).toBe('')
  })
  it('escapes raw HTML from other users', () => {
    const html = renderNoteMarkdown('<img src=x onerror=alert(1)>\n<script>alert(1)</script>')
    expect(html).not.toMatch(/<(img|script)/)
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
  })
  it('drops unsafe links and keeps safe ones inert', () => {
    expect(renderNoteMarkdown('[x](javascript:alert(1))')).not.toContain('href')
    expect(renderNoteMarkdown('[x](data:text/html,hi)')).not.toContain('href')
    const ok = renderNoteMarkdown('[x](https://example.com/"onmouseover="alert(1))')
    expect(ok).not.toContain('"onmouseover')
    expect(renderNoteMarkdown('[x](https://example.com)')).toContain('rel="noopener noreferrer nofollow"')
  })
})
