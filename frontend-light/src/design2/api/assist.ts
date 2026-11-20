// Typed client for timeline (deadlines, year bar) and questions to the archive (API v2, contract
// docs/contract/pl-api-v2.md "PaperRust-Oberfläche (Design 2)"). Until the endpoints respond (404/405/501/503),
// the client throws `Unavailable`; demo data (mock.ts) only with ?mock=1 or localStorage
// `paperrust.design2.mock=1` (tests, demonstration) – never silently.
// Responses are translated into UI types (documents via `legacy_id`, so that links go to the workspace).
import { getCookie } from '../../api/v1'

/** Date kinds according to the contract */
export type DateKind = 'created' | 'due' | 'cancel_by' | 'valid_from' | 'valid_until' | 'warranty_until'
export const DEADLINE_KINDS: string[] = ['due', 'cancel_by', 'valid_until', 'warranty_until']

export interface DocRef {
  id: number
  uuid?: string | null
  title: string
  page?: number | null
  correspondent?: string | null
  documentType?: string | null
}

export interface TimelineItem {
  id: string
  kind: DateKind | string
  /** Reference date (YYYY-MM-DD) */
  date: string
  /** Start of the span (term/warranty), otherwise null */
  start?: string | null
  document: DocRef | null
  title: string
  /** Source location or field name */
  label?: string | null
  amount?: string | null
  source?: string | null
  done?: boolean
  overdue?: boolean
  daysLeft?: number | null
}

/** Lane of the year bar (grouped in the UI: document type or tag, lib/timeline.ts groupItems) */
export interface TimelineLane {
  key: string
  name: string
  color?: string | null
  items: TimelineItem[]
}

export interface TimelineData {
  from: string
  to: string
  today: string
  /** all entries (lanes of the server merged, without duplicates) */
  items: TimelineItem[]
  extractDates?: boolean
  truncated?: boolean
  demo?: boolean
}

export interface Citation {
  n: number
  document: DocRef
  snippet?: string | null
  source?: string | null
}

export interface AskAction {
  type: 'open_documents' | 'create_reminder' | string
  label: string
  legacyIds?: number[]
  request?: { method: string; path: string; body?: unknown } | null
  date?: string | null
  kind?: string | null
}

export type AskEvent =
  | { type: 'meta'; conversation?: string | null; indexState?: string | null; retrieval?: string | null; notes?: string[] }
  | { type: 'delta'; text: string }
  | { type: 'citations'; citations: Citation[] }
  | { type: 'actions'; actions: AskAction[] }
  | { type: 'done'; answer?: string | null; answerError?: string | null; notes?: string[]; conversation?: string | null }
  | { type: 'error'; message: string }

export interface Conversation {
  uuid: string
  title: string
  updated?: string | null
  messageCount?: number
}

export class Unavailable extends Error {
  constructor(public status: number) {
    super(`not available (${status})`)
  }
}

const V2 = '/rs/api/v2'
const UNAVAILABLE = new Set([404, 405, 501, 503])

export function mockEnabled(loc: { search: string } = location, store: Pick<Storage, 'getItem'> | null = safeStorage()): boolean {
  return /[?&]mock=1\b/.test(loc.search) || store?.getItem('paperrust.design2.mock') === '1'
}

function safeStorage(): Storage | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage : null
  } catch {
    return null
  }
}

// ---- Normalization (contract → UI) ------------------------------------------------------------

const str = (v: unknown): string | null => (typeof v === 'string' && v.length ? v : typeof v === 'number' ? String(v) : null)
const day = (v: unknown): string | null => str(v)?.slice(0, 10) ?? null

export function normalizeDocRef(raw: any, extra: any = {}): DocRef | null {
  if (raw == null) return null
  if (typeof raw === 'string') raw = { uuid: raw, ...extra }
  const id = Number(raw.legacy_id ?? extra.legacy_id ?? raw.id)
  if (!Number.isFinite(id)) return null
  return {
    id,
    uuid: str(raw.uuid),
    title: str(raw.title) ?? str(extra.title) ?? '',
    page: raw.page != null ? Number(raw.page) : extra.page != null ? Number(extra.page) : null,
    correspondent: str(raw.correspondent?.name),
    documentType: str(raw.document_type?.name),
  }
}

function amountText(a: any): string | null {
  if (!a) return null
  if (typeof a === 'string') return a
  return a.value != null ? `${a.currency ?? ''} ${a.value}`.trim() : null
}

export function normalizeItem(raw: any): TimelineItem | null {
  const date = day(raw.date)
  if (!date) return null
  const document = normalizeDocRef(raw.document)
  const start = day(raw.span?.start)
  return {
    id: String(raw.id ?? `${document?.uuid ?? '?'}.${raw.kind}.${date}`),
    kind: String(raw.kind ?? 'created'),
    date,
    start: start && start !== date ? start : null,
    document,
    title: document?.title ?? str(raw.title) ?? '',
    label: str(raw.phrase) ?? str(raw.field?.name) ?? str(raw.reminder?.note),
    amount: amountText(raw.amount),
    source: str(raw.source),
    done: !!raw.state?.done,
    overdue: raw.overdue ?? undefined,
    daysLeft: raw.days_left ?? null,
  }
}

