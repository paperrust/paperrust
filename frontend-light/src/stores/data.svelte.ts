// Master data cache (like the listAll() calls of the Angular services) + sidebar data.
import type { CustomField, MatchingObject, SavedView, Tag, User } from '../api/v1'
import { api } from '../api/client'
import { queryString } from '../api/v1'
import { queryParamsFromFilterRules } from '../lib/query-params'
import { t } from '../lib/i18n'
import { session, SETTINGS_KEYS } from './settings.svelte'

const byId = <T extends { id: number }>(xs: T[]) => new Map(xs.map((x) => [x.id, x]))

class DataState {
  tags = $state(new Map<number, Tag>())
  correspondents = $state(new Map<number, MatchingObject>())
  documentTypes = $state(new Map<number, MatchingObject>())
  storagePaths = $state(new Map<number, MatchingObject>())
  users = $state(new Map<number, User>())
  customFields = $state<CustomField[]>([])
  savedViews = $state<SavedView[]>([])
  savedViewsLoading = $state(true)
  viewCounts = $state<Record<number, number>>({})
  needsAttentionTasks = $state(0)
  loaded = $state(false)

  /** SavedViewService.sidebarViews */
  get sidebarViews(): SavedView[] {
    const visible: number[] = session.get(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS) ?? []
    const views = this.savedViews.filter((v) => visible.includes(v.id))
    const sorted: number[] = session.get(SETTINGS_KEYS.SIDEBAR_VIEWS_SORT_ORDER) ?? []
    return sorted?.length > 0
      ? sorted
          .map((id) => views.find((v) => v.id === id))
          .concat(views.filter((v) => !sorted.includes(v.id)))
          .filter((v): v is SavedView => !!v)
      : [...views]
  }

  /** Loads what the frame and document list need (in parallel, errors per endpoint silent). */
  async loadAll() {
    const can = (t: string) => session.can('view', t)
    const all = <T>(ep: string, ok: boolean) => (ok ? api.listAll<T>(ep).then((r) => r.results).catch(() => [] as T[]) : Promise.resolve([] as T[]))
    const [tags, corr, types, paths, users, cfs, views, tasks] = await Promise.all([
      all<Tag>('tags', can('tag')),
      all<MatchingObject>('correspondents', can('correspondent')),
      all<MatchingObject>('document_types', can('documenttype')),
      all<MatchingObject>('storage_paths', can('storagepath')),
      all<User>('users', can('user')),
      all<CustomField>('custom_fields', can('customfield')),
      all<SavedView>('saved_views', can('savedview')),
      can('paperlesstask') ? api.taskStatusCounts().catch(() => null) : Promise.resolve(null),
    ])
    this.tags = byId(tags)
    this.correspondents = byId(corr)
    this.documentTypes = byId(types)
    this.storagePaths = byId(paths)
    this.users = byId(users)
    this.customFields = cfs
    this.savedViews = views
    this.savedViewsLoading = false
    this.needsAttentionTasks = tasks?.needs_attention ?? 0
    this.loaded = true
    this.refreshViewCounts()
  }

  /** SavedViewService.maybeRefreshDocumentCounts: counters of the sidebar views (one row per view) */
  refreshViewCounts(views: SavedView[] = this.sidebarViews) {
    if (!session.get(SETTINGS_KEYS.SIDEBAR_VIEWS_SHOW_COUNT)) return
    for (const view of views) {
      api
        .get<{ count: number }>(
          `/api/documents/${queryString({
            page: 1,
            page_size: 1,
            ordering: view.sort_field ? (view.sort_reverse ? '-' : '') + view.sort_field : undefined,
            fields: 'id',
            truncate_content: true,
            ...queryParamsFromFilterRules(view.filter_rules as any),
          })}`,
        )
        .then((r) => (this.viewCounts = { ...this.viewCounts, [view.id]: r.count }))
        .catch(() => {})
    }
  }

  /** UsernamePipe: "First name Last name" or user name */
  username(id: number | null): string {
    if (!session.can('view', 'user')) return t('common.shared')
    const u = id == null ? undefined : this.users.get(id)
    if (!u) return ''
    const name = [u.first_name, u.last_name].join(' ')
    return name.length > 1 ? name.trim() : u.username
  }
}

export const data = new DataState()
