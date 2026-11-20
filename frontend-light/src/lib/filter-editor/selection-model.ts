// Port of FilterableDropdownSelectionModel & the pure logic of FilterableDropdownComponent
// (src-ui/src/app/components/common/filterable-dropdown/filterable-dropdown.component.ts)
// and ToggleableItemState / item helpers (toggleable-dropdown-button.component.ts),
// plus utils/flatten-tags.ts. Angular signals → plain private fields,
// `changed` Subject → Emitter (`onChange(fn)` / `emit()`).

import { t as translate } from '../i18n'
import { NEGATIVE_NULL_FILTER_VALUE } from '../filter-rule-type'
import { Emitter, identityTranslator, sanitizeName, type Translator } from './emitter'
import { filterPipe } from './text-search'

export const ToggleableItemState = {
  NotSelected: 0,
  Selected: 1,
  PartiallySelected: 2,
  Excluded: 3,
} as const
export type ToggleableItemState = (typeof ToggleableItemState)[keyof typeof ToggleableItemState]

export const LogicalOperator = {
  And: 'and',
  Or: 'or',
} as const
export type LogicalOperator = (typeof LogicalOperator)[keyof typeof LogicalOperator]

export const Intersection = {
  Include: 'include',
  Exclude: 'exclude',
} as const
export type Intersection = (typeof Intersection)[keyof typeof Intersection]

/** Structural subset of MatchingModel / Tag used by the dropdown. */
export interface FilterableItem {
  id: number | null
  name: string
  document_count?: number
  parent?: number
  children?: FilterableItem[]
  depth?: number
  orderIndex?: number
  is_inbox_tag?: boolean
  [key: string]: any
}

export interface SelectionDataItem {
  id: number
  document_count: number
}

export interface ChangedItems {
  itemsToAdd: FilterableItem[]
  itemsToRemove: FilterableItem[]
}

type BranchSummary = {
  items: FilterableItem[]
  firstIndex: number
  special: boolean
  selected: boolean
  hasDocs: boolean
}

/** i18n key (English source) of the null item; translate with `t(NOT_ASSIGNED_KEY)`. */
export const NOT_ASSIGNED_KEY = 'common.not_assigned'
/** XLF meaning of NOT_ASSIGNED_KEY. */
export const NOT_ASSIGNED_MEANING =
  'Filter drop down element to filter for documents with no correspondent/type/tag assigned'

export interface SelectionModelOptions {
  /** Translator for the "Not assigned" null item (default: identity). */
  t?: Translator
}

export class FilterableDropdownSelectionModel {
  readonly changed = new Emitter<FilterableDropdownSelectionModel>()

  manyToOne = false
  singleSelect = false

  private _logicalOperator: LogicalOperator = LogicalOperator.And
  private _temporaryLogicalOperator: LogicalOperator = LogicalOperator.And
  private _intersection: Intersection = Intersection.Include
  private _temporaryIntersection: Intersection = Intersection.Include
  private _documentCounts: SelectionDataItem[] = []
  private _items: FilterableItem[] = []
  private _selectionStates = new Map<number, ToggleableItemState>()
  private _temporarySelectionStates = new Map<number, ToggleableItemState>()

  public editing = false

  private readonly t: Translator

  constructor(manyToOne: boolean = false, options: SelectionModelOptions = {}) {
    this.manyToOne = manyToOne
    this.t = options.t ?? translate
  }

  /** Subscribe to changes (Angular: `changed.subscribe`). Returns unsubscribe. */
  onChange(fn: (model: FilterableDropdownSelectionModel) => void): () => void {
    return this.changed.subscribe(fn)
  }

  /** Fire a change event (Angular: `changed.next(this)`). */
  emit(): void {
    this.changed.next(this)
  }

  private get selectionStates(): ReadonlyMap<number, ToggleableItemState> {
    return this._selectionStates
  }

  private get temporarySelectionStates(): ReadonlyMap<number, ToggleableItemState> {
    return this._temporarySelectionStates
  }

  get temporaryLogicalOperator(): LogicalOperator {
    return this._temporaryLogicalOperator
  }

  get temporaryIntersection(): Intersection {
    return this._temporaryIntersection
  }

  public set documentCounts(counts: SelectionDataItem[]) {
    this._documentCounts = counts
    if (this.editing) {
      this._items = this.sortItems(this.items)
    }
  }

