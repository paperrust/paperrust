import { describe, expect, it } from 'vitest'
import { convertMany, isConvertible, isImage } from './convert-pdf-api'

describe('convert to PDF helpers', () => {
  it('knows which types can become a PDF', () => {
    expect(isConvertible('image/png')).toBe(true)
    expect(isConvertible('image/tiff')).toBe(true)
    expect(isConvertible('image/webp')).toBe(true)
    expect(isConvertible('image/heic')).toBe(false)
    expect(isConvertible('application/pdf')).toBe(false)
    expect(isConvertible('application/vnd.openxmlformats-officedocument.wordprocessingml.document')).toBe(true)
    expect(isConvertible('application/vnd.oasis.opendocument.text')).toBe(true)
    expect(isConvertible('text/rtf; charset=utf-8')).toBe(true)
    expect(isConvertible('application/vnd.openxmlformats-officedocument.spreadsheetml.sheet')).toBe(false)
    expect(isConvertible(undefined)).toBe(false)
    expect(isImage('IMAGE/JPEG')).toBe(true)
    expect(isImage('text/rtf')).toBe(false)
  })

  it('converts one after another and collects failures', async () => {
    const seen: number[] = []
    const progress: number[] = []
    const r = await convertMany(
      [1, 2, 3],
      true,
      async (id, ocr) => {
        expect(ocr).toBe(true)
        seen.push(id)
        if (id === 2) throw { body: { detail: 'This document is already a PDF.' } }
      },
      (e) => (e as { body: { detail: string } }).body.detail,
      (n) => progress.push(n),
    )
    expect(seen).toEqual([1, 2, 3])
    expect(r.done).toEqual([1, 3])
    expect(r.failed).toEqual([{ id: 2, message: 'This document is already a PDF.' }])
    expect(progress).toEqual([1, 2, 3])
  })
})
