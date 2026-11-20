// Logic of manage/saved-views/saved-views.component.ts: load all views (paging only in the display), form per
// view, change detection, save (PATCH only changed views, visibility into the UI settings), delete.
import { listAllOf, deleteOf, patchOf } from '../../../api/admin'
import type { SavedView } from '../../../api/v1'
import { rawValue, session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
import { setSetting, storeSettings } from '../../../stores/settingsSave.svelte'
import { DEFAULT_SAVED_VIEW_ICON } from './saved-view-icons'

export type FullSavedView = SavedView & {
  page_size?: number | null
  display_mode?: string | null
  display_fields?: string[] | null
  owner?: number | null
  permissions?: any
  user_can_change?: boolean
}

export interface ViewForm {
  id: number
  name: string
  icon: string
  show_on_dashboard: boolean
  show_in_sidebar: boolean
  page_size: number | null
  display_mode: string | null
  display_fields: string[] | null
}

const MODEL_FIELDS = ['name', 'icon', 'page_size', 'display_mode', 'display_fields'] as const

export const DASHBOARD_VIEWS_VISIBLE_IDS = 'general-settings:saved-views:dashboard-views-visible-ids'

/** SavedViewService.getVisibleViewIds */
export function visibleIds(key: string): number[] {
  const v = rawValue(session.settings, key)
  return Array.isArray(v) ? (v as number[]) : []
}

/** initialize(): form values from view + UI settings */
export function formFromView(view: FullSavedView, dashboardIds: number[], sidebarIds: number[]): ViewForm {
  return {
    id: view.id,
    name: view.name,
    icon: view.icon ?? DEFAULT_SAVED_VIEW_ICON,
    show_on_dashboard: dashboardIds.includes(view.id),
    show_in_sidebar: sidebarIds.includes(view.id),
    page_size: view.page_size ?? null,
    display_mode: view.display_mode ?? null,
    display_fields: view.display_fields ?? null,
  }
}

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null)

/**
 * save(): changed views (model fields only, editable only) and whether the visibility changed.
 * Angular uses control.dirty; here the value change compared to the loaded state counts (same effect for users).
 */
export function computeSave(forms: ViewForm[], store: Record<number, ViewForm>, canEdit: (id: number) => boolean) {
  const changed: Omit<ViewForm, 'show_on_dashboard' | 'show_in_sidebar'>[] = []
  const dashboard: number[] = []
  const sidebar: number[] = []
  let visibilityChanged = false
  for (const f of forms) {
    const s = store[f.id]
    if (f.show_on_dashboard !== s.show_on_dashboard || f.show_in_sidebar !== s.show_in_sidebar) visibilityChanged = true
    if (f.show_on_dashboard) dashboard.push(f.id)
    if (f.show_in_sidebar) sidebar.push(f.id)
    if (!canEdit(f.id)) continue
    if (MODEL_FIELDS.some((k) => !same(f[k], s[k]))) {
      const { show_on_dashboard: _d, show_in_sidebar: _s, ...value } = f
      changed.push(value)
    }
  }
  return { changed, dashboard, sidebar, visibilityChanged }
}

export const isDirtyForms = (forms: ViewForm[], store: Record<number, ViewForm>) => forms.some((f) => !same(f, store[f.id]))

export class SavedViewsState {
  views = $state<FullSavedView[] | undefined>(undefined)
  forms = $state<ViewForm[]>([])
  store = $state<Record<number, ViewForm>>({})
  loading = $state(true)
  page = $state(1)
  readonly pageSize = 25

  get pagedForms() {
    const start = (this.page - 1) * this.pageSize
    return this.forms.slice(start, start + this.pageSize)
  }
  get dirty() {
    return isDirtyForms(this.forms, this.store)
  }

  async reload() {
    this.loading = true
    const r = await listAllOf<FullSavedView>('saved_views', { full_perms: true })
    this.views = r.results
    const pageCount = Math.ceil(r.results.length / this.pageSize)
    this.page = Math.min(this.page, Math.max(1, pageCount))
    this.initialize()
  }

  initialize() {
    this.loading = false
    const dash = visibleIds(DASHBOARD_VIEWS_VISIBLE_IDS)
    const side = visibleIds(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS)
    const store: Record<number, ViewForm> = {}
    for (const v of this.views ?? []) store[v.id] = formFromView(v, dash, side)
    this.store = store
    this.forms = (this.views ?? []).map((v) => ({ ...store[v.id], display_fields: store[v.id].display_fields ? [...store[v.id].display_fields!] : null }))
  }

  reset() {
    this.forms = (this.views ?? []).map((v) => ({ ...this.store[v.id] }))
  }

  /** Return: true = saved, false = nothing to do */
  async save(canEdit: (id: number) => boolean, canSaveSettings: boolean): Promise<boolean> {
    const { changed, dashboard, sidebar, visibilityChanged } = computeSave(this.forms, this.store, canEdit)
    if (!changed.length && !visibilityChanged) return false
    if (changed.length) await Promise.all(changed.map((v) => patchOf('saved_views', v)))
    if (visibilityChanged && canSaveSettings) {
      setSetting(DASHBOARD_VIEWS_VISIBLE_IDS, [...new Set(dashboard)])
      setSetting(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS, [...new Set(sidebar)])
      await storeSettings()
    }
    return true
  }

  async remove(view: FullSavedView) {
    await deleteOf('saved_views', view.id)
  }
}
