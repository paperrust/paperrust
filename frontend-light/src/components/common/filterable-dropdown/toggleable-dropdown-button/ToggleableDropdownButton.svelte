<!-- Port of common/filterable-dropdown/toggleable-dropdown-button. Host in the parent markup. -->
<script lang="ts">
  import '../../../../styles/components/toggleable-dropdown-button.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { ToggleableItemState, currentCount, getDepth, isTag as isTagItem, toggleItemAction, NOT_ASSIGNED_KEY, type FilterableItem } from '../../../../lib/filter-editor/selection-model'
  import Tag from '../../tag/Tag.svelte'

  let {
    item,
    state,
    count = undefined,
    hideCount = false,
    opacifyCount = true,
    disabled = false,
    ontoggled,
    onexclude,
  }: {
    item: FilterableItem
    state: ToggleableItemState
    count?: number
    hideCount?: boolean
    opacifyCount?: boolean
    disabled?: boolean
    ontoggled: () => void
    onexclude: () => void
  } = $props()

  const isTag = $derived(isTagItem(item))
  const depth = $derived(getDepth(item))
  const cur = $derived(currentCount(item, count))
  // The "Not assigned" element has id null/-1 and an already translated or source name
  const label = $derived(item.name === NOT_ASSIGNED_KEY ? t(NOT_ASSIGNED_KEY) : item.name)

  function toggleItem(e: MouseEvent) {
    if (toggleItemAction(state) === 'exclude') onexclude()
    else ontoggled()
    e.preventDefault?.()
  }
</script>

<button
  class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom"
  class:opacity-50={opacifyCount && !hideCount && cur === 0}
  role="menuitem"
  onclick={toggleItem}
  {disabled}>
  <div class="selected-icon me-1">
    {#if state === ToggleableItemState.Selected}
      <pngx-bs-icon width="1em" height="1em" name="check">{@html icon('check', '1em', '1em')}</pngx-bs-icon>
    {/if}
    {#if state === ToggleableItemState.PartiallySelected}
      <pngx-bs-icon width="1em" height="1em" name="dash">{@html icon('dash', '1em', '1em')}</pngx-bs-icon>
    {/if}
    {#if state === ToggleableItemState.Excluded}
      <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon>
    {/if}
  </div>
  <div class="me-1 name-cell" style="--depth: {isTag ? depth + 1 : 1}">
    {#if isTag && depth > 0}
      <div class="indicator"></div>
    {/if}
    <div>
      {#if isTag}
        <pngx-tag><Tag tag={item as any} clickable={false} /></pngx-tag>
      {:else}
        {label}
      {/if}
    </div>
  </div>
  {#if !hideCount}
    <div class="badge bg-light text-dark rounded-pill ms-auto me-1">{cur ?? ''}</div>
  {/if}
</button>
