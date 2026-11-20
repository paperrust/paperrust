// Ported from filterable-dropdown.component.spec.ts, toggleable-dropdown-button.component.spec.ts
// (logic parts), utils/flatten-tags.spec.ts and utils/text-search.spec.ts.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { NEGATIVE_NULL_FILTER_VALUE } from '../filter-rule-type'
import {
  FILTERABLE_BUTTON_HEIGHT_PX,
  FilterableDropdownSelectionModel,
  FilterableDropdownState,
  Intersection,
  LogicalOperator,
  NOT_ASSIGNED_KEY,
  ToggleableItemState,
  currentCount,
  filterItems,
  flattenTags,
  getDepth,
  isTag,
  toggleItemAction,
  type ChangedItems,
  type FilterableItem,
} from './selection-model'
import { matchesSearchText } from './text-search'

type Tag = FilterableItem

const items: Tag[] = [
  { id: 1, name: 'Tag1', is_inbox_tag: false, matching_algorithm: 6 },
  { id: 2, name: 'Tag2', is_inbox_tag: true, matching_algorithm: 2, match: 'str' },
]

const nullItem = { id: null, name: 'Not set' }
const negativeNullItem = { id: NEGATIVE_NULL_FILTER_VALUE, name: 'Not set' }

let selectionModel: FilterableDropdownSelectionModel

