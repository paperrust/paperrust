// PaperRust-specific: PDF tools redact/shrink (pl-pdftools, docs/contract/pl-pdftools.md §4).
import { api } from '../../../api/client'

/** Rectangle in PDF user coordinates [x0, y0, x1, y1] (point, origin bottom left). */
export type PdfRect = [number, number, number, number]

/** Area on a page (1-based as in the API). */
export interface RedactArea {
  page: number
  rect: PdfRect
}

export interface RedactHit {
  page: number
  term: string
  context: string
  rects: PdfRect[]
}

export interface RedactSearchResult {
  document: number
  version: number
  source: 'original' | 'archive'
  truncated: boolean
  hits: RedactHit[]
}

export interface RedactReport {
  marks: number
  pages: number
  glyphs: number
  images_removed: number
  images_cleared: number
  paths_removed: number
  paths_clipped: number
  annotations_removed: number
  metadata_removed: number
  term_hits: number
}

export interface ShrinkReport {
  preset: string
  size_before: number
  size_after: number
  images: number
  images_resampled: number
  images_recompressed: number
  unused_objects_removed: number
  streams_compressed: number
}

export type ShrinkPreset = 'lossless' | 'standard' | 'strong'

export function redactSearch(documentId: number, terms: string[]) {
  return api.request<RedactSearchResult>('POST', `/api/documents/${documentId}/redact/search/`, { terms })
}

export function redact(documentId: number, areas: RedactArea[], removeMetadata: boolean, versionLabel?: string) {
  return api.request<{ task_id: string; report: RedactReport }>('POST', `/api/documents/${documentId}/redact/`, {
    areas,
    remove_metadata: removeMetadata,
    ...(versionLabel ? { version_label: versionLabel } : {}),
  })
}

export function shrink(documentId: number, preset: ShrinkPreset) {
  return api.request<{ task_id: string | null; new_version: boolean; report: ShrinkReport }>('POST', `/api/documents/${documentId}/shrink/`, { preset })
}

/** Normalize rectangle (x0 < x1, y0 < y1). */
export function normRect(r: PdfRect): PdfRect {
  return [Math.min(r[0], r[2]), Math.min(r[1], r[3]), Math.max(r[0], r[2]), Math.max(r[1], r[3])]
}

/** Affine matrix [a, b, c, d, e, f] like pdf.js `PageViewport.transform`. */
export type Matrix = [number, number, number, number, number, number]

/** PDF-Punkt → Viewport-Pixel (pdf.js `Util.applyTransform`). */
export function toViewport(m: Matrix, x: number, y: number): [number, number] {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
}

/** Viewport pixel → PDF point (inverse of {@link toViewport}). */
export function toPdf(m: Matrix, x: number, y: number): [number, number] {
  const det = m[0] * m[3] - m[1] * m[2]
  const dx = x - m[4]
  const dy = y - m[5]
  return [(m[3] * dx - m[2] * dy) / det, (m[0] * dy - m[1] * dx) / det]
}

/** PDF rectangle → CSS box (left, top, width, height) on the drawn page, also for rotated pages. */
export function cssBox(m: Matrix, r: PdfRect): { left: number; top: number; width: number; height: number } {
  const [ax, ay] = toViewport(m, r[0], r[1])
  const [bx, by] = toViewport(m, r[2], r[3])
  return { left: Math.min(ax, bx), top: Math.min(ay, by), width: Math.abs(bx - ax), height: Math.abs(by - ay) }
}

/** Large enough to be intended (at least 2 × 2 points)? */
export function usableRect(r: PdfRect): boolean {
  const n = normRect(r)
  return n.every(Number.isFinite) && n[2] - n[0] >= 2 && n[3] - n[1] >= 2
}

/** All areas: drawn plus confirmed hits. */
export function collectAreas(drawn: RedactArea[], hits: RedactHit[], confirmed: Set<number>): RedactArea[] {
  const out: RedactArea[] = drawn.map((a) => ({ page: a.page, rect: normRect(a.rect) }))
  hits.forEach((h, i) => {
    if (confirmed.has(i)) for (const r of h.rects) out.push({ page: h.page, rect: normRect(r) })
  })
  return out
}

/** File size readable (B, KB, MB). */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/** Error text from an API response ({detail} or field errors). */
export function errorText(e: unknown): string | undefined {
  const d = (e as { body?: unknown })?.body
  if (!d) return undefined
  if (typeof d === 'string') return d
  if (typeof d === 'object') {
    const o = d as Record<string, unknown>
    if (typeof o.detail === 'string') return o.detail
    const first = Object.values(o)[0]
    if (Array.isArray(first) && typeof first[0] === 'string') return first[0]
  }
  return undefined
}
