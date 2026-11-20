// "Automation" area: pure logic without Svelte (Vitest: automation.test.ts).
// Draft ↔ stored form (ngx workflow + else branch + AI condition, docs/contract/automation.md §3.3),
// short form from the draft (same shape as `summary` of the server, so that sentence and cards are correct
// immediately while editing), condition chips, templates, order, mail trigger.
import {
  MATCH_NONE,
  WorkflowActionType,
  WorkflowTriggerType,
  isRemovalDisabled,
  newAction,
  newTrigger,
  parsePasswords,
} from '../../components/common/edit-dialog/workflow-edit-dialog/workflow-edit'
import type { AiCondition, AiField, CombinedWorkflow, NamedRef, NgxWorkflow, SaveBody, Summary, SummaryAction, SummaryCondition, SummaryItem, SummaryTrigger } from './api'

export { WorkflowActionType, WorkflowTriggerType, MATCH_NONE }
export const SOURCE_MAIL = 3

// ---- Draft ----------------------------------------------------------------------------------------

/** Editing state of a workflow in the editor */
export interface Draft {
  workflow: NgxWorkflow
  else_actions: Record<string, any>[]
  ai_condition: AiCondition | null
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v ?? null))

/** Trigger with all fields (missing ones from newTrigger) */
export function fullTrigger(t: Record<string, any> = {}): Record<string, any> {
  const base = newTrigger()
  const out: Record<string, any> = { ...base, ...clone(t) }
  for (const k of Object.keys(base)) if (out[k] === undefined) out[k] = base[k]
  // Lists never null (ngx delivers [] – legacy data/drafts may contain null)
  for (const k of Object.keys(base)) if (Array.isArray(base[k]) && !Array.isArray(out[k])) out[k] = out[k] == null ? [] : [out[k]]
  return out
}

/** Action with all fields; email/webhook always as an object (the editor binds directly to it) */
export function fullAction(a: Record<string, any> = {}): Record<string, any> {
  const base = newAction()
  const src = clone(a) ?? {}
  const out: Record<string, any> = { ...base, ...src }
  for (const k of Object.keys(base)) if (out[k] === undefined || (out[k] === null && Array.isArray(base[k]))) out[k] = base[k]
  out.email = { ...base.email, ...(src.email ?? {}) }
  out.webhook = { ...base.webhook, ...(src.webhook ?? {}) }
  out.passwords = Array.isArray(src.passwords) ? src.passwords : parsePasswords(src.passwords ?? '')
  if (!out.assign_custom_fields_values || typeof out.assign_custom_fields_values !== 'object') out.assign_custom_fields_values = {}
  return out
}

export function emptyDraft(name: string | null = null): Draft {
  return { workflow: { name, order: 0, enabled: true, triggers: [fullTrigger({ type: WorkflowTriggerType.DocumentAdded })], actions: [] }, else_actions: [], ai_condition: null }
}

/** Server response → draft */
export function draftFromCombined(c: CombinedWorkflow): Draft {
  return {
    workflow: {
      id: c.workflow.id ?? null,
      name: c.workflow.name,
      order: c.workflow.order ?? 0,
      enabled: c.workflow.enabled,
      triggers: (c.workflow.triggers ?? []).map(fullTrigger),
      actions: (c.workflow.actions ?? []).map(fullAction),
    },
    else_actions: (c.else_actions ?? []).map(fullAction),
    ai_condition: c.ai_condition ? { ...c.ai_condition } : null,
  }
}

/** ngx draft (suggestion, AI draft, template) → draft without ids */
export function draftFromNgx(w: Partial<NgxWorkflow>, extra: Partial<Draft> = {}): Draft {
  const strip = (o: Record<string, any>) => {
    const c = clone(o)
    delete c.id
    if (c.email) delete c.email.id
    if (c.webhook) delete c.webhook.id
    return c
  }
  return {
    workflow: {
      name: w.name ?? null,
      order: w.order ?? 0,
      enabled: w.enabled ?? true,
      triggers: (w.triggers ?? []).map((t) => fullTrigger(strip(t))),
      actions: (w.actions ?? []).map((a) => fullAction(strip(a))),
    },
    else_actions: (extra.else_actions ?? []).map((a) => fullAction(strip(a))),
    ai_condition: extra.ai_condition ?? null,
  }
}