describe('FilterableDropdownState & FilterableDropdownSelectionModel', () => {
  let component: FilterableDropdownState

  beforeEach(() => {
    component = new FilterableDropdownState()
    component.selectionModel = new FilterableDropdownSelectionModel()
    selectionModel = new FilterableDropdownSelectionModel()
  })

  it('should sanitize title', () => {
    expect(component.name).toBeNull()
    component.title = 'Foo Bar'
    expect(component.name).toEqual('foo_bar')
  })

  it('should support reset', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    selectionModel.set(items[0].id, ToggleableItemState.Selected)
    expect(selectionModel.getSelectedItems()).toHaveLength(1)
    expect(selectionModel.isDirty()).toBeTruthy()
    component.reset()
    expect(selectionModel.getSelectedItems()).toHaveLength(0)
    expect(selectionModel.isDirty()).toBeFalsy()
  })

  it('should report document counts', () => {
    component.documentCounts = [{ id: items[0].id, document_count: 12 }]
    expect(component.getUpdatedDocumentCount(items[0].id)).toEqual(12)
    expect(component.getUpdatedDocumentCount(items[1].id)).toBeUndefined()
  })

  it('should emit change when items selected', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    let newModel: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((model) => (newModel = model))
    expect(newModel).toBeUndefined()

    selectionModel.set(items[0].id, ToggleableItemState.Selected)
    expect(selectionModel.isDirty()).toBeTruthy()
    expect(newModel.getSelectedItems()).toEqual([items[0]])
    expect(newModel.getExcludedItems()).toEqual([])

    selectionModel.set(items[0].id, ToggleableItemState.NotSelected)
    expect(newModel.getSelectedItems()).toEqual([])

    expect(component.selectionModel.items).toEqual([nullItem, ...items])
  })

  it('should emit change when items toggled', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    let newModel: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((model) => (newModel = model))
    expect(newModel).toBeUndefined()
    selectionModel.toggle(items[0].id)
    expect(newModel.getSelectedItems()).toEqual([items[0]])
  })

  it('should emit change when items excluded', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    let newModel: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((model) => (newModel = model))

    selectionModel.set(items[0].id, ToggleableItemState.Excluded)
    expect(newModel.getSelectedItems()).toEqual([])
    expect(newModel.getExcludedItems()).toEqual([items[0]])

    selectionModel.set(items[0].id, ToggleableItemState.NotSelected)
    expect(newModel.getSelectedItems()).toEqual([])
    expect(newModel.getExcludedItems()).toEqual([])
  })

  it('should exclude items when excluded and not editing', () => {
    component.selectionModel.items = items
    component.selectionModel.manyToOne = true
    component.selectionModel = selectionModel
    selectionModel.set(items[0].id, ToggleableItemState.Selected)
    component.excludeClicked(items[0].id)
    expect(selectionModel.getSelectedItems()).toEqual([])
    expect(selectionModel.getExcludedItems()).toEqual([items[0]])
  })

  it('should toggle when items excluded and editing', () => {
    component.selectionModel.items = items
    component.selectionModel.manyToOne = true
    component.editing = true
    component.selectionModel = selectionModel
    selectionModel.set(items[0].id, ToggleableItemState.NotSelected)
    component.excludeClicked(items[0].id)
    expect(selectionModel.getSelectedItems()).toEqual([items[0]])
    expect(selectionModel.getExcludedItems()).toEqual([])
  })

  it('should hide count for item if adding will increase size of set', () => {
    component.selectionModel.items = items
    component.selectionModel.manyToOne = true
    component.selectionModel = selectionModel
    expect(component.hideCount(items[0])).toBeFalsy()
    selectionModel.logicalOperator = LogicalOperator.Or
    expect(component.hideCount(items[0])).toBeTruthy()
  })

  it('should enforce single select when editing', () => {
    component.editing = true
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    let newModel: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((model) => (newModel = model))

    expect(selectionModel.singleSelect).toEqual(true)
    selectionModel.toggle(items[0].id)
    selectionModel.toggle(items[1].id)
    expect(newModel.getSelectedItems()).toEqual([items[1]])
  })

  it('should support manyToOne selecting', () => {
    component.selectionModel.items = items
    selectionModel.manyToOne = false
    component.selectionModel = selectionModel
    component.selectionModel.manyToOne = true
    expect(component.selectionModel.manyToOne).toBeTruthy()
    let newModel: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((model) => (newModel = model))

    expect(selectionModel.singleSelect).toEqual(false)
    selectionModel.toggle(items[0].id)
    selectionModel.toggle(items[1].id)
    expect(newModel.getSelectedItems()).toEqual([items[0], items[1]])
  })

  it('should dynamically enable / disable modifier toggle', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    expect(component.modifierToggleEnabled).toBeTruthy()
    component.selectionModel.manyToOne = true
    expect(component.modifierToggleEnabled).toBeFalsy()
    selectionModel.toggle(items[0].id)
    selectionModel.toggle(items[1].id)
    expect(component.modifierToggleEnabled).toBeTruthy()
  })

  it('should apply changes when apply button clicked', () => {
    component.selectionModel.items = items
    component.editing = true
    component.selectionModel = selectionModel
    component.dropdownOpenChange(true)
    selectionModel.toggle(items[0].id)
    expect(component.modelIsDirty).toBeTruthy()
    let applyResult: ChangedItems
    component.applied.subscribe((result) => (applyResult = result))
    const closeWith = component.applyClicked()
    expect(closeWith).not.toBeNull() // UI closes the dropdown
    expect(applyResult).toEqual({ itemsToAdd: [items[0]], itemsToRemove: [] })
  })

  it('should apply on close if enabled', () => {
    component.selectionModel.items = items
    component.editing = true
    component.applyOnClose = true
    component.selectionModel = selectionModel
    component.dropdownOpenChange(true)
    selectionModel.toggle(items[0].id)
    expect(component.modelIsDirty).toBeTruthy()
    let applyResult: ChangedItems
    component.applied.subscribe((result) => (applyResult = result))
    component.dropdownOpenChange(false)
    expect(applyResult).toEqual({ itemsToAdd: [items[0]], itemsToRemove: [] })
  })

  it('should support filtering, clear on close', () => {
    component.selectionModel.items = items
    component.dropdownOpenChange(true)
    expect(component.filteredItems).toEqual(items) // all selectable items shown
    component.filterText = 'Tag2'
    expect(component.scrollViewportHeight).toEqual(component.FILTERABLE_BUTTON_HEIGHT_PX) // filtered
    component.dropdownOpenChange(false)
    expect(component.filterText).toHaveLength(0)
  })

  it('should omit disallowed null items from the filtered items', () => {
    component.selectionModel.items = items
    expect(component.filteredItems).toEqual(items)
    expect(component.scrollViewportHeight).toEqual(items.length * FILTERABLE_BUTTON_HEIGHT_PX)
    component.allowSelectNone = true
    expect(component.filteredItems).toEqual([nullItem, ...items])
  })

  it('should toggle & close on enter inside filter field if 1 item remains', () => {
    component.selectionModel.items = items
    expect(component.selectionModel.getSelectedItems()).toEqual([])
    component.dropdownOpenChange(true)
    component.filterText = 'Tag2'
    expect(component.listFilterEnter()).toEqual({ type: 'toggled', then: 'close' })
    expect(component.selectionModel.getSelectedItems()).toEqual([items[1]])
  })

  it('should apply & close on enter inside filter field if 1 item remains if editing', () => {
    component.selectionModel.items = items
    component.editing = true
    let applyResult: ChangedItems
    component.applied.subscribe((result) => (applyResult = result))
    component.dropdownOpenChange(true)
    component.filterText = 'Tag2'
    const action = component.listFilterEnter()
    expect(action).toEqual({ type: 'toggled', then: 'apply' })
    expect(component.selectionModel.getSelectedItems()).toEqual([items[1]])
    component.applyClicked() // UI does this 200 ms later
    expect(applyResult).toEqual({ itemsToAdd: [items[1]], itemsToRemove: [] })
  })

  it('should toggle logical operator', () => {
    component.selectionModel.items = items
    component.selectionModel.manyToOne = true
    selectionModel.set(items[0].id, ToggleableItemState.Selected)
    selectionModel.set(items[1].id, ToggleableItemState.Selected)
    component.selectionModel = selectionModel
    let changedResult: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((result) => (changedResult = result))
    component.dropdownOpenChange(true)

    expect(component.modifierToggleEnabled).toBeTruthy()
    expect(selectionModel.logicalOperator).toEqual(LogicalOperator.And)
    // radio: [(ngModel)]="selectionModel.logicalOperator" (ngModelChange)="toggleOperator()"
    selectionModel.logicalOperator = LogicalOperator.Or
    selectionModel.toggleOperator()
    expect(selectionModel.logicalOperator).toEqual(LogicalOperator.Or)
    expect(changedResult.logicalOperator).toEqual(LogicalOperator.Or)
  })

  it('should toggle intersection include / exclude', () => {
    component.selectionModel.items = items
    selectionModel.set(items[0].id, ToggleableItemState.Selected)
    selectionModel.set(items[1].id, ToggleableItemState.Selected)
    component.selectionModel = selectionModel
    let changedResult: FilterableDropdownSelectionModel
    component.selectionModelChange.subscribe((result) => (changedResult = result))
    component.dropdownOpenChange(true)

    expect(component.modifierToggleEnabled).toBeTruthy()
    expect(selectionModel.intersection).toEqual(Intersection.Include)
    selectionModel.intersection = Intersection.Exclude
    selectionModel.toggleIntersection()
    expect(selectionModel.intersection).toEqual(Intersection.Exclude)
    expect(changedResult.intersection).toEqual(Intersection.Exclude)
    expect(changedResult.getSelectedItems()).toEqual([])
    expect(changedResult.getExcludedItems()).toEqual(items)
  })

  it('should update null item selection on toggleIntersection', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    component.selectionModel.intersection = Intersection.Include
    component.selectionModel.set(null, ToggleableItemState.Selected)
    component.selectionModel.intersection = Intersection.Exclude
    component.selectionModel.toggleIntersection()
    expect(component.selectionModel.getExcludedItems()).toEqual([negativeNullItem])

    component.selectionModel.intersection = Intersection.Include
    component.selectionModel.toggleIntersection()
    expect(component.selectionModel.getSelectedItems()).toEqual([nullItem])
  })

  it('selection model should sort items by state', () => {
    component.selectionModel = selectionModel
    component.selectionModel.items = items.concat([{ id: 3, name: 'Item3' }])
    selectionModel.toggle(items[1].id)
    selectionModel.apply()
    expect(selectionModel.items.length).toEqual(4)
    expect(selectionModel.items).toEqual([nullItem, items[1], { id: 3, name: 'Item3' }, items[0]])

    selectionModel.intersection = Intersection.Exclude
    selectionModel.toggleIntersection()
    selectionModel.apply()
    expect(selectionModel.items).toEqual([negativeNullItem, items[1], { id: 3, name: 'Item3' }, items[0]])

    // coverage
    selectionModel.items = selectionModel.items.reverse()
    selectionModel.apply()
  })

  it('selection model should sort items by state and document counts = 0, if set', () => {
    const tagA = { id: 4, name: 'Tag A' }
    component.selectionModel.items = items.concat([tagA])
    component.selectionModel = selectionModel
    component.documentCounts = [
      { id: 1, document_count: 0 }, // Tag1
      { id: 2, document_count: 1 }, // Tag2
      { id: 4, document_count: 2 }, // Tag A
    ]
    component.selectionModel.apply()
    expect(selectionModel.items).toEqual([nullItem, tagA, items[1], items[0]])

    selectionModel.toggle(items[1].id)
    component.documentCounts = [
      { id: 1, document_count: 0 },
      { id: 2, document_count: 1 },
      { id: 4, document_count: 0 },
    ]
    selectionModel.apply()
    expect(selectionModel.items).toEqual([nullItem, items[1], tagA, items[0]])
  })

  it('keeps children with their parent when parent has document count', () => {
    const parent: Tag = { id: 10, name: 'Parent Tag', orderIndex: 0, document_count: 2 }
    const child: Tag = { id: 11, name: 'Child Tag', parent: parent.id, orderIndex: 1, document_count: 0 }
    const otherRoot: Tag = { id: 20, name: 'Other Tag', orderIndex: 2, document_count: 0 }

    component.selectionModel.items = [parent, child, otherRoot]
    component.selectionModel = selectionModel
    component.documentCounts = [
      { id: parent.id, document_count: 2 },
      { id: otherRoot.id, document_count: 0 },
    ]
    selectionModel.apply()

    expect(component.selectionModel.items).toEqual([nullItem, parent, child, otherRoot])
  })

  it('keeps selected branches ahead of document-based ordering', () => {
    const selectedRoot: Tag = { id: 30, name: 'Selected Root', orderIndex: 0, document_count: 0 }
    const otherRoot: Tag = { id: 40, name: 'Other Root', orderIndex: 1, document_count: 2 }

    component.selectionModel.items = [selectedRoot, otherRoot]
    component.selectionModel = selectionModel
    selectionModel.set(selectedRoot.id, ToggleableItemState.Selected)
    component.documentCounts = [
      { id: selectedRoot.id, document_count: 0 },
      { id: otherRoot.id, document_count: 2 },
    ]
    selectionModel.apply()

    expect(component.selectionModel.items).toEqual([nullItem, selectedRoot, otherRoot])
  })

  it('deselecting a parent clears selected descendants', () => {
    const root: Tag = { id: 100, name: 'Root Tag' }
    const child: Tag = { id: 101, name: 'Child Tag', parent: root.id }
    const grandchild: Tag = { id: 102, name: 'Grandchild Tag', parent: child.id }
    const other: Tag = { id: 103, name: 'Other Tag' }

    selectionModel.items = [root, child, grandchild, other]
    selectionModel.set(root.id, ToggleableItemState.Selected, false)
    selectionModel.set(child.id, ToggleableItemState.Selected, false)
    selectionModel.set(grandchild.id, ToggleableItemState.Selected, false)
    selectionModel.set(other.id, ToggleableItemState.Selected, false)

    selectionModel.toggle(root.id, false)

    expect(selectionModel.getSelectedItems()).toEqual([other])
  })

  it('re-selects ancestors when a child is re-selected while editing', () => {
    // https://github.com/paperless-ngx/paperless-ngx/issues/13970
    const inbox: Tag = { id: 200, name: 'Inbox' }
    const parent: Tag = { id: 201, name: 'Parent Tag' }
    const child: Tag = { id: 202, name: 'Child Tag', parent: parent.id }

    selectionModel.editing = true
    selectionModel.items = [inbox, parent, child]
    selectionModel.init(
      new Map([
        [inbox.id, ToggleableItemState.Selected],
        [parent.id, ToggleableItemState.Selected],
        [child.id, ToggleableItemState.Selected],
      ])
    )

    // deselecting the parent also deselects the child
    selectionModel.toggle(parent.id, false)
    expect(selectionModel.getSelectedItems()).toEqual([inbox])

    // re-selecting the child brings its parent back, so nothing is changed
    selectionModel.toggle(child.id, false)
    expect(
      selectionModel
        .getSelectedItems()
        .map((item) => item.id)
        .sort((a, b) => a - b)
    ).toEqual([inbox.id, parent.id, child.id])
    expect(selectionModel.diff()).toEqual({ itemsToAdd: [], itemsToRemove: [] })
  })

  it('un-excluding a parent clears excluded descendants', () => {
    const root: Tag = { id: 110, name: 'Root Tag' }
    const child: Tag = { id: 111, name: 'Child Tag', parent: root.id }
    const other: Tag = { id: 112, name: 'Other Tag' }

    selectionModel.items = [root, child, other]
    selectionModel.set(root.id, ToggleableItemState.Excluded, false)
    selectionModel.set(child.id, ToggleableItemState.Excluded, false)
    selectionModel.set(other.id, ToggleableItemState.Excluded, false)

    selectionModel.exclude(root.id, false)

    expect(selectionModel.getExcludedItems()).toEqual([other])
  })

  it('excluding a selected parent clears selected descendants', () => {
    const root: Tag = { id: 120, name: 'Root Tag' }
    const child: Tag = { id: 121, name: 'Child Tag', parent: root.id }
    const other: Tag = { id: 122, name: 'Other Tag' }

    selectionModel.manyToOne = true
    selectionModel.items = [root, child, other]
    selectionModel.set(root.id, ToggleableItemState.Selected, false)
    selectionModel.set(child.id, ToggleableItemState.Selected, false)
    selectionModel.set(other.id, ToggleableItemState.Selected, false)

    selectionModel.exclude(root.id, false)

    expect(selectionModel.getExcludedItems()).toEqual([root])
    expect(selectionModel.getSelectedItems()).toEqual([other])
  })

  it('resorts items immediately when document count sorting enabled', () => {
    const apple: Tag = { id: 55, name: 'Apple' }
    const zebra: Tag = { id: 56, name: 'Zebra' }

    selectionModel.editing = true
    selectionModel.items = [apple, zebra]
    expect(selectionModel.items.map((item) => item?.id ?? null)).toEqual([null, apple.id, zebra.id])

    selectionModel.documentCounts = [
      { id: zebra.id, document_count: 5 },
      { id: apple.id, document_count: 0 },
    ]

    expect(selectionModel.items.map((item) => item?.id ?? null)).toEqual([null, zebra.id, apple.id])
  })

  it('does not resort items by default when document counts are set', () => {
    const first: Tag = { id: 57, name: 'First' }
    const second: Tag = { id: 58, name: 'Second' }

    selectionModel.items = [first, second]
    selectionModel.documentCounts = [
      { id: second.id, document_count: 10 },
      { id: first.id, document_count: 0 },
    ]

    expect(selectionModel.items.map((item) => item?.id ?? null)).toEqual([null, first.id, second.id])
  })

  it('uses fallback document counts when selection data is missing', () => {
    const fallbackRoot: Tag = { id: 50, name: 'Fallback Root', orderIndex: 0, document_count: 3 }
    const fallbackChild: Tag = {
      id: 51,
      name: 'Fallback Child',
      parent: fallbackRoot.id,
      orderIndex: 1,
      document_count: 0,
    }
    const otherRoot: Tag = { id: 60, name: 'Other Root', orderIndex: 2, document_count: 0 }

    component.selectionModel = selectionModel
    selectionModel.items = [fallbackRoot, fallbackChild, otherRoot]
    component.documentCounts = [{ id: otherRoot.id, document_count: 0 }]

    selectionModel.apply()

    expect(selectionModel.items).toEqual([nullItem, fallbackRoot, fallbackChild, otherRoot])
  })

  it('handles special and non-numeric ids when promoting branches', () => {
    const rootWithDocs: Tag = { id: 70, name: 'Root With Docs', orderIndex: 0, document_count: 1 }
    const miscItem: any = { id: 'misc', name: 'Misc Item' }

    component.selectionModel = selectionModel
    selectionModel.intersection = Intersection.Exclude
    selectionModel.items = [rootWithDocs, miscItem]
    component.documentCounts = [{ id: rootWithDocs.id, document_count: 1 }]

    selectionModel.apply()

    expect(selectionModel.items.map((item) => item.id)).toEqual([NEGATIVE_NULL_FILTER_VALUE, rootWithDocs.id, 'misc'])
  })

  it('memoizes root document counts between lookups', () => {
    const memoRoot: Tag = { id: 80, name: 'Memo Root' }
    selectionModel.items = [memoRoot]
    selectionModel.documentCounts = [{ id: memoRoot.id, document_count: 9 }]

    const getRootDocCount = (selectionModel as any).createRootDocCounter(selectionModel.items)

    expect(getRootDocCount(memoRoot.id)).toEqual(9)
    selectionModel.documentCounts = []
    expect(getRootDocCount(memoRoot.id)).toEqual(9)
  })

  it('falls back to model stored document counts if selection data missing entry', () => {
    const rootWithoutSelection: Tag = { id: 90, name: 'Fallback Root', document_count: 4 }
    selectionModel.items = [rootWithoutSelection]
    selectionModel.documentCounts = []

    const getRootDocCount = (selectionModel as any).createRootDocCounter(selectionModel.items)

    expect(getRootDocCount(rootWithoutSelection.id)).toEqual(4)
  })

  it('defaults to zero document count when neither selection nor model provide it', () => {
    const rootWithoutCounts: Tag = { id: 91, name: 'Fallback Zero Root' }
    selectionModel.items = [rootWithoutCounts]
    selectionModel.documentCounts = []

    const getRootDocCount = (selectionModel as any).createRootDocCounter(selectionModel.items)

    expect(getRootDocCount(rootWithoutCounts.id)).toEqual(0)
  })

  it('should keep the dropdown open while the create modal is active', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    component.dropdownOpenChange(true)
    component.filterText = 'Test Filter Text'
    component.creating = true
    expect(component.dropdownOpenChange(false)).toEqual('keep-open')
    expect(component.dropdownOpenChange(false)).toEqual('keep-open')
    expect(component.filterText).toEqual('Test Filter Text')
    component.creating = false
    expect(component.dropdownOpenChange(false)).toBeUndefined()
  })

  it('should call create on enter inside filter field if 0 items remain while editing', () => {
    component.selectionModel.items = items
    component.editing = true
    component.canCreate = true
    component.dropdownOpenChange(true)
    component.filterText = 'FooBar'
    expect(component.listFilterEnter()).toEqual({ type: 'create' })
    expect(component.selectionModel.getSelectedItems()).toEqual([])
  })

  it('should only show create when a non-empty filter has no matches', () => {
    component.selectionModel.items = []
    component.editing = true
    component.canCreate = true
    expect(component.showCreate).toBeFalsy()
    expect(component.listFilterEnter()).toEqual({ type: 'none' })
    component.filterText = 'FooBar'
    expect(component.showCreate).toBeTruthy()
  })

  it('should exclude item and trigger change event', () => {
    const id = 1
    const state = ToggleableItemState.Selected
    component.selectionModel = selectionModel
    component.selectionModel.manyToOne = true
    component.selectionModel.singleSelect = true
    component.selectionModel.intersection = Intersection.Include
    ;(component.selectionModel as any)['temporarySelectionStates'].set(id, state)
    const changedSpy = vi.spyOn(component.selectionModel.changed, 'next')
    component.selectionModel.exclude(id)
    expect(component.selectionModel.temporaryLogicalOperator).toBe(LogicalOperator.And)
    expect((component.selectionModel as any)['temporarySelectionStates'].get(id)).toBe(ToggleableItemState.Excluded)
    expect((component.selectionModel as any)['temporarySelectionStates'].size).toBe(1)
    expect(changedSpy).toHaveBeenCalled()
  })

  it('should initialize selection states and apply changes', () => {
    selectionModel.items = items
    const map = new Map<number, ToggleableItemState>()
    map.set(1, ToggleableItemState.Selected)
    map.set(2, ToggleableItemState.Excluded)
    selectionModel.init(map)
    expect(selectionModel.getSelectedItems()).toEqual([items[0]])
    expect(selectionModel.getExcludedItems()).toEqual([items[1]])
  })

  it('should support an extra button and not apply changes when clicked', () => {
    component.selectionModel.items = items
    component.selectionModel = selectionModel
    component.applyOnClose = true
    let extraButtonClicked = false
    let applied = false
    component.extraButton.subscribe(() => (extraButtonClicked = true))
    component.applied.subscribe(() => (applied = true))
    component.dropdownOpenChange(true)
    component.extraButtonClicked()
    expect(extraButtonClicked).toBeTruthy()
    expect(applied).toBeFalsy()
  })

  it('should complete the previous model when a new one is set', () => {
    const old = component.selectionModel
    const spy = vi.fn()
    old.onChange(spy)
    component.selectionModel = selectionModel
    old.emit()
    expect(spy).not.toHaveBeenCalled()
  })

  it('should translate the null item with the given translator', () => {
    const m = new FilterableDropdownSelectionModel(false, { t: (k) => (k === NOT_ASSIGNED_KEY ? 'Nicht zugewiesen' : k) })
    m.items = items
    expect(m.items[0]).toEqual({ id: null, name: 'Nicht zugewiesen' })
  })

  it('should drop the null item for many-to-one OR', () => {
    const m = new FilterableDropdownSelectionModel(true)
    m.items = items
    expect(m.items[0].id).toBeNull()
    m.logicalOperator = LogicalOperator.Or
    expect(m.items).toEqual(items)
  })

  it('should report isNoneSelected / totalCount', () => {
    selectionModel.items = items
    selectionModel.toggle(null)
    expect(selectionModel.isNoneSelected()).toBeTruthy()
    selectionModel.toggle(1)
    expect(selectionModel.isNoneSelected()).toBeFalsy()
    selectionModel.set(2, ToggleableItemState.Excluded)
    expect(selectionModel.totalCount).toEqual(2)
  })
})

