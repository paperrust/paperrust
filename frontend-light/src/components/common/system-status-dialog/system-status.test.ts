import { describe, expect, it } from 'vitest'
import { fileSize, isStale, statusIconClass } from './system-status'

describe('system status helpers', () => {
  it('isStale after 24h', () => {
    const now = new Date('2026-11-13T12:00:00Z')
    expect(isStale('2026-11-13T00:00:00Z', 24, now)).toBe(false)
    expect(isStale('2026-11-12T11:00:00Z', 24, now)).toBe(true)
  })
  it('fileSize like FileSizePipe', () => {
    expect(fileSize(500)).toBe('500 bytes')
    expect(fileSize(1572864)).toBe('1.5 MB')
    expect(fileSize(null)).toBe('?')
  })
  it('icon class per status', () => {
    expect(statusIconClass('ERROR')).toBe('text-danger')
    expect(statusIconClass('WARNING')).toBe('text-warning')
    expect(statusIconClass('DISABLED')).toBe('')
    expect(statusIconClass('DISABLED', true)).toBe('text-muted')
  })
})
