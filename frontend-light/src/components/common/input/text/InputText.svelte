<!-- Port of common/input/text/text.component.html. Host <pngx-input-text> in the parent markup. -->
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
    autocomplete = 'off',
    placeholder = '',
    suggestion = '',
    onchange,
    oninput,
    onremoved,
    onkeyup,
  }: {
    value?: string | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    autocomplete?: AutoFill | string
    placeholder?: string
    suggestion?: string
    onchange?: (v: string | null | undefined) => void
    /** on every keystroke (document detail: save immediately active, A-103) */
    oninput?: (v: string) => void
    onremoved?: () => void
    onkeyup?: (e: KeyboardEvent) => void
  } = $props()
  const inputId = newInputId()
  let inputField: HTMLInputElement | undefined = $state()
  export function focus() {
    inputField?.focus()
  }
  const getSuggestion = () => (value !== suggestion ? suggestion : '')
  function applySuggestion(e: Event) {
    e.preventDefault()
    value = suggestion
    onchange?.(value)
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
        <input bind:this={inputField} type="text" class="form-control" class:is-invalid={error} id={inputId} bind:value onchange={() => onchange?.(value)} oninput={(e) => oninput?.(e.currentTarget.value)} {onkeyup} {disabled} autocomplete={autocomplete as AutoFill} {placeholder}>
        {#if hint}
          <small class="form-text text-muted">{@html hint}</small>
        {/if}
        {#if getSuggestion()?.length > 0}
          <small>
            <span>{t('input.suggestion_label')}</span>&nbsp;
            <a onclick={applySuggestion} href="">{getSuggestion()}</a>&nbsp;
          </small>
        {/if}
        <div class="invalid-feedback position-absolute top-100">
          {error}
        </div>
      </div>
    </div>
  </div>
