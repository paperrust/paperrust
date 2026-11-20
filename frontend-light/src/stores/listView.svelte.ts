// Port of services/document-list-view.service.ts (document list incl. saved views /view/<id>):
// state, reloading via v1 (listFiltered), URL parameters like Angular (paramsFromViewState), localStorage
// "document-list-service:currentViewConfig" (same key/format as Angular), selection incl. "all".
import { api } from '../api/client'
import { msg, t } from '../lib/i18n'
import type { Document } from '../api/v1'
import { queryString } from '../api/v1'
import type { FilterRule } from '../lib/filter-rules'
import { cloneFilterRules, filterRulesDiffer, isFullTextFilterRule } from '../lib/filter-rules'
import { paramsFromViewState, paramsToViewState, paramsToURLSearchParams, queryParamsFromFilterRules } from '../lib/query-params'
import { session, SETTINGS_KEYS } from './settings.svelte'
import { router } from '../router.svelte'
import { searchQueryFromRules, setSearchQuery } from './searchQuery'

export const CURRENT_VIEW_CONFIG = 'document-list-service:currentViewConfig'

/** Fields of a saved view that applySavedViewConfig takes over */
export interface SavedViewConfig {
  id: number
  name: string
  filter_rules: { rule_type: number; value: string | null }[]
  sort_field: string | null
  sort_reverse: boolean
  display_mode?: DisplayMode | null
  page_size?: number | null
  display_fields?: string[] | null
}
export type DisplayMode = 'table' | 'smallCards' | 'largeCards'

/** DEFAULT_DISPLAY_FIELDS (data/document.ts) */
export const DEFAULT_DISPLAY_FIELDS: { id: string; name: string }[] = [
  { id: 'title', name: msg('common.title') },
  { id: 'created', name: msg('common.created') },
  { id: 'added', name: msg('common.added') },
  { id: 'tag', name: msg('common.tags') },
  { id: 'correspondent', name: msg('common.correspondent') },
  { id: 'documenttype', name: msg('common.document_type') },
  { id: 'storagepath', name: msg('common.storage_path') },
  { id: 'note', name: msg('common.notes') },
  { id: 'owner', name: msg('common.owner') },
  { id: 'shared', name: msg('common.shared') },
  { id: 'asn', name: msg('common.asn') },
  { id: 'pagecount', name: msg('common.pages') },
]
export const LIST_DEFAULT_DISPLAY_FIELDS = DEFAULT_DISPLAY_FIELDS.map((f) => f.id).filter((f) => f !== 'added')

/** DOCUMENT_SORT_FIELDS (+ full text "score") */
export const DOCUMENT_SORT_FIELDS = [
  { field: 'archive_serial_number', name: msg('common.asn') },
  { field: 'correspondent__name', name: msg('common.correspondent') },
  { field: 'title', name: msg('common.title') },
  { field: 'document_type__name', name: msg('common.document_type') },
  { field: 'created', name: msg('common.created') },
  { field: 'added', name: msg('common.added') },
  { field: 'modified', name: msg('common.modified') },
  { field: 'num_notes', name: msg('common.notes') },
  { field: 'owner', name: msg('common.owner') },
  { field: 'page_count', name: msg('common.pages') },
]

export interface SelectionDataItem {
  id: number
  document_count: number
}
export interface SelectionData {
  selected_correspondents?: SelectionDataItem[]
  selected_tags?: SelectionDataItem[]
  selected_document_types?: SelectionDataItem[]
  selected_storage_paths?: SelectionDataItem[]
  selected_custom_fields?: SelectionDataItem[]
}

function readStored(): Partial<{
  currentPage: number
  filterRules: FilterRule[]
  sortField: string
  sortReverse: boolean
  displayMode: DisplayMode
  displayFields: string[]
}> {
  try {
    const s = JSON.parse(localStorage.getItem(CURRENT_VIEW_CONFIG) ?? 'null')
    return s && typeof s === 'object' && !Array.isArray(s) ? s : {}
  } catch {
    try {
      localStorage.removeItem(CURRENT_VIEW_CONFIG)
    } catch {}
    return {}
  }
}

