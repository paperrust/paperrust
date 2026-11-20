<!-- Port of common/input/document-link/document-link.component.html (non-minimal). Host
     <pngx-input-document-link> in the parent markup. Search: title contains search text (from 2 characters), like Angular. -->
<script lang="ts">
  import '../../../../styles/components/input-document-link.css'
  import '../../../../styles/parts/select.css'
  import { t, locale } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { formatDate } from '../../../../lib/dates'
  import { api } from '../../../../api/client'
  import { queryString, type Paged } from '../../../../api/v1'
  import { router } from '../../../../router.svelte'
  import { newInputId } from '../input-id'
  type Doc = { id: number; title?: string; created?: string }
  let {
    value = $bindable(),
    title = '',
    disabled = false,
    hint = undefined,
    horizontal = false,
    removable = false,
    parentDocumentID = undefined,
    onchange,
    onremoved,
  }: {
    value?: number[] | null
    title?: string
    disabled?: boolean
    error?: string | string[]
    hint?: string
    horizontal?: boolean
    removable?: boolean
    parentDocumentID?: number
    onchange?: (v: any) => void
    onremoved?: () => void
  } = $props()
  const inputId = newInputId()
  let selected = $state<Doc[]>([])
  let found = $state<Doc[]>([])
  let term = $state('')
  let open = $state(false)

  $effect(() => {
    const ids = (value ?? []).filter((id) => !selected.some((d) => d.id === id))
    if (!ids.length) {
      if (selected.length !== (value ?? []).length) selected = selected.filter((d) => (value ?? []).includes(d.id))
      return
    }
    api
      .get<Paged<Doc>>(`/api/documents/${queryString({ page: 1, page_size: 100000, id__in: ids.join(','), fields: 'id,title,created' })}`)
      .then((r) => (selected = (value ?? []).map((id) => r.results.find((d) => d.id === id) ?? selected.find((d) => d.id === id) ?? { id })))
      .catch(() => (selected = (value ?? []).map((id) => ({ id }))))
  })

  let timer: ReturnType<typeof setTimeout> | undefined
  function search() {
    clearTimeout(timer)
    if (term.length < 2) {
      found = []
      return
    }
    timer = setTimeout(() => {
      api
        .get<Paged<Doc>>(`/api/documents/${queryString({ page: 1, page_size: 50, ordering: '-created', title__icontains: term, fields: 'id,title,created' })}`)
        .then((r) => (found = r.results.filter((d) => d.id !== parentDocumentID && !(value ?? []).includes(d.id))))
    }, 200)
  }
  function emit(docs: Doc[]) {
    selected = docs
    value = docs.map((d) => d.id)
    onchange?.(value)
  }
</script>

<svelte:window onclick={() => (open = false)} />

<div class="mb-3 paperless-input-select" class:disabled>
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
    <div class:col-md-9={horizontal}>
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <ng-select name="inputId" class="ng-select ng-select-multiple ng-select-searchable ng-select-clearable" class:ng-select-opened={open} class:ng-select-bottom={open} class:ng-select-disabled={disabled} class:ng-has-value={selected.length > 0} onclick={(e) => e.stopPropagation()}><div class="ng-select-container ng-has-value" class:ng-has-value={selected.length > 0}><div class="ng-value-container">{#if !selected.length}<div class="ng-placeholder">{t('input.search_documents')}</div>{/if}{#each selected as document (document.id)}<div class="ng-value"><div class="d-flex align-items-center">{#if !disabled}<button class="btn p-0 lh-1" onclick={() => emit(selected.filter((d) => d.id !== document.id))} onmousedown={(e) => e.stopImmediatePropagation()} type="button" title={t('input.remove_link')}><pngx-bs-icon name="x">{@html icon('x')}</pngx-bs-icon></button>{/if}{#if document.title}<a href={router.href(`documents/${document.id}`)} class="badge bg-light text-primary" onmousedown={(e) => e.stopImmediatePropagation()} title={t('input.open_link')}><pngx-bs-icon width="0.9em" height="0.9em" name="file-text" class="me-1">{@html icon('file-text', '0.9em', '0.9em')}</pngx-bs-icon><span>{document.title}</span></a>{:else}<span class="badge bg-light text-muted"><pngx-bs-icon width="0.9em" height="0.9em" name="exclamation-triangle-fill" class="me-1">{@html icon('exclamation-triangle-fill', '0.9em', '0.9em')}</pngx-bs-icon><span>{t('input.unavailable')}</span></span>{/if}</div></div>{/each}<div class="ng-input"><input id={inputId} aria-autocomplete="list" role="combobox" type="text" autocorrect="off" autocapitalize="off" autocomplete="off" aria-expanded={open} {disabled} bind:value={term} oninput={() => { open = true; search() }}></div></div><span class="ng-arrow-wrapper"><span class="ng-arrow"></span></span></div>{#if open && term.length >= 2}<div role="listbox" class="ng-dropdown-panel ng-select-bottom"><div class="ng-dropdown-panel-items scroll-host"><div></div><div>{#each found as document (document.id)}<div role="option" tabindex="-1" aria-selected="false" class="ng-option" onmousedown={(e) => { e.preventDefault(); term = ''; open = false; emit([...selected, document]) }}><div>{document.title} <small class="text-muted">({formatDate(document.created, 'shortDate', locale())})</small></div></div>{:else}<div class="ng-option ng-option-disabled">{t('common.no_documents_found')}</div>{/each}</div></div></div>{/if}</ng-select>
      {#if hint}
        <small class="form-text text-muted">{hint}</small>
      {/if}
    </div>
  </div>
</div>
