<!-- Port of common/input/file/file.component.html. Host <pngx-input-file> in the parent markup. Upload reports
     onupload(file); the value is the stored file name/path. -->
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
    onupload,
    onremoved,
    onchange,
  }: {
    value?: string | null
    title?: string
    disabled?: boolean
    error?: string | string[] | null
    hint?: string
    horizontal?: boolean
    removable?: boolean
    onupload?: (f: File) => void
    onremoved?: () => void
    onchange?: (v: string | null) => void
  } = $props()
  const inputId = newInputId()
  let file = $state<File | undefined>(undefined)
  let fileInput: HTMLInputElement
  const filename = $derived(value ? value.substring(value.lastIndexOf('/') + 1) : null)
  function onFile(e: Event) {
    file = (e.target as HTMLInputElement).files?.[0]
  }
  function uploadClicked() {
    if (file) onupload?.(file)
    clear()
  }
  function clear() {
    file = undefined
    if (fileInput) fileInput.value = ''
    value = null
    onchange?.(null)
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
        <div class="input-group" class:col-md-9={horizontal} class:is-invalid={error}>
          <input bind:this={fileInput} type="file" class="form-control" id={inputId} onchange={onFile} {disabled}>
          <button class="btn btn-outline-primary py-0" type="button" onclick={uploadClicked} disabled={disabled || !file}>{t('common.upload')}</button>
        </div>
        {#if filename}
            <div class="form-text d-flex align-items-center">
              <span class="text-muted">{filename}</span>
              <button type="button" class="btn btn-link btn-sm text-danger ms-2" onclick={clear}>
                <pngx-bs-icon name="x">{@html icon('x')}</pngx-bs-icon><small>{t('input.remove')}</small>
              </button>
            </div>
        {/if}
        <input type="hidden" class="form-control small" value={value ?? ''} disabled={true}>
        {#if hint}
            <small class="form-text text-muted">{@html hint}</small>
        {/if}
        <div class="invalid-feedback position-absolute top-100">
            {error}
        </div>
      </div>
    </div>
