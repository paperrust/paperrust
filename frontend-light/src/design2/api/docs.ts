// Document calls of the workspace (v1, like the classic UI: same permissions, audit log, workflows).
import { api } from '../../api/client'
import { getCookie } from '../../api/v1'

export type BulkMethod = 'add_tag' | 'remove_tag' | 'modify_tags' | 'set_correspondent' | 'set_document_type' | 'set_storage_path' | 'delete' | 'reprocess' | 'set_permissions'

/** POST /api/documents/bulk_edit/ */
export function bulkEdit(documents: number[], method: BulkMethod, parameters: Record<string, unknown> = {}) {
  return api.request<unknown>('POST', '/api/documents/bulk_edit/', { documents, method, parameters })
}

/** POST /api/documents/bulk_download/ → download ZIP */
export async function bulkDownload(documents: number[], content: 'archive' | 'originals' | 'both' = 'archive') {
  const csrf = getCookie('csrftoken')
  const res = await fetch('/api/documents/bulk_download/', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json; version=10', ...(csrf ? { 'X-CSRFToken': csrf } : {}) },
    body: JSON.stringify({ documents, content, follow_formatting: false }),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const blob = await res.blob()
  const name = /filename="?([^";]+)"?/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ?? 'documents.zip'
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 5000)
}

/** Display of the preview like the document view (getRenderType) */
export function renderType(mime: string | null | undefined, hasArchive: boolean): 'pdf' | 'image' | 'tiff' | 'text' | 'other' {
  if (hasArchive) return 'pdf'
  const m = (mime ?? '').toLowerCase()
  if (m === 'application/pdf') return 'pdf'
  if (m === 'image/tiff') return 'tiff'
  if (m.startsWith('image/')) return 'image'
  if (m.startsWith('text/') || m === 'application/csv') return 'text'
  return 'other'
}
