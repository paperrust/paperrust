// Workspace (concept A, docs/DESIGN2.md §4): list context from the route, continuous loading (v1 list like
// the classic UI), selection, active document. Filter rules/URL parameters = classic logic.
import { api } from '../../api/client'
import type { Document } from '../../api/v1'
import type { FilterRule } from '../../lib/filter-rules'
import { queryParamsFromFilterRules } from '../../lib/query-params'
import { router } from '../../router.svelte'
import { data } from '../../stores/data.svelte'
import { INBOX_RULES, queryFromState, stateFromQuery, type ListState } from '../lib/filters'
import { folderParams, folderRoute, parseFolderRoute, type FolderKey } from '../api/folders'
import { clickSelect } from '../lib/folder-dnd'

export const PAGE_SIZE = 50

export interface Ctx {
  kind: 'documents' | 'inbox' | 'view' | 'folder'
  viewId: number | null
  activeId: number | null
  /** folder view: folder id or "unsorted" */
  folder?: FolderKey
}

/** Route → context: documents[/:id[/section]], inbox[/:id], view/:vid[/:id] */
export function parseRoute(path: string): Ctx | null {
  let m = /^documents(?:\/(\d+)(?:\/[a-z]+)?)?$/.exec(path)
  if (m) return { kind: 'documents', viewId: null, activeId: m[1] ? +m[1] : null }
  m = /^inbox(?:\/(\d+))?$/.exec(path)
  if (m) return { kind: 'inbox', viewId: null, activeId: m[1] ? +m[1] : null }
  m = /^view\/(\d+)(?:\/(\d+))?$/.exec(path)
  if (m) return { kind: 'view', viewId: +m[1], activeId: m[2] ? +m[2] : null }
  const f = parseFolderRoute(path)
  if (f) return { kind: 'folder', viewId: null, activeId: f.activeId, folder: f.key }
  return null
}

/** Path of the context with active document */
export function ctxPath(c: Pick<Ctx, 'kind' | 'viewId' | 'folder'>, id: number | null): string {
  if (c.kind === 'folder') return folderRoute(c.folder ?? 'unsorted', id)
  const base = c.kind === 'view' ? `view/${c.viewId}` : c.kind
  return id ? `${base}/${id}` : base
}

class WorkspaceState {
  ctx = $state<Ctx>({ kind: 'documents', viewId: null, activeId: null })
  /** Rules of the filter bar (without the base rule of the context) */
  state = $state<ListState>({ rules: [], sortField: 'created', sortReverse: true })
  docs = $state<Document[]>([])
  total = $state<number | null>(null)
  allIds = $state<number[]>([])
  loading = $state(false)
  error = $state<string | null>(null)
  page = 1
  selected = $state<number[]>([])
  /** Start of a shift-click range (list and tiles) */
  anchor: number | null = null
  focusIndex = $state(0)
  /** Mobile: list or document visible */
  private key = ''
  private seq = 0

  get hasMore() {
    return this.total !== null && this.docs.length < this.total
  }

  /** Base rules of the context (inbox, saved view) */
  baseRules(): FilterRule[] {
    if (this.ctx.kind === 'inbox') return INBOX_RULES
    if (this.ctx.kind === 'view') {
      const v = data.savedViews.find((x) => x.id === this.ctx.viewId)
      return (v?.filter_rules as FilterRule[]) ?? []
    }
    return []
  }

  effectiveRules(): FilterRule[] {
    return [...this.baseRules(), ...this.state.rules]
  }

  title(): string {
    if (this.ctx.kind === 'view') return data.savedViews.find((x) => x.id === this.ctx.viewId)?.name ?? ''
    return ''
  }

