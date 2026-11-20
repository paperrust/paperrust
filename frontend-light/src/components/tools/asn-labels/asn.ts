// ASN label generator: API v2 /rs/api/v2/asn/… (crates/pl-api-v2/src/asn.rs, pl-labels) and
// pure helpers (grid, occupied slots, job) – tested in asn.test.ts.
import { api } from '../../../api/client'
import { ApiError, getCookie } from '../../../api/v1'

export const BASE = '/rs/api/v2/asn'

export type CodeType = 'qr' | 'data_matrix' | 'code128'

export interface Sheet {
  id: string
  name: string
  vendor: string
  paper: string
  page_width: number
  page_height: number
  label_width: number
  label_height: number
  cols: number
  rows: number
  margin_left: number
  margin_top: number
  gap_x: number
  gap_y: number
  corner_radius: number
  verified: boolean
  source: string
}

export interface NextAsn {
  documents_next: number
  max_document: number
  max_printed: number
  suggested: number
}

export interface Hint {
  code: string
  message: string
}

export interface AsnConfig {
  barcode: { barcodes_enabled: boolean; asn_barcode_enabled: boolean; retain_split_pages: boolean; prefix: string; separator_string: string }
  split_on_asn: boolean
  next_asn: NextAsn
  max_asn: number
  default_digits: number
  default_preset: string
  max_labels: number
  max_sheets: number
  hints: Hint[]
  /** Switch "Multiple number ranges" (paperrust_asn_config) */
  ranges_enabled: boolean
  location_field_name: string
  can_manage_ranges: boolean
}

/** Number range with occupancy (pl_labels::ranges::RangeStatus). */
export interface AsnRange {
  id: number
  name: string
  code: string
  location: string
  first_asn: number
  last_asn: number
  note: string
  created: string
  modified: string
  used: number
  next_asn: number | null
  remaining: number
}

/** Input for create/change (pl_labels::ranges::RangeInput). */
export interface RangeInput {
  name: string
  code: string
  location: string
  first_asn: number
  last_asn: number
  note: string
}

export interface PrintedRange {
  id: number
  first_asn: number
  last_asn: number
  prefix: string
  digits: number
  code_type: string
  sheet: string
  label_count: number
  user_id: number | null
  username: string | null
  created: string
  used: number
}

export interface Collisions {
  documents: number[]
  documents_total: number
  printed: PrintedRange[]
}

export interface Preview {
  svg: string
  page: number
  pages: number
  per_sheet: number
  count: number
  first_asn: number
  last_asn: number
  first_content: string
  last_content: string
  page_first_asn: number | null
  page_last_asn: number | null
  sheet: Sheet
  skip: number[]
  warnings: Hint[]
  collisions: Collisions
  range: AsnRange | null
}

/** Form state of the page. */
export interface Form {
  preset: string
  custom: Sheet
  code: CodeType
  showText: boolean
  digits: number
  start: number | null
  mode: 'sheets' | 'count'
  sheets: number
  count: number
  skip: number[]
  offsetX: number
  offsetY: number
  outline: boolean
  /** selected number range (only with number ranges enabled) */
  range?: number | null
}

/** Job for /preview and /labels (fields like pl_labels::LabelJob). */
export interface LabelJob {
  preset?: string
  sheet?: Sheet
  code: CodeType
  show_text: boolean
  digits: number
  start?: number
  count?: number
  sheets?: number
  skip: number[]
  offset_x: number
  offset_y: number
  outline: boolean
  range?: number
}

export const CUSTOM = 'custom'

export function emptyCustom(): Sheet {
  return {
    id: CUSTOM,
    name: '',
    vendor: '',
    paper: 'a4',
    page_width: 210,
    page_height: 297,
    label_width: 48.5,
    label_height: 25.4,
    cols: 4,
    rows: 10,
    margin_left: 8,
    margin_top: 21.5,
    gap_x: 0,
    gap_y: 0,
    corner_radius: 0,
    verified: false,
    source: '',
  }
}