  public get documentCounts(): SelectionDataItem[] {
    return this._documentCounts
  }

  get items(): FilterableItem[] {
    return this._items
  }

  set items(items: FilterableItem[]) {
    if (items) {
      this._items = this.withNullItem(this.sortItems(Array.from(items)))
    }
  }

  private withNullItem(items: FilterableItem[]): FilterableItem[] {
    if (this.manyToOne && this.logicalOperator === LogicalOperator.Or) {
      return items[0]?.id === null ? items.slice(1) : items
    }

    const nullItem = {
      name: this.t(NOT_ASSIGNED_KEY),
      id: this.manyToOne || this.intersection === Intersection.Include ? null : NEGATIVE_NULL_FILTER_VALUE,
    }

    return items[0]?.id === null || items[0]?.id === NEGATIVE_NULL_FILTER_VALUE
      ? [nullItem, ...items.slice(1)]
      : [nullItem, ...items]
  }

  private sortItems(items: FilterableItem[]): FilterableItem[] {
    const sorted = [...items].sort((a, b) => {
      if (
        (a.id == null && b.id != null) ||
        (a.id == NEGATIVE_NULL_FILTER_VALUE && b.id != NEGATIVE_NULL_FILTER_VALUE)
      ) {
        return -1
      } else if (
        (a.id != null && b.id == null) ||
        (a.id != NEGATIVE_NULL_FILTER_VALUE && b.id == NEGATIVE_NULL_FILTER_VALUE)
      ) {
        return 1
      }

      // Preserve hierarchical order when provided (e.g., Tags)
      const ao = a['orderIndex']
      const bo = b['orderIndex']
      if (ao !== undefined && bo !== undefined) {
        return ao - bo
      } else if (
        this.getNonTemporary(a.id) == ToggleableItemState.NotSelected &&
        this.getNonTemporary(b.id) != ToggleableItemState.NotSelected
      ) {
        return 1
      } else if (
        this.getNonTemporary(a.id) != ToggleableItemState.NotSelected &&
        this.getNonTemporary(b.id) == ToggleableItemState.NotSelected
      ) {
        return -1
      } else if (
        this._documentCounts.length &&
        this.getDocumentCount(b.id) === 0 &&
        this.getDocumentCount(a.id) > this.getDocumentCount(b.id)
      ) {
        return -1
      } else if (
        this._documentCounts.length &&
        this.getDocumentCount(a.id) === 0 &&
        this.getDocumentCount(a.id) < this.getDocumentCount(b.id)
      ) {
        return 1
      } else {
        return a.name.localeCompare(b.name)
      }
    })

    return this._documentCounts.length ? this.promoteBranchesWithDocumentCounts(sorted) : sorted
  }

  getSelectedItems(): FilterableItem[] {
    return this.items.filter((i) => this.temporarySelectionStates.get(i.id) == ToggleableItemState.Selected)
  }

  getExcludedItems(): FilterableItem[] {
    return this.items.filter((i) => this.temporarySelectionStates.get(i.id) == ToggleableItemState.Excluded)
  }

  set(id: number | null, state: ToggleableItemState, fireEvent = true): void {
    const states = new Map(this.temporarySelectionStates)
    if (state == ToggleableItemState.NotSelected) {
      states.delete(id)
    } else {
      states.set(id, state)
    }
    this._temporarySelectionStates = states
    if (fireEvent) {
      this.emit()
    }
  }

  toggle(id: number | null, fireEvent = true): void {
    const states = new Map(this.temporarySelectionStates)
    let state = states.get(id)
    if (state == undefined || (state != ToggleableItemState.Selected && state != ToggleableItemState.Excluded)) {
      if (this.manyToOne || this.singleSelect) {
        states.set(id, ToggleableItemState.Selected)

        if (this.singleSelect) {
          for (let key of states.keys()) {
            if (key != id) {
              states.delete(key)
            }
          }
        }
      } else {
        let newState: ToggleableItemState =
          this.intersection == Intersection.Include ? ToggleableItemState.Selected : ToggleableItemState.Excluded
        if (!id) newState = ToggleableItemState.Selected
        if (state == ToggleableItemState.Excluded && this.intersection == Intersection.Exclude) {
          newState = ToggleableItemState.NotSelected
        }
        states.set(id, newState)
      }
      if (this.editing && states.get(id) == ToggleableItemState.Selected) {
        this.addAncestorSelections(states, id)
      }
    } else if (state == ToggleableItemState.Selected || state == ToggleableItemState.Excluded) {
      states.delete(id)
      this.clearDescendantSelections(states, id)
    }

    if (!id) {
      for (let key of states.keys()) {
        if (key) {
          states.delete(key)
        }
      }
    } else {
      states.delete(null)
    }
    this._temporarySelectionStates = states

    if (fireEvent) {
      this.emit()
    }
  }