/** Action into the sent form (like workflowFormValues of the classic UI) */
export function actionToSave(a: Record<string, any>): Record<string, any> {
  const v = clone(a)
  if (v.type !== WorkflowActionType.Webhook) v.webhook = null
  if (v.type !== WorkflowActionType.Email) v.email = null
  if (v.webhook && v.webhook.id == null) delete v.webhook.id
  if (v.email && v.email.id == null) delete v.email.id
  v.passwords = parsePasswords(v.passwords ?? [])
  for (const f of Object.keys(v)) if (isRemovalDisabled(v, f)) delete v[f]
  if (v.id == null) delete v.id
  return v
}

export function triggerToSave(t: Record<string, any>): Record<string, any> {
  const v = clone(t)
  delete v.filters
  if (v.id == null) delete v.id
  if (v.matching_algorithm === MATCH_NONE) v.match = v.match ?? ''
  return v
}

/** Draft → body for POST/PUT /workflows (contract §3.3) */
export function toSaveBody(d: Draft): SaveBody {
  const body: SaveBody = {
    name: d.workflow.name,
    order: d.workflow.order ?? 0,
    enabled: d.workflow.enabled,
    triggers: d.workflow.triggers.map(triggerToSave),
    actions: d.workflow.actions.map(actionToSave),
    else_actions: d.else_actions.map(actionToSave),
    ai_condition: d.ai_condition ? { field: d.ai_condition.field, object_id: d.ai_condition.object_id, min_confidence: d.ai_condition.min_confidence } : null,
  }
  if (d.workflow.id != null) body.id = d.workflow.id
  return body
}

/** Copy for duplicating: without ids, name with suffix, switched off (so nothing runs twice) */
export function duplicateDraft(d: Draft, name: string): Draft {
  const c = draftFromNgx(d.workflow, { else_actions: d.else_actions, ai_condition: d.ai_condition })
  c.workflow.name = name
  c.workflow.enabled = false
  return c
}

/** Fingerprint for detecting unsaved changes */
export const fingerprint = (d: Draft) => JSON.stringify(toSaveBody(d))

/** Move element (dragging, keyboard up/down) */
export function moveItem<T>(arr: T[], from: number, to: number): T[] {
  const a = [...arr]
  if (from < 0 || from >= a.length || to < 0 || to >= a.length) return a
  const [m] = a.splice(from, 1)
  a.splice(to, 0, m)
  return a
}

/** New order of the list: order = position × 10; only changed entries back */
export function reorderPlan(items: { id: number; order: number }[], from: number, to: number): { id: number; order: number }[] {
  const moved = moveItem(items, from, to)
  return moved.map((x, i) => ({ id: x.id, order: (i + 1) * 10 })).filter((x) => items.find((y) => y.id === x.id)?.order !== x.order)
}

// ---- Mail as trigger --------------------------------------------------------------------------------

/** Trigger "Email arrives" = ingestion trigger with mail rule (contract §3.11) */
export const isMailTrigger = (t: Record<string, any>) => t.type === WorkflowTriggerType.Consumption && t.filter_mailrule != null
export const mailTriggerIndex = (w: NgxWorkflow) => w.triggers.findIndex(isMailTrigger)

export function newMailRule(account: number | null = null): Record<string, any> {
  return {
    name: null,
    account,
    enabled: true,
    folder: 'INBOX',
    filter_from: null,
    filter_to: null,
    filter_subject: null,
    filter_body: null,
    filter_attachment_filename_include: null,
    filter_attachment_filename_exclude: null,
    maximum_age: 30,
    attachment_type: 1,
    pdf_layout: 0,
    consumption_scope: 1,
    order: 0,
    action: 3,
    action_parameter: null,
    assign_title_from: 1,
    assign_tags: [],
    assign_document_type: null,
    assign_correspondent_from: 1,
    assign_correspondent: null,
    assign_owner_from_rule: true,
    stop_processing: false,
  }
}

// ---- Conditions (chips) --------------------------------------------------------------------------------

/** Trigger on existing documents (content, tags, … can be queried) */
export const hasDocumentFilters = (type: number) => type !== WorkflowTriggerType.Consumption

export type ObjField = 'tags' | 'correspondent' | 'document_type' | 'storage_path'
export type ObjMode = 'any' | 'all' | 'none' | 'is'

