import { describe, expect, it } from 'vitest'
import { ApiError } from '../../../api/v1'
import en from '../../../../i18n/en-US.json'
import { choices, defaultResolution, effectiveSource, ERROR_KEYS, errorCode, fieldMessage, isFinished, smtpHost, type Capabilities } from './scan-api'

const caps: Capabilities = {
  scanner: 1,
  make_and_model: 'HP OfficeJet Pro 9010 series',
  state: 'Idle',
  adf_state: 'ScannerAdfLoaded',
  sources: [
    { source: 'flatbed', color_modes: ['bw', 'grayscale', 'color'], resolutions: [75, 100, 200, 300, 600, 1200], formats: [], max_width: 2550, max_height: 3508 },
    { source: 'adf', color_modes: ['bw', 'grayscale', 'color'], resolutions: [75, 100, 200, 300], formats: [], max_width: 2550, max_height: 4200 },
    { source: 'adf_duplex', color_modes: ['grayscale', 'color'], resolutions: [75, 100, 200, 300], formats: [], max_width: 2550, max_height: 4200 },
  ],
}

describe('scan helpers', () => {
  it('builds choices from capabilities', () => {
    const c = choices(caps, 'flatbed')
    expect(c.sources).toEqual(['adf', 'flatbed'])
    expect(c.duplex).toBe(true)
    expect(c.resolutions).toContain(1200)
    expect(choices(caps, 'adf_duplex').colors).toEqual(['grayscale', 'color'])
    expect(choices(null, 'adf')).toEqual({ sources: [], duplex: false, resolutions: [], colors: [] })
  })

  it('combines source and duplex', () => {
    expect(effectiveSource('adf', true)).toBe('adf_duplex')
    expect(effectiveSource('flatbed', true)).toBe('flatbed')
    expect(effectiveSource('adf', false)).toBe('adf')
  })

  it('picks a default resolution', () => {
    expect(defaultResolution([75, 150, 300, 600])).toBe(300)
    expect(defaultResolution([100, 200, 600])).toBe(200)
    expect(defaultResolution([600, 1200])).toBe(600)
    expect(defaultResolution([])).toBeUndefined()
  })

  it('reads error codes and field messages', () => {
    expect(errorCode(new ApiError(409, '/x', { code: 'busy', detail: 'scanner is busy' }))).toBe('busy')
    expect(errorCode({ code: 'jam', message: 'paper jam' })).toBe('jam')
    expect(errorCode(new Error('x'))).toBe('protocol')
    expect(fieldMessage(new ApiError(400, '/x', { url: ['No eSCL scanner found.'] }))).toBe('No eSCL scanner found.')
    expect(isFinished('done') && isFinished('failed') && isFinished('cancelled') && !isFinished('scanning')).toBe(true)
  })

  it('maps every server error code (ScanError::code) to an existing text', () => {
    const codes = ['unreachable', 'busy', 'adf_empty', 'jam', 'cancelled', 'unsupported', 'protocol', 'too_large', 'too_many_pages', 'job_too_large', 'pdf', 'intake']
    const catalog = en as Record<string, string>
    for (const c of codes) {
      expect(ERROR_KEYS[c], c).toBeDefined()
      expect(catalog[ERROR_KEYS[c]], ERROR_KEYS[c]).toBeTruthy()
    }
  })

  it('formats the SMTP address', () => {
    expect(smtpHost({ enabled: true, listen: '0.0.0.0:2525', port: 2525, tls: false, require_tls: false, implicit_tls: false, max_mb: 50, error: null }, 'nas.local')).toBe('nas.local:2525')
    expect(smtpHost(null, 'nas.local')).toBe('nas.local')
  })
})
