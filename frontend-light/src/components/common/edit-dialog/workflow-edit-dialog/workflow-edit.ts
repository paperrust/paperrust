// Logic of common/edit-dialog/workflow-edit-dialog/workflow-edit-dialog.component.ts (framework-free for Vitest):
// Selection lists, trigger filters (TRIGGER_FILTER_DEFINITIONS/FILTER_HANDLERS), form from workflow, sent value.
import { msg } from '../../../../lib/i18n'

// Manual (5): PaperRust, idea from paperless-ngx dev #14704 – runs only on button press (document list/view)
export const WorkflowTriggerType = { Consumption: 1, DocumentAdded: 2, DocumentUpdated: 3, Scheduled: 4, Manual: 5 } as const
/** Trigger on existing documents (content matching and advanced filters possible). */
export const triggerHasDocumentFilters = (type: number) =>
  type === WorkflowTriggerType.DocumentAdded || type === WorkflowTriggerType.DocumentUpdated || type === WorkflowTriggerType.Scheduled || type === WorkflowTriggerType.Manual
export const WorkflowActionType = { Assignment: 1, Removal: 2, Email: 3, Webhook: 4, PasswordRemoval: 5, MoveToTrash: 6, RemoteOcr: 7, ApplyAiSuggestions: 8 } as const
export const MATCH_NONE = 0
export const MATCH_AUTO = 6

export const DOCUMENT_SOURCE_OPTIONS = [
  { id: 1, name: msg('dialog.workflow.consume_folder') },
  { id: 2, name: msg('common.api_upload') },
  { id: 3, name: msg('common.mail_fetch') },
  { id: 4, name: msg('common.web_ui') },
]
export const SCHEDULE_DATE_FIELD_OPTIONS = [
  { id: 'added', name: msg('common.added') },
  { id: 'created', name: msg('common.created') },
  { id: 'modified', name: msg('common.modified') },
  { id: 'custom_field', name: msg('dialog.workflow.custom_field') },
]
export const WORKFLOW_TYPE_OPTIONS = [
  { id: WorkflowTriggerType.Consumption, name: msg('dialog.workflow.consumption_started') },
  { id: WorkflowTriggerType.DocumentAdded, name: msg('dialog.workflow.document_added') },
  { id: WorkflowTriggerType.DocumentUpdated, name: msg('dialog.workflow.document_updated') },
  { id: WorkflowTriggerType.Scheduled, name: msg('common.scheduled') },
  { id: WorkflowTriggerType.Manual, name: msg('common.manual') },
]
export const WORKFLOW_ACTION_OPTIONS = [
  { id: WorkflowActionType.Assignment, name: msg('dialog.workflow.assignment') },
  { id: WorkflowActionType.Removal, name: msg('dialog.workflow.removal') },
  { id: WorkflowActionType.Email, name: msg('common.email') },
  { id: WorkflowActionType.Webhook, name: msg('dialog.workflow.webhook') },
  { id: WorkflowActionType.PasswordRemoval, name: msg('dialog.workflow.password_removal') },
  { id: WorkflowActionType.MoveToTrash, name: msg('common.move_trash') },
  { id: WorkflowActionType.RemoteOcr, name: msg('common.remote_ocr') },
  { id: WorkflowActionType.ApplyAiSuggestions, name: msg('common.apply_ai_suggestions') },
]
export const AI_SUGGESTION_FIELD_OPTIONS = [
  { id: 'title', name: msg('common.title') },
  { id: 'tags', name: msg('common.tags') },
  { id: 'correspondent', name: msg('common.correspondent') },
  { id: 'document_type', name: msg('common.document_type') },
  { id: 'storage_path', name: msg('common.storage_path') },
  { id: 'created', name: msg('dialog.workflow.created_date') },
]

export type TriggerFilterType =
  | 'tags_any' | 'tags_all' | 'tags_none'
  | 'correspondent_any' | 'correspondent_is' | 'correspondent_not'
  | 'document_type_any' | 'document_type_is' | 'document_type_not'
  | 'storage_path_any' | 'storage_path_is' | 'storage_path_not'
  | 'custom_field_query'