/** Fields per object kind and mode (ngx trigger fields) */
export const OBJ_FIELDS: Record<ObjField, Partial<Record<ObjMode, string>>> = {
  tags: { any: 'filter_has_tags', all: 'filter_has_all_tags', none: 'filter_has_not_tags' },
  correspondent: { is: 'filter_has_correspondent', any: 'filter_has_any_correspondents', none: 'filter_has_not_correspondents' },
  document_type: { is: 'filter_has_document_type', any: 'filter_has_any_document_types', none: 'filter_has_not_document_types' },
  storage_path: { is: 'filter_has_storage_path', any: 'filter_has_any_storage_paths', none: 'filter_has_not_storage_paths' },
}

/** Common conditions (chips) and advanced ones – keys for "+ Condition" */
export type CondKey = 'text' | 'tags' | 'correspondent' | 'document_type' | 'storage_path' | 'filename' | 'path' | 'mail_rule' | 'sources' | 'custom_field_query' | 'ai'
export const COMMON_CONDITIONS: CondKey[] = ['text', 'tags', 'correspondent', 'document_type', 'storage_path', 'filename']
export const ADVANCED_CONDITIONS: CondKey[] = ['custom_field_query', 'path', 'mail_rule', 'sources']

/** Is this condition possible for the trigger type? (like the classic UI) */
export function conditionAllowed(key: CondKey, type: number): boolean {
  if (key === 'filename') return true
  if (key === 'path' || key === 'mail_rule' || key === 'sources') return type === WorkflowTriggerType.Consumption
  if (key === 'ai') return type !== WorkflowTriggerType.Consumption
  return hasDocumentFilters(type)
}

const nonEmpty = (v: unknown) => (Array.isArray(v) ? v.length > 0 : v !== null && v !== undefined && v !== '')

/** Is the condition set on the trigger? */
export function conditionActive(t: Record<string, any>, key: CondKey): boolean {
  switch (key) {
    case 'text':
      return (t.matching_algorithm ?? MATCH_NONE) !== MATCH_NONE
    case 'filename':
      return nonEmpty(t.filter_filename)
    case 'path':
      return nonEmpty(t.filter_path)
    case 'mail_rule':
      return t.filter_mailrule != null
    case 'sources':
      return nonEmpty(t.sources)
    case 'custom_field_query':
      return nonEmpty(t.filter_custom_field_query)
    case 'ai':
      return false
    default:
      return Object.values(OBJ_FIELDS[key as ObjField]).some((f) => nonEmpty(t[f as string]))
  }
}

/** Clear condition */
export function clearCondition(t: Record<string, any>, key: CondKey) {
  switch (key) {
    case 'text':
      t.matching_algorithm = MATCH_NONE
      t.match = ''
      break
    case 'filename':
      t.filter_filename = null
      break
    case 'path':
      t.filter_path = null
      break
    case 'mail_rule':
      t.filter_mailrule = null
      break
    case 'sources':
      t.sources = []
      break
    case 'custom_field_query':
      t.filter_custom_field_query = null
      break
    case 'ai':
      break
    default:
      for (const [mode, f] of Object.entries(OBJ_FIELDS[key as ObjField])) t[f as string] = mode === 'is' ? null : []
  }
}

/** Current mode and selection of an object condition (first set mode) */
export function objCondition(t: Record<string, any>, field: ObjField): { mode: ObjMode; ids: number[] } {
  const fields = OBJ_FIELDS[field]
  for (const [mode, f] of Object.entries(fields) as [ObjMode, string][]) {
    const v = t[f]
    if (mode === 'is' && v != null) return { mode, ids: [v] }
    if (mode !== 'is' && Array.isArray(v) && v.length) return { mode, ids: [...v] }
  }
  return { mode: field === 'tags' ? 'any' : 'is', ids: [] }
}

/** Set object condition (other modes of the same kind stay untouched, except `exclusive`) */
export function setObjCondition(t: Record<string, any>, field: ObjField, mode: ObjMode, ids: number[], exclusive = true) {
  const fields = OBJ_FIELDS[field]
  if (exclusive) for (const [m, f] of Object.entries(fields)) t[f as string] = m === 'is' ? null : []
  const f = fields[mode]
  if (!f) return
  if (mode === 'is') t[f] = ids[0] ?? null
  else t[f] = [...ids]
}

/** Simple text matching: "contains" (wording, 3) as default */
export function setTextCondition(t: Record<string, any>, text: string, algorithm = 3) {
  t.matching_algorithm = text.trim() ? algorithm : MATCH_NONE
  t.match = text
}