  exclude(id: number | null, fireEvent: boolean = true): void {
    const states = new Map(this.temporarySelectionStates)
    let state = states.get(id)
    if (id && (state == null || state != ToggleableItemState.Excluded)) {
      const operator = this.manyToOne ? LogicalOperator.And : LogicalOperator.Or
      this._temporaryLogicalOperator = operator
      this._logicalOperator = operator

      if (this.manyToOne || this.singleSelect) {
        states.set(id, ToggleableItemState.Excluded)
        this.clearDescendantSelections(states, id)

        if (this.singleSelect) {
          for (let key of states.keys()) {
            if (key != id) {
              states.delete(key)
            }
          }
        }
      } else {
        let newState: ToggleableItemState =
          this.intersection == Intersection.Include ? ToggleableItemState.Selected : ToggleableItemState.Excluded
        if (state == ToggleableItemState.Selected && this.intersection == Intersection.Include) {
          newState = ToggleableItemState.NotSelected
        }
        states.set(id, newState)
        if (newState == ToggleableItemState.Excluded) {
          this.clearDescendantSelections(states, id)
        }
      }
    } else if (!id || state == ToggleableItemState.Excluded) {
      states.delete(id)
      if (id) {
        this.clearDescendantSelections(states, id)
      }
    }
    this._temporarySelectionStates = states

    if (fireEvent) {
      this.emit()
    }
  }

  private getNonTemporary(id: number | null): ToggleableItemState {
    return this.selectionStates.get(id) || ToggleableItemState.NotSelected
  }

  private clearDescendantSelections(states: Map<number, ToggleableItemState>, id: number): void {
    for (const descendantID of this.getDescendantIDs(id)) {
      states.delete(descendantID)
    }
  }

  private addAncestorSelections(states: Map<number, ToggleableItemState>, id: number): void {
    const parentById = this.buildParentById(this.items)
    const seen = new Set<number>([id])
    let parentID = parentById.get(id)

    while (typeof parentID === 'number' && !seen.has(parentID)) {
      seen.add(parentID)
      states.set(parentID, ToggleableItemState.Selected)
      parentID = parentById.get(parentID)
    }
  }

  private getDescendantIDs(id: number): number[] {
    const descendants: number[] = []
    const queue: number[] = [id]

    while (queue.length) {
      const parentID = queue.shift()
      for (const item of this.items) {
        if (typeof item?.id === 'number' && typeof item['parent'] === 'number' && item['parent'] === parentID) {
          descendants.push(item.id)
          queue.push(item.id)
        }
      }
    }

    return descendants
  }

  get logicalOperator(): LogicalOperator {
    return this._temporaryLogicalOperator
  }

  set logicalOperator(operator: LogicalOperator) {
    this._temporaryLogicalOperator = operator
    this._items = this.withNullItem(this.items)
  }

  toggleOperator(): void {
    this.emit()
  }

  get intersection(): Intersection {
    return this._temporaryIntersection
  }

  set intersection(intersection: Intersection) {
    this._temporaryIntersection = intersection
    this._items = this.withNullItem(this.items)
  }

  toggleIntersection(): void {
    if (this.temporarySelectionStates.size === 0) return
    let newState: ToggleableItemState =
      this.intersection == Intersection.Include ? ToggleableItemState.Selected : ToggleableItemState.Excluded

    const states = new Map(this.temporarySelectionStates)
    states.forEach((state, key) => {
      if (key === null && this.intersection === Intersection.Exclude) {
        states.set(NEGATIVE_NULL_FILTER_VALUE, newState)
      } else if (key === NEGATIVE_NULL_FILTER_VALUE && this.intersection === Intersection.Include) {
        states.set(null, newState)
      } else {
        states.set(key, newState)
      }
    })
    this._temporarySelectionStates = states

    this.emit()
  }

  get(id: number | null): ToggleableItemState {
    return this.temporarySelectionStates.get(id) || ToggleableItemState.NotSelected
  }

  selectionSize(): number {
    return this.getSelectedItems().length
  }

