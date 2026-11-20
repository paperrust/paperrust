<!-- Port of common/dates-dropdown/dates-dropdown.component.html. Logic: lib/filter-editor/dates.ts.
     Date selection popup (ngbDatepicker) = common/datepicker (loaded lazily); input via keyboard (yyyy-mm-dd). -->
<script lang="ts">
  import '../../../styles/components/dates-dropdown.css'
  import { onDestroy, untrack } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { formatDate } from '../../../lib/dates'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { DatesModel, getRelativeDates, isAllowedDateInputKey, type RelativeDateOption } from '../../../lib/filter-editor/dates'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import ClearableBadge from '../clearable-badge/ClearableBadge.svelte'
  import NgSelect from '../ng-select/NgSelect.svelte'
  import { loadDatepicker } from '../datepicker/lazy'

  let { model, title, placement = 'bottom-start', disabled = false, version = 0 }: { model: DatesModel; title: string; placement?: string; disabled?: boolean; version?: number } = $props()

  let v = $state(0)
  const bump = () => untrack(() => v++) // safe also from $effect (A-151)
  // svelte-ignore state_referenced_locally
  const off = model.onChange(() => bump())
  onDestroy(() => off())

  // svelte-ignore state_referenced_locally
  const dd = new Dropdown({ placement, pngxPopper: true })
  const dropdownMenuId = 'pngx-dates-dropdown-0'
  const relativeDates = getRelativeDates().map((r) => ({ ...r, name: t(r.name) }))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  // Placeholder like LocalizedDateParserFormatter (here: ISO pattern of the Angular default)
  const datePlaceHolder = 'yyyy-mm-dd'
  const m = $derived.by(() => {
    v
    version
    return { ...model, isActive: model.isActive }
  })
  const medium = (d: Date | number | undefined) => (d == null ? '' : formatDate(new Date(d), 'mediumDate', dateLocale))

  function keypress(e: KeyboardEvent) {
    if (!isAllowedDateInputKey(e.key)) e.preventDefault()
  }
  function set(field: 'createdDateFrom' | 'createdDateTo' | 'addedDateFrom' | 'addedDateTo', e: Event) {
    ;(model as any)[field] = (e.target as HTMLInputElement).value || null
    model.onChangeDebounce()
    bump()
  }
  type DateField = 'createdDateFrom' | 'createdDateTo' | 'addedDateFrom' | 'addedDateTo'
  // ngbDatepicker per input field (#createdDateFromPicker …), popup loaded lazily
  let pickerOpen = $state<DateField | null>(null)
  const inputs: Record<string, HTMLInputElement> = $state({})
  const today = new Date().toLocaleDateString('en-CA')
  function setValue(field: DateField, value: string | null) {
    ;(model as any)[field] = value
    model.onChangeDebounce()
    bump()
  }
  function reset() {
    model.reset()
    bump()
  }
</script>

