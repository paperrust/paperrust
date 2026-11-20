<!-- Port of common/suggestions-dropdown/suggestions-dropdown.component.html. Host <pngx-suggestions-dropdown> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/suggestions-dropdown.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import type { DocumentSuggestions } from '../../../api/v1'
  import { fieldSuggestions as countField, novelSuggestions as countNovel, noSuggestions as isEmpty, type Applied, type SuggestionSource } from './suggestions'
  let {
    suggestions = null,
    aiEnabled = false,
    loading = false,
    disabled = false,
    source = 'ml',
    defaultSource = 'ml',
    fetchedSources = [],
    applied = {},
    ongetsuggestions,
    onsourcechange,
    onaddtag,
    onadddocumenttype,
    onaddcorrespondent,
  }: {
    suggestions?: DocumentSuggestions | null
    aiEnabled?: boolean
    loading?: boolean
    disabled?: boolean
    source?: SuggestionSource
    defaultSource?: SuggestionSource
    fetchedSources?: SuggestionSource[]
    applied?: Applied
    ongetsuggestions?: (s: SuggestionSource) => void
    onsourcechange?: (s: SuggestionSource) => void
    onaddtag?: (name: string) => void
    onadddocumenttype?: (name: string) => void
    onaddcorrespondent?: (name: string) => void
  } = $props()
  const dd = new Dropdown({ pngxPopper: true })
  const ddOpts = new Dropdown({ pngxPopper: true, autoClose: 'outside', placement: 'bottom-end' })
  const novel = $derived(countNovel(suggestions))
  const field = $derived(countField(suggestions, applied))
  const total = $derived(novel + field)
  const none = $derived(isEmpty(suggestions, loading))
  const useML = $derived(source !== 'ai')
  const useAI = $derived(source !== 'ml')
  const hasUnfetched = $derived((useML && !fetchedSources.includes('ml')) || (useAI && !fetchedSources.includes('ai')))

  function clickSuggest() {
    if (disabled || loading) return
    if (!suggestions) ongetsuggestions?.(source)
    else if (hasUnfetched) {
      ongetsuggestions?.(source)
      dd.openMenu()
    } else dd.toggle()
  }
  function setSources(ml: boolean, ai: boolean) {
    if (ml && ai) onsourcechange?.('both')
    else if (ml) onsourcechange?.('ml')
    else if (ai) onsourcechange?.('ai')
  }
</script>