describe('ToggleableDropdownButton helpers', () => {
  it('should recognize tags, depth and counts', () => {
    expect(isTag(items[0])).toBeTruthy()
    expect(isTag({ id: 1, name: 'Corr' })).toBeFalsy()
    expect(getDepth({ id: 1, name: 'x', depth: 2 })).toEqual(2)
    expect(getDepth({ id: 1, name: 'x' })).toEqual(0)
    expect(currentCount({ id: 1, name: 'x', document_count: 4 }, undefined)).toEqual(4)
    expect(currentCount({ id: 1, name: 'x', document_count: 4 }, 7)).toEqual(7)
  })

  it('should exclude when selected, otherwise toggle', () => {
    expect(toggleItemAction(ToggleableItemState.Selected)).toEqual('exclude')
    expect(toggleItemAction(ToggleableItemState.NotSelected)).toEqual('toggle')
    expect(toggleItemAction(ToggleableItemState.Excluded)).toEqual('toggle')
  })
})

describe('filterItems', () => {
  it('filters by name accent-insensitively and hides the null item', () => {
    const list = [nullItem, { id: 1, name: 'Résumé' }, { id: 2, name: 'Søren' }, { id: 3, name: 'Œuvre' }]
    expect(filterItems(list, 'resume')).toEqual([list[1]])
    expect(filterItems(list, 'soren')).toEqual([list[2]])
    expect(filterItems(list, 'oeuvre')).toEqual([list[3]])
    expect(filterItems(list, '')).toEqual(list.slice(1))
    expect(filterItems(list, '', true)).toEqual(list)
  })
})