export interface TriggerFilterDefinition {
  id: TriggerFilterType
  name: string
  inputType: 'tags' | 'select' | 'customFieldQuery'
  allowMultipleEntries: boolean
  allowMultipleValues: boolean
  selectItems?: 'correspondents' | 'documentTypes' | 'storagePaths'
}

const def = (id: TriggerFilterType, name: string, inputType: TriggerFilterDefinition['inputType'], allowMultipleValues: boolean, selectItems?: TriggerFilterDefinition['selectItems']): TriggerFilterDefinition => ({
  id, name, inputType, allowMultipleEntries: false, allowMultipleValues, ...(selectItems ? { selectItems } : {}),
})

export const TRIGGER_FILTER_DEFINITIONS: TriggerFilterDefinition[] = [
  def('tags_any', msg('dialog.workflow.any_tags'), 'tags', true),
  def('tags_all', msg('dialog.workflow.all_tags'), 'tags', true),
  def('tags_none', msg('dialog.workflow.not_tags'), 'tags', true),
  def('correspondent_any', msg('dialog.workflow.any_correspondents'), 'select', true, 'correspondents'),
  def('correspondent_is', msg('dialog.workflow.correspondent'), 'select', false, 'correspondents'),
  def('correspondent_not', msg('dialog.workflow.not_correspondents'), 'select', true, 'correspondents'),
  def('document_type_is', msg('dialog.workflow.document_type'), 'select', false, 'documentTypes'),
  def('document_type_any', msg('dialog.workflow.any_document_types'), 'select', true, 'documentTypes'),
  def('document_type_not', msg('dialog.workflow.not_document_types'), 'select', true, 'documentTypes'),
  def('storage_path_is', msg('dialog.workflow.storage_path'), 'select', false, 'storagePaths'),
  def('storage_path_any', msg('dialog.workflow.any_storage_paths'), 'select', true, 'storagePaths'),
  def('storage_path_not', msg('dialog.workflow.not_storage_paths'), 'select', true, 'storagePaths'),
  def('custom_field_query', msg('dialog.workflow.matches_custom_field_query'), 'customFieldQuery', false),
]

/** Filter type ↔ trigger field; "single" = single value (…_is) */
const FIELD: Record<TriggerFilterType, [string, 'multi' | 'single' | 'query']> = {
  tags_any: ['filter_has_tags', 'multi'],
  tags_all: ['filter_has_all_tags', 'multi'],
  tags_none: ['filter_has_not_tags', 'multi'],
  correspondent_any: ['filter_has_any_correspondents', 'multi'],
  correspondent_is: ['filter_has_correspondent', 'single'],
  correspondent_not: ['filter_has_not_correspondents', 'multi'],
  document_type_is: ['filter_has_document_type', 'single'],
  document_type_any: ['filter_has_any_document_types', 'multi'],
  document_type_not: ['filter_has_not_document_types', 'multi'],
  storage_path_is: ['filter_has_storage_path', 'single'],
  storage_path_any: ['filter_has_any_storage_paths', 'multi'],
  storage_path_not: ['filter_has_not_storage_paths', 'multi'],
  custom_field_query: ['filter_custom_field_query', 'query'],
}

export const getFilterDefinition = (type: TriggerFilterType) => TRIGGER_FILTER_DEFINITIONS.find((d) => d.id === type)
export const isTagsFilter = (type: TriggerFilterType) => getFilterDefinition(type)?.inputType === 'tags'
export const isCustomFieldQueryFilter = (type: TriggerFilterType) => getFilterDefinition(type)?.inputType === 'customFieldQuery'
export const isMultiValueFilter = (type: TriggerFilterType) => FIELD[type]?.[1] === 'multi'
export const isSelectMultiple = (type: TriggerFilterType) => !isTagsFilter(type) && isMultiValueFilter(type)

