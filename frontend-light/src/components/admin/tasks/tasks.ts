// Logic of admin/tasks/tasks.component.ts + TasksService (framework-free for Vitest).
import { msg, t } from '../../../lib/i18n'

export type TaskSection = 'all' | 'needs_attention' | 'in_progress' | 'completed'
export const SECTIONS: TaskSection[] = ['needs_attention', 'in_progress', 'completed']
export const FILTER_TARGETS = [
  { id: 0, name: msg('common.name') },
  { id: 1, name: msg('admin.tasks.result') },
]
export const SECTION_LABELS: Record<TaskSection, string> = {
  all: msg('common.all'),
  needs_attention: msg('admin.tasks.needs_attention'),
  in_progress: msg('admin.tasks.in_progress'),
  completed: msg('admin.tasks.recently_completed'),
}
export const TASK_TYPE_OPTIONS = [
  { value: 'consume_file', label: msg('admin.tasks.consume_file') },
  { value: 'train_classifier', label: msg('admin.tasks.train_classifier') },
  { value: 'sanity_check', label: msg('admin.tasks.sanity_check') },
  { value: 'mail_fetch', label: msg('common.mail_fetch') },
  { value: 'llm_index', label: msg('admin.tasks.llm_index') },
  { value: 'empty_trash', label: msg('common.empty_trash') },
  { value: 'check_workflows', label: msg('admin.tasks.check_workflows') },
  { value: 'bulk_update', label: msg('admin.tasks.bulk_update') },
  { value: 'reprocess_document', label: msg('admin.tasks.reprocess_document') },
  { value: 'build_share_link', label: msg('admin.tasks.build_share_link') },
  { value: 'bulk_delete', label: msg('admin.tasks.bulk_delete') },
  { value: 'apply_ai_suggestions', label: msg('common.apply_ai_suggestions') },
]
export const TRIGGER_SOURCE_OPTIONS = [
  { value: 'scheduled', label: msg('common.scheduled') },
  { value: 'web_ui', label: msg('common.web_ui') },
  { value: 'api_upload', label: msg('common.api_upload') },
  { value: 'folder_consume', label: msg('admin.tasks.folder_consume') },
  { value: 'email_consume', label: msg('admin.tasks.email_consume') },
  { value: 'system', label: msg('common.system') },
  { value: 'manual', label: msg('common.manual') },
]

export interface PaperlessTask {
  id: number
  task_id?: string
  task_type: string
  task_type_display: string
  trigger_source: string
  trigger_source_display: string
  status: string
  date_created?: string
  input_data?: Record<string, any> | null
  result_data?: Record<string, any> | null
  related_document_ids?: number[] | null
}

export interface TaskFilters {
  taskType: string | null
  triggerSource: string | null
  filterText: string
  filterTargetID: number
}

export function statusesForSection(section: TaskSection): string[] {
  switch (section) {
    case 'needs_attention':
      return ['failure', 'revoked']
    case 'in_progress':
      return ['pending', 'started']
    case 'completed':
      return ['success']
    default:
      return []
  }
}

export const taskBelongsToSection = (task: PaperlessTask, section: TaskSection) => statusesForSection(section).includes(task.status)

/** getParamsForSection: order like HttpParams (acknowledged, status…, task_type, trigger_source, name/result) */
export function paramsForSection(section: TaskSection, f: TaskFilters): [string, string][] {
  const p: [string, string][] = [['acknowledged', 'false']]
  for (const s of statusesForSection(section)) p.push(['status', s])
  if (f.taskType !== null) p.push(['task_type', f.taskType])
  if (f.triggerSource !== null) p.push(['trigger_source', f.triggerSource])
  if (f.filterText.length) p.push([f.filterTargetID === 0 ? 'name' : 'result', f.filterText])
  return p
}

export const query = (p: [string, string][]) => (p.length ? '?' + p.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join('&') : '')

/**
 * Task type/source in the UI language: the server translates *_display according to Accept-Language (browser
 * language), not according to the selected UI language → German names in an English UI. Therefore via the
 * key task_type/trigger_source from the own catalog; unknown values → server label.
 */
export function taskTypeLabel(task: Pick<PaperlessTask, 'task_type' | 'task_type_display'>): string {
  const o = TASK_TYPE_OPTIONS.find((x) => x.value === task.task_type)
  return o ? t(o.label) : task.task_type_display
}
export function triggerSourceLabel(task: Pick<PaperlessTask, 'trigger_source' | 'trigger_source_display'>): string {
  const o = TRIGGER_SOURCE_OPTIONS.find((x) => x.value === task.trigger_source)
  return o ? t(o.label) : task.trigger_source_display
}

export function taskDisplayName(task: PaperlessTask): string {
  return task.input_data?.filename?.toString() || taskTypeLabel(task)
}

export function taskResultMessage(task: PaperlessTask): string | null {
  const r = task.result_data
  if (!r) return null
  if (typeof r['document_id'] === 'number') return t('admin.tasks.success_new_document_id', { PH: r['document_id'] })
  if (typeof r['reason'] === 'string') return r['reason']
  if (typeof r['duplicate_of'] === 'number') return t('admin.tasks.duplicate_document', { PH: r['duplicate_of'] })
  if (typeof r['error_message'] === 'string') return r['error_message']
  return null
}

export function taskResultPreview(task: PaperlessTask): string | null {
  const m = taskResultMessage(task)
  if (!m) return null
  return m.length > 50 ? `${m.slice(0, 50)}...` : m
}

export const duplicateDocumentId = (task: PaperlessTask): number | null => (typeof task.result_data?.['duplicate_of'] === 'number' ? task.result_data['duplicate_of'] : null)

/**
 * Type and trigger filter in the browser. Only the server filters the text (`name`/`result`):
 * upstream fix fefce7be1 (#14736, paperless-ngx `dev` after v3.4.2) – the second filter in the
 * browser compared the translated display texts (e.g. "E-Mail") and thus discarded hits that the
 * server had already found.
 */
export function taskMatchesFilters(task: PaperlessTask, f: TaskFilters): boolean {
  if (f.taskType !== null && task.task_type !== f.taskType) return false
  if (f.triggerSource !== null && task.trigger_source !== f.triggerSource) return false
  return true
}

/** Angular JsonPipe: JSON.stringify(v, null, 2) */
export const json = (v: unknown) => JSON.stringify(v, null, 2)