// ---- Custom field query (simple builder) ------------------------------------------------------------

export type CfAtom = [number, string, unknown]
export interface CfSimple {
  op: 'AND' | 'OR'
  atoms: CfAtom[]
}

/** Query → simple form (one level of AND/OR with atoms); complex queries → null (edit raw text) */
export function parseCfQuery(q: string | null | undefined): CfSimple | null {
  if (!q) return { op: 'AND', atoms: [] }
  let v: unknown
  try {
    v = JSON.parse(q)
  } catch {
    return null
  }
  const isAtom = (a: unknown): a is CfAtom => Array.isArray(a) && a.length === 3 && typeof a[0] === 'number' && typeof a[1] === 'string' && !(a[1] === 'AND' || a[1] === 'OR')
  if (isAtom(v)) return { op: 'AND', atoms: [v] }
  if (Array.isArray(v) && v.length === 2 && (v[0] === 'AND' || v[0] === 'OR') && Array.isArray(v[1]) && v[1].every(isAtom)) return { op: v[0], atoms: v[1] as CfAtom[] }
  return null
}

export function serializeCfQuery(s: CfSimple): string | null {
  const atoms = s.atoms.filter((a) => a[0] > 0 && a[1])
  if (!atoms.length) return null
  return JSON.stringify([s.op, atoms])
}

/** Operators per field type (filters.py SUPPORTED_EXPR_CATEGORIES, shortened to the understandable ones) */
export function cfOperators(dataType: string): string[] {
  const basic = ['exists', 'isnull']
  switch (dataType) {
    case 'string':
    case 'url':
    case 'longtext':
      return ['exact', 'icontains', ...basic]
    case 'date':
      return ['exact', 'gte', 'lte', ...basic]
    case 'boolean':
      return ['exact', ...basic]
    case 'integer':
    case 'float':
      return ['exact', 'gt', 'gte', 'lt', 'lte', ...basic]
    case 'monetary':
      return ['exact', 'icontains', 'gt', 'gte', 'lt', 'lte', ...basic]
    case 'documentlink':
      return ['contains', ...basic]
    case 'select':
      return ['exact', 'in', ...basic]
    default:
      return ['exact', ...basic]
  }
}

// ---- Short form from the draft (shape like `summary` of the server) -------------------------------------------------

export type NameKind = 'tag' | 'correspondent' | 'document_type' | 'storage_path' | 'user' | 'group' | 'custom_field' | 'mail_rule'
export type Names = (kind: NameKind, id: number) => string | null

export const TRIGGER_KEY: Record<number, SummaryTrigger['type_key']> = { 1: 'consumption', 2: 'document_added', 3: 'document_updated', 4: 'scheduled', 5: 'manual' }
export const ACTION_KEY: Record<number, SummaryAction['type_key']> = {
  1: 'assignment',
  2: 'removal',
  3: 'email',
  4: 'webhook',
  5: 'password_removal',
  6: 'move_to_trash',
  7: 'remote_ocr',
  8: 'ai_suggestions',
}

const COND_FIELDS: [string, string, NameKind, boolean][] = [
  ['filter_has_tags', 'tags_any', 'tag', true],
  ['filter_has_all_tags', 'tags_all', 'tag', true],
  ['filter_has_not_tags', 'tags_none', 'tag', true],
  ['filter_has_correspondent', 'correspondent_is', 'correspondent', false],
  ['filter_has_any_correspondents', 'correspondent_any', 'correspondent', true],
  ['filter_has_not_correspondents', 'correspondent_none', 'correspondent', true],
  ['filter_has_document_type', 'document_type_is', 'document_type', false],
  ['filter_has_any_document_types', 'document_type_any', 'document_type', true],
  ['filter_has_not_document_types', 'document_type_none', 'document_type', true],
  ['filter_has_storage_path', 'storage_path_is', 'storage_path', false],
  ['filter_has_any_storage_paths', 'storage_path_any', 'storage_path', true],
  ['filter_has_not_storage_paths', 'storage_path_none', 'storage_path', true],
  ['filter_mailrule', 'mail_rule', 'mail_rule', false],
]
const ACTION_FIELDS: [string, NameKind, boolean][] = [
  ['assign_tags', 'tag', true],
  ['assign_correspondent', 'correspondent', false],
  ['assign_document_type', 'document_type', false],
  ['assign_storage_path', 'storage_path', false],
  ['assign_owner', 'user', false],
  ['assign_view_users', 'user', true],
  ['assign_view_groups', 'group', true],
  ['assign_change_users', 'user', true],
  ['assign_change_groups', 'group', true],
  ['assign_custom_fields', 'custom_field', true],
  ['remove_tags', 'tag', true],
  ['remove_correspondents', 'correspondent', true],
  ['remove_document_types', 'document_type', true],
  ['remove_storage_paths', 'storage_path', true],
  ['remove_owners', 'user', true],
  ['remove_view_users', 'user', true],
  ['remove_view_groups', 'group', true],
  ['remove_change_users', 'user', true],
  ['remove_change_groups', 'group', true],
  ['remove_custom_fields', 'custom_field', true],
]
const REMOVE_ALL = ['remove_all_tags', 'remove_all_correspondents', 'remove_all_document_types', 'remove_all_storage_paths', 'remove_all_owners', 'remove_all_permissions', 'remove_all_custom_fields']

