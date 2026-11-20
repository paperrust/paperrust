<!-- Port of common/input/monetary/monetary.component.html. Host <pngx-input-monetary> in the parent markup. -->
<script lang="ts">
  import '../../../../styles/components/input-monetary.css'
  import { t, locale } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  import { formatCurrency, localeCurrencyCode, parseCurrencyCode, parseMonetaryValue } from './monetary'
  let {
    value = $bindable(),
    defaultCurrency = undefined,
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    removable = false,
    onchange,
    onremoved,
  }: {
    value?: string | null
    defaultCurrency?: string
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    onchange?: (v: any) => void
    onremoved?: () => void
  } = $props()
  const inputId = newInputId()
  const defaultCode = (() => defaultCurrency || localeCurrencyCode(locale()))()
  let currency = $state(parseCurrencyCode((() => value)(), defaultCode))
  let monetaryValue = $state(parseMonetaryValue((() => value)(), true))
  function setMonetary(v: any) {
    if (v || v?.toString() === '0') monetaryValue = v.toString()
  }
  function monetaryValueChange(fixed = false) {
    setMonetary(parseMonetaryValue(monetaryValue, fixed))
    if (monetaryValue === '0') monetaryValue = '0.00'
    value = currency + monetaryValue
    onchange?.(value)
  }
  function currencyChange() {
    if (currency.length) {
      currency = parseCurrencyCode(currency, defaultCode)
      value = currency + (monetaryValue?.toString() ?? '')
      onchange?.(value)
    }
  }
</script>

<div class="mb-3" class:pb-3={error}>
  <div class="row">
    <div class="d-flex align-items-center position-relative hidden-button-container" class:col-md-3={horizontal}>
      {#if title}
        <label class="form-label" class:mb-md-0={horizontal} for={inputId}>{title}</label>
      {/if}
      {#if removable}
        <button type="button" class="btn btn-sm btn-danger position-absolute left-0" onclick={() => onremoved?.()}>
          <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('input.remove')}
        </button>
      {/if}
    </div>
    <div class="position-relative" class:col-md-9={horizontal}>
      <div class="input-group" class:is-invalid={error}>
        <span class="input-group-text fw-bold bg-light">{formatCurrency(monetaryValue, currency, locale())}</span>
        <input class="form-control text-muted mw-60" bind:value={currency} oninput={currencyChange} maxlength="3" class:is-invalid={error} {disabled}>
        <input type="number" class="form-control text-muted" step=".01" id={inputId} value={monetaryValue} oninput={(e) => { setMonetary(e.currentTarget.value); monetaryValueChange() }} onchange={() => monetaryValueChange(true)} class:is-invalid={error} {disabled}>
      </div>
      <div class="invalid-feedback position-absolute top-100">
        {error}
      </div>
      {#if hint}
        <small class="form-text text-muted">{hint}</small>
      {/if}
    </div>
  </div>
</div>
