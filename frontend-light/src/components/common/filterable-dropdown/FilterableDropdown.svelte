<!-- Port of common/filterable-dropdown/filterable-dropdown.component.html.
     Logic: lib/filter-editor/selection-model.ts (FilterableDropdownState). Host <pngx-filterable-dropdown> in the parent markup.
     cdk-virtual-scroll-viewport: same classes/height; all entries are rendered (same look, scroll height via spacer). -->
<script lang="ts">
  import '../../../styles/components/filterable-dropdown.css'
  import '../../../styles/parts/virtual-scroll.css'
  import { onDestroy, tick, untrack } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import {
    FilterableDropdownSelectionModel,
    FilterableDropdownState,
    LogicalOperator,
    Intersection,
    ToggleableItemState,
    type ChangedItems,
    type SelectionDataItem,
  } from '../../../lib/filter-editor/selection-model'
  import ToggleableDropdownButton from './toggleable-dropdown-button/ToggleableDropdownButton.svelte'
  import ClearableBadge from '../clearable-badge/ClearableBadge.svelte'
  import { registerHotkey } from '../../../lib/hotkeys-registry'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { ListKeyboard } from './list-keydown'

  let {
    title,
    icon: iconName,
    name,
    filterPlaceholder = '',
    model = undefined,
    dropdownState = undefined,
    documentCounts = undefined,
    allowSelectNone = false,
    editing = false,
    disabled = false,
    applyOnClose = false,
    extraButtonTitle = undefined,
    showExtraButtonIfEmpty = false,
    onchange,
    onapply,
    onopened,
    onextrabutton,
    version = 0,
    shortcutKey = undefined,
    createRef = undefined,
  }: {
    title: string
    icon: string
    /** id suffix like Angular `name` (sanitizeName(title)) */
    name?: string
    filterPlaceholder?: string
    model?: FilterableDropdownSelectionModel
    /** existing state (e.g. from FilterEditorState); otherwise one is created for the model */
    dropdownState?: FilterableDropdownState
    documentCounts?: SelectionDataItem[]
    allowSelectNone?: boolean
    editing?: boolean
    disabled?: boolean
    applyOnClose?: boolean
    extraButtonTitle?: string
    showExtraButtonIfEmpty?: boolean
    onchange?: (m: FilterableDropdownSelectionModel) => void
    onapply?: (c: ChangedItems) => void
    onopened?: () => void
    onextrabutton?: (c: ChangedItems) => void
    /** counter of the parent component (changes to the shared state) */
    version?: number
    /** keyboard shortcut to open (upstream shortcutKey, HotKeyService) */
    shortcutKey?: string
    /** open create dialog for a name (upstream createRef); "Create" only appears with it */
    createRef?: (name: string) => ModalRef<any> | Promise<ModalRef<any> | undefined> | undefined
  } = $props()

  // svelte-ignore state_referenced_locally
  const st = dropdownState ?? new FilterableDropdownState(model)
  // svelte-ignore state_referenced_locally
  st.editing = editing
  let v = $state(0) // counter: logic object is not reactive
  // untrack: bump() also runs in $effect – v++ reads v, which made the effect depend on v → endless loop (A-151)
  const bump = () => untrack(() => v++)
  const off = [
    st.onChange((m) => {
      bump()
      onchange?.(m)
    }),
    st.applied.subscribe((c) => onapply?.(c)),
    st.opened.subscribe(() => onopened?.()),
  ]
  onDestroy(() => {
    off.forEach((f) => f())
    st.destroy()
  })
  $effect(() => {
    if (model && st.selectionModel !== model) {
      st.selectionModel = model
      bump()
    }
    st.allowSelectNone = allowSelectNone
    st.disabled = disabled
    st.applyOnClose = applyOnClose
    st.title = title
    st.canCreate = !!createRef
  })
  $effect(() => {
    if (documentCounts) {
      st.documentCounts = documentCounts
      bump()
    }
  })

  let filterInput: HTMLInputElement | undefined = $state()
  // listKeyDown: arrow keys between filter field and entries (list-keydown.ts)
  let buttonsViewport: HTMLElement | undefined = $state()
  const keys = new ListKeyboard()
  const listKeyDown = (e: KeyboardEvent) => keys.keyDown(e, filterInput, Array.from(buttonsViewport?.querySelectorAll('button') ?? []), st.filterText)
  const dd = new Dropdown({
    pngxPopper: true,
    onOpenChange: async (open) => {
      if (st.dropdownOpenChange(open) === 'keep-open') {
        // upstream autoClose=false while creating: reopen the menu, leave focus in the dialog
        const prev = document.activeElement as HTMLElement | null
        dd.openMenu()
        prev?.focus?.()
        return
      }
      bump()
      if (open && !st.creating) {
        await tick()
        setTimeout(() => filterInput?.focus(), 0)
      }
    },
  })

  const view = $derived.by(() => {
    v
    version
    const m = st.selectionModel
    return {
      m,
      selectionSize: m.selectionSize(),
      totalCount: m.totalCount,
      manyToOne: m.manyToOne,
      logicalOperator: m.logicalOperator,
      intersection: m.intersection,
      items: m.items,
      filtered: st.filteredItems,
      height: st.scrollViewportHeight,
      modifierToggleEnabled: st.modifierToggleEnabled,
      dirty: st.modelIsDirty,
      filterText: st.filterText,
      // per entry: state, count, hide count (logic object is not reactive → compute here as well)
      rows: st.filteredItems.map((item) => ({ item, state: m.get(item.id), count: st.getUpdatedDocumentCount(item.id), hide: st.hideCount(item) })),
    }
  })
  // upstream ngOnInit: addShortcut({ keys: shortcutKey, description: `Open ${title} filter` }), only if not disabled
  $effect(() => {
    if (!shortcutKey) return
    return registerHotkey({
      keys: shortcutKey,
      description: t('filter.open_filter', { PH: title }),
      enabled: () => !disabled,
      handler: () => dd.openMenu(),
    })
  })
  /** createClicked: open dialog, until it closes "creating" applies (menu stays open) */
  async function createClicked() {
    if (!createRef) return
    st.creating = true
    try {
      const ref = await createRef(st.filterText)
      if (ref) await ref.result
    } catch {
      // dialog could not be loaded
    } finally {
      st.creating = false
      bump()
    }
  }

  const id = $derived(name ?? (title ? title.replace(/[\s]/g, '_').toLowerCase() : ''))

  function setOperator(op: LogicalOperator) {
    st.selectionModel.logicalOperator = op
    st.selectionModel.toggleOperator()
    bump()
  }
  function setIntersection(i: Intersection) {
    st.selectionModel.intersection = i
    st.selectionModel.toggleIntersection()
    bump()
  }
  function filterTextInput(e: Event) {
    st.filterText = (e.target as HTMLInputElement).value
    bump()
  }
  function listFilterEnter() {
    const r = st.listFilterEnter()
    bump()
    if (r.type === 'create') createClicked()
    else if (r.type === 'toggled') {
      setTimeout(() => {
        if (r.then === 'apply') applyClicked()
        else dd.close()
      }, 200)
    }
  }
  function applyClicked() {
    if (st.applyClicked()) dd.close()
    bump()
  }
  function extraButtonClicked() {
    const diff = st.extraButtonClicked()
    onextrabutton?.(diff)
    dd.close()
  }
  function reset() {
    st.reset()
    bump()
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="btn-group w-100 {dd.placementClass}" role="group" class:show={dd.open} use:dd.hostEl onkeydown={listKeyDown}>
  <button class="dropdown-toggle btn btn-sm {!editing && view.selectionSize > 0 ? 'btn-primary' : 'btn-outline-primary'}" id="dropdown_{id}" {disabled} aria-label={title} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon>{@html icon(iconName)}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{title}</div>
    {#if !editing && view.totalCount > 0}
      <pngx-clearable-badge><ClearableBadge number={view.totalCount} selected={view.selectionSize > 0} oncleared={reset} /></pngx-clearable-badge>
    {/if}
  </button>
  {#if dd.open}
  <div class="dropdown-menu py-0 shadow show" aria-labelledby="dropdown_{id}" use:dd.menuEl>
    <div class="list-group list-group-flush">
      {#if !editing && view.manyToOne}
        <div class="list-group-item d-flex">
          <div class="btn-group btn-group-xs flex-fill" role="group">
            <input checked={view.logicalOperator === LogicalOperator.And} onchange={() => setOperator(LogicalOperator.And)} disabled={!view.modifierToggleEnabled} type="radio" class="btn-check" id="logicalOperatorAnd_{id}" name="logicalOperatorAnd_{id}" value="and">
            <label class="btn btn-outline-primary" for="logicalOperatorAnd_{id}">{t('common.all')}</label>
            <input checked={view.logicalOperator === LogicalOperator.Or} onchange={() => setOperator(LogicalOperator.Or)} disabled={!view.modifierToggleEnabled} type="radio" class="btn-check" id="logicalOperatorOr_{id}" name="logicalOperatorOr_{id}" value="or">
            <label class="btn btn-outline-primary" for="logicalOperatorOr_{id}">{t('ui.any')}</label>
          </div>
        </div>
      {/if}
      {#if !editing && !view.manyToOne}
        <div class="list-group-item d-flex">
          <div class="btn-group btn-group-xs flex-fill" role="group">
            <input checked={view.intersection === Intersection.Include} onchange={() => setIntersection(Intersection.Include)} disabled={!view.modifierToggleEnabled} type="radio" class="btn-check" id="intersectionInclude_{id}" name="intersectionInclude_{id}" value="include">
            <label class="btn btn-outline-primary" for="intersectionInclude_{id}">{t('ui.include')}</label>
            <input checked={view.intersection === Intersection.Exclude} onchange={() => setIntersection(Intersection.Exclude)} disabled={!view.modifierToggleEnabled} type="radio" class="btn-check" id="intersectionExclude_{id}" name="intersectionExclude_{id}" value="exclude">
            <label class="btn btn-outline-primary" for="intersectionExclude_{id}">{t('ui.exclude')}</label>
          </div>
        </div>
      {/if}
      <div class="list-group-item">
        <div class="input-group input-group-sm">
          <input bind:this={filterInput} class="form-control" type="text" spellcheck="false" value={st.filterText} oninput={filterTextInput} placeholder={filterPlaceholder} onkeyup={(e) => e.key === 'Enter' && listFilterEnter()}>
        </div>
      </div>
      {#if view.items}
        <cdk-virtual-scroll-viewport bind:this={buttonsViewport} class="cdk-virtual-scroll-viewport items cdk-virtual-scrollable cdk-virtual-scroll-orientation-vertical" style="height: {view.height}px"><div class="cdk-virtual-scroll-content-wrapper" style={"transform: translateY(0px)"}>{#each view.rows as row, i (row.item.id ?? `n${i}`)}<div>
            <pngx-toggleable-dropdown-button><ToggleableDropdownButton
              item={row.item}
              hideCount={row.hide}
              opacifyCount={!editing}
              state={row.state}
              count={row.count}
              ontoggled={() => { view.m.toggle(row.item.id); bump() }}
              onexclude={() => { st.excludeClicked(row.item.id); bump() }}
              {disabled} /></pngx-toggleable-dropdown-button>
          </div>{/each}</div><div class="cdk-virtual-scroll-spacer" style="height: {view.filtered.length * st.FILTERABLE_BUTTON_HEIGHT_PX}px"></div></cdk-virtual-scroll-viewport>
      {/if}
      {#if editing}
        {#if view.filtered.length === 0 && createRef && view.filterText?.length > 0}
          <button class="list-group-item list-group-item-action bg-light" onclick={createClicked} {disabled}>
            <span class="ms-2">{t('common.create')} "{view.filterText}"</span>
            <pngx-bs-icon width="1.5em" height="1em" name="plus">{@html icon('plus', '1.5em', '1em')}</pngx-bs-icon>
          </button>
        {/if}
        {#if view.filtered.length > 0}
          <button class="list-group-item list-group-item-action bg-light d-flex align-items-center" onclick={applyClicked} disabled={!view.dirty || disabled}>
            <span class="ms-2" class:fw-bold={view.dirty}>{t('ui.apply')}</span>
            <pngx-bs-icon width="1.5em" height="1em" name="arrow-right">{@html icon('arrow-right', '1.5em', '1em')}</pngx-bs-icon>
          </button>
        {/if}
      {/if}
      {#if extraButtonTitle && (showExtraButtonIfEmpty || view.filtered?.length > 0)}
        <button class="list-group-item list-group-item-action bg-light d-flex align-items-center" onclick={extraButtonClicked} {disabled}>
          <span class="ms-2 fw-bold">{extraButtonTitle}</span>
          <pngx-bs-icon width="1.5em" height="1em" name="arrow-right">{@html icon('arrow-right', '1.5em', '1em')}</pngx-bs-icon>
        </button>
      {/if}
      {#if !editing && view.manyToOne}
        <div class="list-group-item list-group-item-note pt-1 pb-2">
          <small>{t('ui.click_again_exclude_items')}</small>
        </div>
      {/if}
    </div>
  </div>
  {/if}
</div>