const ids = (v: unknown, multi: boolean): number[] => (multi ? (Array.isArray(v) ? v.filter((x): x is number => typeof x === 'number') : []) : typeof v === 'number' ? [v] : [])
const str = (v: unknown) => (typeof v === 'string' && v.trim() ? v : null)

export function summarizeTrigger(t: Record<string, any>, names: Names): SummaryTrigger {
  const conds: SummaryCondition[] = []
  const ty = t.type
  if (ty === WorkflowTriggerType.Consumption) {
    const s = ids(t.sources, true)
    if (s.length && s.length < 4) conds.push({ kind: 'sources', values: s })
  }
  if (str(t.filter_filename)) conds.push({ kind: 'filename', value: t.filter_filename })
  if (str(t.filter_path)) conds.push({ kind: 'path', value: t.filter_path })
  const algo = t.matching_algorithm ?? 0
  if (algo !== 0 && ty !== WorkflowTriggerType.Consumption) conds.push({ kind: 'content_match', algorithm: algo, value: t.match ?? '', insensitive: t.is_insensitive ?? true })
  for (const [f, kind, nk, multi] of COND_FIELDS) {
    const v = ids(t[f], multi)
    if (!v.length) continue
    if (multi) conds.push({ kind, objects: v.map((id) => ({ id, name: names(nk, id) })) })
    else conds.push({ kind, object: { id: v[0], name: names(nk, v[0]) } })
  }
  if (str(t.filter_custom_field_query)) conds.push({ kind: 'custom_field_query', value: t.filter_custom_field_query })
  const cf = typeof t.schedule_date_custom_field === 'number' ? t.schedule_date_custom_field : null
  return {
    id: t.id ?? null,
    type: ty,
    type_key: TRIGGER_KEY[ty] ?? 'unknown',
    conditions: conds,
    schedule:
      ty === WorkflowTriggerType.Scheduled
        ? {
            offset_days: t.schedule_offset_days ?? null,
            is_recurring: t.schedule_is_recurring ?? null,
            recurring_interval_days: t.schedule_recurring_interval_days ?? null,
            date_field: t.schedule_date_field ?? null,
            custom_field: cf === null ? null : { id: cf, name: names('custom_field', cf) },
          }
        : null,
  }
}

export function summarizeAction(a: Record<string, any>, names: Names): SummaryAction {
  const ty = a.type
  const items: SummaryItem[] = []
  if (ty === WorkflowActionType.Assignment && str(a.assign_title)) items.push({ kind: 'assign_title', value: a.assign_title })
  if (ty === WorkflowActionType.Removal) for (const f of REMOVE_ALL) if (a[f]) items.push({ kind: f })
  if (ty === WorkflowActionType.Email) items.push({ kind: 'email', to: a.email?.to ?? null, subject: a.email?.subject ?? null, include_document: !!a.email?.include_document })
  if (ty === WorkflowActionType.Webhook) items.push({ kind: 'webhook', url: a.webhook?.url ?? null, as_json: !!a.webhook?.as_json, include_document: !!a.webhook?.include_document })
  if (ty === WorkflowActionType.ApplyAiSuggestions)
    items.push({ kind: 'ai_suggestions', fields: a.ai_suggestion_fields ?? [], create_missing: !!a.ai_create_missing, overwrite_existing: !!a.ai_overwrite_existing })
  if (ty === WorkflowActionType.Assignment || ty === WorkflowActionType.Removal) {
    for (const [f, nk, multi] of ACTION_FIELDS) {
      if ((ty === WorkflowActionType.Assignment) !== f.startsWith('assign_')) continue
      if (isRemovalDisabled(a, f)) continue
      const v = ids(a[f], multi)
      if (!v.length) continue
      const item: SummaryItem = { kind: f }
      if (multi) item.objects = v.map((id) => ({ id, name: names(nk, id) }))
      else item.object = { id: v[0], name: names(nk, v[0]) }
      if (f === 'assign_custom_fields') item.values = a.assign_custom_fields_values ?? {}
      items.push(item)
    }
  }
  return { id: a.id ?? null, type: ty, type_key: ACTION_KEY[ty] ?? 'unknown', items }
}

