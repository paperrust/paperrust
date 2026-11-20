// After common/pdf-editor/pdf-editor.component.spec.ts (logic without DOM).
import { describe, expect, it } from 'vitest'
import { PdfEditorEditMode, computeDocIndex, editModeFrom, getOperations, initialPages, moveItem, rotateBy } from './pdf-editor'

describe('PDF-Editor', () => {
  it('pdfLoaded: eine Operation je Seite', () => {
    expect(initialPages(3)).toEqual([
      { page: 1, rotate: 0, splitAfter: false, selected: false, loaded: false },
      { page: 2, rotate: 0, splitAfter: false, selected: false, loaded: false },
      { page: 3, rotate: 0, splitAfter: false, selected: false, loaded: false },
    ])
  })
  it('rotate: im und gegen den Uhrzeigersinn, normiert', () => {
    expect(rotateBy(0, 90)).toBe(90)
    expect(rotateBy(0, -90)).toBe(270)
    expect(rotateBy(270, 90)).toBe(0)
  })
  it('drop: moveItemInArray', () => {
    expect(moveItem([1, 2, 3, 4], 0, 2)).toEqual([2, 3, 1, 4])
    expect(moveItem([1, 2, 3, 4], 3, 0)).toEqual([4, 1, 2, 3])
    expect(moveItem([1, 2], 1, 1)).toEqual([1, 2])
  })
  it('getOperations: Dokumentindex nach Trennung, Reihenfolge und Drehung', () => {
    const pages = initialPages(3)
    pages[0].rotate = 90
    pages[1].splitAfter = true
    const reordered = moveItem(pages, 2, 0) // Page 3 first
    expect(getOperations(reordered)).toEqual([
      { page: 3, rotate: 0, doc: 0 },
      { page: 1, rotate: 90, doc: 0 },
      { page: 2, rotate: 0, doc: 0 },
    ])
    expect(getOperations(pages)).toEqual([
      { page: 1, rotate: 90, doc: 0 },
      { page: 2, rotate: 0, doc: 0 },
      { page: 3, rotate: 0, doc: 1 },
    ])
    expect(computeDocIndex(pages, 2)).toBe(1)
  })
  it('Standardmodus aus Einstellung', () => {
    expect(editModeFrom('update')).toBe(PdfEditorEditMode.Update)
    expect(editModeFrom('create')).toBe(PdfEditorEditMode.Create)
    expect(editModeFrom(undefined)).toBe(PdfEditorEditMode.Create)
  })
})
