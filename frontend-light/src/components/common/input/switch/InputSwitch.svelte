<!-- Port of common/input/switch/switch.component.html (without tooltip "not set yet"). Host <pngx-input-switch>
     in the parent markup. -->
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
    compact = false,
    removable = false,
    showUnsetNote = false,
    onchange,
    onremoved,
  }: {
    value?: boolean | null
    title?: string
    disabled?: boolean
    hint?: string | null
    horizontal?: boolean
    compact?: boolean
    removable?: boolean
    /** Configuration: hint as long as no value is set (tooltip text only in the classic UI) */
    showUnsetNote?: boolean
    onchange?: (v: boolean) => void
    onremoved?: () => void
  } = $props()
  const inputId = newInputId()
  const isUnset = $derived(value === null || value === undefined)
</script>

<div class:mb-3={!compact}>
  <div class:row={!compact}>
    {#if !horizontal && !compact}
      <div class="d-flex align-items-center position-relative hidden-button-container col-md-3">
        <label class="form-label" for={inputId}>
          {title}
          {#if showUnsetNote && isUnset}
            <pngx-bs-icon width="0.9em" height="0.9em" name="exclamation-triangle">{@html icon('exclamation-triangle', '0.9em', '0.9em')}</pngx-bs-icon>
          {/if}
        </label>
        {#if removable}
          <button type="button" class="btn btn-sm btn-danger position-absolute left-0" onclick={() => onremoved?.()}>
            <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('input.remove')}
          </button>
        {/if}
      </div>
    {/if}
    <div class:align-items-center={horizontal} class:d-flex={horizontal}>
      <div class="form-check form-switch">
        <input type="checkbox" class="form-check-input" id={inputId} bind:checked={value} onchange={() => onchange?.(!!value)} {disabled} aria-label={compact ? title : null}>
        {#if horizontal && !compact}
          <label class="form-check-label" class:text-muted={showUnsetNote && isUnset} for={inputId}>
            {title}
            {#if showUnsetNote && isUnset}
              <pngx-bs-icon class="ms-1" width="0.9em" height="0.9em" name="exclamation-triangle">{@html icon('exclamation-triangle', '0.9em', '0.9em')}</pngx-bs-icon>
            {/if}
          </label>
        {/if}
        {#if hint}
          <div class="form-text text-muted">{hint}</div>
        {/if}
      </div>
    </div>
  </div>
</div>
