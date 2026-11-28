// AI area of the PaperRust design: client for /api/x-ai/* (contract docs/contract/pl-ai.md, sections
// "Prüfliste", "Einstellungen KI", "Archiv nachbearbeiten"). Types and thin calls only.
import { api } from '../../api/client'
import { queryString } from '../../api/v1'

export type ReviewField = 'title' | 'tags' | 'correspondent' | 'document_type' | 'storage_path' | 'created' | 'custom_fields'

export interface ReviewChange {
  field: ReviewField
  existing_id: number | null
  new_name: string | null
  value: string | null
  confidence: number
  apply: boolean
  why: string
  data?: unknown
  applied?: boolean
  dismissed?: boolean
  changed_by_user?: boolean
}

export type ReviewStatus = 'pending' | 'partial' | 'applied' | 'rejected' | 'undone'

export interface Review {
  id: number
  document: number
  created: string
  run_id: string
  trigger: 'consumed' | 'tag' | 'workflow' | 'manual'
  mode: 'suggest' | 'apply'
  status: ReviewStatus
  changes: ReviewChange[]
  audit: number | null
  decided_by: number | null
  decided_at: string | null
}

export interface AiStatus {
  enabled: boolean
  configured: boolean
  auto_mode: 'off' | 'suggest' | 'apply'
  trigger_tag: string | null
  pending: number | null
  vision_enabled: boolean
  custom_fields_enabled: boolean
  can_configure: boolean
  can_review: boolean
}

export interface Override {
  value?: unknown
  existing_id?: number
}

export const getStatus = () => api.get<AiStatus>('/api/x-ai/status/')

export const listReviews = (o: { status?: string; document?: number; page?: number; pageSize?: number }) =>
  api.get<{ count: number; results: Review[] }>(
    `/api/x-ai/review/${queryString({ status: o.status, document: o.document, page: o.page ?? 1, page_size: o.pageSize ?? 25 })}`,
  )

/** Accept individual suggestions (with changed values) and/or discard. */
export const applyReview = (id: number, body: { changes?: number[]; overrides?: Record<string, Override>; dismiss?: number[] } = {}) =>
  api.request<Review>('POST', `/api/x-ai/review/${id}/apply/`, body)
export const rejectReview = (id: number) => api.request<Review>('POST', `/api/x-ai/review/${id}/reject/`, {})
export const undoReview = (id: number) => api.request<Review>('POST', `/api/x-ai/review/${id}/undo/`, {})

export interface PromptInfo {
  kind: 'classification' | 'title' | 'custom_fields' | 'vision_ocr' | 'chat_system'
  template: string
  default: string
  custom: boolean
  placeholders: string[]
  required: string[]
  updated: string | null
}
export const listPrompts = () => api.get<{ count: number; results: PromptInfo[] }>('/api/x-ai/prompts/')
export const savePrompt = (kind: string, template: string) => api.request<PromptInfo>('PUT', `/api/x-ai/prompts/${kind}/`, { template })
export const resetPrompt = (kind: string) => api.request<PromptInfo>('DELETE', `/api/x-ai/prompts/${kind}/`)
export const previewPrompt = (kind: string, body: { document?: number; template?: string; run?: boolean }) =>
  api.request<{ prompt: string; result: unknown; error: string | null }>('POST', `/api/x-ai/prompts/${kind}/preview/`, body)

export interface VisionLimits {
  max_pages: number
  max_image_bytes: number
  max_pages_per_day: number
  max_tokens_per_page: number
}
export type TagAfter = { action: 'remove' } | { action: 'keep' } | { action: 'replace'; tag: string }
export interface AiOptions {
  custom_fields_enabled: boolean
  custom_fields: number[]
  vision_enabled: boolean
  vision_model: string
  /** Legacy (before 0.1.4); only used while `vision_mode` is empty. */
  vision_when: 'off' | 'difficult' | 'always'
  /** KI-Abschrift mode; '' = derived from `vision_when`. */
  vision_mode: '' | 'off' | 'uncertain_lines' | 'uncertain_pages' | 'always'
  vision_sensitivity: 'sparing' | 'normal' | 'thorough'
  /** Long side of whole-page images in pixels (512–4096, default 1540). */
  vision_max_side: number
  vision_limits: VisionLimits
  tag_after: TagAfter
  backfill_per_minute: number
  backfill_max_documents: number
}
export interface FieldInfo {
  id: number
  name: string
  data_type: string
  supported: boolean
}
export interface OptionsResponse {
  options: AiOptions
  custom_fields: FieldInfo[]
  /** Mode actually in effect (also when `options.vision_mode` is empty). */
  vision_mode_effective?: Exclude<AiOptions['vision_mode'], ''>
  /** The LLM endpoint is not on the local network (page images would leave the house); null = unknown. */
  vision_remote?: boolean | null
}
export const getOptions = () => api.get<OptionsResponse>('/api/x-ai/options/')
export const patchOptions = (values: Partial<AiOptions>) => api.request<OptionsResponse>('PATCH', '/api/x-ai/options/', values)

export interface ConnectionCheck {
  ok: boolean
  models: string[]
  model_available: boolean | null
  error: string | null
}
export const checkProvider = (body: { backend?: string; endpoint?: string; model?: string; api_key?: string }) =>
  api.request<ConnectionCheck>('POST', '/api/x-ai/provider/check/', body)

export interface JobFilter {
  tags?: number[]
  without_correspondent?: boolean
  without_document_type?: boolean
  without_tags?: boolean
  created_from?: string | null
  created_to?: string | null
  only_unprocessed?: boolean
  max_documents?: number
}
export interface Job {
  id: number
  created: string | null
  mode: 'suggest' | 'apply'
  filter: JobFilter
  status: 'queued' | 'running' | 'done' | 'cancelled' | 'failed' | 'interrupted'
  total: number
  processed: number
  applied: number
  suggested: number
  errors: number
  cancel_requested: boolean
  finished: string | null
  message: string | null
}
export const listJobs = () => api.get<{ count: number; results: Job[] }>('/api/x-ai/jobs/')
export const previewJob = (filter: JobFilter) => api.request<{ count: number; limit: number; per_minute: number }>('POST', '/api/x-ai/jobs/preview/', { filter })
export const createJob = (mode: 'suggest' | 'apply', filter: JobFilter) => api.request<Job>('POST', '/api/x-ai/jobs/', { mode, filter })
export const cancelJob = (id: number) => api.request<Job>('POST', `/api/x-ai/jobs/${id}/cancel/`, {})
export const resumeJob = (id: number) => api.request<Job>('POST', `/api/x-ai/jobs/${id}/resume/`, {})
