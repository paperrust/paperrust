// Sentence with clickable parts (topic 3 B, docs/WORKFLOWS-NEU.md) from the short form (`summary`).
// Sentence structure without glued-together word fragments: each language has whole clauses per component
// (m.auto.s.*, with placeholders), enumerations are built by Intl.ListFormat ("A, B and C" / "A or B").
// The sentence templates are filled with markers and split into clickable parts at the markers.
import type { Summary, SummaryAction, SummaryCondition, SummaryItem, SummaryTrigger } from './api'
import { refName } from './model'

export type Target = { card: 'trigger'; index: number } | { card: 'conditions'; index: number } | { card: 'ai' } | { card: 'action'; index: number } | { card: 'else'; index: number }
export interface Seg {
  text: string
  target?: Target
  /** Placeholder "please choose" (no trigger/no action) */
  missing?: boolean
}
export type Tr = (key: string, params?: Record<string, string | number | null | undefined>) => string

const MARK = '\u0001'

/** Fill template `key` with pieces: {$NAME} → segments from `slots`, other parameters normal */
export function fill(tr: Tr, key: string, slots: Record<string, Seg[]>, params: Record<string, string | number> = {}): Seg[] {
  const names = Object.keys(slots)
  const p: Record<string, string | number> = { ...params }
  names.forEach((n, i) => (p[n] = `${MARK}${i}${MARK}`))
  const text = tr(key, p)
  const out: Seg[] = []
  const parts = text.split(MARK)
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 1) {
      const idx = Number(parts[i])
      if (Number.isInteger(idx) && names[idx] !== undefined) out.push(...slots[names[idx]])
      continue
    }
    if (parts[i]) out.push({ text: parts[i] })
  }
  return merge(out)
}

/** merge adjacent text parts without a target */
function merge(segs: Seg[]): Seg[] {
  const out: Seg[] = []
  for (const s of segs) {
    const last = out[out.length - 1]
    if (last && !last.target && !s.target && !last.missing && !s.missing) last.text += s.text
    else out.push({ ...s })
  }
  return out
}

/** Enumeration (Intl.ListFormat) from segment groups */
export function list(groups: Seg[][], locale: string, type: 'conjunction' | 'disjunction'): Seg[] {
  if (groups.length <= 1) return groups[0] ?? []
  let parts: { type: string; value: string }[]
  try {
    parts = new Intl.ListFormat(locale, { type, style: 'long' }).formatToParts(groups.map((_, i) => `${MARK}${i}${MARK}`))
  } catch {
    parts = groups.flatMap((_, i) => (i ? [{ type: 'literal', value: ', ' }] : []).concat([{ type: 'element', value: `${MARK}${i}${MARK}` }]))
  }
  const out: Seg[] = []
  for (const p of parts) {
    if (p.type === 'element') out.push(...groups[Number(p.value.split(MARK)[1])])
    else out.push({ text: p.value })
  }
  return merge(out)
}

/** Names as a text enumeration in quotation marks ("A", "B" or "C") */
function names(tr: Tr, refs: { id: number; name: string | null }[] | undefined, locale: string, type: 'conjunction' | 'disjunction'): string {
  return list((refs ?? []).map((r) => [{ text: tr('m.auto.s.quote', { PH: refName(r) }) }]), locale, type)
    .map((s) => s.text)
    .join('')
}

const SOURCE_KEYS: Record<number, string> = { 1: 'dialog.workflow.consume_folder', 2: 'common.api_upload', 3: 'common.mail_fetch', 4: 'common.web_ui' }

