<!-- Port of common/input/date/date.component.html. Host <pngx-input-date> in the parent markup.
     Value = ISO date (ISODateAdapter), displayed in the input format like LocalizedDateParserFormatter.
     Calendar popup (ngbDatepicker) = common/datepicker (loaded lazily), footer "Today"/"Close" like datePickerFooterTemplate. -->
<script lang="ts">
  import '../../../../styles/components/input-date.css'
  import { t, locale } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import { dateInputFormat, formatDateInput, isoFromModel, isoToModel, parseDateInput, type DateStruct } from '../../../../lib/date-input'
  import { newInputId } from '../input-id'
  import { loadDatepicker } from '../../datepicker/lazy'
  let {
    value = $bindable(),
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    removable = false,
    suggestions = undefined,
    showFilter = false,
    filterButtonTitle = undefined,
    onchange,
    onremoved,
    onfilterdocuments,
  }: {
    value?: string | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    suggestions?: string[]
    showFilter?: boolean
    filterButtonTitle?: string
    onchange?: (v: string | null | undefined) => void
    onremoved?: () => void
    onfilterdocuments?: (d: DateStruct[]) => void
  } = $props()
  const inputId = newInputId()
  const fmt = $derived(dateInputFormat(session.get(SETTINGS_KEYS.DATE_LOCALE), session.get(SETTINGS_KEYS.LANGUAGE), locale()))
  const placeholder = $derived(fmt)
  const display = $derived(formatDateInput(isoFromModel(value), fmt) ?? '')
  const today = new Date().toLocaleDateString('en-CA')
  let inputEl: HTMLInputElement | undefined = $state()
  let pickerOpen = $state(false)

  function set(v: string | null) {
    value = v
    onchange?.(value)
  }
  function onInput(e: Event) {
    // ngbDatepicker: valid input → ISO value, otherwise invalid (null)
    const text = (e.currentTarget as HTMLInputElement).value
    const d = parseDateInput(text, fmt)
    value = d ? isoToModel(d) : text ? value : null
  }
  function onChangeEvent(e: Event) {
    const text = (e.currentTarget as HTMLInputElement).value
    const d = parseDateInput(text, fmt)
    set(d ? isoToModel(d) : null)
  }
  function onKeyPress(e: KeyboardEvent) {
    if (e.key !== 'Enter' && !(e.altKey || e.metaKey || e.ctrlKey) && !/[0-9,\.\/-]+/.test(e.key)) e.preventDefault()
  }
  function onPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData('text')
    if (text == null) return
    e.preventDefault()
    const d = parseDateInput(text.replace(/[\sa-z#!$%\^&\*;:{}=\-_`~()]+/g, ''), fmt)
    if (d) set(isoToModel(d))
  }
  function openPicker() {
    pickerOpen = !pickerOpen
  }
  const getSuggestions = () =>
    (suggestions ?? [])
      .map((s) => parseDateInput(s, fmt))
      .filter((d) => value === null || value != isoToModel(d))
      .map((d) => formatDateInput(d, fmt))
  function onSuggestionClick(e: Event, s: string | null) {
    e.preventDefault()
    set(isoToModel(parseDateInput(s ?? '', fmt)))
  }
</script>

<div class="mb-3" class:pb-3={error}>
  <div class="row">
    <div class="d-flex align-items-center position-relative hidden-button-container" class:col-md-3={horizontal}>
      <label class="form-label" class:mb-md-0={horizontal} for={inputId}>{title}</label>
      {#if removable}
        <button type="button" class="btn btn-sm btn-danger position-absolute left-0" onclick={() => onremoved?.()}>
          <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('input.remove')}
          </button>
        {/if}
      </div>
      <div class="position-relative" class:col-md-9={horizontal}>
        <div class="input-group" class:is-invalid={error}>
          <input bind:this={inputEl} class="form-control" class:is-invalid={error} {placeholder} id={inputId} maxlength="10"
            onchange={onChangeEvent} oninput={onInput} onkeypress={onKeyPress} onpaste={onPaste}
            name="dp" value={display} {disabled}>
          {#if pickerOpen}
            {#await loadDatepicker() then Dp}
              <Dp anchor={inputEl} value={isoFromModel(value)} onselect={(d: DateStruct) => set(isoToModel(d))} onclose={() => (pickerOpen = false)} footer={datePickerFooterTemplate} />
            {/await}
          {/if}
          <button class="btn btn-outline-secondary calendar" onclick={openPicker} type="button" {disabled} aria-label={t('common.open_date_picker')}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="calendar">{@html icon('calendar', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
          {#snippet datePickerFooterTemplate(close: () => void)}
            <div class="btn-group-xs border-top p-2 d-flex">
              <button type="button" class="btn btn-primary" onclick={() => { set(today); close() }}>{t('common.today')}</button>
              <button type="button" class="btn btn-secondary ms-auto" onclick={() => close()}>{t('common.close')}</button>
            </div>
          {/snippet}
          {#if showFilter}
            <button class="btn btn-outline-secondary" type="button" onclick={() => onfilterdocuments?.([parseDateInput(display, fmt)].filter(Boolean) as DateStruct[])} disabled={value === null} title={filterButtonTitle ?? t('input.filter_documents', { PH: title })}>
              <pngx-bs-icon width="1.2em" height="1.2em" name="filter">{@html icon('filter', '1.2em', '1.2em')}</pngx-bs-icon>
            </button>
          {/if}
        </div>
        <div class="invalid-feedback position-absolute top-100">{t('input.invalid_date')}</div>
        {#if hint}
          <small class="form-text text-muted">{hint}</small>
        {/if}
        {#if getSuggestions().length > 0}
          <small>
            <span>{t('input.suggestions_label')}</span>&nbsp;
            {#each getSuggestions() as s}
              <a onclick={(e) => onSuggestionClick(e, s)} href="">{s}</a>&nbsp;
            {/each}
          </small>
        {/if}
      </div>
    </div>
  </div>
