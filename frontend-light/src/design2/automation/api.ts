// "Automation" area: client for /rs/api/v2/automation (contract docs/contract/automation.md §3) and the
// v1 paths that the automation also uses (deleting a workflow, mail accounts/rules, groups).
// Types and thin calls only – logic is in model.ts / sentence.ts.
import { api } from '../../api/client'
import { ApiError, queryString } from '../../api/v1'

export const AUTO = '/rs/api/v2/automation'

// ---- Shapes ---------------------------------------------------------------------------------------

export type TriggerKey = 'consumption' | 'document_added' | 'document_updated' | 'scheduled' | 'manual' | 'unknown'
export type ActionKey = 'assignment' | 'removal' | 'email' | 'webhook' | 'password_removal' | 'move_to_trash' | 'remote_ocr' | 'ai_suggestions' | 'unknown'

export interface NamedRef {
  id: number
  name: string | null
}

/** Condition of the short form (kind + values) */
export interface SummaryCondition {
  kind: string
  value?: unknown
  values?: number[]
  algorithm?: number
  insensitive?: boolean
  object?: NamedRef
  objects?: NamedRef[]
}
export interface SummaryTrigger {
  id: number | null
  type: number
  type_key: TriggerKey
  conditions: SummaryCondition[]
  schedule: null | {
    offset_days: number | null
    is_recurring: boolean | null
    recurring_interval_days: number | null
    date_field: string | null
    custom_field: NamedRef | null
  }
}
export interface SummaryItem {
  kind: string
  value?: unknown
  object?: NamedRef
  objects?: NamedRef[]
  values?: unknown
  to?: string | null
  subject?: string | null
  include_document?: boolean | null
  url?: string | null
  as_json?: boolean | null
  fields?: string[] | null
  create_missing?: boolean | null
  overwrite_existing?: boolean | null
}
export interface SummaryAction {
  id: number | null
  type: number
  type_key: ActionKey
  items: SummaryItem[]
}
export interface SummaryAi {
  field: AiField
  object: NamedRef
  min_confidence: number
  active: boolean
  inactive_reason: 'ai_unavailable' | 'ai_disabled' | 'consumption_only' | null
}
export interface Summary {
  trigger_types: number[]
  triggers: SummaryTrigger[]
  ai_condition: SummaryAi | null
  actions: SummaryAction[]
  else_actions: SummaryAction[]
}

export type AiField = 'document_type' | 'correspondent' | 'storage_path' | 'tags'
export interface AiCondition {
  field: AiField
  object_id: number
  min_confidence: number
}

/** ngx workflow like /api/workflows/ (triggers/actions as open objects) */
export interface NgxWorkflow {
  id?: number | null
  name: string | null
  order: number | null
  enabled: boolean
  triggers: Record<string, any>[]
  actions: Record<string, any>[]
}

export interface CombinedWorkflow {
  workflow: NgxWorkflow
  else_actions: Record<string, any>[]
  ai_condition: AiCondition | null
  ai: { available: boolean; enabled: boolean }
  summary: Summary
}

export interface ListItem {
  id: number
  name: string
  order: number
  enabled: boolean
  runs_this_week: number
  last_run: { id: number; at: string; result: RunResult; code: string } | null
  has_else: boolean
  summary: Summary
}

export type RunResult = 'ok' | 'skipped' | 'error'
export interface RunItem {
  id: number
  trigger_type: number
  trigger_key: TriggerKey
  document: { id: number; title: string | null; in_trash?: boolean; deleted?: boolean } | { id: null; hidden: true } | null
  run_at: string
  result: RunResult
  branch: 'main' | 'else' | 'none'
  code: string
  message: string
  changed_fields: string[]
  source: number | null
}
export interface RunsPage {
  items: RunItem[]
  next_cursor: number | null
  counts: { this_week: number; errors_this_week: number; total: number }
  retention_days: number
}

export type DryResult = 'main' | 'else' | 'skipped' | 'error'
export interface DryChange {
  field: string
  before?: unknown
  after?: unknown
  added?: unknown[]
  removed?: unknown
  custom_field?: unknown
}
export interface DryDoc {
  id: number
  title: string
  added: string | null
  result: DryResult
  code: string
  reason: string | null
  changes: DryChange[]
  effects: { action_id: number | null; kind: 'email' | 'webhook' | 'password_removal' | 'trash' | 'ai_suggestions'; target: string | null }[]
  before: Record<string, any> | null
  after: Record<string, any> | null
}
export interface DryRun {
  workflow_id: number | null
  trigger_type: number
  trigger_key: TriggerKey
  approximate: boolean
  ai: { available: boolean; enabled: boolean; checked: boolean }
  notes: string[]
  counts: { main: number; else: number; skipped: number; error: number; checked: number }
  documents: DryDoc[]
}