class ListViewState {
  documents = $state<Document[]>([])
  collectionSize = $state<number | null>(null)
  currentPage = $state(1)
  sortField = $state('created')
  sortReverse = $state(true)
  filterRules = $state<FilterRule[]>([])
  displayMode = $state<DisplayMode | undefined>(undefined)
  displayFieldsRaw = $state<string[] | undefined>(undefined)
  selected = $state(new Set<number>())
  excluded = $state(new Set<number>())
  allSelected = $state(false)
  selectionData = $state<SelectionData | null>(null)
  isReloading = $state(false)
  initialized = $state(false)
  error = $state<string | null>(null)
  private seq = 0
  // Shift-click range selection (rangeSelectionAnchorIndex / lastRangeSelectionToIndex)
  private rangeAnchor: number | null = null
  private rangeTo: number | null = null

  constructor() {
    const s = readStored()
    for (const k of ['currentPage', 'filterRules', 'sortField', 'sortReverse', 'displayMode', 'displayFields'] as const) {
      const v = s[k]
      if (v == null) continue
      if (k === 'displayFields') this.displayFieldsRaw = v as string[]
      else (this as any)[k] = v
    }
  }

  get pageSize(): number {
    return this.pageSizeOverride ?? (session.get(SETTINGS_KEYS.DOCUMENT_LIST_SIZE) || 50)
  }

  // ---- Saved view for back/forward (upstream #14699, frontend/patches/10) ----------------
  // Angular keeps its own list state per view; here the default state is saved on activation and
  // restored with activateSavedView(null) (document list). As long as a view is active, nothing is written to
  // localStorage (like saveDocumentListView only for the default state).
  activeSavedViewId = $state<number | null>(null)
  activeSavedViewTitle = $state<string | null>(null)
  private pageSizeOverride = $state<number | null>(null)
  private defaultState: Record<string, any> | null = null

  /** Save the default state before a view becomes active (Angular: listViewStates.get(null) is kept) */
  private rememberDefaultState() {
    if (this.activeSavedViewId != null) return
    this.defaultState = {
      documents: this.documents, collectionSize: this.collectionSize, currentPage: this.currentPage, filterRules: this.filterRules,
      sortField: this.sortField, sortReverse: this.sortReverse, displayMode: this.displayMode, displayFieldsRaw: this.displayFieldsRaw,
      initialized: this.initialized, selectionData: this.selectionData,
      selected: this.selected, excluded: this.excluded, allSelected: this.allSelected,
    }
    // Angular: every view has its own (initially empty) selection
    this.selected = new Set()
    this.excluded = new Set()
    this.allSelected = false
  }

  /**
   * activateSavedView(view) + loadSavedView(view): take over the view's configuration (filter, sorting,
   * display mode, fields, page size, title). Does not navigate and does not reload (DocumentList does that).
   */
  activateSavedView(view: SavedViewConfig | null) {
    this.rangeAnchor = this.rangeTo = null
    if (view === null) return this.deactivateSavedView()
    this.seq++ // cancelPending
    this.rememberDefaultState()
    this.activeSavedViewId = view.id
    this.activeSavedViewTitle = view.name
    this.applySavedViewConfig(view)
  }

  /** activateSavedViewWithQueryParams: like activateSavedView, page from the URL parameters (?page=) */
  activateSavedViewWithQueryParams(view: SavedViewConfig, search: string) {
    const page = paramsToViewState(new URLSearchParams(search)).currentPage
    this.activateSavedView(view)
    this.currentPage = Number.isFinite(page) && page > 0 ? page : 1
  }

  /**
   * Activate a saved view using documents that were already fetched, e.g. by a
   * dashboard widget. Does not navigate or reload.
   */
  activateSavedViewWithDocuments(view: SavedViewConfig, documents: Document[], collectionSize: number, pageSize: number) {
    this.seq++ // cancelPending
    this.rememberDefaultState()
    this.activeSavedViewId = view.id
    this.activeSavedViewTitle = view.name
    this.applySavedViewConfig(view)
    this.pageSizeOverride = pageSize // match the page size the documents were fetched with
    this.currentPage = 1
    this.documents = documents
    this.collectionSize = collectionSize
    this.initialized = true
    this.isReloading = false
  }

  /** loadSavedView: own list state of the view (missing display mode/fields → default like Angular) */
  private applySavedViewConfig(view: SavedViewConfig) {
    this.filterRules = cloneFilterRules(view.filter_rules as FilterRule[])
    this.sortField = view.sort_field || 'created'
    this.sortReverse = view.sort_reverse
    this.displayMode = view.display_mode ?? undefined
    this.pageSizeOverride = view.page_size ?? null
    this.displayFieldsRaw = view.display_fields?.length ? [...view.display_fields] : undefined
  }

