// Logic of common/pdf-editor/pdf-editor.component.ts + pdf-editor-edit-mode.ts (framework-free, testable).

export enum PdfEditorEditMode {
  Update = 'update',
  Create = 'create',
}

export interface PageOperation {
  page: number
  rotate: number
  splitAfter: boolean
  selected?: boolean
  loaded?: boolean
}

/** pdfLoaded: one operation per page */
export function initialPages(totalPages: number): PageOperation[] {
  return Array.from({ length: totalPages }, (_, i) => ({ page: i + 1, rotate: 0, splitAfter: false, selected: false, loaded: false }))
}

/** rotate/rotateSelected: normalized to 0..270 */
export function rotateBy(rotate: number, dir: number): number {
  return (rotate + dir + 360) % 360
}

/** moveItemInArray (@angular/cdk/drag-drop) */
export function moveItem<T>(arr: T[], from: number, to: number): T[] {
  const a = arr.slice()
  const clamp = (v: number) => Math.max(0, Math.min(v, a.length - 1))
  const f = clamp(from)
  const t = clamp(to)
  if (f === t) return a
  const [item] = a.splice(f, 1)
  a.splice(t, 0, item)
  return a
}

/** computeDocIndex: number of splits before the page */
export function computeDocIndex(pages: PageOperation[], index: number): number {
  let docIndex = 0
  for (let i = 0; i <= index; i++) {
    if (pages[i].splitAfter && i < index) docIndex++
  }
  return docIndex
}

/** getOperations: request operations for /api/documents/edit_pdf/ */
export function getOperations(pages: PageOperation[]): { page: number; rotate: number; doc: number }[] {
  return pages.map((p, idx) => ({ page: p.page, rotate: p.rotate, doc: computeDocIndex(pages, idx) }))
}

/** Stored default mode (setting "default-edit-mode"), otherwise create new */
export function editModeFrom(v: unknown): PdfEditorEditMode {
  return v === PdfEditorEditMode.Update ? PdfEditorEditMode.Update : PdfEditorEditMode.Create
}
