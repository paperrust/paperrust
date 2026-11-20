<!-- Port of common/custom-fields-query-dropdown (dropdown variant). Logic: lib/filter-editor/custom-field-queries.ts.
     Transition: document link values as text field. Date with fixed/relative switch (toggleRelativeDate, values like
     upstream e.g. "30 days ago", "in 2 months", "today"); calendar popup = common/datepicker (loaded lazily). -->
<script lang="ts">
  import '../../../styles/components/custom-fields-query-dropdown.css'
  import { onDestroy, tick, untrack } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import {
    CustomFieldsQueryDropdownState,
    CustomFieldQueryAtom,
    CustomFieldQueryExpression,
    type CustomFieldQueryElement,
  } from '../../../lib/filter-editor/custom-field-queries'
  import { CustomFieldQueryElementType, CustomFieldQueryOperator, CUSTOM_FIELD_QUERY_MAX_ATOMS, CUSTOM_FIELD_QUERY_MAX_DEPTH } from '../../../lib/custom-field-query'
  import ClearableBadge from '../clearable-badge/ClearableBadge.svelte'
  import NgSelect from '../ng-select/NgSelect.svelte'
  import { loadDatepicker } from '../datepicker/lazy'

  let { queryState: st, title, icon: iconName = 'ui-radios', disabled = false, version = 0, useDropdown = true }: { queryState: CustomFieldsQueryDropdownState; title: string; icon?: string; disabled?: boolean; version?: number; /** false: query editor without dropdown (workflow filter) */ useDropdown?: boolean } = $props()

  let v = $state(0)
  const bump = () => untrack(() => v++) // safe also from $effect (A-151)
  // svelte-ignore state_referenced_locally
  const offs = [st.onChange(() => bump()), st.selectionModel.onChange(() => bump())]
  onDestroy(() => offs.forEach((f) => f()))
  const dropdownMenuId = 'pngx-custom-fields-query-dropdown-0'
  let focusFirst = $state(false)
  const dd = new Dropdown({
    pngxPopper: true,
    onOpenChange: async (open) => {
      st.onOpenChange(open)
      focusFirst = open && st.shouldFocusFirstField
      bump()
      await tick()
      dd.update()
    },
  })
  const m = $derived.by(() => {
    v
    version
    return { queries: [...st.selectionModel.queries], isActive: st.isActive, fields: st.customFields }
  })
  // ngbDatepicker (#d) per date condition, popup loaded lazily
  let pickerAtom = $state<string | null>(null)
  const dateInputs: Record<string, HTMLInputElement> = $state({})
  const today = new Date().toLocaleDateString('en-CA')
  const isAtom =(e: CustomFieldQueryElement) => e.type === CustomFieldQueryElementType.Atom
  const dt = (atom: CustomFieldQueryAtom) => st.getCustomFieldByID(atom.field)?.data_type
  const change = (atom: CustomFieldQueryElement, key: 'field' | 'operator' | 'value', val: any) => {
    ;(atom as any)[key] = val
    bump()
  }
</script>

