// Scanner integration (PaperRust extension): API /api/x-scan/… (crates/pl-scan/src/api.rs) and pure helpers
// for the pages "Scan" (tools/scan) and "Scanners" (tools/scanners).
import { api } from '../../../api/client'
import { ApiError } from '../../../api/v1'

export type SourceId = 'flatbed' | 'adf' | 'adf_duplex'
export type ColorId = 'color' | 'grayscale' | 'bw'
export type JobStateId = 'scanning' | 'processing' | 'done' | 'failed' | 'cancelled'

export interface Scanner {
  id: number
  name: string
  model: string | null
  base_url?: string
  uuid?: string | null
}

export interface SourceCaps {
  source: SourceId
  color_modes: ColorId[]
  resolutions: number[]
  formats: string[]
  max_width: number
  max_height: number
}

export interface Capabilities {
  scanner: number
  make_and_model: string | null
  sources: SourceCaps[]
  state: string | null
  adf_state: string | null
}

export interface ScanJob {
  id: string
  scanner: number
  source: SourceId
  state: JobStateId
  pages: number
  error: { code: string; message: string } | null
  task_id: string | null
  created: string
}

export interface Found {
  name: string
  base_url: string
  model: string | null
  uuid: string | null
  tls: boolean
  known: boolean
}

export interface Device {
  id: number
  name: string
  token_hint: string
  owner_id: number
  owner_username: string | null
  allowed_ips: string
  created: string
  revoked_at: string | null
  last_used: string | null
  last_ip: string | null
  documents_received: number
  token?: string
}

export interface SmtpInfo {
  enabled: boolean
  listen: string | null
  port: number | null
  tls: boolean
  require_tls: boolean
  implicit_tls: boolean
  max_mb: number | null
  error: string | null
}

interface List<T> {
  count: number
  results: T[]
}

const BASE = '/api/x-scan'

export const listScanners = () => api.get<List<Scanner>>(`${BASE}/scanners/`).then((r) => r.results)
export const capabilities = (id: number) => api.get<Capabilities>(`${BASE}/scanners/${id}/capabilities/`)
export const addScanner = (url: string, name?: string) => api.request<Scanner>('POST', `${BASE}/scanners/`, name ? { url, name } : { url })
export const renameScanner = (id: number, name: string) => api.request<Scanner>('PATCH', `${BASE}/scanners/${id}/`, { name })
export const deleteScanner = (id: number) => api.request<void>('DELETE', `${BASE}/scanners/${id}/`)
export const discover = () => api.get<List<Found> & { error?: string }>(`${BASE}/discover/`)

export interface ScanRequest {
  scanner: number
  source: SourceId
  resolution?: number
  color_mode?: ColorId
  title?: string
}
export const startScan = (req: ScanRequest) => api.request<ScanJob>('POST', `${BASE}/jobs/`, req)
export const getJob = (id: string) => api.get<ScanJob>(`${BASE}/jobs/${id}/`)
export const cancelJob = (id: string) => api.request<{ cancel_requested: boolean; job: ScanJob }>('POST', `${BASE}/jobs/${id}/cancel/`)

export const listDevices = () => api.get<List<Device> & { smtp: SmtpInfo }>(`${BASE}/devices/`)
export const createDevice = (name: string, owner: number, allowed_ips: string) => api.request<Device>('POST', `${BASE}/devices/`, { name, owner, allowed_ips })
export const revokeDevice = (id: number) => api.request<{ revoked: boolean }>('POST', `${BASE}/devices/${id}/revoke/`)
export const deleteDevice = (id: number) => api.request<void>('DELETE', `${BASE}/devices/${id}/`)

/** i18n key per error code of the server (ScanError::code). */
export const ERROR_KEYS: Record<string, string> = {
  unreachable: 'scan.error_unreachable',
  busy: 'scan.error_busy',
  adf_empty: 'scan.error_adf_empty',
  jam: 'scan.error_jam',
  cancelled: 'scan.error_cancelled',
  unsupported: 'scan.error_unsupported',
  protocol: 'scan.error_protocol',
  too_large: 'scan.error_too_large',
  pdf: 'scan.error_pdf',
  intake: 'scan.error_intake',
}

/** Error code from an API response or a job. */
export function errorCode(e: unknown): string {
  if (e instanceof ApiError && e.body && typeof e.body === 'object' && 'code' in e.body) return String((e.body as { code: unknown }).code)
  if (e && typeof e === 'object' && 'code' in e) return String((e as { code: unknown }).code)
  return 'protocol'
}

/** First field message of a DRF validation response (e.g. {"url": ["…"]}). */
export function fieldMessage(e: unknown): string {
  if (e instanceof ApiError && e.body && typeof e.body === 'object') {
    for (const v of Object.values(e.body as Record<string, unknown>)) {
      if (Array.isArray(v) && v.length) return String(v[0])
      if (typeof v === 'string') return v
    }
  }
  return e instanceof Error ? e.message : String(e)
}

export const SOURCE_KEYS: Record<SourceId, string> = {
  flatbed: 'scan.source_flatbed',
  adf: 'scan.source_adf',
  adf_duplex: 'scan.source_adf_duplex',
}

export const COLOR_KEYS: Record<ColorId, string> = {
  color: 'scan.color_color',
  grayscale: 'scan.color_grayscale',
  bw: 'scan.color_bw',
}

/** Selection from the capabilities: source (feeder before glass), duplex as switch, valid values. */
export interface Choice {
  sources: SourceId[] // without adf_duplex (duplex is a switch)
  duplex: boolean
  resolutions: number[]
  colors: ColorId[]
}

export function choices(caps: Capabilities | null, source: SourceId): Choice {
  const all = caps?.sources ?? []
  const has = (s: SourceId) => all.some((c) => c.source === s)
  const sources = (['adf', 'flatbed'] as SourceId[]).filter(has)
  const sc = all.find((c) => c.source === source) ?? all.find((c) => c.source === 'adf') ?? all[0]
  return {
    sources,
    duplex: has('adf_duplex'),
    resolutions: sc?.resolutions ?? [],
    colors: sc?.color_modes ?? [],
  }
}

/** Effective source from selection + duplex switch. */
export function effectiveSource(source: SourceId, duplex: boolean): SourceId {
  return source === 'adf' && duplex ? 'adf_duplex' : source
}

/** Default resolution: 300, otherwise the next smaller one, otherwise the smallest. */
export function defaultResolution(list: number[]): number | undefined {
  if (!list.length) return undefined
  if (list.includes(300)) return 300
  const lower = list.filter((r) => r < 300)
  return lower.length ? lower[lower.length - 1] : list[0]
}

export const isFinished = (s: JobStateId) => s === 'done' || s === 'failed' || s === 'cancelled'

/** SMTP address for the instructions: host name of the page + port. */
export function smtpHost(info: SmtpInfo | null, hostname: string): string {
  return info?.port ? `${hostname}:${info.port}` : hostname
}