const num = (v: unknown, d = 0) => (typeof v === 'number' && Number.isFinite(v) ? v : d)

export function buildJob(f: Form, overrides: Partial<LabelJob> = {}): LabelJob {
  const job: LabelJob = {
    code: f.code,
    show_text: f.showText,
    digits: Math.round(num(f.digits, 5)),
    skip: [...f.skip].sort((a, b) => a - b),
    offset_x: num(f.offsetX),
    offset_y: num(f.offsetY),
    outline: f.outline,
  }
  if (f.preset === CUSTOM) job.sheet = { ...f.custom, id: CUSTOM }
  else job.preset = f.preset
  if (f.start !== null && Number.isFinite(f.start)) job.start = Math.max(0, Math.round(f.start))
  if (f.mode === 'count') job.count = Math.max(1, Math.round(num(f.count, 1)))
  else job.sheets = Math.max(1, Math.round(num(f.sheets, 1)))
  if (f.range != null) job.range = f.range
  return { ...job, ...overrides }
}

/** Position of a label in mm (like SheetLayout::slot, row by row) incl. printer correction. */
export function slotRect(s: Sheet, index: number, dx = 0, dy = 0) {
  const cols = Math.max(1, s.cols)
  const col = index % cols
  const row = Math.floor(index / cols)
  return {
    index,
    x: s.margin_left + col * (s.label_width + s.gap_x) + dx,
    y: s.margin_top + row * (s.label_height + s.gap_y) + dy,
    w: s.label_width,
    h: s.label_height,
  }
}

export function slots(s: Sheet, dx = 0, dy = 0) {
  return Array.from({ length: Math.max(0, s.cols * s.rows) }, (_, i) => slotRect(s, i, dx, dy))
}

/** Mark/free a slot as used; at least one slot stays free. */
export function toggleSkip(skip: number[], index: number, perSheet: number): number[] {
  if (skip.includes(index)) return skip.filter((i) => i !== index)
  const next = [...skip, index].sort((a, b) => a - b)
  return next.length >= perSheet ? skip : next
}

/** Mark all slots before `index` (inclusive) as used – "continue here". */
export function skipUpTo(index: number): number[] {
  return Array.from({ length: Math.max(0, index) }, (_, i) => i)
}

/** Templates grouped by manufacturer (order as delivered). */
export function groupPresets(items: Sheet[]): [string, Sheet[]][] {
  const m = new Map<string, Sheet[]>()
  for (const s of items) {
    const g = m.get(s.vendor) ?? []
    g.push(s)
    m.set(s.vendor, g)
  }
  return [...m.entries()]
}

export function content(prefix: string, digits: number, n: number): string {
  return prefix + String(n).padStart(digits, '0')
}

export function emptyRange(): RangeInput {
  return { name: '', code: '', location: '', first_asn: 0, last_asn: 0, note: '' }
}

/** Suggestion for a new number range: directly after the last one, same size (default 1000). */
export function suggestRange(ranges: Pick<AsnRange, 'first_asn' | 'last_asn'>[]): RangeInput {
  const r = emptyRange()
  const last = ranges.reduce((m, x) => Math.max(m, x.last_asn), 0)
  r.first_asn = ranges.length ? last + 1 : 1
  r.last_asn = r.first_asn + 999
  return r
}