<div class="d-flex align-items-center">
  <div class="btn-group">
    <button type="button" class="btn btn-sm btn-outline-primary" onclick={clickSuggest} disabled={disabled || loading} aria-label={none ? 'No suggestions' : 'Suggest'}>
      {#if loading}
        <div class="spinner-border spinner-border-sm" role="status"></div>
      {:else if none}
        <pngx-bs-icon width="1.2em" height="1.2em" name="check-circle">{@html icon('check-circle', '1.2em', '1.2em')}</pngx-bs-icon>
      {:else}
        <pngx-bs-icon width="1.2em" height="1.2em" name="lightbulb">{@html icon('lightbulb', '1.2em', '1.2em')}</pngx-bs-icon>
      {/if}
      {#if none}
        <span class="d-none d-lg-inline ps-1">{t('ui.no_suggestions')}</span>
      {:else}
        <span class="d-none d-lg-inline ps-1">{t('ui.suggest')}</span>
      {/if}
      {#if total > 0}
        <span class="badge bg-primary ms-2">{total}</span>
      {/if}
    </button>

    <div class="btn-group {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
      <button type="button" class="btn btn-sm btn-outline-primary dropdown-toggle" disabled={disabled || loading || !suggestions} aria-expanded={dd.open} aria-controls="suggestionsDropdown" aria-label="Suggestions dropdown" use:dd.anchorEl onclick={dd.toggle}>
        <span class="visually-hidden">{t('ui.show_suggestions')}</span>
      </button>

      {#if dd.open}
      <div aria-labelledby="suggestionsDropdown" class="dropdown-menu shadow suggestions-dropdown show" use:dd.menuEl>
        <div class="list-group list-group-flush small pb-0">
          {#if novel === 0 && field === 0}
            <div class="list-group-item text-muted fst-italic">
              <small class="text-muted small fst-italic">{t('ui.no_novel_suggestions')}</small>
            </div>
          {/if}
          {#if (suggestions?.suggested_tags?.length ?? 0) > 0}
            <small class="list-group-item text-uppercase text-muted small"><pngx-bs-icon class="me-2" name="tags">{@html icon('tags')}</pngx-bs-icon>{t('common.tags')}</small>
            {#each suggestions?.suggested_tags ?? [] as tag (tag)}
              <button type="button" class="list-group-item list-group-item-action bg-light" onclick={() => onaddtag?.(tag)}>{tag}</button>
            {/each}
          {/if}
          {#if (suggestions?.suggested_document_types?.length ?? 0) > 0}
            <div class="list-group-item text-uppercase text-muted small"><pngx-bs-icon class="me-2" name="hash">{@html icon('hash')}</pngx-bs-icon>{t('common.document_types')}</div>
            {#each suggestions?.suggested_document_types ?? [] as type (type)}
              <button type="button" class="list-group-item list-group-item-action bg-light" onclick={() => onadddocumenttype?.(type)}>{type}</button>
            {/each}
          {/if}
          {#if (suggestions?.suggested_correspondents?.length ?? 0) > 0}
            <div class="list-group-item text-uppercase text-muted small"><pngx-bs-icon class="me-2" name="person">{@html icon('person')}</pngx-bs-icon>{t('common.correspondents')}</div>
            {#each suggestions?.suggested_correspondents ?? [] as c (c)}
              <button type="button" class="list-group-item list-group-item-action bg-light" onclick={() => onaddcorrespondent?.(c)}>{c}</button>
            {/each}
          {/if}
          {#if field > 0}
            <div class="list-group-item text-muted fst-italic">
              <small class="text-muted small fst-italic">{t('ui.suggestions_available_below', { VAR_PLURAL: field, INTERPOLATION: field })}</small>
            </div>
          {/if}
        </div>
      </div>
      {/if}
    </div>
  </div>

  {#if aiEnabled}
    <div class={ddOpts.placementClass} class:show={ddOpts.open} use:ddOpts.hostEl>
      <button type="button" class="btn btn-sm btn-link position-relative dropdown-toggle" disabled={disabled || loading} title={t('ui.suggestion_options')} aria-expanded={ddOpts.open} use:ddOpts.anchorEl onclick={ddOpts.toggle}>
        <pngx-bs-icon name="three-dots">{@html icon('three-dots')}</pngx-bs-icon>
        {#if source !== defaultSource}
          <span class="position-absolute top-0 start-100 translate-middle p-1 bg-primary border border-light rounded-circle">
            <span class="visually-hidden">{t('ui.not_using_default')}</span>
          </span>
        {/if}
      </button>
      {#if ddOpts.open}
      <div class="dropdown-menu shadow p-3 show" use:ddOpts.menuEl>
        <div class="small text-muted mb-2">{t('ui.suggest_using_label')}</div>
        <div class="form-check small">
          <input class="form-check-input" type="checkbox" id="suggestionSourceML" checked={useML} disabled={useML && !useAI} onchange={(e) => setSources(e.currentTarget.checked, useAI)}>
          <label class="form-check-label d-inline-flex align-items-center gap-1" for="suggestionSourceML"><pngx-bs-icon name="cpu">{@html icon('cpu')}</pngx-bs-icon>{t('ui.ml')}</label>
        </div>
        <div class="form-check small">
          <input class="form-check-input" type="checkbox" id="suggestionSourceAI" checked={useAI} disabled={useAI && !useML} onchange={(e) => setSources(useML, e.currentTarget.checked)}>
          <label class="form-check-label d-inline-flex align-items-center gap-1" for="suggestionSourceAI"><pngx-bs-icon name="stars">{@html icon('stars')}</pngx-bs-icon>{t('common.ai')}</label>
        </div>
      </div>
      {/if}
    </div>
  {/if}
</div>
