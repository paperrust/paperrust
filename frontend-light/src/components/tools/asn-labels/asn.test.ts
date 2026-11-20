import { describe, expect, it } from 'vitest'
import { buildJob, checkRange, content, CUSTOM, emptyCustom, fitsRange, groupPresets, skipUpTo, slotRect, slots, suggestRange, toggleSkip, type Form, type Sheet } from './asn'

const l4731: Sheet = {
  ...emptyCustom(),
  id: 'avery-l4731',
  vendor: 'Avery Zweckform',
  label_width: 25.4,
  label_height: 10,
  cols: 7,
  rows: 27,
  margin_left: 8.48,
  margin_top: 13.5,
  gap_x: 2.54,
  gap_y: 0,
}

const form = (o: Partial<Form> = {}): Form => ({
  preset: 'avery-l4731',
  custom: emptyCustom(),
  code: 'qr',
  showText: true,
  digits: 5,
  start: null,
  mode: 'sheets',
  sheets: 1,
  count: 10,
  skip: [],
  offsetX: 0,
  offsetY: 0,
  outline: false,
  ...o,
})

describe('asn-labels', () => {
  it('slot geometry matches the server (row-major, pitch, calibration offset)', () => {
    expect(slotRect(l4731, 0)).toEqual({ index: 0, x: 8.48, y: 13.5, w: 25.4, h: 10 })
    expect(slotRect(l4731, 1).x - slotRect(l4731, 0).x).toBeCloseTo(27.94)
    expect(slotRect(l4731, 7)).toMatchObject({ x: 8.48, y: 23.5 })
    expect(slotRect(l4731, 0, 1, -0.5)).toMatchObject({ x: 9.48, y: 13 })
    const all = slots(l4731)
    expect(all).toHaveLength(189)
    for (const s of all) {
      expect(s.x + s.w).toBeLessThanOrEqual(210 + 0.05)
      expect(s.y + s.h).toBeLessThanOrEqual(297 + 0.05)
    }
  })

  it('toggles used labels but keeps at least one free', () => {
    expect(toggleSkip([], 3, 4)).toEqual([3])
    expect(toggleSkip([3, 1], 1, 4)).toEqual([3])
    expect(toggleSkip([0, 1], 2, 4)).toEqual([0, 1, 2])
    expect(toggleSkip([0, 1, 2], 3, 4)).toEqual([0, 1, 2])
    expect(skipUpTo(3)).toEqual([0, 1, 2])
  })

  it('builds the request like pl_labels::LabelJob', () => {
    expect(buildJob(form({ skip: [5, 2] }))).toEqual({
      preset: 'avery-l4731',
      code: 'qr',
      show_text: true,
      digits: 5,
      sheets: 1,
      skip: [2, 5],
      offset_x: 0,
      offset_y: 0,
      outline: false,
    })
    const j = buildJob(form({ preset: CUSTOM, mode: 'count', count: 12.4, start: 42 }), { outline: true })
    expect(j.preset).toBeUndefined()
    expect(j.sheet?.id).toBe(CUSTOM)
    expect(j).toMatchObject({ count: 12, start: 42, outline: true })
    expect(j.sheets).toBeUndefined()
  })

  it('groups presets by vendor and formats content', () => {
    const g = groupPresets([l4731, { ...l4731, id: 'x', vendor: 'HERMA' }, { ...l4731, id: 'y' }])
    expect(g.map(([v, s]) => [v, s.length])).toEqual([['Avery Zweckform', 2], ['HERMA', 1]])
    expect(content('ASN', 5, 42)).toBe('ASN00042')
    expect(content('ASN', 2, 1234)).toBe('ASN1234')
  })

  it('number ranges: job field, suggestion, checks and fit', () => {
    expect(buildJob(form()).range).toBeUndefined()
    expect(buildJob(form({ range: null })).range).toBeUndefined()
    expect(buildJob(form({ range: 7 })).range).toBe(7)
    expect(suggestRange([])).toMatchObject({ first_asn: 1, last_asn: 1000 })
    expect(suggestRange([{ first_asn: 1, last_asn: 999 }, { first_asn: 5000, last_asn: 5999 }])).toMatchObject({ first_asn: 6000, last_asn: 6999 })
    const others = [{ id: 1, name: 'Bern', code: 'BE', first_asn: 100, last_asn: 199 }]
    const r = { name: 'Zürich', code: 'ZH', location: 'Archiv', first_asn: 200, last_asn: 299, note: '' }
    expect(checkRange(r, others, null)).toEqual({})
    expect(checkRange({ ...r, first_asn: 150 }, others, null)).toEqual({ first_asn: 'overlap' })
    expect(checkRange({ ...r, first_asn: 150 }, others, 1)).toEqual({})
    expect(checkRange({ ...r, code: 'be' }, others, null)).toEqual({ code: 'code_taken' })
    expect(checkRange({ ...r, code: 'ZÜRICH' }, others, null)).toEqual({ code: 'code_format' })
    expect(checkRange({ ...r, code: 'ABCDEFGHI' }, others, null)).toEqual({ code: 'code_format' })
    expect(checkRange({ ...r, name: ' ', code: '' }, others, null)).toEqual({ name: 'required', code: 'required' })
    expect(checkRange({ ...r, last_asn: 199 }, others, null)).toEqual({ last_asn: 'order' })
    expect(fitsRange({ first_asn: 100, last_asn: 199 }, 100, 100)).toBe('ok')
    expect(fitsRange({ first_asn: 100, last_asn: 199 }, 150, 51)).toBe('overflow')
    expect(fitsRange({ first_asn: 100, last_asn: 199 }, 99, 1)).toBe('outside')
  })
})