function hasValue(type: TriggerFilterType, v: any): boolean {
  const kind = FIELD[type][1]
  if (kind === 'multi') return Array.isArray(v) && v.length > 0
  if (kind === 'single') return v !== null && v !== undefined
  return typeof v === 'string' && v.trim().length > 0
}

export const defaultFilterValue = (type: TriggerFilterType) => (type === 'custom_field_query' ? null : isMultiValueFilter(type) ? [] : null)

export function normalizeFilterValue(type: TriggerFilterType, value?: any) {
  if (value === undefined || value === null) return defaultFilterValue(type)
  if (type === 'custom_field_query') return typeof value === 'string' ? value : value ? JSON.stringify(value) : null
  if (isMultiValueFilter(type)) return Array.isArray(value) ? [...value] : [value]
  if (Array.isArray(value)) return value.length > 0 ? value[0] : null
  return value
}

export interface FilterForm {
  type: TriggerFilterType
  values: any
}

/** buildFiltersFormArray: one filter per definition with value, in definition order */
export function buildFilters(trigger: Record<string, any>): FilterForm[] {
  const out: FilterForm[] = []
  for (const d of TRIGGER_FILTER_DEFINITIONS) {
    const v = trigger[FIELD[d.id][0]]
    if (!hasValue(d.id, v)) continue
    out.push({ type: d.id, values: normalizeFilterValue(d.id, v) })
  }
  return out
}

/** canAddFilter / addFilter: first type not yet used */
export function nextFilterType(filters: FilterForm[]): TriggerFilterType | null {
  const used = new Set(filters.map((f) => f.type))
  return TRIGGER_FILTER_DEFINITIONS.find((d) => d.allowMultipleEntries || !used.has(d.id))?.id ?? null
}

/** getFilterTypeOptions: already used types (except its own) disabled */
export function filterTypeOptions(filters: FilterForm[], index: number) {
  const used = new Set(filters.map((f) => f.type))
  const current = filters[index]?.type
  return TRIGGER_FILTER_DEFINITIONS.map((d) => ({ ...d, disabled: d.allowMultipleEntries ? false : used.has(d.id) && d.id !== current }))
}

/** getFormValues: distribute filters back onto the trigger fields (skip empty values) */
export function applyFilters(trigger: Record<string, any>, filters: FilterForm[]) {
  const agg: Record<string, any> = {
    filter_has_tags: [],
    filter_has_all_tags: [],
    filter_has_not_tags: [],
    filter_has_any_correspondents: [],
    filter_has_not_correspondents: [],
    filter_has_any_document_types: [],
    filter_has_not_document_types: [],
    filter_has_any_storage_paths: [],
    filter_has_not_storage_paths: [],
    filter_has_correspondent: null,
    filter_has_document_type: null,
    filter_has_storage_path: null,
    filter_custom_field_query: null,
  }
  for (const f of filters) {
    const v = f.values
    if (v === null || v === undefined) continue
    if (Array.isArray(v) && v.length === 0) continue
    const [field, kind] = FIELD[f.type]
    if (kind === 'multi') agg[field] = Array.isArray(v) ? [...v] : [v]
    else if (kind === 'single') agg[field] = Array.isArray(v) ? (v[0] ?? null) : v
    else agg[field] = v
  }
  Object.assign(trigger, agg)
  delete trigger.filters
  return trigger
}

export const formatPasswords = (p: string[] = []) => p.join('\n')
export const parsePasswords = (v: string | string[] = '') =>
  (Array.isArray(v) ? v : (v ?? '').split(/[\n,]+/)).map((e) => e.trim()).filter((e) => e.length > 0)