  get totalCount(): number {
    return this.getSelectedItems().length + this.getExcludedItems().length
  }

  clear(fireEvent = true): void {
    this._temporarySelectionStates = new Map()
    this._temporaryLogicalOperator = LogicalOperator.And
    this._logicalOperator = LogicalOperator.And
    this._temporaryIntersection = Intersection.Include
    this._intersection = Intersection.Include
    this._items = this.withNullItem(this.items)
    if (fireEvent) {
      this.emit()
    }
  }

  isDirty(): boolean {
    if (
      !Array.from(this.temporarySelectionStates.keys()).every(
        (id) => this.temporarySelectionStates.get(id) == this.selectionStates.get(id)
      )
    ) {
      return true
    } else if (
      !Array.from(this.selectionStates.keys()).every(
        (id) => this.selectionStates.get(id) == this.temporarySelectionStates.get(id)
      )
    ) {
      return true
    } else if (this._temporaryLogicalOperator !== this._logicalOperator) {
      return true
    } else if (this._temporaryIntersection !== this._intersection) {
      return true
    } else {
      return false
    }
  }

  isNoneSelected(): boolean {
    return (
      (this.selectionSize() == 1 && this.get(null) == ToggleableItemState.Selected) ||
      (this.intersection == Intersection.Exclude &&
        this.get(NEGATIVE_NULL_FILTER_VALUE) == ToggleableItemState.Excluded)
    )
  }

  getDocumentCount(id: number | null): number | undefined {
    return this._documentCounts.find((c) => c.id === id)?.document_count
  }

  private promoteBranchesWithDocumentCounts(items: FilterableItem[]): FilterableItem[] {
    const parentById = this.buildParentById(items)
    const findRootId = this.createRootFinder(parentById)
    const getRootDocCount = this.createRootDocCounter(items)
    const summaries = this.buildBranchSummaries(items, findRootId, getRootDocCount)
    const orderedBranches = this.orderBranchesByPriority(summaries)

    return orderedBranches.flatMap((summary) => summary.items)
  }

  private buildParentById(items: FilterableItem[]): Map<number, number | null> {
    const parentById = new Map<number, number | null>()

    for (const item of items) {
      if (typeof item?.id === 'number') {
        const parentValue = item['parent']
        parentById.set(item.id, typeof parentValue === 'number' ? parentValue : null)
      }
    }

    return parentById
  }

  private createRootFinder(parentById: Map<number, number | null>): (id: number) => number {
    const rootMemo = new Map<number, number>()

    const findRootId = (id: number): number => {
      const cached = rootMemo.get(id)
      if (cached !== undefined) {
        return cached
      }

      const parentId = parentById.get(id)
      if (parentId === undefined || parentId === null) {
        rootMemo.set(id, id)
        return id
      }

      const rootId = findRootId(parentId)
      rootMemo.set(id, rootId)
      return rootId
    }

    return findRootId
  }

  private createRootDocCounter(items: FilterableItem[]): (rootId: number) => number {
    const docCountMemo = new Map<number, number>()

    return (rootId: number): number => {
      const cached = docCountMemo.get(rootId)
      if (cached !== undefined) {
        return cached
      }

      const explicit = this.getDocumentCount(rootId)
      if (typeof explicit === 'number') {
        docCountMemo.set(rootId, explicit)
        return explicit
      }

      const rootItem = items.find((i) => i.id === rootId)
      const fallback = typeof rootItem?.['document_count'] === 'number' ? rootItem['document_count'] : 0

      docCountMemo.set(rootId, fallback)
      return fallback
    }
  }

  private buildBranchSummaries(
    items: FilterableItem[],
    findRootId: (id: number) => number,
    getRootDocCount: (rootId: number) => number
  ): Map<string, BranchSummary> {
    const summaries = new Map<string, BranchSummary>()

    for (const [index, item] of items.entries()) {
      const { key, special, rootId } = this.describeBranchItem(item, index, findRootId)

      let summary = summaries.get(key)
      if (!summary) {
        summary = {
          items: [],
          firstIndex: index,
          special,
          selected: false,
          hasDocs: special || rootId === null ? false : getRootDocCount(rootId) > 0,
        }
        summaries.set(key, summary)
      }

      summary.items.push(item)

      if (this.shouldMarkSummarySelected(summary, item)) {
        summary.selected = true
      }
    }

    return summaries
  }

