import { describe, expect, it } from 'vitest'
import { safeHref } from './safe-href'

describe('safeHref', () => {
  it('allows http(s) and mailto', () => {
    expect(safeHref('https://example.com/a?b=1')).toBe('https://example.com/a?b=1')
    expect(safeHref('http://x.invalid')).toBe('http://x.invalid')
    expect(safeHref('mailto:a@example.invalid')).toBe('mailto:a@example.invalid')
  })
  it('rejects script and data URLs in any spelling', () => {
    for (const v of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', ' javascript:alert(1)', 'java\tscript:alert(1)',
      'data:text/html,<script>alert(1)</script>', 'vbscript:msgbox(1)', 'file:///etc/passwd', '', 'not a url', null, 42]) {
      expect(safeHref(v)).toBeUndefined()
    }
  })
})