export interface Suggestion {
  key: string
  kind: string
  from: { field: string; id?: number; name?: string | null; value?: string }
  to: { field: string; id: number; name: string | null }
  matches: number
  total: number
  share: number
  covered_by_workflow: boolean
}

export interface MailFlowListItem {
  mail_rule: { id: number; name: string; enabled: boolean; account: number | null }
  workflows: { id: number; name: string; enabled: boolean }[]
}
export interface MailFlow {
  mail_rule: Record<string, any>
  workflows: CombinedWorkflow[]
}

/** Workflow body for saving (ngx + extensions) */
export interface SaveBody extends NgxWorkflow {
  else_actions: Record<string, any>[]
  ai_condition: AiCondition | null
}

// ---- Errors ---------------------------------------------------------------------------------------

/** Error shape v2 {error:{code,message,fields}} → code + field errors (DRF shape like /api/workflows/) */
export function errorOf(e: unknown): { status: number; code: string; message: string; fields: Record<string, any> | null } {
  if (e instanceof ApiError) {
    const b = (e.body ?? {}) as { error?: { code?: string; message?: string; fields?: Record<string, any> } }
    return { status: e.status, code: b.error?.code ?? 'internal', message: b.error?.message ?? '', fields: b.error?.fields ?? null }
  }
  return { status: 0, code: 'network', message: '', fields: null }
}

// ---- Calls --------------------------------------------------------------------------------------

export const listWorkflows = () => api.get<{ items: ListItem[]; week_start: string }>(`${AUTO}/workflows`)
export const getWorkflow = (id: number) => api.get<CombinedWorkflow>(`${AUTO}/workflows/${id}`)
export const createWorkflow = (b: SaveBody) => api.request<CombinedWorkflow>('POST', `${AUTO}/workflows`, b)
export const putWorkflow = (id: number, b: SaveBody) => api.request<CombinedWorkflow>('PUT', `${AUTO}/workflows/${id}`, b)
export const patchWorkflow = (id: number, b: Partial<SaveBody>) => api.request<CombinedWorkflow>('PATCH', `${AUTO}/workflows/${id}`, b)
/** Delete via v1 (also deletes the else branch, contract §3.3) */
export const deleteWorkflow = (id: number) => api.request<void>('DELETE', `/api/workflows/${id}/`)

export const listRuns = (id: number, o: { cursor?: number | null; limit?: number; result?: RunResult | null } = {}) =>
  api.get<RunsPage>(`${AUTO}/workflows/${id}/runs${queryString({ cursor: o.cursor ?? undefined, limit: o.limit ?? 50, result: o.result ?? undefined })}`)

export interface DryRunBody {
  workflow?: SaveBody
  workflow_id?: number
  limit?: number
  document_ids?: number[]
  trigger_type?: number
  with_ai?: boolean
}
export const dryRun = (b: DryRunBody) => api.request<DryRun>('POST', `${AUTO}/dry-run`, b)
export const applyWorkflow = (id: number, b: { document_ids: number[]; trigger_type?: number }) =>
  api.request<{ task_id: string; documents: number; trigger_type: number; filter_documents: boolean }>('POST', `${AUTO}/workflows/${id}/apply`, b)

export const listSuggestions = (refresh = false) =>
  api.get<{ items: Suggestion[]; generated_at: string; min_documents: number; min_share: number; cached: boolean }>(
    `${AUTO}/suggestions${queryString({ refresh: refresh ? 'true' : undefined })}`,
  )
export const draftFromSuggestion = (key: string) => api.request<{ workflow: NgxWorkflow }>('POST', `${AUTO}/suggestions/draft`, { key })
export const draftFromDescription = (description: string) =>
  api.request<{ workflow: NgxWorkflow | null; missing?: unknown[]; explanation?: string | null; dropped?: unknown[] }>('POST', `${AUTO}/draft-from-description`, { description })

export const listMailFlows = () => api.get<{ items: MailFlowListItem[] } | MailFlowListItem[]>(`${AUTO}/mail-flows`)
export const getMailFlow = (rule: number) => api.get<MailFlow>(`${AUTO}/mail-flows/${rule}`)
export interface MailFlowBody {
  mail_rule?: Record<string, any>
  workflows?: SaveBody[]
  delete_workflow_ids?: number[]
}
export const createMailFlow = (b: MailFlowBody) => api.request<MailFlow>('POST', `${AUTO}/mail-flows`, b)
export const putMailFlow = (rule: number, b: MailFlowBody) => api.request<MailFlow>('PUT', `${AUTO}/mail-flows/${rule}`, b)
export const patchMailFlow = (rule: number, b: MailFlowBody) => api.request<MailFlow>('PATCH', `${AUTO}/mail-flows/${rule}`, b)
