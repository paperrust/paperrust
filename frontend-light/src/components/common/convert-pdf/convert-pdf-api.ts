// PaperRust-specific: "Convert to PDF" for images and office files (docs/contract/pl-pdftools.md §7).
import { api } from '../../../api/client'

export interface ConvertResult {
  task_id: string
  document: number
  source_version: number
  kind: 'image' | 'office'
  ocr: boolean
}

/** Image formats the server converts (not HEIC). */
const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/tiff', 'image/bmp', 'image/gif', 'image/webp']

/** Word-processing formats for WordCraft (like pl-pdftools `OFFICE_FORMATS`). */
const OFFICE_TYPES = [
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  'application/vnd.ms-word.document.macroenabled.12',
  'application/vnd.openxmlformats-officedocument.wordprocessingml.template',
  'application/vnd.oasis.opendocument.text',
  'application/vnd.oasis.opendocument.text-template',
  'text/rtf',
  'application/rtf',
  'text/markdown',
  'text/html',
]

function norm(mime: string | null | undefined): string {
  return (mime ?? '').split(';')[0].trim().toLowerCase()
}

export function isImage(mime: string | null | undefined): boolean {
  return IMAGE_TYPES.includes(norm(mime))
}

/** Can a document of this type be converted to a PDF? */
export function isConvertible(mime: string | null | undefined): boolean {
  const m = norm(mime)
  return IMAGE_TYPES.includes(m) || OFFICE_TYPES.includes(m)
}

export function convertToPdf(documentId: number, ocr: boolean, versionLabel?: string) {
  return api.request<ConvertResult>('POST', `/api/documents/${documentId}/convert_pdf/`, {
    ocr,
    ...(versionLabel ? { version_label: versionLabel } : {}),
  })
}

export interface ConvertSummary {
  done: number[]
  failed: { id: number; message: string }[]
}

/**
 * Convert several documents one after another (the server computes per document; one after another so
 * a small server is not loaded with many OCR runs at once).
 */
export async function convertMany(
  ids: number[],
  ocr: boolean,
  run: (id: number, ocr: boolean) => Promise<unknown>,
  errorText: (e: unknown) => string | undefined,
  onprogress?: (n: number) => void,
): Promise<ConvertSummary> {
  const out: ConvertSummary = { done: [], failed: [] }
  for (const [i, id] of ids.entries()) {
    try {
      await run(id, ocr)
      out.done.push(id)
    } catch (e) {
      out.failed.push({ id, message: errorText(e) ?? '' })
    }
    onprogress?.(i + 1)
  }
  return out
}