{#snippet relOption(item: RelativeDateOption)}
  <div class="d-flex">
    {item.name}
    <span class="ms-auto text-muted small">
      {#if item.dateEnd}
        {medium(item.date)} &ndash; {medium(item.dateEnd)}
      {:else if item.dateTilNow}
        {medium(item.dateTilNow)} &ndash; {t('ui.now')}
      {:else}
        {medium(item.date)}
      {/if}
    </span>
  </div>
{/snippet}

{#snippet clearLink(label: string, onclick: () => void, dark: boolean)}
  <!-- svelte-ignore a11y_invalid_attribute -->
  <a class="text-light focus-variants" href="javascript:void(0)" {onclick} aria-label={label}>
    <pngx-bs-icon width="1em" height="1em" name="check" class="variant-unfocused" class:text-dark={dark}>{@html icon('check', '1em', '1em')}</pngx-bs-icon>
    <pngx-bs-icon width="1em" height="1em" name="x" class="variant-focused text-primary">{@html icon('x', '1em', '1em')}</pngx-bs-icon>
  </a>
{/snippet}

<div class="btn-group w-100 {dd.placementClass}" role="group" class:show={dd.open} use:dd.hostEl>
  <button class="dropdown-toggle btn btn-sm {m.createdDateTo || m.createdDateFrom ? 'btn-primary' : 'btn-outline-primary'}" id="dropdown{title}" {disabled} aria-label={title} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon width="1em" height="1em" name="calendar-event-fill">{@html icon('calendar-event-fill', '1em', '1em')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{title}</div>
    <pngx-clearable-badge><ClearableBadge selected={m.isActive} oncleared={reset} /></pngx-clearable-badge><span class="visually-hidden">selected</span>
  </button>
  {#if dd.open}
  <div id={dropdownMenuId} class="dropdown-menu date-dropdown shadow p-2 show" aria-labelledby="dropdown{title}" use:dd.menuEl>
    {#each [['created', t('common.created')], ['added', t('common.added')]] as [kind, heading]}
    <h6 class="dropdown-header border-bottom">{heading}</h6>
    <div class="list-group list-group-flush">
      <div class="list-group-item d-flex p-2 select-item" role="menuitem">
        <div class="selected-icon">
          {#if (m as any)[`${kind}RelativeDate`]}
            {@render clearLink(kind === 'created' ? 'Clear created relative date' : 'Clear added relative date', () => { kind === 'created' ? model.clearCreatedRelativeDate() : model.clearAddedRelativeDate(); bump() }, true)}
          {/if}
        </div>
        <div class="input-group input-group-sm ps-1 pe-2">
          <NgSelect class="w-100" name="{kind}RelativeDate"
          items={relativeDates} value={(m as any)[`${kind}RelativeDate`]}
          bindValue="id"
          bindLabel="name"
          clearable={false}
          placeholder={t('ui.relative_dates')}
          onchange={(val) => { kind === 'created' ? model.onSetCreatedRelativeDate(val) : model.onSetAddedRelativeDate(val); bump() }}
          option={relOption} />
        </div>
      </div>
      {#each [['From', 'DateFrom'], ['To', 'DateTo']] as [lbl, suffix]}
      {@const field = `${kind}${suffix}` as DateField}
      <div class="list-group-item d-flex p-2" role="menuitem">
        <div class="selected-icon">
          {#if (m as any)[`${kind}${suffix}`]}
            {@render clearLink(`Clear ${kind} ${lbl.toLowerCase()} date`, () => { (model as any)[`clear${kind === 'created' ? 'Created' : 'Added'}${lbl}`](); bump() }, false)}
          {/if}
        </div>
        <div class="input-group input-group-sm small ps-1 pe-2">
          <span class="input-group-text w-25 small text-muted">{t(lbl === 'From' ? 'common.from' : 'common.to')}</span>
          <input bind:this={inputs[field]} class="form-control small" placeholder={datePlaceHolder} onchange={(e) => set(field, e)} onkeypress={keypress}
            maxlength="10" value={(m as any)[field] ?? ''}>
          {#if pickerOpen === field}
            {#await loadDatepicker() then Dp}
              <Dp anchor={inputs[field]} value={(m as any)[field]} onselect={(_: unknown, iso: string) => setValue(field, iso)} onclose={() => (pickerOpen = null)} footer={pickerFooter} />
            {/await}
          {/if}
          {#snippet pickerFooter(close: () => void)}
            <div class="btn-group-xs border-top p-2 d-flex">
              <button class="btn btn-primary" onclick={() => setValue(field, today)}>{t('common.today')}</button>
              <button class="btn btn-secondary ms-auto" onclick={() => close()}>{t('common.close')}</button>
            </div>
          {/snippet}
          <button class="btn btn-outline-secondary" onclick={() => (pickerOpen = pickerOpen === field ? null : field)} type="button" aria-label="Open {kind} {lbl.toLowerCase()} date picker">
            <pngx-bs-icon width="1em" height="1em" name="calendar">{@html icon('calendar', '1em', '1em')}</pngx-bs-icon>
          </button>
        </div>
      </div>
      {/each}
    </div>
    {/each}
  </div>
  {/if}
</div>
