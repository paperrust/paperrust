import { describe, expect, it } from 'vitest'
import { collectAreas, cssBox, errorText, formatBytes, normRect, toPdf, toViewport, usableRect } from './pdf-tools-api'

describe('pdf-tools helpers', () => {
  it('normalizes and filters rectangles', () => {
    expect(normRect([10, 20, 0, 5])).toEqual([0, 5, 10, 20])
    expect(usableRect([0, 0, 1, 10])).toBe(false)
    expect(usableRect([0, 0, 5, 5])).toBe(true)
    expect(usableRect([0, 0, NaN, 5])).toBe(false)
  })

  it('collects drawn areas and only confirmed hits', () => {
    const drawn = [{ page: 1, rect: [5, 5, 1, 1] as [number, number, number, number] }]
    const hits = [
      { page: 2, term: 'a', context: 'a', rects: [[1, 1, 2, 2] as [number, number, number, number]] },
      { page: 3, term: 'a', context: 'a', rects: [[3, 3, 4, 4] as [number, number, number, number]] },
    ]
    expect(collectAreas(drawn, hits, new Set([1]))).toEqual([
      { page: 1, rect: [1, 1, 5, 5] },
      { page: 3, rect: [3, 3, 4, 4] },
    ])
  })

  it('converts between PDF points and viewport pixels like pdf.js', () => {
    // pdf.js PageViewport for 612×792, scale 1.25, without rotation: [s, 0, 0, -s, 0, h*s]
    const s = 1.25
    const m = [s, 0, 0, -s, 0, 792 * s] as [number, number, number, number, number, number]
    expect(toViewport(m, 72, 700)).toEqual([90, 115])
    const [x, y] = toPdf(m, 90, 115)
    expect(x).toBeCloseTo(72)
    expect(y).toBeCloseTo(700)
    expect(cssBox(m, [72, 690, 172, 720])).toEqual({ left: 90, top: 90, width: 125, height: 37.5 })
    // rotated 90°: [0, s, s, 0, 0, 0]
    const r: [number, number, number, number, number, number] = [0, s, s, 0, 0, 0]
    const [rx, ry] = toPdf(r, ...toViewport(r, 10, 20))
    expect(rx).toBeCloseTo(10)
    expect(ry).toBeCloseTo(20)
  })

  it('formats sizes and errors', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
    expect(errorText({ body: { detail: 'nope' } })).toBe('nope')
    expect(errorText({ body: { terms: ['bad'] } })).toBe('bad')
    expect(errorText(new Error('x'))).toBeUndefined()
  })
})