export function newTrigger(): Record<string, any> {
  return {
    type: WorkflowTriggerType.Consumption,
    sources: [],
    filter_filename: null,
    filter_path: null,
    filter_mailrule: null,
    filter_has_tags: [],
    filter_has_all_tags: [],
    filter_has_not_tags: [],
    filter_has_any_correspondents: [],
    filter_has_not_correspondents: [],
    filter_has_any_document_types: [],
    filter_has_not_document_types: [],
    filter_has_any_storage_paths: [],
    filter_has_not_storage_paths: [],
    filter_custom_field_query: null,
    filter_has_correspondent: null,
    filter_has_document_type: null,
    filter_has_storage_path: null,
    matching_algorithm: MATCH_NONE,
    match: '',
    is_insensitive: true,
    schedule_offset_days: 0,
    schedule_is_recurring: false,
    schedule_recurring_interval_days: 1,
    schedule_date_field: 'added',
    schedule_date_custom_field: null,
  }
}

export function newAction(): Record<string, any> {
  return {
    type: WorkflowActionType.Assignment,
    assign_title: null,
    assign_tags: [],
    assign_document_type: null,
    assign_correspondent: null,
    assign_storage_path: null,
    assign_owner: null,
    assign_view_users: [],
    assign_view_groups: [],
    assign_change_users: [],
    assign_change_groups: [],
    assign_custom_fields: [],
    assign_custom_fields_values: {},
    remove_tags: [],
    remove_all_tags: false,
    remove_document_types: [],
    remove_all_document_types: false,
    remove_correspondents: [],
    remove_all_correspondents: false,
    remove_storage_paths: [],
    remove_all_storage_paths: false,
    remove_owners: [],
    remove_all_owners: false,
    remove_view_users: [],
    remove_view_groups: [],
    remove_change_users: [],
    remove_change_groups: [],
    remove_all_permissions: false,
    remove_custom_fields: [],
    remove_all_custom_fields: false,
    email: { id: null, subject: null, body: null, to: null, include_document: false },
    webhook: { id: null, url: null, use_params: true, as_json: false, params: null, body: null, headers: null, include_document: false },
    passwords: [],
    ai_suggestion_fields: [],
    ai_create_missing: false,
    ai_overwrite_existing: false,
  }
}

/** createTriggerField: controls in Angular order */
export function triggerForm(t: Record<string, any>) {
  return {
    id: t.id,
    type: t.type,
    sources: t.sources,
    filter_filename: t.filter_filename,
    filter_path: t.filter_path,
    filter_mailrule: t.filter_mailrule,
    matching_algorithm: t.matching_algorithm,
    match: t.match,
    is_insensitive: t.is_insensitive,
    filters: buildFilters(t),
    schedule_offset_days: t.schedule_offset_days,
    schedule_is_recurring: t.schedule_is_recurring,
    schedule_recurring_interval_days: t.schedule_recurring_interval_days,
    schedule_date_field: t.schedule_date_field,
    schedule_date_custom_field: t.schedule_date_custom_field,
  }
}

/** createActionField */
export function actionForm(a: Record<string, any>) {
  const keys = [
    'id', 'type', 'assign_title', 'assign_tags', 'assign_owner', 'assign_document_type', 'assign_correspondent', 'assign_storage_path',
    'assign_view_users', 'assign_view_groups', 'assign_change_users', 'assign_change_groups', 'assign_custom_fields', 'assign_custom_fields_values',
    'remove_tags', 'remove_all_tags', 'remove_document_types', 'remove_all_document_types', 'remove_correspondents', 'remove_all_correspondents',
    'remove_storage_paths', 'remove_all_storage_paths', 'remove_owners', 'remove_all_owners', 'remove_view_users', 'remove_view_groups',
    'remove_change_users', 'remove_change_groups', 'remove_all_permissions', 'remove_custom_fields', 'remove_all_custom_fields',
  ]
  const f: Record<string, any> = {}
  for (const k of keys) f[k] = a[k]
  f.email = { id: a.email?.id, subject: a.email?.subject, body: a.email?.body, to: a.email?.to, include_document: !!a.email?.include_document }
  f.webhook = {
    id: a.webhook?.id,
    url: a.webhook?.url,
    use_params: a.webhook?.use_params,
    as_json: a.webhook?.as_json,
    params: a.webhook?.params,
    body: a.webhook?.body,
    headers: a.webhook?.headers,
    include_document: !!a.webhook?.include_document,
  }
  f.passwords = formatPasswords(a.passwords ?? [])
  f.ai_suggestion_fields = a.ai_suggestion_fields ?? []
  f.ai_create_missing = !!a.ai_create_missing
  f.ai_overwrite_existing = !!a.ai_overwrite_existing
  return f
}

