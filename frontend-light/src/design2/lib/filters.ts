// Filter model of the workspace (docs/DESIGN2.md §4.2) on the filter rules of the classic UI
// (lib/filter-rule-type, lib/query-params): same URL parameters, same server semantics. Pure functions.
import type { FilterRule } from '../../lib/filter-rules'
import { msg } from '../../lib/i18n'
import {
  FILTER_ADDED_FROM,
  FILTER_CORRESPONDENT,
  FILTER_CREATED_FROM,
  FILTER_CREATED_TO,
  FILTER_DOCUMENT_TYPE,
  FILTER_HAS_ANY_TAG,
  FILTER_HAS_CORRESPONDENT_ANY,
  FILTER_HAS_DOCUMENT_TYPE_ANY,
  FILTER_HAS_STORAGE_PATH_ANY,
  FILTER_HAS_TAGS_ALL,
  FILTER_IS_IN_INBOX,
  FILTER_SIMPLE_TEXT,
  FILTER_STORAGE_PATH,
  FILTER_TITLE_CONTENT,
} from '../../lib/filter-rule-type'
import { filterRulesFromQueryParams, paramsFromViewState, paramsToURLSearchParams, paramsToViewState } from '../../lib/query-params'

export type ObjKind = 'tags' | 'correspondent' | 'document_type' | 'storage_path'

/** Rule type "has one of" or "not assigned" per object kind. `noneValue`: value of the not-assigned rule
 *  (default null = isnull; tags: is_tagged=0 like the former quick filter "Without tags") */
export const OBJ_RULES: Record<ObjKind, { any: number; none: number | null; noneValue?: string }> = {
  tags: { any: FILTER_HAS_TAGS_ALL, none: FILTER_HAS_ANY_TAG, noneValue: 'false' },
  correspondent: { any: FILTER_HAS_CORRESPONDENT_ANY, none: FILTER_CORRESPONDENT },
  document_type: { any: FILTER_HAS_DOCUMENT_TYPE_ANY, none: FILTER_DOCUMENT_TYPE },
  storage_path: { any: FILTER_HAS_STORAGE_PATH_ANY, none: FILTER_STORAGE_PATH },
}

const TEXT_RULES = [FILTER_SIMPLE_TEXT, FILTER_TITLE_CONTENT]

export interface ListState {
  rules: FilterRule[]
  sortField: string
  sortReverse: boolean
}

export function stateFromQuery(query: string): ListState {
  const v = paramsToViewState(query.replace(/^\?/, ''))
  return { rules: filterRulesFromQueryParams(query.replace(/^\?/, '')), sortField: v.sortField ?? 'created', sortReverse: v.sortReverse ?? true }
}

/** URL query (without page – the workspace loads continuously) */
export function queryFromState(s: ListState): string {
  const p = paramsFromViewState({ sortField: s.sortField, sortReverse: s.sortReverse, currentPage: 1, filterRules: s.rules } as any)
  delete p.page
  const qs = paramsToURLSearchParams(p).toString()
  return qs ? `?${qs}` : ''
}

export function getText(rules: FilterRule[]): string {
  return rules.find((r) => TEXT_RULES.includes(r.rule_type))?.value ?? ''
}

export function setText(rules: FilterRule[], text: string): FilterRule[] {
  const rest = rules.filter((r) => !TEXT_RULES.includes(r.rule_type))
  return text.trim() ? [...rest, { rule_type: FILTER_SIMPLE_TEXT, value: text.trim() }] : rest
}

/** Rule is "not assigned" of this object kind (tags: is_tagged=false; otherwise isnull) */
export function isNoneRule(kind: ObjKind, x: FilterRule): boolean {
  const r = OBJ_RULES[kind]
  if (r.none === null || x.rule_type !== r.none) return false
  if (r.noneValue === undefined) return x.value == null
  return x.value === r.noneValue || (r.noneValue === 'false' && x.value === '0')
}

/** Rule "not assigned" (null if the object kind has none) */
export function noneRule(kind: ObjKind): FilterRule | null {
  const r = OBJ_RULES[kind]
  return r.none === null ? null : { rule_type: r.none, value: (r.noneValue ?? null) as any }
}