/**
 * Timeline response → flat list. The server groups into lanes (without its own division only `other`); an
 * entry can appear in several lanes there – here each entry counts once (grouping is done by the UI).
 */
export function normalizeTimeline(raw: any, from: string, to: string): TimelineData {
  const seen = new Set<string>()
  const items: TimelineItem[] = []
  const rawItems: any[] = [...(raw?.items ?? []), ...((raw?.lanes ?? []) as any[]).flatMap((l) => l?.items ?? [])]
  for (const r of rawItems) {
    const i = normalizeItem(r)
    if (!i || seen.has(i.id)) continue
    seen.add(i.id)
    items.push(i)
  }
  return { from: day(raw?.from) ?? from, to: day(raw?.to) ?? to, today: day(raw?.today) ?? new Date().toISOString().slice(0, 10), items, extractDates: raw?.extract_dates, truncated: raw?.truncated }
}

export function normalizeCitation(raw: any, i = 0): Citation | null {
  const document = normalizeDocRef(raw.document ?? raw, raw)
  if (!document) return null
  return { n: Number(raw.n ?? i + 1), document, snippet: str(raw.snippet), source: str(raw.source) }
}

export function normalizeAction(raw: any): AskAction {
  const ids = raw.legacy_ids ?? (raw.legacy_id != null ? [raw.legacy_id] : undefined)
  return { type: String(raw.type ?? raw.kind ?? ''), label: String(raw.label ?? ''), legacyIds: ids?.map(Number), request: raw.request ?? null, date: day(raw.date), kind: str(raw.kind) }
}

/** SSE event (name from `event:`, data from `data:`) → AskEvent */
export function toAskEvent(name: string, d: any): AskEvent | null {
  switch (name) {
    case 'meta':
      return { type: 'meta', conversation: str(d?.conversation), indexState: str(d?.index_state), retrieval: str(d?.retrieval), notes: d?.notes ?? [] }
    case 'delta':
      return { type: 'delta', text: String(d?.text ?? '') }
    case 'citations':
      return { type: 'citations', citations: ((d?.citations ?? []) as any[]).map(normalizeCitation).filter(Boolean) as Citation[] }
    case 'actions':
      return { type: 'actions', actions: ((d?.actions ?? []) as any[]).map(normalizeAction) }
    case 'done':
      return { type: 'done', answer: d?.answer ?? null, answerError: d?.answer_error ?? null, notes: d?.notes ?? [], conversation: str(d?.conversation) }
    case 'error':
      return { type: 'error', message: String(d?.error?.message ?? d?.message ?? 'error') }
  }
  return null
}

/** Non-streamed response `{conversation, message: AskMessage}` → events */
export function messageToEvents(raw: any): AskEvent[] {
  const m = raw?.message ?? raw
  const ev: AskEvent[] = [{ type: 'meta', conversation: str(raw?.conversation ?? m?.conversation), indexState: str(m?.index_state), retrieval: str(m?.retrieval), notes: m?.notes ?? [] }]
  const c = ((m?.citations ?? []) as any[]).map(normalizeCitation).filter(Boolean) as Citation[]
  if (c.length) ev.push({ type: 'citations', citations: c })
  if (m?.answer) ev.push({ type: 'delta', text: String(m.answer) })
  if (m?.actions?.length) ev.push({ type: 'actions', actions: m.actions.map(normalizeAction) })
  ev.push({ type: 'done', answer: m?.answer ?? null, answerError: m?.answer_error ?? null, notes: m?.notes ?? [], conversation: str(raw?.conversation) })
  return ev
}

/**
 * SSE parser (`event: x` + `data: {…}`, event ends with a blank line). Returns: finished events and the
 * incomplete remainder of the buffer.
 */
export function parseSse(buffer: string): { events: AskEvent[]; rest: string } {
  const events: AskEvent[] = []
  const blocks = buffer.split(/\r?\n\r?\n/)
  const rest = blocks.pop() ?? ''
  for (const block of blocks) {
    let name = 'message'
    const data: string[] = []
    for (const line of block.split(/\r?\n/)) {
      if (line.startsWith('event:')) name = line.slice(6).trim()
      else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''))
    }
    if (!data.length) continue
    let d: any
    try {
      d = JSON.parse(data.join('\n'))
    } catch {
      d = { text: data.join('\n') }
    }
    const e = toAskEvent(name === 'message' ? String(d?.type ?? 'delta') : name, d)
    if (e) events.push(e)
  }
  return { events, rest }
}

// ---- Transport ------------------------------------------------------------------------------