  /** activateSavedView(null): back to the default state of the document list */
  private deactivateSavedView() {
    if (this.activeSavedViewId == null) return
    this.seq++
    this.activeSavedViewId = null
    this.activeSavedViewTitle = null
    this.pageSizeOverride = null
    if (this.defaultState) Object.assign(this, this.defaultState)
    this.defaultState = null
  }
  get mode(): DisplayMode {
    return this.displayMode ?? 'smallCards'
  }
  set mode(m: DisplayMode) {
    this.displayMode = m
    this.save()
  }
  get displayFields(): string[] {
    return this.displayFieldsRaw ?? LIST_DEFAULT_DISPLAY_FIELDS
  }
  set displayFields(f: string[]) {
    this.displayFieldsRaw = f
    this.save()
  }
  get hasSelection() {
    return this.allSelected || this.selected.size > 0
  }
  get selectedCount() {
    return this.allSelected ? (this.collectionSize ?? 0) - this.excluded.size : this.selected.size
  }

  private save() {
    if (this.activeSavedViewId != null) return
    try {
      localStorage.setItem(
        CURRENT_VIEW_CONFIG,
        JSON.stringify({
          collectionSize: this.collectionSize,
          currentPage: this.currentPage,
          filterRules: this.filterRules,
          sortField: this.sortField,
          sortReverse: this.sortReverse,
          displayMode: this.displayMode,
          displayFields: this.displayFieldsRaw,
        }),
      )
    } catch {}
  }

  /** loadFromQueryParams: URL parameters take precedence, otherwise stored state. */
  loadFromQuery(search: string) {
    const q = new URLSearchParams(search)
    const empty = [...q.keys()].length === 0
    if (!empty) {
      const v = paramsToViewState(q)
      if (
        !this.initialized ||
        v.sortField !== this.sortField ||
        v.sortReverse !== this.sortReverse ||
        v.currentPage !== this.currentPage ||
        filterRulesDiffer(this.filterRules, v.filterRules)
      ) {
        this.filterRules = v.filterRules
        this.sortField = v.sortField
        this.sortReverse = v.sortReverse
        this.currentPage = v.currentPage
      } else return
    }
    this.reload(true)
  }

  async reload(updateQueryParams = true) {
    const my = ++this.seq
    this.isReloading = true
    this.error = null
    // documentService.searchQuery (FilterEditor.updateTextFilter / GlobalSearch.runFullSearch)
    setSearchQuery(searchQueryFromRules(this.filterRules))
    const extra: Record<string, any> = { truncate_content: true, include_selection_data: true, ...queryParamsFromFilterRules(this.filterRules) }
    try {
      const r = await api.listDocuments({ page: this.currentPage, pageSize: this.pageSize, sortField: this.sortField, sortReverse: this.sortReverse, extra })
      if (my !== this.seq) return
      this.initialized = true
      this.isReloading = false
      this.collectionSize = r.count
      this.documents = r.results
      this.selectionData = (r.selection_data as SelectionData) ?? null
      this.syncSelectedToCurrentPage()
      this.rangeAnchor = this.rangeTo = null
      this.save()
      if (this.activeSavedViewId != null) {
        // Angular: router.navigate([], { queryParams: paramsFromViewState(state, true), queryParamsHandling: 'merge' })
        const path = `view/${this.activeSavedViewId}`
        if (router.path === path) {
          const q = new URLSearchParams(location.search)
          const page = paramsFromViewState(this, true).page
          if (page == null) q.delete('page')
          else q.set('page', String(page))
          const url = router.href(path) + (q.toString() ? `?${q}` : '')
          if (url !== location.pathname + location.search) history.replaceState(null, '', url)
        }
      } else if (updateQueryParams) {
        const params = paramsToURLSearchParams(paramsFromViewState(this))
        const url = router.href('documents') + (params.toString() ? `?${params}` : '')
        if (url !== location.pathname + location.search) history.replaceState(null, '', url)
      }
    } catch (e: any) {
      if (my !== this.seq) return
      this.isReloading = false
      if (this.currentPage != 1 && e?.status == 404) {
        this.currentPage = 1
        this.reload()
      } else {
        this.selectionData = null
        const body = e?.body
        this.error =
          body && typeof body === 'object' && Object.keys(body).length
            ? Object.keys(body)
                .map((k) => `${t(DOCUMENT_SORT_FIELDS.find((f) => f.field.split('__')[0] == k.split('__')[0])?.name ?? k.split('__')[0])}: ${(body[k] as string[])[0]}`)
                .join(', ')
            : String(body ?? e)
      }
    }
  }

