<!-- Port of common/input/select/select.component.html. Host <pngx-input-select> in the parent markup.
     "Create new" (createNew) is reported by the component via oncreatenew; the parent component decides. -->
<script lang="ts">
  import '../../../../styles/components/input-select.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import NgSelect from '../../ng-select/NgSelect.svelte'
  import { newInputId } from '../input-id'
  import { withPrivateItems } from './select'
  let {
    value = $bindable(),
    items = [],
    title = '',
    disabled = false,
    error = undefined,
    hint = undefined,
    horizontal = false,
    removable = false,
    allowNull = false,
    suggestions = undefined,
    placeholder = undefined,
    multiple = false,
    bindLabel = 'name',
    showFilter = false,
    notFoundText = undefined,
    hideAddButton = false,
    disableCreateNew = false,
    iconField = undefined,
    onchange,
    onremoved,
    oncreatenew,
    onfilterdocuments,
  }: {
    value?: any
    items?: any[] | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    allowNull?: boolean
    suggestions?: number[]
    placeholder?: string
    multiple?: boolean
    bindLabel?: string
    showFilter?: boolean
    notFoundText?: string
    hideAddButton?: boolean
    disableCreateNew?: boolean
    /** Field with icon name per entry (saved views); icon must be registered (lib/route-icons) */
    iconField?: string
    onchange?: (v: any) => void
    onremoved?: () => void
    oncreatenew?: (name: string | null) => void
    onfilterdocuments?: (items: any[]) => void
  } = $props()
  const inputId = newInputId()
  const allItems = $derived(withPrivateItems(items ?? [], value, t('common.private')))
  const allowCreateNew = $derived(!disableCreateNew && !!oncreatenew)
  const isPrivate = $derived(allItems.find((i) => i.id === value)?.private)
  const getSuggestions = () => (suggestions && items ? suggestions.filter((id) => id != value).map((id) => allItems.find((i) => i.id == id)) : [])
  function changed(v: any) {
    value = v
    onchange?.(value)
  }
</script>

<div class="mb-3 paperless-input-select" class:disabled>
  <div class="row">
    {#if title || removable}
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
    {/if}
    <div class:col-md-9={horizontal}>
      <div class:input-group={allowCreateNew || showFilter} class:is-invalid={error}>
        <NgSelect name="inputId" {value} onchange={changed}
          {disabled}
          class={isPrivate ? 'private' : ''}
          clearable={allowNull}
          items={allItems}
          taggable={allowCreateNew}
          {placeholder}
          {notFoundText}
          {multiple}
          {bindLabel}
          bindValue="id">
          {#snippet label(item, clear)}
            {#if multiple && !disabled}
              <span class="ng-value-icon left" role="button" tabindex="0" onclick={clear} onkeydown={(e) => e.key === 'Enter' && clear()} aria-label={t('input.remove_item')}>×</span>
            {/if}
            <span class="ng-value-label" title={item[bindLabel]}>
              {#if iconField && item[iconField]}
                <pngx-bs-icon class="me-2" name={item[iconField]}>{@html icon(item[iconField])}</pngx-bs-icon>
              {/if}
              {item[bindLabel]}
            </span>
          {/snippet}
          {#snippet option(item)}
              {#if iconField && item[iconField]}
                <pngx-bs-icon class="me-2" name={item[iconField]}>{@html icon(item[iconField])}</pngx-bs-icon>
              {/if}
              <span title={item[bindLabel]}>{item[bindLabel]}</span>
          {/snippet}
        </NgSelect>
        {#if allowCreateNew && !hideAddButton}
          <button class="btn btn-outline-secondary" type="button" onclick={() => oncreatenew?.(null)} {disabled} aria-label={t('input.create_new_item')}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="plus">{@html icon('plus', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
        {/if}
        {#if showFilter}
          <button class="btn btn-outline-secondary" type="button" onclick={() => onfilterdocuments?.([allItems.find((i) => i.id === value)])} disabled={isPrivate || value === null} title={t('input.filter_documents', { PH: title })}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="filter">{@html icon('filter', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
        {/if}
      </div>
      <div class="invalid-feedback">
        {error}
      </div>
      {#if hint}
        <small class="form-text text-muted">{hint}</small>
      {/if}
      {#if getSuggestions().length > 0}
        <small>
          <span>{t('input.suggestions_label')}</span>&nbsp;
          {#each getSuggestions() as s}
            <a onclick={(e) => { e.preventDefault(); changed(s.id) }} href="">{s.name}</a>&nbsp;
          {/each}
        </small>
      {/if}
    </div>
  </div>
</div>