async function v2(method: string, path: string, body?: unknown, signal?: AbortSignal, accept = 'application/json'): Promise<Response> {
  const headers: Record<string, string> = { Accept: accept }
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  if (method !== 'GET') {
    const csrf = getCookie('csrftoken')
    if (csrf) headers['X-CSRFToken'] = csrf
  }
  const res = await fetch(V2 + path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body), signal })
  // missing endpoint: 404/405/501/503, or (older servers without v2) redirect to the login/HTML page
  const ct = res.headers.get('Content-Type') ?? ''
  if (UNAVAILABLE.has(res.status) || (res.ok && (res.redirected || ct.includes('text/html')))) throw new Unavailable(res.status)
  if (!res.ok) {
    let msg = `HTTP ${res.status}`
    try {
      const j = await res.json()
      msg = j?.error?.message ?? msg
    } catch {}
    throw Object.assign(new Error(msg), { status: res.status })
  }
  return res
}

const mock = () => import('./mock')
const orMock = async <T>(p: () => Promise<T>, m: () => Promise<T>): Promise<T> => {
  try {
    return await p()
  } catch (e) {
    if (e instanceof Unavailable && mockEnabled()) return m()
    throw e
  }
}

export function timeline(from: string, to: string): Promise<TimelineData> {
  return orMock(
    async () => normalizeTimeline(await (await v2('GET', `/timeline?from=${from}&to=${to}`)).json(), from, to),
    async () => (await mock()).mockTimeline(from, to),
  )
}

/** "Upcoming" (deadlines in the next `days` days + overdue) */
export function upcoming(days = 60, overdueDays = 30): Promise<TimelineItem[]> {
  return orMock(
    async () => {
      const j = await (await v2('GET', `/timeline/upcoming?days=${days}&overdue_days=${overdueDays}`)).json()
      return ((j.items ?? []) as any[]).map((i) => normalizeItem(i)).filter(Boolean) as TimelineItem[]
    },
    async () => (await mock()).mockUpcoming(days),
  )
}

/** Mark entry done / snooze */
export async function setItemState(id: string, state: { done?: boolean; snooze_days?: number }) {
  try {
    await v2('PUT', `/timeline/items/${encodeURIComponent(id)}/state`, state)
  } catch (e) {
    if (e instanceof Unavailable && mockEnabled()) return
    throw e
  }
}

export function conversations(): Promise<Conversation[]> {
  return orMock(
    async () => {
      const j = await (await v2('GET', '/ask/conversations?limit=30')).json()
      return ((j.items ?? []) as any[]).map((c) => ({ uuid: String(c.uuid), title: String(c.title ?? ''), updated: str(c.updated), messageCount: c.message_count }))
    },
    async () => [],
  )
}

export interface StoredMessage {
  role: 'user' | 'assistant'
  content: string
  citations: Citation[]
  actions: AskAction[]
  notes: string[]
  answerError: string | null
}
/** History of a conversation (`GET /ask/conversations/{uuid}`) */
export async function conversation(uuid: string): Promise<StoredMessage[]> {
  const j = await (await v2('GET', `/ask/conversations/${encodeURIComponent(uuid)}`)).json()
  return ((j.messages ?? []) as any[]).map((m) => ({
    role: m.role === 'user' ? 'user' : 'assistant',
    content: String(m.content ?? ''),
    citations: ((m.citations ?? []) as any[]).map(normalizeCitation).filter(Boolean) as Citation[],
    actions: ((m.actions ?? []) as any[]).map(normalizeAction),
    notes: m.meta?.notes ?? [],
    answerError: m.meta?.answer_error ?? null,
  }))
}

/** Execute the action of an assistant suggestion (v2 paths only, e.g. create reminder) */
export async function runRequest(r: { method: string; path: string; body?: unknown }) {
  if (!r.path.startsWith(V2 + '/')) throw new Error('unsupported action path')
  await v2(r.method.toUpperCase(), r.path.slice(V2.length), r.body)
}

/** Ask a question; events arrive via SSE (contract) or as a single JSON response. */
export async function ask(question: string, conversation: string | null, onEvent: (e: AskEvent) => void, signal?: AbortSignal, documents?: string[]): Promise<void> {
  let res: Response
  try {
    res = await v2('POST', '/ask', { question, conversation: conversation ?? undefined, documents: documents?.length ? documents : undefined, stream: true }, signal, 'text/event-stream, application/json')
  } catch (e) {
    if (e instanceof Unavailable && mockEnabled()) return (await mock()).mockAsk(question, onEvent, signal)
    throw e
  }
  const type = res.headers.get('Content-Type') ?? ''
  if (!type.includes('event-stream') || !res.body) {
    for (const ev of messageToEvents(await res.json())) onEvent(ev)
    return
  }
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ''
  let ended = false
  for (;;) {
    const r = await reader.read()
    if (r.done) break
    buf += dec.decode(r.value, { stream: true })
    const p = parseSse(buf)
    buf = p.rest
    for (const ev of p.events) {
      if (ev.type === 'done' || ev.type === 'error') ended = true
      onEvent(ev)
    }
  }
  if (buf.trim()) for (const ev of parseSse(buf + '\n\n').events) onEvent(ev)
  if (!ended) onEvent({ type: 'done' })
}
