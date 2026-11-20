<!-- Port of common/permissions-select/permissions-select.component.html (global permissions as checkbox grid).
     Host <pngx-permissions-select> in the parent markup. Value: own permissions (code names); inherited group permissions only displayed. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Popover } from '../../../lib/popover.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import {
    SORTED_ACTIONS,
    allowedTypes as getAllowedTypes,
    checkedState,
    isActionSupported,
    isInherited,
    selectedPermissions,
    stripAppLabel,
    typeHasAllActions,
    type ActionKey,
  } from './permissions-select'

  let {
    value = $bindable(),
    title = 'Permissions',
    error = undefined,
    disabled = false,
    inheritedPermissions = [],
    onchange,
  }: { value?: string[] | null; title?: string; error?: string | string[] | null; disabled?: boolean; inheritedPermissions?: string[]; onchange?: (v: string[]) => void } = $props()

  const allowedTypes = getAllowedTypes(!!session.get(SETTINGS_KEYS.AUDITLOG_ENABLED))
  const inherited = $derived(stripAppLabel(inheritedPermissions))
  const grid = $derived(checkedState(allowedTypes, value ?? [], inherited))
  const info = new Popover({ autoClose: true })

  function emit(next: Record<string, Record<ActionKey, boolean>>) {
    value = selectedPermissions(next, inherited)
    onchange?.(value)
  }
  function toggle(type: string, action: ActionKey, checked: boolean) {
    const next = structuredClone($state.snapshot(grid)) as Record<string, Record<ActionKey, boolean>>
    next[type][action] = checked
    emit(next)
  }
  function toggleAll(e: Event, type: string) {
    const checked = (e.target as HTMLInputElement).checked
    const next = structuredClone($state.snapshot(grid)) as Record<string, Record<ActionKey, boolean>>
    for (const a of SORTED_ACTIONS) if (isActionSupported(type, a) && !(isInherited(inherited, type, a) || disabled)) next[type][a] = checked
    emit(next)
  }
</script>

<form class:opacity-50={disabled} onsubmit={(e) => e.preventDefault()}>
  <label class="form-label">
    {title}
    <button class="btn btn-sm btn-link text-muted me-auto p-0 p-md-2" title={t("common.whats")} type="button" use:info.triggerEl>
      <pngx-bs-icon name="question-circle">{@html icon('question-circle')}</pngx-bs-icon>
    </button>
  </label>
  {#if info.open}
    <ngb-popover-window role="tooltip" class="popover" id={info.id} use:info.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
      <p>{t('ui.global_permissions_define_what')}</p>
      <a href="rs/docs/permissions.md" target="_blank" referrerpolicy={'noopener noreferrer' as any}>{t('common.read_more')}</a>
      <pngx-bs-icon class="ms-1" width=".8em" height=".8em" name="box-arrow-up-right">{@html icon('box-arrow-up-right', '.8em', '.8em')}</pngx-bs-icon>
    </div></ngb-popover-window>
  {/if}
  <ul class="list-group">
    <li class="list-group-item d-flex">
      <div class="col-3">{t('ui.type')}</div>
      <div class="col">{t('common.all')}</div>
      <div class="col">{t('common.add')}</div>
      <div class="col">{t('ui.change')}</div>
      <div class="col">{t('common.delete')}</div>
      <div class="col">{t('common.view')}</div>
    </li>
    {#each allowedTypes as type (type)}
      <li class="list-group-item d-flex">
        <div class="col-3">{type}:</div>
        <div class="col form-check form-check-inline form-switch">
          <input type="checkbox" class="form-check-input" id="{type}_all" onchange={(e) => toggleAll(e, type)} checked={typeHasAllActions(grid[type], type) || isInherited(inherited, type)} disabled={disabled || isInherited(inherited, type) ? true : null}>
          <label class="form-check-label visually-hidden" for="{type}_all">{t('common.all')}</label>
        </div>
        {#each SORTED_ACTIONS as action (action)}
          <div class="col form-check form-check-inline" class:invisible={!isActionSupported(type, action)}>
            <input type="checkbox" class="form-check-input" id="{type}_{action}" checked={grid[type][action]} disabled={!isActionSupported(type, action) || isInherited(inherited, type, action) || disabled} onchange={(e) => toggle(type, action, (e.target as HTMLInputElement).checked)}>
            <label class="form-check-label visually-hidden" for="{type}_{action}">{action}</label>
          </div>
        {/each}
      </li>
    {/each}
    {#if error}
      <div class="invalid-feedback d-block">{error}</div>
    {/if}
  </ul>
</form>
