<!-- Port of common/input/number/number.component.html. Host <pngx-input-number> in the parent markup. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { api } from '../../../../api/client'
  import { newInputId } from '../input-id'
  import { normalizeNumber } from './number'
  let {
    value = $bindable(),
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    removable = false,
    showAdd = true,
    step = 1,
    onchange,
    onremoved,
  }: {
    value?: number | string | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    showAdd?: boolean
    step?: number
    onchange?: (v: any) => void
    onremoved?: () => void
  } = $props()
  const inputId = newInputId()
  function changed() {
    onchange?.(normalizeNumber(value, step))
  }
  function nextAsn() {
    if (value) return
    api.getNextAsn().then((n) => {
      value = n
      changed()
    })
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
          <input type="number" class="form-control" {step} id={inputId} bind:value onchange={changed} class:is-invalid={error} {disabled}>
          {#if showAdd}
            <button class="btn btn-outline-secondary" type="button" id="button-addon1" onclick={nextAsn} {disabled}>+1</button>
          {/if}
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