export function summarize(d: Draft, names: Names, ai: { available: boolean; enabled: boolean } = { available: false, enabled: false }): Summary {
  const triggers = d.workflow.triggers.map((t) => summarizeTrigger(t, names))
  const onlyConsumption = triggers.length > 0 && triggers.every((t) => t.type === WorkflowTriggerType.Consumption)
  const c = d.ai_condition
  return {
    trigger_types: [...new Set(triggers.map((t) => t.type))].sort(),
    triggers,
    ai_condition: c
      ? {
          field: c.field,
          object: { id: c.object_id, name: names(aiNameKind(c.field), c.object_id) },
          min_confidence: c.min_confidence,
          active: ai.available && ai.enabled && !onlyConsumption,
          inactive_reason: !ai.available ? 'ai_unavailable' : !ai.enabled ? 'ai_disabled' : onlyConsumption ? 'consumption_only' : null,
        }
      : null,
    actions: d.workflow.actions.map((a) => summarizeAction(a, names)),
    else_actions: d.else_actions.map((a) => summarizeAction(a, names)),
  }
}

export const aiNameKind = (f: AiField): NameKind => (f === 'tags' ? 'tag' : f)

/** Object names for sentence/cards: name or "#id" (invisible objects) */
export const refName = (r: NamedRef | undefined | null) => (r ? (r.name ?? `#${r.id}`) : '')

// ---- Templates ----------------------------------------------------------------------------------------------

export type TemplateKey = 'invoices' | 'ai_presort' | 'passwords' | 'tax' | 'notify' | 'blank'
export interface TemplateDef {
  key: TemplateKey
  icon: string
  /** only visible when AI is on */
  ai?: boolean
  /** only if email sending is set up */
  email?: boolean
}
export const TEMPLATES: TemplateDef[] = [
  { key: 'invoices', icon: 'tags' },
  { key: 'ai_presort', icon: 'stars', ai: true },
  { key: 'passwords', icon: 'shield-check' },
  { key: 'tax', icon: 'folder' },
  { key: 'notify', icon: 'envelope', email: true },
  { key: 'blank', icon: 'plus-lg' },
]

export const visibleTemplates = (o: { aiOn: boolean; emailOn: boolean }) => TEMPLATES.filter((t) => (!t.ai || o.aiOn) && (!t.email || o.emailOn))

/**
 * Draft of a template. Names come in translated; objects (tag "Rechnung" …) are looked up by `find` in the inventory
 * (no silent creation) – if one is missing, the field stays empty and the editor shows the card to fill in.
 */
