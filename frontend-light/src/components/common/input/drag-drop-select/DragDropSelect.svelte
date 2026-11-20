<!-- Port of common/input/drag-drop-select/drag-drop-select.component.html (cdkDropList/cdkDrag → HTML5 dragging).
     Host <pngx-input-drag-drop-select> in the parent markup. Value: list of IDs in display order. -->
<script lang="ts">
  import '../../../../styles/components/input-drag-drop-select.css'
  import { t } from '../../../../lib/i18n'
  import { dropItem, selectedFromValue, unselected, type DdItem } from './drag-drop'

  let {
    value = $bindable(),
    title = t('input.selected_items'),
    items = [],
    emptyText = t('input.no_items_selected'),
    disabled = false,
    onchange,
  }: { value?: string[] | null; title?: string; items?: DdItem[]; emptyText?: string; disabled?: boolean; onchange?: (v: string[]) => void } = $props()

  const selectedItems = $derived(selectedFromValue(items, value))
  const unselectedItems = $derived(unselected(items, selectedItems))
  let drag: { from: 'selected' | 'unselected'; index: number } | null = null

  function start(e: DragEvent, from: 'selected' | 'unselected', index: number) {
    if (disabled) return e.preventDefault()
    drag = { from, index }
    e.dataTransfer?.setData('text/plain', String(index))
  }
  /** Target index = position of the next entry to the left of/above the pointer (like cdkDropListOrientation="mixed") */
  function targetIndex(list: HTMLElement, e: DragEvent) {
    const badges = [...list.querySelectorAll<HTMLElement>('[data-dd-item]')]
    let i = 0
    for (const b of badges) {
      const r = b.getBoundingClientRect()
      if (e.clientY > r.bottom || (e.clientY >= r.top && e.clientX > r.left + r.width / 2)) i++
    }
    return i
  }
  function drop(e: DragEvent, to: 'selected' | 'unselected') {
    e.preventDefault()
    if (disabled || !drag) return
    const next = dropItem(items, selectedItems, drag.from, to, drag.index, targetIndex(e.currentTarget as HTMLElement, e))
    drag = null
    value = next.map((i) => i.id)
    onchange?.(value)
  }
</script>

<div class="d-flex flex-row mt-2 align-items-center">
    <span class="me-2">{title}:</span>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="d-flex flex-wrap flex-row gap-2 w-100 mh-1 cdk-drop-list" class:cdk-drop-list-disabled={disabled} style={"min-height: 1em;"} ondragover={(e) => e.preventDefault()} ondrop={(e) => drop(e, 'selected')}>
        {#each selectedItems as item, i (item.id)}
            <div class="badge bg-primary cdk-drag" class:cdk-drag-disabled={disabled} draggable={!disabled} data-dd-item role="listitem" ondragstart={(e) => start(e, 'selected', i)}>{item.name}</div>
        {/each}
        {#if selectedItems.length === 0}
            <div class="badge bg-light text-secondary fst-italic">{emptyText}</div>
        {/if}
    </div>
</div>
<div class="mt-2 align-items-center bg-light p-2">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="d-flex flex-wrap flex-row gap-2 w-100 cdk-drop-list" class:cdk-drop-list-disabled={disabled} style={"min-height: 1em;"} ondragover={(e) => e.preventDefault()} ondrop={(e) => drop(e, 'unselected')}>
        {#each unselectedItems as item, i (item.id)}
            <div class="badge bg-secondary opacity-50 cdk-drag" class:cdk-drag-disabled={disabled} draggable={!disabled} data-dd-item role="listitem" ondragstart={(e) => start(e, 'unselected', i)}>{item.name}</div>
        {/each}
    </div>
</div>
