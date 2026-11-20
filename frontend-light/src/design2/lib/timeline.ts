// Timeline geometry (docs/DESIGN2.md §4.5, year bar of the home page docs/DESIGN2-ENTWIRREN.md §4):
// month axis, position per date, lanes by document type or tag, rows per lane without overlap,
// "Deadlines and appointments" (fallback when /timeline/upcoming is missing). Pure functions (Vitest), date YYYY-MM-DD (UTC).
import { DEADLINE_KINDS, type TimelineItem, type TimelineLane } from '../api/assist'

const ms = (d: string) => Date.UTC(+d.slice(0, 4), +d.slice(5, 7) - 1, +d.slice(8, 10))
export const DAY = 86400000

export const isoDay = (t: number) => new Date(t).toISOString().slice(0, 10)
export const addDays = (d: string, n: number) => isoDay(ms(d) + n * DAY)
export const daysBetween = (a: string, b: string) => Math.round((ms(b) - ms(a)) / DAY)

/** Year window: 1 Jan to 31 Dec. */
export function yearRange(year: number): { from: string; to: string } {
  return { from: `${year}-01-01`, to: `${year}-12-31` }
}

/** Share 0..1 of a date in the window (clamped) */
export function position(date: string, from: string, to: string): number {
  const a = ms(from)
  const b = ms(to) + DAY
  return Math.min(1, Math.max(0, (ms(date) - a) / (b - a)))
}

export interface Month {
  key: string // YYYY-MM
  start: number
  width: number
  index: number // 0..11
}

export function months(from: string, to: string): Month[] {
  const out: Month[] = []
  let y = +from.slice(0, 4)
  let m = +from.slice(5, 7) - 1
  const end = ms(to)
  for (;;) {
    const first = `${y}-${String(m + 1).padStart(2, '0')}-01`
    if (ms(first) > end) break
    const next = m === 11 ? `${y + 1}-01-01` : `${y}-${String(m + 2).padStart(2, '0')}-01`
    const s = position(first < from ? from : first, from, to)
    const e = ms(next) > end ? 1 : position(next, from, to)
    out.push({ key: first.slice(0, 7), start: s, width: e - s, index: m })
    m++
    if (m === 12) {
      m = 0
      y++
    }
  }
  return out
}

export interface Placed {
  item: TimelineItem
  /** left edge of the span (or date) */
  x: number
  /** width of the span, 0 = no span */
  w: number
  /** reference date as share (marker) */
  at: number
  /** Row of the marker */
  row: number
  /** Row of the span (only w > 0) */
  spanRow: number
}

export interface PlacedLane {
  lane: TimelineLane
  /** Rows of the markers */
  rows: number
  /** Rows of the (thin) spans above the markers */
  spanRows: number
  items: Placed[]
}

/** First row whose end is ≤ x (otherwise new row); occupied until x + len */
function pack(ends: number[], x: number, len: number): number {
  let r = ends.findIndex((e) => e <= x)
  if (r < 0) {
    r = ends.length
    ends.push(0)
  }
  ends[r] = x + len
  return r
}

/**
 * Per lane: spans (term/warranty) as thin stacked bars, below them markers (reference dates,
 * document dates) in overlap-free rows (minimum distance `minGap`).
 */
export function place(lanes: TimelineLane[], from: string, to: string, minGap = 0.028, hideKinds: string[] = []): PlacedLane[] {
  return lanes.map((lane) => {
    const its = lane.items.filter((i) => !hideKinds.includes(i.kind)).sort((a, b) => ((a.start ?? a.date) < (b.start ?? b.date) ? -1 : (a.start ?? a.date) > (b.start ?? b.date) ? 1 : 0))
    const spanEnds: number[] = []
    const markEnds: number[] = []
    const placed = its.map((item) => {
      const at = position(item.date, from, to)
      const x = item.start ? position(item.start, from, to) : at
      const w = item.start ? Math.max(0, at - x) : 0
      const spanRow = w > 0 ? pack(spanEnds, x, w + 0.004) : 0
      return { item, x, w, at, row: 0, spanRow }
    })
    // only reference dates in the window get a marker (spans beyond the year end: bar only)
    for (const p of [...placed].sort((a, b) => a.at - b.at)) {
      if (p.item.date < from || p.item.date > to) p.row = -1
      else p.row = pack(markEnds, p.at - minGap / 2, minGap)
    }
    return { lane, rows: Math.max(1, markEnds.length), spanRows: spanEnds.length, items: placed }
  })
}

export function isDeadline(kind: string) {
  return DEADLINE_KINDS.includes(kind)
}

/** "Deadlines and appointments" from entries (if the server delivers no list): deadlines in [today − overdue, today + horizon] */
export function upcomingFrom(items: TimelineItem[], today: string, horizon = 60, overdue = 30): TimelineItem[] {
  const seen = new Set<string>()
  const out: TimelineItem[] = []
  for (const i of items) {
    if (!isDeadline(i.kind) || i.done || seen.has(i.id)) continue
    const d = daysBetween(today, i.date)
    if (d < -overdue || d > horizon) continue
    seen.add(i.id)
    out.push({ ...i, daysLeft: d, overdue: d < 0 })
  }
  return out.sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : a.title.localeCompare(b.title)))
}

/** Urgency: overdue/≤ 7 days high, ≤ 30 medium */
export function urgency(daysLeft: number): 'high' | 'medium' | 'low' {
  return daysLeft <= 7 ? 'high' : daysLeft <= 30 ? 'medium' : 'low'
}

export type LaneGrouping = 'type' | 'tag'
/** Key of the lane for entries without document type or without tag (always last) */
export const NO_GROUP = 'none'

/**
 * Lanes of the year bar without setup: per document type (name from the timeline response) or per tag
 * (`tagsOf` returns the tag IDs of a document, `tagInfo` name/color). A document with several tags appears in
 * each of its tag lanes. Order: by name, "none" last (only if it has entries).
 */
export function groupItems(
  items: TimelineItem[],
  by: LaneGrouping,
  noneName: string,
  tagsOf: (docId: number) => number[] = () => [],
  tagInfo: (id: number) => { name: string; color?: string | null } | undefined = () => undefined,
): TimelineLane[] {
  const lanes = new Map<string, TimelineLane>()
  const put = (key: string, name: string, color: string | null, i: TimelineItem) => {
    let l = lanes.get(key)
    if (!l) lanes.set(key, (l = { key, name, color, items: [] }))
    l.items.push(i)
  }
  for (const i of items) {
    if (by === 'type') {
      const n = i.document?.documentType?.trim()
      if (n) put('t:' + n.toLowerCase(), n, null, i)
      else put(NO_GROUP, noneName, null, i)
    } else {
      const known = (i.document ? tagsOf(i.document.id) : []).map((id) => [id, tagInfo(id)] as const).filter(([, x]) => !!x)
      if (!known.length) put(NO_GROUP, noneName, null, i)
      for (const [id, x] of known) put('g:' + id, x!.name, x!.color ?? null, i)
    }
  }
  const out = [...lanes.values()].filter((l) => l.key !== NO_GROUP)
  out.sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key))
  const none = lanes.get(NO_GROUP)
  return none ? [...out, none] : out
}