export function templateDraft(
  key: TemplateKey,
  o: { name: string; find: (kind: NameKind, words: string[]) => number | null; userEmail?: string | null },
): Draft {
  const d = emptyDraft(o.name)
  const wf = d.workflow
  const act = (a: Record<string, any>) => fullAction(a)
  switch (key) {
    case 'invoices': {
      const tag = o.find('tag', ['rechnung', 'invoice', 'facture', 'fattura', 'factura', 'factuur', 'fatura', 'faktura', 'faktur'])
      const type = o.find('document_type', ['rechnung', 'invoice', 'facture', 'fattura', 'factura', 'factuur', 'fatura', 'faktura', 'faktur'])
      wf.triggers = [fullTrigger({ type: WorkflowTriggerType.DocumentAdded, matching_algorithm: 1, match: 'Rechnung Invoice Facture Fattura Factura', is_insensitive: true })]
      wf.actions = [act({ type: WorkflowActionType.Assignment, assign_tags: tag ? [tag] : [], assign_document_type: type })]
      break
    }
    case 'ai_presort':
      wf.triggers = [fullTrigger({ type: WorkflowTriggerType.DocumentAdded })]
      wf.actions = [act({ type: WorkflowActionType.ApplyAiSuggestions, ai_suggestion_fields: ['title', 'tags', 'correspondent', 'document_type'], ai_create_missing: false, ai_overwrite_existing: false })]
      break
    case 'passwords':
      wf.triggers = [fullTrigger({ type: WorkflowTriggerType.Consumption, filter_filename: '*.pdf' })]
      wf.actions = [act({ type: WorkflowActionType.PasswordRemoval, passwords: [] })]
      break
    case 'tax': {
      const path = o.find('storage_path', ['steuer', 'tax', 'impôt', 'impot', 'tasse', 'impuesto', 'belasting', 'imposto', 'podatek', 'daně', 'dane', 'skatt'])
      const tag = o.find('tag', ['steuer', 'tax', 'impôt', 'impot', 'tasse', 'impuesto', 'belasting', 'imposto', 'podatek', 'daně', 'dane', 'skatt'])
      wf.triggers = [fullTrigger({ type: WorkflowTriggerType.DocumentUpdated, filter_has_tags: tag ? [tag] : [] })]
      wf.actions = [act({ type: WorkflowActionType.Assignment, assign_storage_path: path })]
      break
    }
    case 'notify':
      wf.triggers = [fullTrigger({ type: WorkflowTriggerType.DocumentAdded })]
      wf.actions = [act({ type: WorkflowActionType.Email, email: { subject: '{{ doc_title }}', body: '{{ doc_url }}', to: o.userEmail ?? '', include_document: false } })]
      break
    case 'blank':
      break
  }
  return d
}

/** Field name in the middle of the sentence ("For correspondent …"): lowercase except in German (nouns uppercase). */
export function fieldInSentence(label: string, locale: string): string {
  if (!label || locale.toLowerCase().startsWith('de')) return label
  return label.charAt(0).toLocaleLowerCase(locale) + label.slice(1)
}

/** Find object by name (whole word, case-insensitive) – for templates */
export function findByWords(list: { id: number; name: string }[], words: string[]): number | null {
  const norm = (s: string) => s.toLocaleLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
  const ws = words.map(norm)
  const exact = list.find((x) => ws.includes(norm(x.name)))
  if (exact) return exact.id
  // whole word, also with a short ending (plural: "Rechnungen", "Invoices")
  const hit = (token: string, w: string) => token === w || (token.startsWith(w) && token.length - w.length <= 2)
  const part = list.find((x) => ws.some((w) => norm(x.name).split(/[^\p{L}\p{N}]+/u).some((tok) => hit(tok, w))))
  return part?.id ?? null
}

// ---- Validation before saving -----------------------------------------------------------------------------

export type Problem = 'no_name' | 'no_trigger' | 'no_action' | 'ai_condition_incomplete'
export function problems(d: Draft): Problem[] {
  const out: Problem[] = []
  if (!d.workflow.name?.trim()) out.push('no_name')
  if (!d.workflow.triggers.length) out.push('no_trigger')
  if (!d.workflow.actions.length && !d.else_actions.length) out.push('no_action')
  if (d.ai_condition && !(d.ai_condition.object_id > 0)) out.push('ai_condition_incomplete')
  return out
}