  /** Route/query → state; reloads when the list context changes */
  sync(path: string, query: string) {
    const c = parseRoute(path)
    if (!c) return
    const s = stateFromQuery(query)
    if (c.kind === 'view' && !/[?&]sort=/.test(query)) {
      const v = data.savedViews.find((x) => x.id === c.viewId)
      if (v?.sort_field) {
        s.sortField = v.sort_field
        s.sortReverse = v.sort_reverse
      }
    }
    // Base rule of the inbox is not in the query
    if (c.kind === 'inbox') s.rules = s.rules.filter((r) => !INBOX_RULES.some((b) => b.rule_type === r.rule_type))
    this.ctx = c
    this.state = s
    const key = `${c.kind}:${c.viewId}:${c.folder ?? ''}:${queryFromState(s)}:${c.kind === 'view' ? data.savedViews.length : ''}`
    if (key !== this.key) {
      this.key = key
      this.selected = []
      this.anchor = null
      this.reload()
    }
  }

  /** Change filter/sort → URL (context stays; filters changed in a view → All documents) */
  apply(next: Partial<ListState>) {
    const s = { ...this.state, ...next }
    if (this.ctx.kind === 'view') {
      router.navigate('documents' + queryFromState({ ...s, rules: [...this.baseRules(), ...s.rules] }))
      return
    }
    router.navigate(ctxPath(this.ctx, null) + queryFromState(s), true)
  }

  async reload() {
    this.page = 1
    this.docs = []
    this.total = null
    this.focusIndex = 0
    await this.fetch()
  }

  async loadMore() {
    if (this.loading || !this.hasMore) return
    this.page++
    await this.fetch()
  }

  private async fetch() {
    const my = ++this.seq
    this.loading = true
    this.error = null
    try {
      const r = await api.listDocuments({
        page: this.page,
        pageSize: PAGE_SIZE,
        sortField: this.state.sortField,
        sortReverse: this.state.sortReverse,
        extra: { ...queryParamsFromFilterRules(this.effectiveRules()), ...(this.ctx.kind === 'folder' ? folderParams(this.ctx.folder ?? 'unsorted') : {}) },
      })
      if (my !== this.seq) return
      this.docs = this.page === 1 ? r.results : [...this.docs, ...r.results.filter((d) => !this.docs.some((x) => x.id === d.id))]
      this.total = r.count
      this.allIds = r.all ?? []
    } catch (e: any) {
      if (my !== this.seq) return
      // Page out of range (e.g. after deleting): back to the start
      if (e?.status === 404 && this.page > 1) {
        this.page = 1
        return this.fetch()
      }
      this.error = e?.message ?? String(e)
    } finally {
      if (my === this.seq) this.loading = false
    }
  }

  /** Update document in the list (after autosave) */
  patchLocal(doc: Partial<Document> & { id: number }) {
    this.docs = this.docs.map((d) => (d.id === doc.id ? { ...d, ...doc } : d))
  }

  /** Same change for several documents of the list (e.g. folder after a move) */
  patchMany(ids: number[], patch: Partial<Document>) {
    this.docs = this.docs.map((d) => (ids.includes(d.id) ? { ...d, ...patch } : d))
  }

  removeLocal(ids: number[]) {
    this.docs = this.docs.filter((d) => !ids.includes(d.id))
    this.allIds = this.allIds.filter((i) => !ids.includes(i))
    if (this.total !== null) this.total = Math.max(0, this.total - ids.length)
    this.selected = this.selected.filter((i) => !ids.includes(i))
  }

  open(id: number | null, replace = false) {
    router.navigate(ctxPath(this.ctx, id) + router.query, replace)
  }

  /** Click on a document (list or tiles): plain = open, ctrl/cmd = toggle, shift = range from the last click */
  click(id: number, mods: { shift?: boolean; ctrl?: boolean }) {
    const r = clickSelect({ selected: this.selected, anchor: this.anchor }, this.docs.map((d) => d.id), id, mods)
    this.anchor = r.anchor
    if (r.selected !== this.selected) this.selected = r.selected
    if (r.open) this.open(id)
  }

  toggle(id: number) {
    this.selected = this.selected.includes(id) ? this.selected.filter((x) => x !== id) : [...this.selected, id]
    this.anchor = id
  }

  selectAll() {
    this.selected = this.allIds.length ? [...this.allIds] : this.docs.map((d) => d.id)
  }

  clearSelection() {
    this.selected = []
  }
}

export const ws = new WorkspaceState()
