<!-- Port of common/input/url/url.component.html. Host <pngx-input-url> in the parent markup. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  import { safeHref } from '../../../../lib/safe-href'
  let {
    value = $bindable(),
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
    <div class:col-md-9={horizontal}>
      <div class="input-group" class:is-invalid={error}>
        <input type="url" class="form-control" class:is-invalid={error} placeholder="https://" id={inputId} bind:value onchange={() => onchange?.(value)} oninput={(e) => onchange?.(e.currentTarget.value)} {disabled}>
        <a class="btn btn-outline-secondary rounded-end" title={t('input.open_link')} href={safeHref(value)} target="_blank" rel="noopener noreferrer">
          <pngx-bs-icon width="1.2em" height="1.2em" name="box-arrow-up-right">{@html icon('box-arrow-up-right', '1.2em', '1.2em')}</pngx-bs-icon>
        </a>
        <div class="invalid-feedback position-absolute top-100">
          {error}
        </div>
      </div>
      {#if hint}
        <small class="form-text text-muted">{@html hint}</small>
      {/if}
    </div>
  </div>
</div>
