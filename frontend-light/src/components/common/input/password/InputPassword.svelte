<!-- Port of common/input/password/password.component.html. Host <pngx-input-password> in the parent markup. -->
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
    showReveal = false,
    disableRevealToggle = false,
    autocomplete = undefined,
    onchange,
    onfocus,
    onfocusout,
  }: {
    value?: string | null
    title?: string
    disabled?: boolean
    error?: string | string[] | null
    hint?: string
    horizontal?: boolean
    showReveal?: boolean
    disableRevealToggle?: boolean
    autocomplete?: any
    onchange?: (v: string | null | undefined) => void
    onfocus?: () => void
    onfocusout?: () => void
  } = $props()
  const inputId = newInputId()
  let textVisible = $state(false)
</script>

<div class="mb-3" class:pb-3={error}>
  <div class="row">
    <div class="d-flex align-items-center position-relative hidden-button-container" class:col-md-3={horizontal}>
      {#if title}
        <label class="form-label" class:mb-md-0={horizontal} for={inputId}>{title}</label>
      {/if}
    </div>
  <div class="position-relative" class:col-md-9={horizontal}>
    <div class="input-group" class:is-invalid={error}>
      <input type={showReveal && textVisible ? 'text' : 'password'} class="form-control" class:is-invalid={error} id={inputId} bind:value onfocus={() => onfocus?.()} onfocusout={() => onfocusout?.()} onchange={() => onchange?.(value)} {disabled} {autocomplete}>
      {#if showReveal}
        <button type="button" class="btn btn-outline-secondary" onclick={() => (textVisible = !textVisible)} title={t('input.show_password')} disabled={disabled || disableRevealToggle}>
          <pngx-bs-icon name="eye">{@html icon('eye')}</pngx-bs-icon>
        </button>
      {/if}
    </div>
    <div class="invalid-feedback">
      {error}
    </div>
    {#if hint}
      <small class="form-text text-muted">{@html hint}</small>
    {/if}
  </div>
</div>
</div>
