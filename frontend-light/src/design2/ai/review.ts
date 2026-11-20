// Pure logic of the AI review list (Vitest: review.test.ts): name confidence understandably, old → new value per
// field, open suggestions, request body for "Accept/Change/Discard".
import type { Override, Review, ReviewChange, ReviewField } from './api'

/** Thresholds like the server (auto::plan: existing object 0.95, new 0.5; acceptance from 0.9). */
export const SURE = 0.9
export const LIKELY = 0.6

export type Certainty = 'sure' | 'likely' | 'unsure'
export function certainty(c: number): Certainty {
  return c >= SURE ? 'sure' : c >= LIKELY ? 'likely' : 'unsure'
}

/** Translation key of the confidence */
export const CERTAINTY_KEY: Record<Certainty, string> = {
  sure: 'm.ai.sure',
  likely: 'm.ai.likely',
  unsure: 'm.ai.unsure',
}

/** Translation key of the fields */
export const FIELD_KEY: Record<ReviewField, string> = {
  title: 'common.title',
  tags: 'common.tags',
  correspondent: 'common.correspondent',
  document_type: 'common.document_type',
  storage_path: 'common.storage_path',
  created: 'm.ai.date',
  custom_fields: 'm.ai.custom_field',
}

export const isOpen = (c: ReviewChange) => !c.applied && !c.dismissed

export function openIndexes(r: Review): number[] {
  return r.changes.map((c, i) => (isOpen(c) ? i : -1)).filter((i) => i >= 0)
}

export const reviewIsOpen = (r: Review) => r.status === 'pending' || r.status === 'partial'

/** Undo only if something was accepted and the entry has not already been reverted. */
export const canUndo = (r: Review) => r.audit !== null && r.status !== 'undone' && r.changes.some((c) => c.applied)

/** Current values of the document (excerpt of the v1 document response). */
export interface DocNow {
  title: string
  created: string | null
  correspondent: number | null
  document_type: number | null
  storage_path: number | null
  tags: number[]
  custom_fields: { field: number; value: unknown }[]
}

/** Names of the objects (from the master data store). */
export interface Names {
  tag(id: number): string | null
  correspondent(id: number): string | null
  documentType(id: number): string | null
  storagePath(id: number): string | null
  field(id: number): string | null
}

const objName = (f: ReviewField, id: number, n: Names) =>
  f === 'tags' ? n.tag(id) : f === 'correspondent' ? n.correspondent(id) : f === 'document_type' ? n.documentType(id) : f === 'storage_path' ? n.storagePath(id) : null

/** New value as text; `isNew` = object would be newly created. */
export function newValue(c: ReviewChange, n: Names): { text: string; isNew: boolean } {
  if (c.field === 'title' || c.field === 'created' || c.field === 'custom_fields') return { text: c.value ?? '', isNew: false }
  if (c.existing_id !== null) return { text: objName(c.field, c.existing_id, n) ?? `#${c.existing_id}`, isNew: false }
  return { text: c.new_name ?? '', isNew: true }
}

/** Previous value (tags: `null` because a tag is added; empty → ''). */
export function oldValue(c: ReviewChange, doc: DocNow | null, n: Names): string | null {
  if (!doc) return ''
  switch (c.field) {
    case 'title':
      return doc.title ?? ''
    case 'created':
      return doc.created ? doc.created.slice(0, 10) : ''
    case 'correspondent':
      return doc.correspondent !== null ? (n.correspondent(doc.correspondent) ?? `#${doc.correspondent}`) : ''
    case 'document_type':
      return doc.document_type !== null ? (n.documentType(doc.document_type) ?? `#${doc.document_type}`) : ''
    case 'storage_path':
      return doc.storage_path !== null ? (n.storagePath(doc.storage_path) ?? `#${doc.storage_path}`) : ''
    case 'custom_fields': {
      const cur = doc.custom_fields.find((x) => x.field === c.existing_id)
      return cur && cur.value !== null && cur.value !== undefined ? String(cur.value) : ''
    }
    default:
      return null
  }
}

/** Is the suggestion already fulfilled (same value or tag already present)? */
export function alreadyThere(c: ReviewChange, doc: DocNow | null): boolean {
  if (!doc) return false
  switch (c.field) {
    case 'tags':
      return c.existing_id !== null && doc.tags.includes(c.existing_id)
    case 'title':
      return (c.value ?? '') === doc.title
    case 'created':
      return !!doc.created && doc.created.slice(0, 10) === c.value
    case 'correspondent':
    case 'document_type':
    case 'storage_path':
      return c.existing_id !== null && doc[c.field] === c.existing_id
    default:
      return false
  }
}

/** Body for "Accept" of a selection, with changed values and discarded suggestions. */
export function applyBody(take: number[], edits: Record<number, Override>, dismiss: number[] = []): { changes: number[]; overrides?: Record<string, Override>; dismiss?: number[] } {
  const body: { changes: number[]; overrides?: Record<string, Override>; dismiss?: number[] } = { changes: [...new Set(take)].sort((a, b) => a - b) }
  const ov: Record<string, Override> = {}
  for (const i of body.changes) if (edits[i]) ov[String(i)] = edits[i]
  if (Object.keys(ov).length) body.overrides = ov
  const d = [...new Set(dismiss)].filter((i) => !body.changes.includes(i)).sort((a, b) => a - b)
  if (d.length) body.dismiss = d
  return body
}

/** Summary "3 suggestions, 2 certain" */
export function summary(r: Review): { open: number; sure: number } {
  const open = r.changes.filter(isOpen)
  return { open: open.length, sure: open.filter((c) => certainty(c.confidence) === 'sure').length }
}

/** Translation key of the triggers */
export const TRIGGER_KEY: Record<Review['trigger'], string> = {
  consumed: 'm.ai.trigger_consumed',
  tag: 'm.ai.trigger_tag',
  workflow: 'm.ai.trigger_workflow',
  manual: 'm.ai.trigger_manual',
}

/** Translation key of the status */
export const STATUS_KEY: Record<Review['status'], string> = {
  pending: 'm.ai.status_pending',
  partial: 'm.ai.status_partial',
  applied: 'm.ai.status_applied',
  rejected: 'm.ai.status_rejected',
  undone: 'm.ai.status_undone',
}