/** Condition → clause (fits after "if …" or "and …") */
export function conditionText(c: SummaryCondition, tr: Tr, locale: string): string {
  const q = (v: unknown) => String(v ?? '')
  const objs = (type: 'conjunction' | 'disjunction') => names(tr, c.objects ?? (c.object ? [c.object] : []), locale, type)
  const n = (c.objects ?? (c.object ? [c.object] : [])).length
  switch (c.kind) {
    case 'sources':
      return tr('m.auto.s.c_sources', { PH: list((c.values ?? []).map((v) => [{ text: tr(SOURCE_KEYS[v] ?? String(v)) }]), locale, 'disjunction').map((s) => s.text).join('') })
    case 'filename':
      return tr('m.auto.s.c_filename', { PH: q(c.value) })
    case 'path':
      return tr('m.auto.s.c_path', { PH: q(c.value) })
    case 'content_match': {
      const key = ({ 1: 'm.auto.s.c_text_any', 2: 'm.auto.s.c_text_all', 3: 'm.auto.s.c_text_literal', 4: 'm.auto.s.c_text_regex', 5: 'm.auto.s.c_text_fuzzy', 6: 'm.auto.s.c_text_auto' } as Record<number, string>)[c.algorithm ?? 3] ?? 'm.auto.s.c_text_literal'
      return tr(key, { PH: q(c.value) })
    }
    case 'tags_any':
      return tr('m.auto.s.c_tags_any', { N: n, PH: objs('disjunction') })
    case 'tags_all':
      return tr('m.auto.s.c_tags_all', { N: n, PH: objs('conjunction') })
    case 'tags_none':
      return tr('m.auto.s.c_tags_none', { N: n, PH: objs('disjunction') })
    case 'correspondent_is':
    case 'correspondent_any':
      return tr('m.auto.s.c_corr_is', { PH: objs('disjunction') })
    case 'correspondent_none':
      return tr('m.auto.s.c_corr_none', { PH: objs('disjunction') })
    case 'document_type_is':
    case 'document_type_any':
      return tr('m.auto.s.c_type_is', { PH: objs('disjunction') })
    case 'document_type_none':
      return tr('m.auto.s.c_type_none', { PH: objs('disjunction') })
    case 'storage_path_is':
    case 'storage_path_any':
      return tr('m.auto.s.c_path_is', { PH: objs('disjunction') })
    case 'storage_path_none':
      return tr('m.auto.s.c_path_none', { PH: objs('disjunction') })
    case 'mail_rule':
      return tr('m.auto.s.c_mail_rule', { PH: objs('disjunction') })
    case 'custom_field_query':
      return tr('m.auto.s.c_custom_field')
    default:
      return c.kind
  }
}

/** Trigger → clause (without "When"); email trigger pulls the mail rule into the clause */
export function triggerText(t: SummaryTrigger, tr: Tr): string {
  const rule = t.conditions.find((c) => c.kind === 'mail_rule')
  switch (t.type_key) {
    case 'consumption':
      return rule ? tr('m.auto.s.t_mail', { PH: refName(rule.object) }) : tr('m.auto.s.t_consumption')
    case 'document_added':
      return tr('m.auto.s.t_added')
    case 'document_updated':
      return tr('m.auto.s.t_updated')
    case 'scheduled': {
      const s = t.schedule
      const field = s?.date_field === 'custom_field' ? refName(s.custom_field) : tr(({ added: 'common.added', created: 'common.created', modified: 'common.modified' } as Record<string, string>)[s?.date_field ?? 'added'] ?? 'common.added')
      const p = { DAYS: s?.offset_days ?? 0, FIELD: field, EVERY: s?.recurring_interval_days ?? 1 }
      return tr(s?.is_recurring ? 'm.auto.s.t_scheduled_recurring' : 'm.auto.s.t_scheduled', p)
    }
    case 'manual':
      return tr('m.auto.s.t_manual')
    default:
      return tr('m.auto.s.t_unknown')
  }
}

/** Conditions of a trigger that appear as "and …" in the sentence (the mail rule is already included for the email trigger) */
export const sentenceConditions = (t: SummaryTrigger) => (t.type_key === 'consumption' && t.conditions.some((c) => c.kind === 'mail_rule') ? t.conditions.filter((c) => c.kind !== 'mail_rule') : t.conditions)

