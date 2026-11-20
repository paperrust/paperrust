// Pure logic from document-detail.component.ts (testable without DOM).
import type { Document } from '../../api/v1'
import type { FilterRule } from '../../lib/filter-rules'
import {
  FILTER_CORRESPONDENT,
  FILTER_CREATED_AFTER,
  FILTER_CREATED_BEFORE,
  FILTER_DOCUMENT_TYPE,
  FILTER_HAS_TAGS_ALL,
  FILTER_STORAGE_PATH,
} from '../../lib/filter-rule-type'
import { isoToModel, type DateStruct } from '../../lib/date-input'
import { PdfZoomLevel, PdfZoomScale } from '../common/pdf-viewer/pdfjs'

/** DocumentDetailNavIDs (order = Angular enum) */
export const NAV_IDS = { details: 1, content: 2, metadata: 3, preview: 4, notes: 5, permissions: 6, history: 7, duplicates: 8 } as const
export type NavKey = keyof typeof NAV_IDS

export type ContentRenderType = 'pdf' | 'image' | 'text' | 'other' | 'unknown' | 'tiff'

export function getRenderType(mimeType: string | null | undefined): ContentRenderType {
  if (!mimeType) return 'unknown'
  if (mimeType === 'application/pdf') return 'pdf'
  if (['text/plain', 'application/csv', 'text/csv'].includes(mimeType)) return 'text'
  if (mimeType.indexOf('tiff') >= 0) return 'tiff'
  if (mimeType.indexOf('image/') === 0) return 'image'
  return 'other'
}

export interface PermissionsFormValue {
  owner: number | null
  set_permissions: { view: { users: number[]; groups: number[] }; change: { users: number[]; groups: number[] } }
}

export interface DocumentFormValue {
  title: string
  content: string
  created: string | null
  correspondent: number | null
  document_type: number | null
  storage_path: number | null
  archive_serial_number: number | null
  tags: number[]
  permissions_form: PermissionsFormValue | null
  custom_fields: { field: number; value: unknown }[]
}

export const FORM_KEYS: (keyof DocumentFormValue)[] = [
  'title', 'content', 'created', 'correspondent', 'document_type', 'storage_path', 'archive_serial_number', 'tags', 'permissions_form', 'custom_fields',
]

/** mapDocToForm / store value for the "unsaved" check (setupDirtyTracking) */
export function formFromDocument(doc: Document): DocumentFormValue {
  return {
    title: doc.title,
    content: doc.content ?? '',
    created: doc.created,
    correspondent: doc.correspondent,
    document_type: doc.document_type,
    storage_path: doc.storage_path,
    archive_serial_number: doc.archive_serial_number,
    tags: [...(doc.tags ?? [])],
    permissions_form: { owner: doc.owner, set_permissions: (doc.permissions as any) ?? undefined },
    custom_fields: (doc.custom_fields ?? []).map((f) => ({ field: f.field, value: f.value })),
  }
}

/** Deep comparison like isEqual in @ngneat/dirty-check-forms (undefined keys do not count) */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true
  if (a == null || b == null) return a == b
  if (typeof a !== 'object' || typeof b !== 'object') return false
  if (Array.isArray(a) !== Array.isArray(b)) return false
  if (Array.isArray(a)) {
    const bb = b as unknown[]
    return a.length === bb.length && a.every((x, i) => deepEqual(x, bb[i]))
  }
  const ka = Object.keys(a as object).filter((k) => (a as any)[k] !== undefined)
  const kb = Object.keys(b as object).filter((k) => (b as any)[k] !== undefined)
  return ka.length === kb.length && ka.every((k) => deepEqual((a as any)[k], (b as any)[k]))
}

/** getChangedFields: only changed ("dirty") controls; permissions_form → owner + set_permissions */
export function changedFields(id: number, form: DocumentFormValue, dirty: Set<string>): Record<string, unknown> & { id: number } {
  const changes: Record<string, unknown> & { id: number } = { id }
  for (const key of FORM_KEYS) {
    if (!dirty.has(key)) continue
    if (key === 'permissions_form') {
      changes.owner = form.permissions_form?.owner
      changes.set_permissions = form.permissions_form?.set_permissions
    } else changes[key] = form[key]
  }
  return changes
}

/** filterDocuments: values of a field → filter rules of the document list */
export function filterRulesFor(items: (DateStruct | { id: number })[], type?: 'correspondent' | 'document_type' | 'storage_path' | 'tag'): FilterRule[] {
  return items.flatMap((i): FilterRule[] => {
    if ('year' in i) {
      const after = new Date(isoToModel(i)!)
      after.setDate(after.getDate() - 1)
      const before = new Date(isoToModel(i)!)
      before.setDate(before.getDate() + 1)
      return [
        { rule_type: FILTER_CREATED_AFTER, value: after.toISOString().substring(0, 10) },
        { rule_type: FILTER_CREATED_BEFORE, value: before.toISOString().substring(0, 10) },
      ]
    }
    const v = String(i.id)
    switch (type) {
      case 'correspondent':
        return [{ rule_type: FILTER_CORRESPONDENT, value: v }]
      case 'document_type':
        return [{ rule_type: FILTER_DOCUMENT_TYPE, value: v }]
      case 'storage_path':
        return [{ rule_type: FILTER_STORAGE_PATH, value: v }]
      case 'tag':
        return [{ rule_type: FILTER_HAS_TAGS_ALL, value: v }]
    }
    return []
  })
}

// ---- Zoom (setZoom/increaseZoom/decreaseZoom/currentZoom) --------------------------------
export interface ZoomState {
  scale: PdfZoomScale
  level: PdfZoomLevel
}
const LEVELS = Object.values(PdfZoomLevel)

export function setZoom(setting: string): ZoomState {
  if (setting === PdfZoomScale.PageFit || setting === PdfZoomScale.PageWidth) return { scale: setting as PdfZoomScale, level: PdfZoomLevel.One }
  return { scale: PdfZoomScale.PageWidth, level: setting as PdfZoomLevel }
}
export function increaseZoom(z: ZoomState): ZoomState {
  let i = LEVELS.indexOf(z.level)
  if (z.scale === PdfZoomScale.PageFit) i = LEVELS.indexOf(PdfZoomLevel.One)
  return { scale: PdfZoomScale.PageWidth, level: LEVELS[Math.min(LEVELS.length - 1, i + 1)] }
}
export function decreaseZoom(z: ZoomState): ZoomState {
  let i = LEVELS.indexOf(z.level)
  if (z.scale === PdfZoomScale.PageFit) i = LEVELS.indexOf(PdfZoomLevel.ThreeQuarters)
  return { scale: PdfZoomScale.PageWidth, level: LEVELS[Math.max(0, i - 1)] }
}
export const currentZoom = (z: ZoomState) => (z.scale === PdfZoomScale.PageFit ? PdfZoomScale.PageFit : z.level)
export const zoomSettings = (): string[] => [PdfZoomScale.PageFit, ...LEVELS]

/** FileSizePipe (default precision: bytes/KB 0, MB/GB 1, TB/PB 2) */
export function fileSize(bytes: number | null | undefined = 0): string {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB', 'PB']
  const precision: Record<string, number> = { bytes: 0, KB: 0, MB: 1, GB: 1, TB: 2, PB: 2 }
  if (bytes == null || isNaN(parseFloat(String(bytes))) || !isFinite(bytes)) return '?'
  let i = 0
  while (bytes >= 1024) {
    bytes /= 1024
    i++
  }
  return `${bytes.toFixed(precision[units[i]])} ${units[i]}`
}
