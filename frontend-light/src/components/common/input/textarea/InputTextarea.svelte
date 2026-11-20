<!-- Port of common/input/textarea/textarea.component.html. Host <pngx-input-textarea> in the parent markup. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  let {
    value = $bindable(),
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    removable = false,
    placeholder = '',
    monospace = false,
    onchange,
    oninput,
    onremoved,
  }: {
    value?: string | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    placeholder?: string
    monospace?: boolean
    onchange?: (v: any) => void
    /** on every keystroke (A-103) */
    oninput?: (v: string) => void
    onremoved?: () => void
  } = $props()
  const inputId = newInputId()
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
        <textarea
          id={inputId}
          class="form-control"
          class:is-invalid={error}
          class:font-monospace={monospace}
          bind:value
          onchange={() => onchange?.(value)}
          oninput={(e) => oninput?.(e.currentTarget.value)}
          {disabled}
          {placeholder}
          rows="4"></textarea>
        {#if hint}
          <small class="form-text text-muted">{@html hint}</small>
        {/if}
        <div class="invalid-feedback position-absolute top-100">
          {error}
        </div>
      </div>
    </div>
  </div>