/** Part of an action → clause (fits after "then …") */
export function itemText(i: SummaryItem, tr: Tr, locale: string): string {
  const n = (i.objects ?? (i.object ? [i.object] : [])).length
  const o = (type: 'conjunction' | 'disjunction' = 'conjunction') => names(tr, i.objects ?? (i.object ? [i.object] : []), locale, type)
  switch (i.kind) {
    case 'assign_title':
      return tr('m.auto.s.a_title', { PH: String(i.value ?? '') })
    case 'assign_tags':
      return tr('m.auto.s.a_tags', { N: n, PH: o() })
    case 'assign_correspondent':
      return tr('m.auto.s.a_correspondent', { PH: o() })
    case 'assign_document_type':
      return tr('m.auto.s.a_document_type', { PH: o() })
    case 'assign_storage_path':
      return tr('m.auto.s.a_storage_path', { PH: o() })
    case 'assign_owner':
      return tr('m.auto.s.a_owner', { PH: o() })
    case 'assign_view_users':
    case 'assign_view_groups':
      return tr('m.auto.s.a_view', { PH: o() })
    case 'assign_change_users':
    case 'assign_change_groups':
      return tr('m.auto.s.a_change', { PH: o() })
    case 'assign_custom_fields':
      return tr('m.auto.s.a_custom_fields', { N: n, PH: o() })
    case 'remove_tags':
      return tr('m.auto.s.r_tags', { N: n, PH: o() })
    case 'remove_correspondents':
      return tr('m.auto.s.r_correspondents', { PH: o('disjunction') })
    case 'remove_document_types':
      return tr('m.auto.s.r_document_types', { PH: o('disjunction') })
    case 'remove_storage_paths':
      return tr('m.auto.s.r_storage_paths', { PH: o('disjunction') })
    case 'remove_owners':
      return tr('m.auto.s.r_owners', { PH: o('disjunction') })
    case 'remove_view_users':
    case 'remove_view_groups':
      return tr('m.auto.s.r_view', { PH: o() })
    case 'remove_change_users':
    case 'remove_change_groups':
      return tr('m.auto.s.r_change', { PH: o() })
    case 'remove_custom_fields':
      return tr('m.auto.s.r_custom_fields', { N: n, PH: o() })
    case 'remove_all_tags':
      return tr('m.auto.s.r_all_tags')
    case 'remove_all_correspondents':
      return tr('m.auto.s.r_all_correspondents')
    case 'remove_all_document_types':
      return tr('m.auto.s.r_all_document_types')
    case 'remove_all_storage_paths':
      return tr('m.auto.s.r_all_storage_paths')
    case 'remove_all_owners':
      return tr('m.auto.s.r_all_owners')
    case 'remove_all_permissions':
      return tr('m.auto.s.r_all_permissions')
    case 'remove_all_custom_fields':
      return tr('m.auto.s.r_all_custom_fields')
    case 'email':
      return i.to ? tr('m.auto.s.a_email', { PH: i.to }) : tr('m.auto.s.a_email_noto')
    case 'webhook':
      return i.url ? tr('m.auto.s.a_webhook', { PH: hostOf(i.url) }) : tr('m.auto.s.a_webhook_nourl')
    case 'ai_suggestions':
      return tr('m.auto.s.a_ai')
    default:
      return i.kind
  }
}

