<!-- Port of common/permissions-filter-dropdown. Logic: lib/filter-editor/permissions-model.ts. -->
<script lang="ts">
  import '../../../styles/components/permissions-filter-dropdown.css'
  import { onDestroy, untrack } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { OwnerFilterType, PermissionsFilterState } from '../../../lib/filter-editor/permissions-model'
  import { session } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import ClearableBadge from '../clearable-badge/ClearableBadge.svelte'
  import NgSelect from '../ng-select/NgSelect.svelte'

  let { filterState: st, title, disabled = false, version = 0 }: { filterState: PermissionsFilterState; title: string; disabled?: boolean; version?: number } = $props()

  let v = $state(0)
  const bump = () => untrack(() => v++) // safe also from $effect (A-151)
  // svelte-ignore state_referenced_locally
  const off = st.onChange(() => bump())
  onDestroy(() => off())
  const dd = new Dropdown({ pngxPopper: true })
  const dropdownMenuId = 'pngx-permissions-filter-dropdown-0'
  const users = $derived([...data.users.values()])
  const m = $derived.by(() => {
    v
    version
    st.users = users
    return {
      ownerFilter: st.selectionModel.ownerFilter,
      includeUsers: st.selectionModel.includeUsers,
      hideUnowned: st.selectionModel.hideUnowned,
      isActive: st.isActive,
      ownerFilterLabel: st.ownerFilterLabel,
      ownerExclusionFilterLabel: st.ownerExclusionFilterLabel,
      sharedByFilterLabel: st.sharedByFilterLabel,
    }
  })
  const setFilter = (f: OwnerFilterType) => () => {
    st.setFilter(f)
    bump()
  }
</script>

{#snippet check(on: boolean)}
  <div class="selected-icon me-1">
    {#if on}
      <pngx-bs-icon width="1em" height="1em" name="check">{@html icon('check', '1em', '1em')}</pngx-bs-icon>
    {/if}
  </div>
{/snippet}

<div class="btn-group w-100 {dd.placementClass}" role="group" class:show={dd.open} use:dd.hostEl>
  <button class="dropdown-toggle btn btn-sm {m.isActive ? 'btn-primary' : 'btn-outline-primary'}" id="dropdown{title}" {disabled} aria-label={title} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon name="person-fill-lock">{@html icon('person-fill-lock')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{title}</div>
    <pngx-clearable-badge><ClearableBadge selected={m.isActive} oncleared={() => { st.reset(); bump() }} /></pngx-clearable-badge><span class="visually-hidden">selected</span>
  </button>
  {#if dd.open}
  <div id={dropdownMenuId} class="dropdown-menu permission-filter-dropdown shadow py-0 w-2 show" aria-labelledby="dropdown{title}" use:dd.menuEl>
    <div class="list-group list-group-flush">
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" onclick={setFilter(OwnerFilterType.NONE)} {disabled}>
        {@render check(m.ownerFilter === OwnerFilterType.NONE)}
        <div class="me-1">
          <span>{t('common.all')}</span>
        </div>
      </button>
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" onclick={setFilter(OwnerFilterType.SELF)} {disabled}>
        {@render check(m.ownerFilter === OwnerFilterType.SELF)}
        <div class="me-1">
          {m.ownerFilterLabel}
        </div>
      </button>
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" onclick={setFilter(OwnerFilterType.NOT_SELF)} {disabled}>
        {@render check(m.ownerFilter === OwnerFilterType.NOT_SELF)}
        <div class="me-1">
          {m.ownerExclusionFilterLabel}
        </div>
      </button>
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" onclick={setFilter(OwnerFilterType.SHARED_BY_ME)} {disabled}>
        {@render check(m.ownerFilter === OwnerFilterType.SHARED_BY_ME)}
        <div class="me-1">
          {m.sharedByFilterLabel}
        </div>
      </button>
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" onclick={setFilter(OwnerFilterType.UNOWNED)} {disabled}>
        {@render check(m.ownerFilter === OwnerFilterType.UNOWNED)}
        <div class="me-1">
          <span>{t('ui.unowned')}</span>
        </div>
      </button>
      {#if session.can('view', 'user')}
      <button class="list-group-item list-group-item-action d-flex align-items-center p-2 border-top-0 border-start-0 border-end-0 border-bottom" role="menuitem" {disabled}>
        <div class="selected-icon me-1">
          {#if m.ownerFilter === OwnerFilterType.OTHERS}
            <!-- svelte-ignore a11y_invalid_attribute -->
            <a class="text-reset focus-variants" href="javascript:void(0)" onclick={() => { st.clearIncludeUsers(); bump() }} aria-label={t('ui.clear_selected_users')}>
              <pngx-bs-icon width="1em" height="1em" name="check" class="variant-unfocused">{@html icon('check', '1em', '1em')}</pngx-bs-icon>
              <pngx-bs-icon width="1em" height="1em" name="x" class="variant-focused text-primary">{@html icon('x', '1em', '1em')}</pngx-bs-icon>
            </a>
          {/if}
        </div>
        <div class="me-1 w-100">
          <NgSelect
            name="user"
            class="user-select"
            value={m.includeUsers}
            {disabled}
            clearable={false}
            items={users}
            bindLabel="username"
            multiple
            bindValue="id"
            placeholder={t('common.users')}
            onchange={(val) => { st.setIncludeUsers(val); bump() }} />
        </div>
      </button>
      {/if}
      {#if m.ownerFilter === OwnerFilterType.NONE || m.ownerFilter === OwnerFilterType.NOT_SELF}
        <div class="list-group-item list-group-item-action d-flex align-items-center p-2 ps-3 border-bottom-0 border-start-0 border-end-0">
          <div class="form-check form-switch w-100">
            <input type="checkbox" class="form-check-input" id="hideUnowned" checked={m.hideUnowned} onchange={(e) => { st.setHideUnowned(e.currentTarget.checked); bump() }} {disabled}>
            <label class="form-check-label w-100" for="hideUnowned">{t('ui.hide_unowned')}</label>
          </div>
        </div>
      {/if}
    </div>
  </div>
  {/if}
</div>