describe('flattenTags', () => {
  it('returns empty array for empty input', () => {
    expect(flattenTags([])).toEqual([])
  })

  it('orders roots and children by name (case-insensitive, numeric) and sets depth/orderIndex', () => {
    const input: Tag[] = [
      { id: 11, name: 'A-root' },
      { id: 10, name: 'B-root' },
      { id: 101, name: 'Child 10', parent: 11 },
      { id: 102, name: 'child 2', parent: 11 },
      { id: 201, name: 'beta', parent: 10 },
      { id: 202, name: 'Alpha', parent: 10 },
      { id: 103, name: 'Sub 1', parent: 102 },
    ]

    const flat = flattenTags(input)

    expect(flat.map((t) => t.name)).toEqual(['A-root', 'child 2', 'Sub 1', 'Child 10', 'B-root', 'Alpha', 'beta'])
    expect(flat.map((t) => t.depth)).toEqual([0, 1, 2, 1, 0, 1, 1])
    expect(flat.map((t) => t.orderIndex)).toEqual([0, 1, 2, 3, 4, 5, 6])

    const aRoot = flat.find((t) => t.name === 'A-root')
    expect(new Set(aRoot.children?.map((c) => c.name))).toEqual(new Set(['child 2', 'Child 10']))
    const bRoot = flat.find((t) => t.name === 'B-root')
    expect(new Set(bRoot.children?.map((c) => c.name))).toEqual(new Set(['Alpha', 'beta']))
    const child2 = flat.find((t) => t.name === 'child 2')
    expect(new Set(child2.children?.map((c) => c.name))).toEqual(new Set(['Sub 1']))
  })

  it('excludes orphaned nodes (with missing parent)', () => {
    const flat = flattenTags([
      { id: 1, name: 'Root' },
      { id: 2, name: 'Child', parent: 1 },
      { id: 3, name: 'Orphan', parent: 999 },
    ])
    expect(flat.map((t) => t.name)).toEqual(['Root', 'Child'])
  })

  it('keeps hierarchical order inside the selection model', () => {
    const m = new FilterableDropdownSelectionModel(true)
    m.items = flattenTags([
      { id: 1, name: 'Zeta' },
      { id: 2, name: 'Alpha child', parent: 1 },
      { id: 3, name: 'Beta' },
    ])
    expect(m.items.map((i) => i.name)).toEqual(['Not set', 'Beta', 'Zeta', 'Alpha child'])
  })
})