/** Distribute field errors of the API (DRF shape) onto cards: triggers[i], actions[i], else_actions[i], general */
export function errorTargets(fields: Record<string, any> | null): { general: string[]; triggers: Record<number, string[]>; actions: Record<number, string[]>; else: Record<number, string[]>; ai: string[]; mail: string[] } {
  const out = { general: [] as string[], triggers: {} as Record<number, string[]>, actions: {} as Record<number, string[]>, else: {} as Record<number, string[]>, ai: [] as string[], mail: [] as string[] }
  if (!fields) return out
  const flat = (v: unknown, prefix = ''): string[] => {
    if (v == null) return []
    if (typeof v === 'string') return [prefix ? `${prefix}: ${v}` : v]
    if (Array.isArray(v)) return v.flatMap((x) => flat(x, prefix))
    if (typeof v === 'object') return Object.entries(v as Record<string, unknown>).flatMap(([k, x]) => flat(x, k === 'non_field_errors' ? prefix : k))
    return [String(v)]
  }
  const perIndex = (v: unknown, into: Record<number, string[]>) => {
    if (Array.isArray(v) && v.some((x) => x && typeof x === 'object' && !Array.isArray(x))) v.forEach((x, i) => {
      const m = flat(x)
      if (m.length) into[i] = m
    })
    else if (v != null) (into[-1] ??= []).push(...flat(v))
  }
  for (const [k, v] of Object.entries(fields)) {
    if (k === 'triggers') perIndex(v, out.triggers)
    else if (k === 'actions') perIndex(v, out.actions)
    else if (k === 'else_actions') perIndex(v, out.else)
    else if (k === 'ai_condition') out.ai.push(...flat(v))
    else if (k === 'mail_rule') out.mail.push(...flat(v))
    else out.general.push(...flat(v, k === 'non_field_errors' ? '' : k))
  }
  return out
}

// ---- History/test run: codes -------------------------------------------------------------------------------

const CODE_KEYS: Record<string, string> = {
  ok: 'm.auto.code.ok',
  else: 'm.auto.code.else',
  filters_bypassed: 'm.auto.code.filters_bypassed',
  conditions_not_met: 'm.auto.code.conditions_not_met',
  match_error: 'm.auto.code.match_error',
  error: 'm.auto.code.error',
  ai_met: 'm.auto.code.ai_met',
  ai_not_met: 'm.auto.code.ai_not_met',
  ai_not_met_else: 'm.auto.code.ai_not_met_else',
  ai_error: 'm.auto.code.ai_error',
  ai_not_checked: 'm.auto.code.ai_not_checked',
  ai_deferred: 'm.auto.code.ai_deferred',
  ai_ignored_consumption: 'm.auto.code.ai_ignored_consumption',
  ai_ignored_unavailable: 'm.auto.code.ai_ignored_unavailable',
  ai_ignored_disabled: 'm.auto.code.ai_ignored_disabled',
}
/** i18n key of a result code (unknown → generic text) */
export const codeKey = (code: string) => CODE_KEYS[code] ?? 'm.auto.code.other'

/** Monday 00:00 of the current week (local) – for display only */
export function startOfWeek(now = new Date()): Date {
  const d = new Date(now)
  d.setHours(0, 0, 0, 0)
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7))
  return d
}

/** "5 minutes ago", "yesterday" … (Intl.RelativeTimeFormat) */
export function ago(iso: string, locale: string, now = Date.now()): string {
  const t = Date.parse(iso)
  if (Number.isNaN(t)) return ''
  const s = Math.round((t - now) / 1000)
  const abs = Math.abs(s)
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })
  if (abs < 60) return rtf.format(s, 'second')
  if (abs < 3600) return rtf.format(Math.round(s / 60), 'minute')
  if (abs < 86400) return rtf.format(Math.round(s / 3600), 'hour')
  if (abs < 86400 * 30) return rtf.format(Math.round(s / 86400), 'day')
  return new Date(t).toLocaleDateString(locale)
}

/** Date + time short */
export const dateTime = (iso: string, locale: string) => {
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' })
}

// ---- Quick actions ("+ Action" in the sentence) ---------------------------------------------------------------

export type QuickAction = 'tags' | 'correspondent' | 'document_type' | 'storage_path' | 'title' | 'email' | 'ai' | 'trash' | 'other'

/** New action for a quick pick + field that gets the focus afterwards */
export function quickAction(k: QuickAction): { action: Record<string, any>; focus: string | null } {
  switch (k) {
    case 'tags':
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'assign_tags' }
    case 'correspondent':
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'assign_correspondent' }
    case 'document_type':
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'assign_document_type' }
    case 'storage_path':
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'assign_storage_path' }
    case 'title':
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'assign_title' }
    case 'email':
      return { action: fullAction({ type: WorkflowActionType.Email }), focus: 'email_to' }
    case 'ai':
      return { action: fullAction({ type: WorkflowActionType.ApplyAiSuggestions, ai_suggestion_fields: ['title', 'tags', 'correspondent', 'document_type'] }), focus: null }
    case 'trash':
      return { action: fullAction({ type: WorkflowActionType.MoveToTrash }), focus: null }
    default:
      return { action: fullAction({ type: WorkflowActionType.Assignment }), focus: 'type' }
  }
}
