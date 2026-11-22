import { describe, expect, it } from 'vitest'
import { Conflict, v2WriteUnsupported } from './v2docs'

// Desktop app in front of paperless-ngx (docs/DESKTOP.md §26): v2 writes are not possible there,
// the editor falls back to v1 instead of showing an error.
describe('v2 write fallback', () => {
  const err = (status: number) => Object.assign(new Error('x'), { status })

  it('falls back to v1 when the counterpart has no v2 writes', () => {
    for (const s of [404, 405, 501, 503]) expect(v2WriteUnsupported(err(s))).toBe(true)
  })

  it('keeps real answers (conflict, validation, auth, server error)', () => {
    expect(v2WriteUnsupported(new Conflict(3))).toBe(false)
    for (const s of [400, 401, 403, 412, 500]) expect(v2WriteUnsupported(err(s))).toBe(false)
    expect(v2WriteUnsupported(new Error('network'))).toBe(false)
    expect(v2WriteUnsupported(null)).toBe(false)
  })
})
