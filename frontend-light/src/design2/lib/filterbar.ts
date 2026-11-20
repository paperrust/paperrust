// Additional filters of the filter bar (docs/DESIGN2-ENTWIRREN.md §6): fields (custom fields present)
// and permissions (owner/shared) – same rule types as the classic UI
// (lib/filter-editor/permissions-model.ts, FilterEditor), so URLs and saved views match. Pure functions.
import type { FilterRule } from '../../lib/filter-rules'
import {
  FILTER_CUSTOM_FIELDS_QUERY,
  FILTER_OWNER,
  FILTER_OWNER_DOES_NOT_INCLUDE,
  FILTER_OWNER_ISNULL,
  FILTER_SHARED_BY_USER,
} from '../../lib/filter-rule-type'
import { getDate, getObjects, otherRules, type ObjKind } from './filters'

// ---- Fields ------------------------------------------------------------------------------------
// Like the classic UI (lib/query-params transformLegacyFilterRules): "has field" as
// custom_field_query ["AND", [[id, "exists", true], …]]. Other queries (values, OR, nested) from
// the classic UI or saved views are shown by the bar only as "Query" (resettable).

export type FieldFilter = { kind: 'none' } | { kind: 'exists'; ids: number[] } | { kind: 'query' }

export function getFieldFilter(rules: FilterRule[]): FieldFilter {
  const r = rules.filter((x) => x.rule_type === FILTER_CUSTOM_FIELDS_QUERY && x.value)
  if (!r.length) return { kind: 'none' }
  if (r.length > 1) return { kind: 'query' }
  try {
    const q = JSON.parse(r[0].value)
    if (Array.isArray(q) && q.length === 2 && (q[0] === 'AND' || (q[0] === 'OR' && Array.isArray(q[1]) && q[1].length === 1)) && Array.isArray(q[1])) {
      const ids: number[] = []
      for (const a of q[1]) {
        if (!Array.isArray(a) || a.length !== 3 || typeof a[0] !== 'number' || a[1] !== 'exists' || a[2] !== true) return { kind: 'query' }
        ids.push(a[0])
      }
      return ids.length ? { kind: 'exists', ids } : { kind: 'none' }
    }
  } catch {}
  return { kind: 'query' }
}

export function getFields(rules: FilterRule[]): number[] {
  const f = getFieldFilter(rules)
  return f.kind === 'exists' ? f.ids : []
}

/** replaces an existing field query */
export function setFields(rules: FilterRule[], ids: number[]): FilterRule[] {
  const rest = rules.filter((r) => r.rule_type !== FILTER_CUSTOM_FIELDS_QUERY)
  if (!ids.length) return rest
  return [...rest, { rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: JSON.stringify(['AND', ids.map((id) => [id, 'exists', true])]) }]
}

// ---- Permissions ------------------------------------------------------------------------------------

/** like OwnerFilterType of the classic UI; `custom` = other combination (e.g. from a saved view) */
export type OwnerMode = 'all' | 'mine' | 'shared_with_me' | 'shared_by_me' | 'unowned' | 'custom'
export const OWNER_MODES: Exclude<OwnerMode, 'custom'>[] = ['all', 'mine', 'shared_with_me', 'shared_by_me', 'unowned']

const OWNER_RULES = [FILTER_OWNER, FILTER_OWNER_DOES_NOT_INCLUDE, FILTER_OWNER_ISNULL, FILTER_SHARED_BY_USER]

export function getOwnerMode(rules: FilterRule[], me: number | null | undefined): OwnerMode {
  const own = rules.filter((r) => OWNER_RULES.includes(r.rule_type))
  if (!own.length) return 'all'
  if (own.length === 1) {
    const r = own[0]
    const v = String(r.value)
    if (me != null && r.rule_type === FILTER_OWNER && v === String(me)) return 'mine'
    if (me != null && r.rule_type === FILTER_SHARED_BY_USER && v === String(me)) return 'shared_by_me'
    if (r.rule_type === FILTER_OWNER_ISNULL && (v === 'true' || v === '1')) return 'unowned'
  }
  // "Shared with me": not my property (and optionally without ownerless)
  const ex = own.filter((r) => r.rule_type === FILTER_OWNER_DOES_NOT_INCLUDE)
  const rest = own.filter((r) => r.rule_type !== FILTER_OWNER_DOES_NOT_INCLUDE)
  if (me != null && ex.length === 1 && String(ex[0].value) === String(me)) {
    if (!rest.length) return 'shared_with_me'
    if (rest.length === 1 && rest[0].rule_type === FILTER_OWNER_ISNULL && (rest[0].value === 'false' || rest[0].value === '0')) return 'shared_with_me'
  }
  return 'custom'
}

export function setOwnerMode(rules: FilterRule[], mode: OwnerMode, me: number | null | undefined): FilterRule[] {
  if (mode === 'custom') return rules
  const rest = rules.filter((r) => !OWNER_RULES.includes(r.rule_type))
  if (mode === 'all') return rest
  if (mode === 'unowned') return [...rest, { rule_type: FILTER_OWNER_ISNULL, value: 'true' }]
  if (me == null) return rest
  if (mode === 'mine') return [...rest, { rule_type: FILTER_OWNER, value: String(me) }]
  if (mode === 'shared_by_me') return [...rest, { rule_type: FILTER_SHARED_BY_USER, value: String(me) }]
  // like the classic UI (NOT_SELF + "hide ownerless"): others' property that I may see
  return [...rest, { rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: String(me) }, { rule_type: FILTER_OWNER_ISNULL, value: 'false' }]
}

// ---- Overview ---------------------------------------------------------------------------------

const BAR_RULES = new Set([FILTER_CUSTOM_FIELDS_QUERY, ...OWNER_RULES])

/** Rules that the bar shows neither in filters.ts nor individually here (hint "n more rules") */
export function remainingRules(rules: FilterRule[]): FilterRule[] {
  return otherRules(rules).filter((r) => !BAR_RULES.has(r.rule_type))
}

const KINDS: ObjKind[] = ['tags', 'correspondent', 'document_type', 'storage_path']

/** Number of set filter groups without text (counter on the "Filter" button on the phone) */
export function activeFilterCount(rules: FilterRule[], me: number | null | undefined): number {
  let n = KINDS.filter((k) => getObjects(rules, k).length > 0).length
  if (getFieldFilter(rules).kind !== 'none') n++
  if (getDate(rules, 'from') || getDate(rules, 'to')) n++
  if (getOwnerMode(rules, me) !== 'all') n++
  if (remainingRules(rules).length) n++
  return n
}

