<!-- Port of common/input/entries/entries.component.html (key/value list, e.g. webhook parameters).
     Host <pngx-input-entries> in the parent markup. Value: object; empty keys are removed when reporting. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  import { entriesFromValue, valueFromEntries } from './entries'

  let {
    value = $bindable(),
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    onchange,
  }: {
    value?: Record<string, string> | null
    title?: string
    disabled?: boolean
    error?: string | string[] | null
    hint?: string
    horizontal?: boolean
    onchange?: (v: Record<string, string>) => void
  } = $props()
  const inputId = newInputId()
  // writeValue: entries once from the value (editing afterwards local as in Angular)
  // svelte-ignore state_referenced_locally
  let entries = $state<[string, string][]>(entriesFromValue(value))

  function inputChange() {
    value = valueFromEntries(entries)
    onchange?.(value)
  }
  function addEntry() {
    entries.push(['', ''])
    inputChange()
  }
  function removeEntry(i: number) {
    entries.splice(i, 1)
    inputChange()
  }
</script>

<div class="mb-3" class:pb-3={error}>
  <div class="row">
    <div class="d-flex align-items-center mb-2">
      {#if title}
        <label class="form-label mb-0" class:mb-md-0={horizontal} for={inputId}>{title}</label>
      {/if}
      <button type="button" class="btn btn-sm btn-outline-primary ms-4" onclick={addEntry}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('common.add')}
      </button>
    </div>
    <div class="position-relative">
      {#each entries as entry, i (i)}
        <div class="input-group mb-3">
          <input type="text" class="form-control" bind:value={entry[0]} onchange={inputChange} {disabled} autocomplete="off">
          <input type="text" class="form-control" bind:value={entry[1]} onchange={inputChange} {disabled} autocomplete="off">
          <button type="button" class="btn btn-outline-secondary" onclick={() => removeEntry(i)} aria-label={t('input.remove_entry')}>
            <pngx-bs-icon class="text-danger" name="trash">{@html icon('trash')}</pngx-bs-icon>
          </button>
        </div>
      {/each}
      {#if hint}
        <small class="form-text text-muted">{@html hint}</small>
      {/if}
      <div class="invalid-feedback position-absolute top-100">
        {error}
      </div>
    </div>
  </div>
</div>
