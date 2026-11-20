<!-- Port of common/input/check/check.component.html. Host <pngx-input-check> in the parent markup. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  let {
    value = $bindable(),
    title = '',
    disabled = false,
    hint = undefined,
    horizontal = false,
    removable = false,
    onchange,
    onremoved,
  }: {
    value?: boolean | null
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
</script>

<div class="mb-3">
  <div class="row">
    {#if horizontal}
      <div class="d-flex align-items-center position-relative hidden-button-container col-md-3">
        <label class="form-label" class:mb-md-0={horizontal} for={inputId}>{title}</label>
        {#if removable}
          <button type="button" class="btn btn-sm btn-danger position-absolute left-0" onclick={() => onremoved?.()}>
            <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('input.remove')}
            </button>
          {/if}
        </div>
      {/if}
      <div class:col-md-9={horizontal} class:align-items-center={horizontal} class:d-flex={horizontal}>
        <div class="form-check">
          <input type="checkbox" class="form-check-input" id={inputId} bind:checked={value} onchange={() => onchange?.(value)} {disabled}>
          {#if !horizontal}
            <label class="form-check-label" for={inputId}>{title}</label>
          {/if}
          {#if hint}
            <div class="form-text text-muted">{hint}</div>
          {/if}
        </div>
      </div>
    </div>
  </div>