/** Form validation like pl_labels::ranges::validate (the server validates authoritatively). Returns field → error code. */
export function checkRange(r: RangeInput, others: Pick<AsnRange, 'id' | 'name' | 'code' | 'first_asn' | 'last_asn'>[], selfId: number | null): Record<string, string> {
  const e: Record<string, string> = {}
  const code = r.code.trim()
  if (!r.name.trim()) e.name = 'required'
  if (!code) e.code = 'required'
  else if (code.length > 8 || !/^[ -~]+$/.test(code)) e.code = 'code_format'
  const f = Number(r.first_asn)
  const l = Number(r.last_asn)
  if (!Number.isInteger(f) || f < 0) e.first_asn = 'number'
  else if (!Number.isInteger(l) || l < f) e.last_asn = 'order'
  for (const o of others) {
    if (o.id === selfId) continue
    if (code && o.code.toUpperCase() === code.toUpperCase()) e.code = 'code_taken'
    if (!e.first_asn && !e.last_asn && o.first_asn <= l && o.last_asn >= f) e.first_asn = 'overlap'
  }
  return e
}

/** Does the print (start…start+count-1) fit into the range? */
export function fitsRange(r: Pick<AsnRange, 'first_asn' | 'last_asn'>, start: number, count: number): 'ok' | 'outside' | 'overflow' {
  if (start < r.first_asn || start > r.last_asn) return 'outside'
  return start + Math.max(1, count) - 1 > r.last_asn ? 'overflow' : 'ok'
}

/** v2 error from ApiError. */
export function v2Error(e: any): { code: string; message: string; fields: Record<string, string[]>; collisions?: Collisions } {
  const err = e?.body?.error ?? {}
  return { code: err.code ?? 'error', message: err.message ?? String(e?.message ?? e), fields: err.fields ?? {}, collisions: err.collisions }
}

export const getPresets = () => api.get<{ items: Sheet[]; default: string }>(`${BASE}/presets`)
export const getConfig = () => api.get<AsnConfig>(`${BASE}/config`)
export const getPreview = (job: LabelJob, page: number) => api.request<Preview>('POST', `${BASE}/preview?page=${page}`, job)
export const getPrinted = () => api.get<{ items: PrintedRange[] }>(`${BASE}/printed?limit=100`)
export const releasePrinted = (id: number) => api.request<void>('DELETE', `${BASE}/printed/${id}`)
export const putConfig = (body: { ranges_enabled?: boolean; location_field_name?: string }) => api.request<AsnConfig>('PUT', `${BASE}/config`, body)
export const getRanges = () => api.get<{ enabled: boolean; items: AsnRange[] }>(`${BASE}/ranges`)
export const createRange = (r: RangeInput) => api.request<AsnRange>('POST', `${BASE}/ranges`, r)
export const updateRange = (id: number, r: RangeInput) => api.request<AsnRange>('PUT', `${BASE}/ranges/${id}`, r)
export const deleteRange = (id: number) => api.request<void>('DELETE', `${BASE}/ranges/${id}`)

/** POST with JSON body → PDF (blob + file name); errors as ApiError with v2 body. */
async function postPdf(path: string, body: unknown): Promise<{ blob: Blob; filename: string }> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json', Accept: 'application/pdf, application/json' }
  const csrf = getCookie('csrftoken')
  if (csrf) headers['X-CSRFToken'] = csrf
  const res = await fetch(path, { method: 'POST', headers, credentials: 'same-origin', body: JSON.stringify(body) })
  if (!res.ok) {
    let detail: unknown = null
    try {
      detail = await res.json()
    } catch {}
    throw new ApiError(res.status, path, detail)
  }
  const cd = res.headers.get('content-disposition') ?? ''
  const filename = /filename="([^"]+)"/.exec(cd)?.[1] ?? 'labels.pdf'
  return { blob: await res.blob(), filename }
}

export function labelsPdf(job: LabelJob, opts: { allowCollisions?: boolean; record?: boolean } = {}) {
  const q = new URLSearchParams()
  if (opts.allowCollisions) q.set('allow_collisions', 'true')
  if (opts.record === false) q.set('record', 'false')
  const qs = q.toString()
  return postPdf(`${BASE}/labels${qs ? '?' + qs : ''}`, job)
}

export const separatorPdf = (job: { paper: string; code: CodeType; copies: number; lang: string }) => postPdf(`${BASE}/separator`, job)

/** Save blob as file. */
export function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10000)
}
