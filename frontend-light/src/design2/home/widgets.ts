// Modular home page (docs/DESIGN2-ENTWIRREN.md §4): components, default, reading/writing the selection per user
// (ui_settings `paperrust.home.widgets` = [{ id, on }] in order). Pure functions (Vitest, home.test.ts).

export const HOME_KEY = 'paperrust.home.widgets'

export type WidgetId = 'ask' | 'deadlines' | 'inbox' | 'recent' | 'views' | 'year' | 'stats' | 'upload' | 'ai'
export interface WidgetEntry {
  id: WidgetId
  on: boolean
}

/** All components in default order */
export const WIDGET_IDS: WidgetId[] = ['ask', 'inbox', 'deadlines', 'recent', 'views', 'year', 'stats', 'upload', 'ai']
/** Default for new users (+ "Question to the archive" and "AI suggestions" when AI is on) */
const DEFAULT_ON: WidgetId[] = ['inbox', 'deadlines', 'recent', 'views']
/** Components across the full width */
export const WIDE: WidgetId[] = ['ask', 'year']

const isId = (v: unknown): v is WidgetId => typeof v === 'string' && (WIDGET_IDS as string[]).includes(v)

export function defaultWidgets(aiOn: boolean): WidgetEntry[] {
  return WIDGET_IDS.map((id) => ({ id, on: id === 'ask' || id === 'ai' ? aiOn : DEFAULT_ON.includes(id) }))
}

/**
 * Read the stored value. Nothing stored (or unreadable) → default. Unknown and duplicate entries are
 * dropped; components that did not exist at save time are appended switched off. Also a plain list of
 * IDs (= switched-on components) is understood.
 */
export function normalizeWidgets(raw: unknown, aiOn: boolean): WidgetEntry[] {
  if (!Array.isArray(raw)) return defaultWidgets(aiOn)
  const out: WidgetEntry[] = []
  for (const x of raw) {
    const id = typeof x === 'string' ? x : x && typeof x === 'object' ? (x as any).id : null
    if (!isId(id) || out.some((e) => e.id === id)) continue
    out.push({ id, on: typeof x === 'string' ? true : (x as any).on !== false })
  }
  for (const id of WIDGET_IDS) if (!out.some((e) => e.id === id)) out.push({ id, on: false })
  return out
}

/** Visible is what is switched on and available ("Question to the archive" and "AI suggestions" only with AI) */
export function available(id: WidgetId, aiOn: boolean): boolean {
  return (id !== 'ask' && id !== 'ai') || aiOn
}
export function visibleWidgets(list: WidgetEntry[], aiOn: boolean): WidgetId[] {
  return list.filter((e) => e.on && available(e.id, aiOn)).map((e) => e.id)
}

/** Move entry `index` by `delta` (clamped at the edges); new list */
export function moveWidget(list: WidgetEntry[], index: number, delta: number): WidgetEntry[] {
  return moveWidgetTo(list, index, index + delta)
}
export function moveWidgetTo(list: WidgetEntry[], from: number, to: number): WidgetEntry[] {
  const out = [...list]
  if (from < 0 || from >= out.length) return out
  const t = Math.max(0, Math.min(out.length - 1, to))
  const [e] = out.splice(from, 1)
  out.splice(t, 0, e)
  return out
}

export function toggleWidget(list: WidgetEntry[], id: WidgetId, on: boolean): WidgetEntry[] {
  return list.map((e) => (e.id === id ? { ...e, on } : e))
}

/** Storage form (only id/on, order) */
export function serializeWidgets(list: WidgetEntry[]): WidgetEntry[] {
  return list.map(({ id, on }) => ({ id, on }))
}

/** Saved views with "On the home page" (same keys as the classic dashboard) */
export const VIEWS_VISIBLE_KEY = 'general-settings:saved-views:dashboard-views-visible-ids'
export const VIEWS_SORT_KEY = 'general-settings:saved-views:dashboard-views-sort-order'

/** SavedViewService.dashboardViews: visible views, sorted by setting, rest appended */
export function homeViews<T extends { id: number }>(views: T[], visibleIds: unknown, sortOrder: unknown): T[] {
  const visible = Array.isArray(visibleIds) ? (visibleIds as unknown[]).map(Number) : []
  const shown = views.filter((v) => visible.includes(v.id))
  const sorted = Array.isArray(sortOrder) ? (sortOrder as unknown[]).map(Number) : []
  const head = sorted.map((id) => shown.find((v) => v.id === id)).filter((v): v is T => !!v)
  return [...head, ...shown.filter((v) => !head.includes(v))]
}
