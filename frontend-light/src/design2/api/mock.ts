// Demo adapter for timeline/assistant while the backend is missing (only with ?mock=1, docs/DESIGN2.md §7).
// Links real, visible documents of the user (v1 list), so that clicks open a document;
// deadlines/terms are made up and marked as demo. Shapes like the contract (after normalization).
import { api } from '../../api/client'
import type { Document } from '../../api/v1'
import { data } from '../../stores/data.svelte'
import type { AskEvent, TimelineData, TimelineItem } from './assist'

const iso = (d: Date) => d.toISOString().slice(0, 10)
const add = (d: Date, days: number) => new Date(d.getTime() + days * 86400000)

async function docs(n = 40): Promise<Document[]> {
  try {
    return (await api.listDocuments({ page: 1, pageSize: n, sortField: 'created', sortReverse: true })).results
  } catch {
    return []
  }
}

function items(ds: Document[]): TimelineItem[] {
  const today = new Date()
  const out: TimelineItem[] = []
  ds.forEach((d, i) => {
    const documentType = d.document_type != null ? (data.documentTypes.get(d.document_type)?.name ?? null) : null
    const document = { id: d.id, title: d.title, documentType }
    const base = { document, title: d.title, source: 'demo' }
    out.push({ ...base, id: `${d.id}.created`, kind: 'created', date: d.created.slice(0, 10) })
    if (i % 4 === 0) out.push({ ...base, id: `${d.id}.due`, kind: 'due', date: iso(add(today, -4 + i * 3)), label: 'zahlbar bis', amount: `CHF ${(40 + i * 7.5).toFixed(2)}` })
    if (i % 6 === 1) out.push({ ...base, id: `${d.id}.valid_until`, kind: 'valid_until', start: iso(add(today, -200 + i * 5)), date: iso(add(today, 60 + i * 9)), label: 'gültig bis' })
    if (i % 6 === 3) out.push({ ...base, id: `${d.id}.cancel_by`, kind: 'cancel_by', date: iso(add(today, 12 + i * 2)), label: 'Kündigung bis' })
    if (i % 7 === 2) out.push({ ...base, id: `${d.id}.warranty_until`, kind: 'warranty_until', start: d.created.slice(0, 10), date: iso(add(today, 120 + i * 3)), label: 'Garantie bis' })
  })
  return out
}

export async function mockTimeline(from: string, to: string): Promise<TimelineData> {
  const all = items(await docs()).filter((x) => x.date >= from && (x.start ?? x.date) <= to)
  return { from, to, today: iso(new Date()), items: all, demo: true }
}

export async function mockUpcoming(days: number): Promise<TimelineItem[]> {
  const today = iso(new Date())
  const lim = iso(add(new Date(), days))
  const min = iso(add(new Date(), -30))
  return items(await docs())
    .filter((x) => ['due', 'cancel_by', 'valid_until', 'warranty_until'].includes(x.kind) && x.date >= min && x.date <= lim)
    .map((x) => ({ ...x, overdue: x.date < today }))
    .sort((a, b) => (a.date < b.date ? -1 : 1))
}

export async function mockAsk(question: string, onEvent: (e: AskEvent) => void, signal?: AbortSignal): Promise<void> {
  let hits: Document[] = []
  try {
    hits = (await api.listDocuments({ page: 1, pageSize: 3, sortField: 'created', sortReverse: true, extra: { title_content: question.split(/\s+/).slice(0, 2).join(' ') } })).results
  } catch {}
  if (!hits.length) hits = await docs(3)
  onEvent({ type: 'meta', conversation: 'demo', indexState: 'disabled', retrieval: 'fulltext', notes: ['Demo-Daten: das Backend /rs/api/v2/ask ist noch nicht verfügbar.'] })
  onEvent({ type: 'citations', citations: hits.map((d, i) => ({ n: i + 1, document: { id: d.id, title: d.title, page: 1 }, snippet: (d.content ?? '').slice(0, 160), source: 'fulltext' })) })
  const words = `Zu „${question}“ passen ${hits.length} Dokumente: ` + hits.map((d, i) => `„${d.title}“ [${i + 1}]`).join(', ') + '. Öffne eine Quelle, um Details zu sehen.'
  for (const w of words.split(/(?<= )/)) {
    if (signal?.aborted) return
    await new Promise((r) => setTimeout(r, 15))
    onEvent({ type: 'delta', text: w })
  }
  if (hits.length) onEvent({ type: 'actions', actions: [{ type: 'open_documents', label: `${hits.length} Dokumente öffnen`, legacyIds: hits.map((d) => d.id) }] })
  onEvent({ type: 'done' })
}