  private describeBranchItem(
    item: FilterableItem,
    index: number,
    findRootId: (id: number) => number
  ): { key: string; special: boolean; rootId: number | null } {
    if (item?.id === null) {
      return { key: 'null', special: true, rootId: null }
    }

    if (item?.id === NEGATIVE_NULL_FILTER_VALUE) {
      return { key: 'neg-null', special: true, rootId: null }
    }

    if (typeof item?.id === 'number') {
      const rootId = findRootId(item.id)
      return { key: `root-${rootId}`, special: false, rootId }
    }

    return { key: `misc-${index}`, special: false, rootId: null }
  }

  private shouldMarkSummarySelected(summary: BranchSummary, item: FilterableItem): boolean {
    if (summary.special) {
      return false
    }

    if (typeof item?.id !== 'number') {
      return false
    }

    return this.getNonTemporary(item.id) !== ToggleableItemState.NotSelected
  }

  private orderBranchesByPriority(summaries: Map<string, BranchSummary>): BranchSummary[] {
    return Array.from(summaries.values()).sort((a, b) => {
      const rankDiff = this.branchRank(a) - this.branchRank(b)
      if (rankDiff !== 0) {
        return rankDiff
      }
      if (a.hasDocs !== b.hasDocs) {
        return a.hasDocs ? -1 : 1
      }
      return a.firstIndex - b.firstIndex
    })
  }

  private branchRank(summary: BranchSummary): number {
    if (summary.special) {
      return -1
    }
    if (summary.selected) {
      return 0
    }
    return 1
  }

  init(map: Map<number, ToggleableItemState>): void {
    this._temporarySelectionStates = new Map(map)
    this.apply()
  }

  apply(): void {
    this._selectionStates = new Map(this.temporarySelectionStates)
    this._logicalOperator = this._temporaryLogicalOperator
    this._intersection = this._temporaryIntersection
    this._items = this.sortItems(this.items)
  }

  reset(complete: boolean = false): void {
    if (complete) {
      this._selectionStates = new Map()
      this._temporarySelectionStates = new Map()
    } else {
      this._temporarySelectionStates = new Map(this.selectionStates)
    }
  }

  diff(): ChangedItems {
    return {
      itemsToAdd: this.items.filter(
        (item) =>
          this.temporarySelectionStates.get(item.id) == ToggleableItemState.Selected &&
          this.selectionStates.get(item.id) != ToggleableItemState.Selected
      ),
      itemsToRemove: this.items.filter(
        (item) => !this.temporarySelectionStates.has(item.id) && this.selectionStates.has(item.id)
      ),
    }
  }
}

// ---- utils/flatten-tags.ts ------------------------------------------------------------------

/** Orders tags hierarchically (roots/children by name) and sets `depth` / `orderIndex`. */
export function flattenTags<T extends FilterableItem>(all: T[]): T[] {
  const map = new Map<number, T>(all.map((t) => [t.id, { ...t, children: [] }]))
  // rebuild children
  for (const t of map.values()) {
    if (t.parent) {
      const p = map.get(t.parent)
      p?.children.push(t)
    }
  }
  const roots = Array.from(map.values()).filter((t) => !t.parent)
  const sortByName = (a: T, b: T) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true })
  const ordered: T[] = []
  let idx = 0
  const walk = (node: T, depth: number) => {
    node.depth = depth
    node.orderIndex = idx++
    ordered.push(node)
    if (node.children?.length) {
      for (const child of [...(node.children as T[])].sort(sortByName)) {
        walk(child, depth + 1)
      }
    }
  }
  roots.sort(sortByName)
  roots.forEach((r) => walk(r, 0))
  return ordered
}

// ---- ToggleableDropdownButtonComponent helpers ------------------------------------------------

/** `'is_inbox_tag' in item` */
export function isTag(item: FilterableItem): boolean {
  return 'is_inbox_tag' in item
}

/** Indentation depth of a (tag) item. */
export function getDepth(item: FilterableItem): number {
  return item.depth ?? 0
}

/** Count shown on the button: explicit count from selection data, else the item's own count. */
export function currentCount(item: FilterableItem, count: number | undefined): number {
  return count ?? item.document_count
}

/** Button click: a selected item gets excluded, anything else toggled. */
export function toggleItemAction(state: ToggleableItemState): 'exclude' | 'toggle' {
  return state == ToggleableItemState.Selected ? 'exclude' : 'toggle'
}

