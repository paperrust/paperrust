// Pure logic of the dashboard (dashboard.component.ts, SavedViewService.dashboardViews, SettingsService.offerTour,
// SettingsService.allDisplayFields). Without Svelte – tested in dashboard.test.ts.
import type { SavedView } from '../../api/v1'
import { DEFAULT_DISPLAY_FIELDS } from '../../stores/listView.svelte'
import { session } from '../../stores/settings.svelte'
import { data } from '../../stores/data.svelte'

/** SavedView with the display fields the dashboard needs (v1 delivers them) */
export type DashboardView = SavedView & {
  page_size?: number | null
  display_mode?: 'table' | 'smallCards' | 'largeCards' | null
  display_fields?: string[] | null
}

export const DEFAULT_DASHBOARD_VIEW_PAGE_SIZE = 10
export const DEFAULT_DASHBOARD_DISPLAY_FIELDS = ['created', 'title', 'tag', 'correspondent']

export const DASHBOARD_KEYS = {
  VISIBLE_IDS: 'general-settings:saved-views:dashboard-views-visible-ids',
  SORT_ORDER: 'general-settings:saved-views:dashboard-views-sort-order',
  TOUR_COMPLETE: 'general-settings:tour-complete',
} as const

/** SavedViewService.dashboardViews: visible views, sorted by setting, rest appended */
export function dashboardViews<T extends { id: number }>(views: T[], visibleIds: unknown, sortOrder: unknown): T[] {
  const visible = Array.isArray(visibleIds) ? (visibleIds as number[]) : []
  const dash = views.filter((v) => visible.includes(v.id))
  const sorted = Array.isArray(sortOrder) ? (sortOrder as number[]) : []
  return sorted.length > 0
    ? sorted
        .map((id) => dash.find((v) => v.id === id))
        .concat(dash.filter((v) => !sorted.includes(v.id)))
        .filter((v): v is T => !!v)
    : [...dash]
}

/** SettingsService.allDisplayFields (IDs): permissions, notes, custom fields */
export function displayFieldIds(can: (type: string) => boolean, notesEnabled: boolean, customFieldIds: number[]): string[] {
  const perm: Record<string, string> = { tag: 'tag', correspondent: 'correspondent', documenttype: 'documenttype', storagepath: 'storagepath', owner: 'user', note: 'note' }
  const base = DEFAULT_DISPLAY_FIELDS.filter((f) => {
    if (f.id === 'note' && !notesEnabled) return false
    if (['title', 'created', 'added', 'asn', 'pagecount', 'shared'].includes(f.id)) return true
    return can(perm[f.id])
  }).map((f) => f.id)
  return can('customfield') ? [...base, ...customFieldIds.map((id) => `custom_field_${id}`)] : base
}

export function allDisplayFieldIds(notesEnabled: boolean): string[] {
  return displayFieldIds(
    (type) => session.can('view', type),
    notesEnabled,
    data.customFields.map((c) => c.id),
  )
}

/** Like getSetting: boolean values may be stored as a string */
export function truthy(v: unknown): boolean {
  if (typeof v === 'string') {
    try {
      return !!JSON.parse(v)
    } catch {
      return !!v
    }
  }
  return !!v
}

/** SavedViewWidgetComponent.pageSize (upstream #14699) */
export function dashboardPageSize(view: { page_size?: number | null } | null | undefined): number {
  return view?.page_size ?? DEFAULT_DASHBOARD_VIEW_PAGE_SIZE
}

/** Part of the DocumentListViewService API that the widget needs (activateSavedViewWithDocuments, upstream #14699) */
export interface ViewActivator {
  activateSavedViewWithDocuments?: (view: any, documents: any[], collectionSize: number, pageSize: number) => void
}

/** SavedViewWidgetComponent.activateViewForNavigation: pass the widget's documents to the list (without reloading) */
export function activateViewWithDocuments<D>(service: ViewActivator, view: DashboardView, documents: D[], count: number, pageSize: number): void {
  service.activateSavedViewWithDocuments?.(view, documents, count, pageSize)
}