describe('text search utilities', () => {
  it('matches text accent-insensitively', () => {
    expect(matchesSearchText('Résumé', 'resume')).toBeTruthy()
    expect(matchesSearchText('Søren', 'soren')).toBeTruthy()
    expect(matchesSearchText('Œuvre', 'oeuvre')).toBeTruthy()
    expect(matchesSearchText('Straße', 'strasse')).toBeTruthy()
    expect(matchesSearchText('Invoice', 'receipt')).toBeFalsy()
  })

  it('matches all whitespace-separated search terms independently', () => {
    expect(matchesSearchText('taxes 2026', 'tax 26')).toBeTruthy()
    expect(matchesSearchText('2026 taxes', 'tax 26')).toBeTruthy()
    expect(matchesSearchText('Taxés 2026', 'taxe 26')).toBeTruthy()
    expect(matchesSearchText('taxes 2026', 'tax receipt')).toBeFalsy()
  })

  it('does not let two terms match the same word', () => {
    expect(matchesSearchText('Another Tag', 'another tag th')).toBeFalsy()
    expect(matchesSearchText('Another Tag', 'another tag ag')).toBeFalsy()
    expect(matchesSearchText('Another Tag', 'another tag e')).toBeFalsy()
    expect(matchesSearchText('Another Tag', 'another tag')).toBeTruthy()
    expect(matchesSearchText('Another Tag', 'tag another')).toBeTruthy()
  })

  it('matches a single term anywhere in the value', () => {
    expect(matchesSearchText('Another Tag', 'anoth')).toBeTruthy()
    expect(matchesSearchText('Another Tag', 'th')).toBeTruthy()
  })

  it('treats punctuation as a separator on both sides', () => {
    expect(matchesSearchText('medical-history', 'medical history')).toBeTruthy()
    expect(matchesSearchText('medical history', 'medical-history')).toBeTruthy()
    expect(matchesSearchText('medical-history', 'medical dental')).toBeFalsy()
  })

  it('matches longer terms first so they cannot be starved', () => {
    expect(matchesSearchText('tagger tag', 'tag tagger')).toBeTruthy()
  })

  it('handles a query with no usable terms', () => {
    expect(matchesSearchText('Another Tag', '')).toBeTruthy()
    expect(matchesSearchText('Another Tag', '---')).toBeFalsy()
  })

  it('matches a large set of tag names without blocking input', () => {
    const tagNames = Array.from(
      { length: 1280 },
      (_, index) => `Customer Party ${index} José Müller Ångström`
    )
    const start = performance.now()
    const matches = tagNames.filter((name) => matchesSearchText(name, 'party'))
    const duration = performance.now() - start
    expect(matches).toHaveLength(tagNames.length)
    expect(duration).toBeLessThan(250)
  })
})