{#snippet trueFalse(atom: CustomFieldQueryAtom)}
  <select class="w-25 form-select rounded-end" value={atom.value} onchange={(e) => change(atom, 'value', e.currentTarget.value)} {disabled}>
    <option value="true">{t('ui.true')}</option>
    <option value="false">{t('ui.false')}</option>
  </select>
{/snippet}

{#snippet comparisonValue(atom: CustomFieldQueryAtom)}
  {#if dt(atom) === 'float' || dt(atom) === 'integer'}
    <input class="w-25 form-control rounded-end" type="number" value={atom.value} oninput={(e) => change(atom, 'value', e.currentTarget.value)} {disabled}>
  {:else if dt(atom) === 'boolean'}
    {@render trueFalse(atom)}
  {:else if dt(atom) === 'select'}
    <NgSelect class="paperless-input-select rounded-end" items={st.getSelectOptionsForField(atom.field)} bindLabel="label" bindValue="id" value={atom.value} {disabled} onchange={(val) => change(atom, 'value', val)} />
  {:else if dt(atom) === 'monetary'}
    <input class="w-25 form-control rounded-end" type="text" inputmode="decimal" value={atom.value} oninput={(e) => { st.setMonetaryValue(atom, e.currentTarget.value); bump() }} {disabled}>
  {:else if dt(atom) === 'date'}
    {@const relativeDate = st.getRelativeDate(atom)}
    <div class="d-flex gap-2 w-100 ms-0 mt-1">
      <fieldset class="btn-group btn-group-sm flex-shrink-0 date-mode" aria-label={t('filter.cf_query.date_mode')}>
        <input type="radio" class="btn-check" id="{atom.id}_fixed" name="{atom.id}_dateMode" checked={!relativeDate} onchange={() => { st.toggleRelativeDate(atom); bump() }} {disabled} aria-label={t('filter.cf_query.fixed_date')}>
        <label class="btn btn-outline-secondary text-nowrap" for="{atom.id}_fixed" title={t('filter.cf_query.fixed_date')}>
          <pngx-bs-icon name="calendar">{@html icon('calendar')}</pngx-bs-icon><span class="d-none d-md-inline ms-2">{t('filter.cf_query.fixed')}</span>
        </label>
        <input type="radio" class="btn-check" id="{atom.id}_relative" name="{atom.id}_dateMode" checked={!!relativeDate} onchange={() => { st.toggleRelativeDate(atom); bump() }} {disabled} aria-label={t('filter.cf_query.relative_date')}>
        <label class="btn btn-outline-secondary text-nowrap" for="{atom.id}_relative" title={t('filter.cf_query.relative_date')}>
          <pngx-bs-icon name="clock-history">{@html icon('clock-history')}</pngx-bs-icon><span class="d-none d-md-inline ms-2">{t('filter.cf_query.relative')}</span>
        </label>
      </fieldset>
      <div class="input-group input-group-sm flex-grow-1">
        {#if relativeDate}
          {#if relativeDate.unit.includes('{n}')}
            <input class="form-control w-25 flex-grow-0" type="number" min="0" aria-label={t('filter.cf_query.amount')} value={relativeDate.amount ?? ''}
              oninput={(e) => { const s = e.currentTarget.value; st.updateRelativeDate(atom, { amount: s === '' ? null : Number(s) }); bump() }} />
          {/if}
          <label class="visually-hidden" for="{atom.id}_unit">{t('filter.cf_query.unit')}</label>
          <select class="form-select" id="{atom.id}_unit" value={relativeDate.unit} onchange={(e) => { st.updateRelativeDate(atom, { unit: e.currentTarget.value }); bump() }}>
            {#each st.relativeDateUnits as unit (unit.id)}
              <option value={unit.id}>{t(unit.label)}</option>
            {/each}
          </select>
        {:else}
        <input bind:this={dateInputs[atom.id]} class="form-control" placeholder="yyyy-mm-dd" value={atom.value ?? ''} onchange={(e) => change(atom, 'value', e.currentTarget.value)} />
        {#if pickerAtom === atom.id}
          {#await loadDatepicker() then Dp}
            <Dp anchor={dateInputs[atom.id]} value={atom.value as string} onselect={(_: unknown, iso: string) => change(atom, 'value', iso)} onclose={() => (pickerAtom = null)} footer={datePickerFooterTemplate} />
          {/await}
        {/if}
        <button class="btn btn-sm btn-outline-secondary" onclick={() => (pickerAtom = pickerAtom === atom.id ? null : atom.id)} type="button" aria-label={t('common.open_date_picker')}>
          <pngx-bs-icon name="calendar-event">{@html icon('calendar-event')}</pngx-bs-icon>
        </button>
        {#snippet datePickerFooterTemplate(close: () => void)}
          <div class="btn-group-xs border-top p-2 d-flex">
            <button type="button" class="btn btn-primary" onclick={() => { change(atom, 'value', today); close() }}>{t('common.today')}</button>
            <button type="button" class="btn btn-secondary ms-auto" onclick={() => close()}>{t('common.close')}</button>
          </div>
        {/snippet}
        {/if}
      </div>
    </div>
  {:else}
    <input class="w-25 form-control rounded-end" type="text" value={atom.value ?? ''} oninput={(e) => change(atom, 'value', e.currentTarget.value)} {disabled}>
  {/if}
{/snippet}

{#snippet queryAtom(atom: CustomFieldQueryAtom)}
  <label class="visually-hidden" for="{atom.id}_operator">{t('ui.operator')}</label>
  <div class="input-group input-group-sm">
    <NgSelect
      class="paperless-input-select"
      items={m.fields}
      value={atom.field}
      {disabled}
      bindLabel="name"
      bindValue="id"
      autofocus={focusFirst}
      onchange={(val) => change(atom, 'field', val)}
    />
    <select class="w-25 form-select" id="{atom.id}_operator" class:rounded-end={st.hasDateValueRow(atom)} value={atom.operator} onchange={(e) => change(atom, 'operator', e.currentTarget.value)} {disabled}>
      {#each st.getOperatorsForField(atom.field) as operator (operator.label)}
        <option value={operator.value}>{t(operator.label)}</option>
      {/each}
    </select>
    {#if atom.operator === CustomFieldQueryOperator.Exists || atom.operator === CustomFieldQueryOperator.IsNull}
      {@render trueFalse(atom)}
    {:else if [CustomFieldQueryOperator.GreaterThanOrEqual, CustomFieldQueryOperator.LessThanOrEqual, CustomFieldQueryOperator.GreaterThan, CustomFieldQueryOperator.LessThan, CustomFieldQueryOperator.Exact].includes(atom.operator as any)}
      {@render comparisonValue(atom)}
    {:else if atom.operator === CustomFieldQueryOperator.In}
      <NgSelect class="paperless-input-select rounded-end" items={st.getSelectOptionsForField(atom.field)} bindLabel="label" bindValue="id" value={atom.value} {disabled} multiple onchange={(val) => change(atom, 'value', val)} />
    {:else}
      <input class="w-25 form-control rounded-end" type="text" value={atom.value ?? ''} oninput={(e) => change(atom, 'value', e.currentTarget.value)} {disabled}>
    {/if}
  </div>
  <button class="btn btn-link btn-sm text-danger pe-0 align-self-center" type="button" onclick={() => { st.removeElement(atom); bump(); if (st.selectionModel.isEmpty()) dd.close() }} {disabled} aria-label={t('ui.remove_query')}>
    <pngx-bs-icon name="x-circle">{@html icon('x-circle')}</pngx-bs-icon>
  </button>
{/snippet}

{#snippet queryExpression(expression: CustomFieldQueryExpression)}
  <div class="d-flex w-100">
    <div class="d-flex flex-grow-1 flex-column">
      <div class="btn-group btn-group-xs" role="group">
        <input checked={expression.operator === 'OR'} onchange={() => change(expression, 'operator', 'OR')} type="radio" class="btn-check" id="logicalOperatorOr_{expression.id}" name="logicalOperatorOr_{expression.id}" value="OR" disabled={expression.depth > 0 && (expression.value as any[]).length < 2}>
        <label class="btn btn-outline-primary" for="logicalOperatorOr_{expression.id}">{t('ui.any')}</label>
        <input checked={expression.operator === 'AND'} onchange={() => change(expression, 'operator', 'AND')} type="radio" class="btn-check" id="logicalOperatorAnd_{expression.id}" name="logicalOperatorAnd_{expression.id}" value="AND" disabled={expression.depth > 0 && (expression.value as any[]).length < 2}>
        <label class="btn btn-outline-primary" for="logicalOperatorAnd_{expression.id}">{t('common.all')}</label>
        {#if expression.negatable}
          <input checked={expression.operator === 'NOT'} onchange={() => change(expression, 'operator', 'NOT')} type="radio" class="btn-check" id="logicalOperatorNot_{expression.id}" name="logicalOperatorNot_{expression.id}" value="NOT">
          <label class="btn btn-outline-secondary" for="logicalOperatorNot_{expression.id}">{t('ui.not')}</label>
        {/if}
      </div>
      <div class="list-group list-group-flush mb-n2">
        {#each expression.value as CustomFieldQueryElement[] as element (element.id)}
          <div class="list-group-item px-0 d-flex flex-nowrap">
            {#if isAtom(element)}
              {@render queryAtom(element as CustomFieldQueryAtom)}
            {:else}
              {@render queryExpression(element as CustomFieldQueryExpression)}
            {/if}
          </div>
        {/each}
      </div>
    </div>
    <div class="btn-group-vertical ms-2 ps-2 border-start" role="group" aria-label="Vertical button group">
      <button type="button" class="btn btn-sm btn-outline-secondary text-primary" title={t('ui.add_query')} onclick={() => { st.addAtom(expression); bump() }} disabled={disabled || (expression.value as any[]).length === CUSTOM_FIELD_QUERY_MAX_ATOMS}>
        <pngx-bs-icon name="node-plus">{@html icon('node-plus')}</pngx-bs-icon>
      </button>
      <button type="button" class="btn btn-sm btn-outline-secondary text-primary" title={t('ui.add_expression')} onclick={() => { st.addExpression(expression); bump() }} disabled={disabled || expression.depth === CUSTOM_FIELD_QUERY_MAX_DEPTH}>
        <pngx-bs-icon name="braces">{@html icon('braces')}</pngx-bs-icon>
      </button>
      {#if expression.depth > 0}
        <button type="button" class="btn btn-sm btn-outline-secondary text-danger" onclick={() => { st.removeElement(expression); bump() }} {disabled} aria-label={t('ui.remove_expression')}>
          <pngx-bs-icon name="x-circle">{@html icon('x-circle')}</pngx-bs-icon>
        </button>
      {/if}
    </div>
  </div>
{/snippet}

{#snippet list()}
    <div class="list-group list-group-flush">
      {#each m.queries as element (element.id)}
        <div class="list-group-item px-0 d-flex flex-nowrap">
          {#if isAtom(element)}
            {@render queryAtom(element as CustomFieldQueryAtom)}
          {:else}
            {@render queryExpression(element as CustomFieldQueryExpression)}
          {/if}
        </div>
      {/each}
    </div>
{/snippet}

{#if useDropdown}
<div class="btn-group w-100 {dd.placementClass}" role="group" class:show={dd.open} use:dd.hostEl>
  <button class="dropdown-toggle btn btn-sm {m.isActive ? 'btn-primary' : 'btn-outline-primary'}" id="dropdown_toggle" {disabled} aria-label={title} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon>{@html icon(iconName)}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{title}</div>
    {#if m.isActive}
      <pngx-clearable-badge><ClearableBadge selected={m.isActive} oncleared={() => { st.reset(); bump() }} /></pngx-clearable-badge>
    {/if}
  </button>
  {#if dd.open}
  <div id={dropdownMenuId} class="dropdown-menu px-3 shadow show" aria-labelledby="dropdown_{st.name}" use:dd.menuEl>
    <div class="list-group list-group-flush">
      {#each m.queries as element (element.id)}
        <div class="list-group-item px-0 d-flex flex-nowrap">
          {#if isAtom(element)}
            {@render queryAtom(element as CustomFieldQueryAtom)}
          {:else}
            {@render queryExpression(element as CustomFieldQueryExpression)}
          {/if}
        </div>
      {/each}
    </div>
  </div>
  {/if}
</div>
{:else}
  {@render list()}
{/if}