  setFilterRules(rules: FilterRule[], resetPage = false) {
    if (!isFullTextFilterRule(rules) && this.sortField == 'score') this.sortField = 'created'
    this.filterRules = cloneFilterRules(rules)
    if (resetPage) this.currentPage = 1
    this.reduceSelectionToFilter()
    this.reload()
    this.save()
  }

  setSort(field: string, reverse: boolean) {
    this.sortField = field
    this.sortReverse = reverse
    this.reload()
    this.save()
  }

  /** quickFilter: default view with these filter rules (e.g. global search), then to the document list */
  quickFilter(rules: FilterRule[]) {
    this.deactivateSavedView()
    this.setFilterRules(rules)
    router.navigate('documents')
  }

  setPage(p: number) {
    if (p === this.currentPage) return
    this.currentPage = p
    this.reload()
    this.save()
  }

  // ---- Selection ----------------------------------------------------------------------------
  private syncSelectedToCurrentPage() {
    if (!this.allSelected) return
    this.selected = new Set(this.documents.filter((d) => !this.excluded.has(d.id)).map((d) => d.id))
    if (!this.collectionSize) this.selectNone()
  }
  selectNone() {
    this.allSelected = false
    this.selected = new Set()
    this.excluded = new Set()
    this.rangeAnchor = this.rangeTo = null
  }
  selectAll() {
    this.allSelected = true
    this.excluded = new Set()
    this.syncSelectedToCurrentPage()
  }
  selectPage() {
    this.allSelected = false
    this.excluded = new Set()
    this.selected = new Set(this.documents.map((d) => d.id))
  }
  isSelected(d: Document) {
    return this.allSelected ? !this.excluded.has(d.id) : this.selected.has(d.id)
  }
  toggleSelected(d: Document) {
    const sel = new Set(this.selected)
    const exc = new Set(this.excluded)
    if (this.allSelected) {
      if (exc.has(d.id)) {
        exc.delete(d.id)
        sel.add(d.id)
      } else {
        exc.add(d.id)
        sel.delete(d.id)
      }
    } else if (sel.has(d.id)) sel.delete(d.id)
    else sel.add(d.id)
    this.selected = sel
    this.excluded = exc
    this.rangeAnchor = this.indexOf(d.id)
    this.rangeTo = null
  }
  /** selectRangeTo: shift click selects from the anchor to here (previous range selection is withdrawn) */
  selectRangeTo(d: Document) {
    if (this.allSelected) {
      this.allSelected = false
      this.excluded = new Set()
    }
    if (this.rangeAnchor === null) return this.toggleSelected(d) // e.g. Shift on the first click
    const a = this.rangeAnchor
    const to = this.indexOf(d.id)
    const sel = new Set(this.selected)
    if (this.rangeTo !== null) this.documents.slice(Math.min(a, this.rangeTo), Math.max(a, this.rangeTo) + 1).forEach((x) => sel.delete(x.id))
    this.documents.slice(Math.min(a, to), Math.max(a, to) + 1).forEach((x) => sel.add(x.id))
    this.selected = sel
    this.rangeTo = to
  }
  private indexOf(id: number) {
    return this.documents.findIndex((x) => x.id === id)
  }
  get lastPage() {
    return Math.max(1, Math.ceil((this.collectionSize ?? 0) / this.pageSize))
  }
  private reduceSelectionToFilter() {
    if (this.allSelected) {
      this.excluded = new Set()
      return
    }
    if (this.selected.size > 0) {
      // Angular queries the IDs of the new filter and keeps only the intersection
      const params = queryParamsFromFilterRules(this.filterRules)
      api
        .get<{ results: { id: number }[] }>(`/api/documents/${queryString({ page: 1, page_size: 100000, fields: 'id', ...params })}`)
        .then((r) => {
          const ids = new Set(r.results.map((x) => x.id))
          this.selected = new Set([...this.selected].filter((id) => ids.has(id)))
        })
        .catch(() => {})
    }
  }
}

export const list = new ListViewState()