/** checkRemovalActionFields: "remove all" disables the individual selection (then missing in objectForm.value) */
export const REMOVAL_DISABLES: Record<string, string[]> = {
  remove_all_tags: ['remove_tags'],
  remove_all_document_types: ['remove_document_types'],
  remove_all_correspondents: ['remove_correspondents'],
  remove_all_storage_paths: ['remove_storage_paths'],
  remove_all_custom_fields: ['remove_custom_fields'],
  remove_all_owners: ['remove_owners'],
  remove_all_permissions: ['remove_view_users', 'remove_view_groups', 'remove_change_users', 'remove_change_groups'],
}
export const isRemovalDisabled = (action: Record<string, any>, field: string) =>
  action.type === WorkflowActionType.Removal && Object.entries(REMOVAL_DISABLES).some(([all, fields]) => action[all] && fields.includes(field))

/** save() + getFormValues(): sent value */
export function workflowFormValues(form: { name: any; order: any; enabled: any; triggers: any[]; actions: any[] }) {
  const v = JSON.parse(JSON.stringify(form))
  v.actions = v.actions.map((a: any) => {
    if (a.type !== WorkflowActionType.Webhook) a.webhook = null
    if (a.type !== WorkflowActionType.Email) a.email = null
    a.passwords = parsePasswords(a.passwords)
    for (const f of Object.keys(a)) if (isRemovalDisabled(a, f)) delete a[f]
    return a
  })
  v.triggers = v.triggers.map((t: any) => applyFilters(t, t.filters ?? []))
  return v
}

/** getAllowedActionTypes */
export function allowedActionTypes(form: { triggers: any[]; actions: any[] }, s: { emailEnabled: boolean; remoteOcrConfigured: boolean; aiEnabled: boolean }) {
  let allowed = WORKFLOW_ACTION_OPTIONS
  if (!s.emailEnabled) allowed = allowed.filter((a) => a.id !== WorkflowActionType.Email)
  const remoteOcrUsable =
    s.remoteOcrConfigured &&
    (form.triggers?.some((t) => t.type === WorkflowTriggerType.Consumption) || form.actions?.some((a) => a.type === WorkflowActionType.RemoteOcr))
  if (!remoteOcrUsable) allowed = allowed.filter((a) => a.id !== WorkflowActionType.RemoteOcr)
  const aiUsable =
    s.aiEnabled &&
    (!form.triggers?.length ||
      form.triggers.some((t) => t.type !== WorkflowTriggerType.Consumption) ||
      form.actions?.some((a) => a.type === WorkflowActionType.ApplyAiSuggestions))
  if (!aiUsable) allowed = allowed.filter((a) => a.id !== WorkflowActionType.ApplyAiSuggestions)
  return allowed
}

/** copyWorkflow: copy without IDs (also webhook/email), name with " (copy)" */
export function copyWorkflow(w: Record<string, any>) {
  const c = JSON.parse(JSON.stringify(w))
  c.id = null
  c.name = `${w.name} (copy)`
  c.actions = c.actions.map((a: any) => {
    a.id = null
    if (a.webhook) a.webhook.id = null
    if (a.email) a.email.id = null
    return a
  })
  c.triggers = c.triggers.map((t: any) => ({ ...t, id: null }))
  return c
}

/** moveItemInArray */
export function moveItem<T>(arr: T[], from: number, to: number): T[] {
  const a = [...arr]
  const [m] = a.splice(from, 1)
  a.splice(to, 0, m)
  return a
}