// ---- FilterableDropdownComponent (pure logic) ------------------------------------------------

export const FILTERABLE_BUTTON_HEIGHT_PX = 42

/** Port of the `filteredItems` getter: text filter on `name` + hide the null item unless allowed. */
export function filterItems(items: FilterableItem[], filterText: string, allowSelectNone = false): FilterableItem[] {
  return filterPipe(items, filterText, 'name').filter((item) => allowSelectNone || Boolean(item.id))
}

export function scrollViewportHeight(filteredCount: number): number {
  return Math.min(filteredCount * FILTERABLE_BUTTON_HEIGHT_PX, 400)
}

/** Result of `listFilterEnter()` – what the UI has to do next. */
export type ListFilterEnterAction =
  | { type: 'toggled'; then: 'apply' | 'close' } // Angular waits 200 ms, then applyClicked() / close
  | { type: 'create' }
  | { type: 'none' }

/**
 * Framework-free state of one `<pngx-filterable-dropdown>`. The UI owns the actual
 * open/close state of the dropdown and reacts to return values / emitted events.
 */
export class FilterableDropdownState {
  readonly FILTERABLE_BUTTON_HEIGHT_PX = FILTERABLE_BUTTON_HEIGHT_PX

  /** Fired with the model whenever the model changes (Angular `selectionModelChange`). */
  readonly selectionModelChange = new Emitter<FilterableDropdownSelectionModel>()
  /** Angular `apply` output. */
  readonly applied = new Emitter<ChangedItems>()
  /** Angular `extraButton` output. */
  readonly extraButton = new Emitter<ChangedItems>()
  /** Angular `opened` output. */
  readonly opened = new Emitter<FilterableDropdownState>()

  title: string = null
  allowSelectNone = false
  applyOnClose = false
  disabled = false
  filterText = ''
  creating = false
  /** Whether a create callback exists (Angular `createRef`). */
  canCreate = false

  private _editing = false
  private _selectionModel: FilterableDropdownSelectionModel
  private unsubscribeModel: () => void = () => {}

  constructor(model: FilterableDropdownSelectionModel = new FilterableDropdownSelectionModel()) {
    this.selectionModel = model
  }

  get selectionModel(): FilterableDropdownSelectionModel {
    return this._selectionModel
  }

  set selectionModel(model: FilterableDropdownSelectionModel) {
    if (this._selectionModel) {
      this.unsubscribeModel()
      this._selectionModel.changed.complete()
      model.items = this._selectionModel.items
      model.manyToOne = this._selectionModel.manyToOne
      model.singleSelect = this._editing && !model.manyToOne
    }
    model.editing = this._editing
    this.unsubscribeModel = model.onChange((updated) => this.selectionModelChange.next(updated))
    this._selectionModel = model
  }

  /** Subscribe to model changes forwarded by this dropdown. */
  onChange(fn: (model: FilterableDropdownSelectionModel) => void): () => void {
    return this.selectionModelChange.subscribe(fn)
  }

  get items(): FilterableItem[] {
    return this._selectionModel.items
  }

  get manyToOne(): boolean {
    return this._selectionModel.manyToOne
  }

  get editing(): boolean {
    return this._editing
  }

  set editing(value: boolean) {
    this._editing = value
    if (this._selectionModel) {
      this._selectionModel.singleSelect = this._editing && !this._selectionModel.manyToOne
      this._selectionModel.editing = this._editing
    }
  }

  /** Angular `@Input() documentCounts` (ignored when falsy). */
  set documentCounts(counts: SelectionDataItem[]) {
    if (counts) {
      this._selectionModel.documentCounts = counts
    }
  }

  get name(): string {
    return sanitizeName(this.title)
  }

  /** Toggle button highlighted (`btn-primary`). */
  get isActive(): boolean {
    return !this.editing && this._selectionModel.selectionSize() > 0
  }

  /** Clearable badge with `totalCount` shown on the toggle button. */
  get showBadge(): boolean {
    return !this.editing && this._selectionModel.totalCount > 0
  }

  /** And/Or radios (tags). */
  get showLogicalOperatorToggle(): boolean {
    return !this.editing && this.manyToOne
  }

  /** Include/Exclude radios (correspondent, document type, storage path). */
  get showIntersectionToggle(): boolean {
    return !this.editing && !this.manyToOne
  }

