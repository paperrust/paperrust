// Logic of manage/document-attributes/management-list/management-list.component.ts (+ tag-/correspondent-/
// document-type-/storage-path-list): loading, name filter (debounced 400 ms), sorting, pages, selection,
// bulk actions. Framework-free except for $state (Vitest with fake loader).
import type { ObjectEndpoint } from '../../../../api/objects'
import type { MatchingModel } from '../../../../lib/matching'
import { FILTER_HAS_CORRESPONDENT_ANY, FILTER_HAS_DOCUMENT_TYPE_ANY, FILTER_HAS_STORAGE_PATH_ANY, FILTER_HAS_TAGS_ALL } from '../../../../lib/filter-rule-type'
import { msg } from '../../../../lib/i18n'

export interface ManagementListColumn {
  key: string
  name: string
  valueFn?: (o: any) => string
  badgeFn?: (o: any) => { text: string; textColor?: string; backgroundColor?: string }
  hideOnMobile?: boolean
  monospace?: boolean
}

export type ListKind = 'tags' | 'correspondents' | 'documenttypes' | 'storagepaths'

export interface ListConfig {
  kind: ListKind
  endpoint: ObjectEndpoint
  /** English source text ($localize) */
  typeName: string
  typeNamePlural: string
  /** PermissionType without %s (tag, correspondent, …) */
  permissionType: string
  filterRuleType: number
  deleteMessage: string
}

export const LIST_CONFIGS: Record<ListKind, ListConfig> = {
  tags: {
    kind: 'tags',
    endpoint: 'tags',
    typeName: msg('manage.attributes.tag'),
    typeNamePlural: msg('manage.attributes.tags'),
    permissionType: 'tag',
    filterRuleType: FILTER_HAS_TAGS_ALL,
    deleteMessage: msg('manage.attributes.really_want_delete_tag'),
  },
  correspondents: {
    kind: 'correspondents',
    endpoint: 'correspondents',
    typeName: msg('manage.attributes.correspondent'),
    typeNamePlural: msg('manage.attributes.correspondents'),
    permissionType: 'correspondent',
    filterRuleType: FILTER_HAS_CORRESPONDENT_ANY,
    deleteMessage: msg('manage.attributes.really_want_delete_correspondent'),
  },
  documenttypes: {
    kind: 'documenttypes',
    endpoint: 'document_types',
    typeName: msg('manage.attributes.document_type'),
    typeNamePlural: msg('manage.attributes.document_types'),
    permissionType: 'documenttype',
    filterRuleType: FILTER_HAS_DOCUMENT_TYPE_ANY,
    deleteMessage: msg('manage.attributes.really_want_delete_document'),
  },
  storagepaths: {
    kind: 'storagepaths',
    endpoint: 'storage_paths',
    typeName: msg('manage.attributes.storage_path'),
    typeNamePlural: msg('manage.attributes.storage_paths'),
    permissionType: 'storagepath',
    filterRuleType: FILTER_HAS_STORAGE_PATH_ANY,
    deleteMessage: msg('manage.attributes.really_want_delete_storage'),
  },
}

export interface Results<T> {
  count: number
  display_count?: number
  results: T[]
}

export type Loader<T> = (
  page: number,
  pageSize: number,
  sortField: string | undefined,
  sortReverse: boolean | undefined,
  nameFilter: string | undefined,
  fullPerms: boolean,
  extraParams: Record<string, any> | null,
) => Promise<Results<T>>

/** StoragePathListComponent: path to 49 characters + "..." */
export const truncatePath = (p: string | undefined | null) => `${p?.slice(0, 49)}${(p?.length ?? 0) > 50 ? '...' : ''}`

export class ManagementListState<T extends MatchingModel = MatchingModel> {
  data = $state<T[]>([])
  unfilteredData = $state<T[]>([])
  page = $state(1)
  collectionSize = $state(0)
  displayCollectionSize = $state(0)
  sortField = $state<string | undefined>(undefined)
  sortReverse = $state<boolean | undefined>(undefined)
  loading = $state(true)
  show = $state(false)
  selectedObjects = $state(new Set<number>())
  toggleAll = $state(false)
  private allSelectionActive = $state(false)
  private currentExtraParams: Record<string, any> | null = null
  nameFilter: string | undefined = undefined
  private debounce?: ReturnType<typeof setTimeout>
  private lastFilter: string | null | undefined = undefined
  private seq = 0

  constructor(
    readonly config: ListConfig,
    private loader: Loader<T>,
    private getPageSize: () => number,
  ) {}

  get pageSize() {
    return this.getPageSize()
  }
  get hasSelection() {
    return this.selectedObjects.size > 0 || this.allSelectionActive
  }
  get selectedCount() {
    return this.allSelectionActive ? this.displayCollectionSize : this.selectedObjects.size
  }
  get isAllSelection() {
    return this.allSelectionActive
  }