/** Webhook target in the sentence only as host name (no paths/keys in the overview) */
export function hostOf(url: string): string {
  try {
    return new URL(url).host || url
  } catch {
    return url.replace(/^[a-z]+:\/\//i, '').split(/[/?#]/)[0]
  }
}

/** Action → clauses (one part per set field; actions without fields yield a collective text) */
export function actionTexts(a: SummaryAction, tr: Tr, locale: string): string[] {
  switch (a.type_key) {
    case 'password_removal':
      return [tr('m.auto.s.a_password')]
    case 'move_to_trash':
      return [tr('m.auto.s.a_trash')]
    case 'remote_ocr':
      return [tr('m.auto.s.a_remote_ocr')]
    default: {
      const parts = a.items.map((i) => itemText(i, tr, locale))
      // Permissions for users and groups enumerated separately → summarizing per kind would be nicer, but stays correct
      return parts.length ? parts : [tr('m.auto.s.a_empty')]
    }
  }
}

function actionList(actions: SummaryAction[], card: 'action' | 'else', tr: Tr, locale: string): Seg[] {
  const groups: Seg[][] = []
  actions.forEach((a, index) => {
    for (const text of actionTexts(a, tr, locale)) groups.push([{ text, target: { card, index } }])
  })
  return list(groups, locale, 'conjunction')
}

const sameConditions = (a: SummaryTrigger, b: SummaryTrigger) => JSON.stringify(sentenceConditions(a)) === JSON.stringify(sentenceConditions(b))

/** Main sentence "When …, then …" and – with else branch – "Else …" */
export function sentence(s: Summary, tr: Tr, locale: string): { main: Seg[]; else: Seg[] | null } {
  const and = (segs: Seg[][]) => segs.flatMap((g, i) => (i ? [{ text: tr('m.auto.s.and') }, ...g] : g))
  const condSegs = (t: SummaryTrigger, index: number) => sentenceConditions(t).map((c) => [{ text: conditionText(c, tr, locale), target: { card: 'conditions', index } as Target }])
  let when: Seg[]
  if (!s.triggers.length) when = [{ text: tr('m.auto.s.choose_trigger'), missing: true, target: { card: 'trigger', index: 0 } }]
  else if (s.triggers.every((t) => sameConditions(t, s.triggers[0]))) {
    // same conditions on all triggers: "A or B and c1 and c2"
    const trig = list(
      s.triggers.map((t, index) => [{ text: triggerText(t, tr), target: { card: 'trigger', index } as Target }]),
      locale,
      'disjunction',
    )
    when = and([trig, ...condSegs(s.triggers[0], 0)])
  } else {
    const groups = s.triggers.map((t, index) => and([[{ text: triggerText(t, tr), target: { card: 'trigger', index } as Target }], ...condSegs(t, index)]))
    when = groups.flatMap((g, i) => (i ? [{ text: tr('m.auto.s.or_when') }, ...g] : g))
  }
  if (s.ai_condition) {
    const a = s.ai_condition
    const key = ({ document_type: 'm.auto.s.c_ai_document_type', correspondent: 'm.auto.s.c_ai_correspondent', storage_path: 'm.auto.s.c_ai_storage_path', tags: 'm.auto.s.c_ai_tags' } as Record<string, string>)[a.field] ?? 'm.auto.s.c_ai_document_type'
    when = [...when, { text: tr('m.auto.s.and') }, { text: tr(key, { PH: tr('m.auto.s.quote', { PH: refName(a.object) }), PCT: Math.round(a.min_confidence * 100) }), target: { card: 'ai' } }]
  }
  const then: Seg[] = s.actions.length ? actionList(s.actions, 'action', tr, locale) : [{ text: tr('m.auto.s.choose_action'), missing: true, target: { card: 'action', index: 0 } }]
  const main = fill(tr, 'm.auto.s.main', { WHEN: when, THEN: then })
  const els = s.else_actions.length ? fill(tr, 'm.auto.s.else', { THEN: actionList(s.else_actions, 'else', tr, locale) }) : null
  return { main, else: els }
}

/** Sentence as plain text (overview, screen reader) */
export function sentenceText(s: Summary, tr: Tr, locale: string): string {
  const r = sentence(s, tr, locale)
  return [r.main, r.else].filter(Boolean).map((x) => x!.map((p) => p.text).join('')).join(' ')
}