  /** Radio change: `[(ngModel)]="selectionModel.logicalOperator" (ngModelChange)="toggleOperator()"`. */
  setLogicalOperator(operator: LogicalOperator): void {
    this._selectionModel.logicalOperator = operator
    this._selectionModel.toggleOperator()
  }

  /** Radio change: `[(ngModel)]="selectionModel.intersection" (ngModelChange)="toggleIntersection()"`. */
  setIntersection(intersection: Intersection): void {
    this._selectionModel.intersection = intersection
    this._selectionModel.toggleIntersection()
  }

  /** Item button click (ToggleableDropdownButton: selected → exclude, else toggle). */
  itemClicked(item: FilterableItem): void {
    if (toggleItemAction(this._selectionModel.get(item.id)) === 'exclude') {
      this.excludeClicked(item.id)
    } else {
      this._selectionModel.toggle(item.id)
    }
  }

  get modelIsDirty(): boolean {
    return this._selectionModel.isDirty()
  }

  get modifierToggleEnabled(): boolean {
    return this.manyToOne
      ? this._selectionModel.selectionSize() > 1 && this._selectionModel.getExcludedItems().length == 0
      : true
  }

  /** Items shown in the list for the current `filterText`. */
  get filteredItems(): FilterableItem[] {
    return filterItems(this.items, this.filterText, this.allowSelectNone)
  }

  get scrollViewportHeight(): number {
    return scrollViewportHeight(this.filteredItems.length)
  }

  /** "Create" is offered only when a non-empty filter has no matches. */
  get showCreate(): boolean {
    return this.canCreate && this.filterText?.length > 0 && this.filteredItems.length == 0
  }

  trackByItem(index: number, item: FilterableItem): number | null {
    return item?.id ?? index
  }

  /** Returns the diff when dirty (UI then closes the dropdown), else null. */
  applyClicked(): ChangedItems | null {
    if (this._selectionModel.isDirty()) {
      const diff = this._selectionModel.diff()
      if (!this.applyOnClose) {
        this.applied.next(diff)
      }
      return diff
    }
    return null
  }

  /**
   * Call when the dropdown opens/closes. Returns `'keep-open'` when a close must be
   * reverted because a create dialog is active (Angular re-opens the dropdown).
   */
  dropdownOpenChange(open: boolean): 'keep-open' | void {
    if (open) {
      // Dont let a create modal close this
      if (this.creating) return
      if (this.editing) {
        this._selectionModel.reset()
      }
      this._selectionModel.singleSelect = this.editing && !this._selectionModel.manyToOne
      this.opened.next(this)
    } else {
      if (this.creating) {
        return 'keep-open'
      } else {
        this.filterText = ''
        if (this.applyOnClose && this._selectionModel.isDirty()) {
          this.applied.next(this._selectionModel.diff())
        }
      }
    }
  }

  listFilterEnter(): ListFilterEnterAction {
    const filtered = this.filteredItems
    if (filtered.length == 1) {
      this._selectionModel.toggle(filtered[0].id)
      return { type: 'toggled', then: this.editing ? 'apply' : 'close' }
    } else if (filtered.length == 0 && this.canCreate && this.filterText?.length > 0) {
      return { type: 'create' }
    }
    return { type: 'none' }
  }

  excludeClicked(itemID: number | null): void {
    if (this.editing) {
      this._selectionModel.toggle(itemID)
    } else {
      this._selectionModel.exclude(itemID)
    }
  }

  reset(): void {
    this._selectionModel.reset(true)
    // Angular emits selectionModelChange directly; emitting on the model reaches the same
    // listeners (they are forwarded) and also anyone subscribed to the model itself.
    this._selectionModel.emit()
  }

  getUpdatedDocumentCount(id: number | null): number | undefined {
    return this._selectionModel.getDocumentCount(id)
  }

  hideCount(item: FilterableItem): boolean {
    // counts are pointless when clicking item would add to the set of docs
    return (
      this._selectionModel.logicalOperator === LogicalOperator.Or &&
      this.manyToOne &&
      this._selectionModel.get(item.id) !== ToggleableItemState.Selected
    )
  }

  /** Extra button: emits the diff without applying (UI closes the dropdown). */
  extraButtonClicked(): ChangedItems {
    const diff = this._selectionModel.diff()
    this.extraButton.next(diff)
    return diff
  }

  /** Release subscriptions (call on component destroy). */
  destroy(): void {
    this.unsubscribeModel()
  }
}