  /** nameFilterDebounce (debounceTime 400, distinctUntilChanged) */
  setNameFilter(v: string | null) {
    clearTimeout(this.debounce)
    this.debounce = setTimeout(() => {
      if (v === this.lastFilter) return
      this.lastFilter = v
      this.nameFilter = v ?? undefined
      this.page = 1
      this.reloadData()
    }, 400)
  }

  /** Extensions of the subclasses (TagList: is_root without name filter; CorrespondentList: last_correspondence) */
  private extraParams(): Record<string, any> | null {
    if (this.config.kind === 'tags') return this.nameFilter?.length ? null : { is_root: true }
    if (this.config.kind === 'correspondents') return { last_correspondence: true }
    return null
  }

  /** TagListComponent.filterData */
  filterData(data: T[]): T[] {
    if (this.config.kind !== 'tags') return data
    if (!this.nameFilter?.length) return data.filter((t: any) => !t.parent)
    const ids = new Set(data.map((t) => t.id))
    return data.filter((t: any) => !t.parent || !ids.has(t.parent))
  }

  getSelectableIDs(objects: T[]): number[] {
    if (this.config.kind !== 'tags') return objects.map((o) => o.id)
    const ids: number[] = []
    for (const tag of objects.filter(Boolean) as any[]) {
      if (tag.id != null) ids.push(tag.id)
      if (Array.isArray(tag.children) && tag.children.length) ids.push(...this.getSelectableIDs(tag.children))
    }
    return ids
  }

  getDocumentCount(o: MatchingModel): number {
    return o.document_count ?? this.unfilteredData.find((d) => d.id == o.id)?.document_count ?? 0
  }

  getOriginalObject(o: T): T {
    return this.unfilteredData.find((d) => d?.id == o?.id) || o
  }

  async reloadData() {
    const extra = this.extraParams()
    this.loading = true
    this.currentExtraParams = extra
    this.clearSelection()
    const n = ++this.seq
    try {
      const c = await this.loader(this.page, this.pageSize, this.sortField, this.sortReverse, this.nameFilter, true, extra ? { ...extra } : null)
      if (n !== this.seq) return
      this.unfilteredData = c.results
      this.data = this.filterData(c.results)
      this.collectionSize = c.count
      this.displayCollectionSize = c.display_count ?? c.count
      this.show = true
      this.loading = false
    } catch (e: any) {
      if (n !== this.seq) return
      if (String(e?.body?.detail ?? '').includes('Invalid page')) {
        this.page = 1
        this.reloadData()
      }
    }
  }

  onSort(column: string | null, reverse: boolean) {
    this.sortField = column ?? undefined
    this.sortReverse = reverse
    this.reloadData()
  }

  /** SortableDirective.rotate */
  rotateSort(column: string) {
    if (this.sortField != column) this.onSort(column, false)
    else if (!this.sortReverse) this.onSort(column, true)
    else this.onSort(null, false)
  }

  sortClass(column: string) {
    return this.sortField === column ? (this.sortReverse ? 'des' : 'asc') : ''
  }

  setPage(p: number) {
    this.page = p
    this.reloadData()
  }

  clearSelection() {
    this.allSelectionActive = false
    this.toggleAll = false
    this.selectedObjects = new Set()
  }
  selectNone() {
    this.clearSelection()
  }
  selectPage() {
    this.allSelectionActive = false
    this.selectedObjects = new Set(this.getSelectableIDs(this.data))
    this.toggleAll = this.areAllPageItemsSelected()
  }
  selectAll() {
    if (!this.collectionSize) return this.clearSelection()
    this.allSelectionActive = true
    this.selectedObjects = new Set(this.getSelectableIDs(this.data))
    this.toggleAll = this.areAllPageItemsSelected()
  }
  toggleSelected(o: T) {
    if (this.allSelectionActive) this.allSelectionActive = false
    const s = new Set(this.selectedObjects)
    if (s.has(o.id)) s.delete(o.id)
    else s.add(o.id)
    this.selectedObjects = s
    this.toggleAll = this.areAllPageItemsSelected()
  }
  areAllPageItemsSelected(): boolean {
    if (this.allSelectionActive) return this.data.length > 0
    const ids = this.getSelectableIDs(this.data)
    return ids.length > 0 && ids.every((id) => this.selectedObjects.has(id))
  }

  /** getBulkEditFilters */
  bulkEditFilters(): Record<string, any> {
    const f: Record<string, any> = { ...this.currentExtraParams }
    if (this.nameFilter?.length) f['name__icontains'] = this.nameFilter
    return f
  }
}