/** selected IDs (number) and "not assigned" (null) of an object kind */
export function getObjects(rules: FilterRule[], kind: ObjKind): (number | null)[] {
  const r = OBJ_RULES[kind]
  const out: (number | null)[] = []
  for (const x of rules) {
    if (x.rule_type === r.any && x.value != null) out.push(+x.value)
    else if (isNoneRule(kind, x)) out.push(null)
  }
  return out
}

export function toggleObject(rules: FilterRule[], kind: ObjKind, id: number | null): FilterRule[] {
  const r = OBJ_RULES[kind]
  if (id === null) {
    const nr = noneRule(kind)
    if (!nr) return rules
    const has = rules.some((x) => isNoneRule(kind, x))
    // "not assigned" excludes concrete values (like Angular)
    return has ? rules.filter((x) => !isNoneRule(kind, x)) : [...rules.filter((x) => x.rule_type !== r.any), nr]
  }
  const has = rules.some((x) => x.rule_type === r.any && x.value === String(id))
  if (has) return rules.filter((x) => !(x.rule_type === r.any && x.value === String(id)))
  return [...rules.filter((x) => !isNoneRule(kind, x)), { rule_type: r.any, value: String(id) }]
}

export function clearObjects(rules: FilterRule[], kind: ObjKind): FilterRule[] {
  const r = OBJ_RULES[kind]
  return rules.filter((x) => x.rule_type !== r.any && !isNoneRule(kind, x))
}

export function getDate(rules: FilterRule[], which: 'from' | 'to'): string {
  const t = which === 'from' ? FILTER_CREATED_FROM : FILTER_CREATED_TO
  return rules.find((r) => r.rule_type === t)?.value ?? ''
}

export function setDate(rules: FilterRule[], which: 'from' | 'to', value: string): FilterRule[] {
  const t = which === 'from' ? FILTER_CREATED_FROM : FILTER_CREATED_TO
  const rest = rules.filter((r) => r.rule_type !== t)
  return value ? [...rest, { rule_type: t, value }] : rest
}

const HANDLED = new Set([
  ...TEXT_RULES,
  FILTER_CREATED_FROM,
  FILTER_CREATED_TO,
  ...Object.values(OBJ_RULES).flatMap((r) => (r.none === null || r.noneValue !== undefined ? [r.any] : [r.any, r.none])),
])
const KINDS_WITH_VALUE_NONE = (Object.keys(OBJ_RULES) as ObjKind[]).filter((k) => OBJ_RULES[k].noneValue !== undefined)

/** Rules that the filter bar does not show individually (from saved views, quick filters) */
export function otherRules(rules: FilterRule[]): FilterRule[] {
  return rules.filter((r) => !HANDLED.has(r.rule_type) && !KINDS_WITH_VALUE_NONE.some((k) => isNoneRule(k, r)))
}

export function clearAll(rules: FilterRule[], keep: FilterRule[] = []): FilterRule[] {
  void rules
  return [...keep]
}

/** Quick filters of the sidebar */
export interface QuickFilter {
  id: string
  label: string
  icon: string
  rules: (today: string) => FilterRule[]
}

export const QUICK_FILTERS: QuickFilter[] = [
  { id: 'no-correspondent', label: msg('common.without_correspondent'), icon: 'person', rules: () => [{ rule_type: FILTER_CORRESPONDENT, value: null as any }] },
  { id: 'no-type', label: msg('common.without_document_type'), icon: 'file-earmark-text', rules: () => [{ rule_type: FILTER_DOCUMENT_TYPE, value: null as any }] },
  { id: 'untagged', label: msg('m.app.without_tags'), icon: 'tag', rules: () => [{ rule_type: FILTER_HAS_ANY_TAG, value: 'false' }] },
  { id: 'month', label: msg('m.app.added_month'), icon: 'calendar-event', rules: (today) => [{ rule_type: FILTER_ADDED_FROM, value: today.slice(0, 8) + '01' }] },
]

export const INBOX_RULES: FilterRule[] = [{ rule_type: FILTER_IS_IN_INBOX, value: 'true' }]

/** Equality of two rule sets (order irrelevant) – active marker in the sidebar */
export function sameRules(a: FilterRule[], b: FilterRule[]): boolean {
  const key = (r: FilterRule) => `${r.rule_type}:${r.value ?? '∅'}`
  const sa = a.map(key).sort().join('|')
  const sb = b.map(key).sort().join('|')
  return sa === sb
}
