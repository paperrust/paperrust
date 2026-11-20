<!-- Port of document-list/filter-editor/filter-editor.component.html. Logic: lib/filter-editor/filter-editor-state.ts.
     Transition: full-text suggestions (ngbTypeahead) still without popup. -->
<script lang="ts">
  import '../../../styles/components/filter-editor.css'
  import { onDestroy, onMount, untrack } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { FilterEditorState } from '../../../lib/filter-editor/filter-editor-state'
  import { FILTER_FULLTEXT_MORELIKE } from '../../../lib/filter-rule-type'
  import { session } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { list } from '../../../stores/listView.svelte'
  import FilterableDropdown from '../../common/filterable-dropdown/FilterableDropdown.svelte'
  import DatesDropdown from '../../common/dates-dropdown/DatesDropdown.svelte'
  import PermissionsFilterDropdown from '../../common/permissions-filter-dropdown/PermissionsFilterDropdown.svelte'
  // Load the custom field query (6 KB) right after start instead of in the first call (budget FRONTEND-LIGHT §1)
  let CustomFieldsQueryDropdown = $state<import('svelte').Component<any> | null>(null)
  import('../../common/custom-fields-query-dropdown/CustomFieldsQueryDropdown.svelte').then((m) => (CustomFieldsQueryDropdown = m.default))

  let { disabled = false }: { disabled?: boolean } = $props()

  const fe = new FilterEditorState({
    t,
    currentUserId: session.data?.user.id,
    locale: locale(),
    getDocument: (id) => api.get(`/api/documents/${id}/?fields=id,title`),
    autocomplete: (term) => api.get<string[]>(`/api/search/autocomplete/?term=${encodeURIComponent(term)}`),
  })
  let v = $state(0)
  const bump = () => untrack(() => v++) // safe also from $effect (A-151)
  let applying = false
  const offs = [
    fe.onChange(() => bump()),
    fe.onFilterRulesChange((rules) => {
      if (!applying) list.setFilterRules(rules)
    }),
    fe.onResetFilterRules((rules) => list.setFilterRules(rules, true)),
  ]
  onDestroy(() => {
    offs.forEach((f) => f())
    fe.destroy()
  })

  /** Take over rules from the list without writing them back */
  function applyRules(rules: typeof list.filterRules) {
    applying = true
    try {
      fe.filterRules = rules
    } finally {
      applying = false
    }
    bump()
  }
  // Master data → models (like the listAll() subscriptions in the Angular component)
  $effect(() => {
    const items = [[...data.tags.values()], [...data.correspondents.values()], [...data.documentTypes.values()], [...data.storagePaths.values()], data.customFields, [...data.users.values()]] as any[]
    untrack(() => {
      applying = true
      try {
        fe.setTags(items[0])
        fe.setCorrespondents(items[1])
        fe.setDocumentTypes(items[2])
        fe.setStoragePaths(items[3])
        fe.setCustomFields(items[4])
        if (session.can('view', 'user')) fe.setUsers(items[5])
      } finally {
        applying = false
      }
      applyRules(list.filterRules)
    })
  })
  // List → editor (URL, page change)
  let lastRules = ''
  $effect(() => {
    const r = JSON.stringify(list.filterRules)
    if (r !== lastRules) {
      lastRules = r
      if (JSON.stringify(fe._filterRules) !== r) untrack(() => applyRules(list.filterRules))
    }
  })
  $effect(() => {
    const sd = list.selectionData
    untrack(() => (fe.selectionData = sd as any))
  })
  $effect(() => {
    fe.disabled = disabled
  })

  let textInput: HTMLInputElement
  onMount(() => textInput.focus())

  const ddTarget = new Dropdown()
  const view = $derived.by(() => {
    v
    return {
      targets: fe.textFilterTargets,
      targetName: fe.textFilterTargetName,
      target: fe.textFilterTarget,
      showModifier: fe.showTextFilterModifier,
      modifiers: fe.textFilterModifiers,
      modifier: fe.textFilterModifier,
      text: fe.textFilter,
      rawText: fe._textFilter,
      inputDisabled: fe.textFilterInputDisabled,
      readonly: fe.textFilterInputReadonly,
      rulesModified: fe.rulesModified,
      tags: fe.tagSelectionModel.items.length > 0,
      corr: fe.correspondentSelectionModel.items.length > 0,
      types: fe.documentTypeSelectionModel.items.length > 0,
      paths: fe.storagePathSelectionModel.items.length > 0,
    }
  })

  function changeTarget(id: string) {
    fe.changeTextFilterTarget(id)
    bump()
    textInput?.focus()
  }
  function keydown(e: KeyboardEvent) {
    const r = fe.textFilterKeydown(e.key, { inputValue: (e.target as HTMLInputElement).value, defaultPrevented: e.defaultPrevented })
    if (r === 'blur') textInput.blur()
    bump()
  }

  // ---- Interface for DocumentList (like @ViewChild filterEditor) ----
  export function toggleTag(id: number) {
    fe.toggleTag(id)
  }
  export function toggleCorrespondent(id: number) {
    fe.toggleCorrespondent(id)
  }
  export function toggleDocumentType(id: number) {
    fe.toggleDocumentType(id)
  }
  export function toggleStoragePath(id: number) {
    fe.toggleStoragePath(id)
  }
  export function resetSelected() {
    fe.resetSelected()
  }
  export function moreLike(id: number) {
    list.setFilterRules([{ rule_type: FILTER_FULLTEXT_MORELIKE, value: String(id) }])
  }
  export function isModified() {
    return view.rulesModified
  }
</script>

<div class="row flex-wrap row-gap-3" tourAnchor="tour.documents-filter-editor">
  <div class="col">
    <div class="form-inline d-flex align-items-center fade show">
      <div class="input-group input-group-sm flex-fill w-auto flex-nowrap">
        <div class={ddTarget.placementClass} class:show={ddTarget.open} use:ddTarget.hostEl>
          <button class="dropdown-toggle btn btn-sm btn-outline-primary" aria-expanded={ddTarget.open} use:ddTarget.anchorEl onclick={ddTarget.toggle}>{view.targetName}</button>
          {#if ddTarget.open}
            <div class="dropdown-menu shadow show" use:ddTarget.menuEl>
              {#each view.targets as tg (tg.id)}
                <button class="dropdown-item" tabindex="0" class:active={view.target === tg.id} onclick={() => changeTarget(tg.id)}>{tg.name}</button>
              {/each}
            </div>
          {/if}
        </div>
        {#if view.showModifier}
          <select class="form-select flex-grow-0 w-auto" value={view.modifier} onchange={(e) => { fe.setTextFilterModifier(e.currentTarget.value); bump() }}>
            {#each view.modifiers as m (m.id)}
              <option value={m.id}>{m.label}</option>
            {/each}
          </select>
        {/if}
        {#if view.rawText}
          <button class="btn btn-link btn-sm px-2 position-absolute top-0 end-0 z-10" onclick={() => { fe.resetTextField(); bump() }} aria-label={t('common.clear_search')}>
            <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon>
          </button>
        {/if}
        <input bind:this={textInput} class="form-control form-control-sm" type="text" autocapitalize="off" autocorrect="off" autocomplete="off" role="combobox" aria-autocomplete="list" aria-expanded="false"
          disabled={view.inputDisabled}
          value={view.text}
          oninput={(e) => { fe.textFilter = e.currentTarget.value }}
          onkeydown={keydown}
          readonly={view.readonly}>
      </div>
    </div>
  </div>
  <div class="col-auto">
    <div class="d-flex flex-wrap gap-3">
      <div class="d-flex flex-wrap gap-2">
        {#if session.can('view', 'tag') && view.tags}
          <pngx-filterable-dropdown class="flex-fill fade show" title="Tags" icon="tag-fill" filterPlaceholder="Filter tags" shortcutKey="t"><FilterableDropdown title={t('common.tags')} icon="tag-fill"
          filterPlaceholder={t('list.filter_tags')}
          dropdownState={fe.tagDropdown}
          shortcutKey="t"
          allowSelectNone
          version={v}
          {disabled} /></pngx-filterable-dropdown>
        {/if}
        {#if session.can('view', 'correspondent') && view.corr}
          <pngx-filterable-dropdown class="flex-fill fade show" title="Correspondent" icon="person-fill" filterPlaceholder="Filter correspondents" shortcutKey="y"><FilterableDropdown title={t('common.correspondent')} icon="person-fill"
          filterPlaceholder={t('list.filter_correspondents')}
          dropdownState={fe.correspondentDropdown}
          shortcutKey="y"
          allowSelectNone
          version={v}
          {disabled} /></pngx-filterable-dropdown>
        {/if}
        {#if session.can('view', 'documenttype') && view.types}
            <pngx-filterable-dropdown class="flex-fill fade show" title="Document type" icon="file-earmark-fill" filterPlaceholder="Filter document types" shortcutKey="u"><FilterableDropdown title={t('common.document_type')} icon="file-earmark-fill"
            filterPlaceholder={t('list.filter_document_types')}
            dropdownState={fe.documentTypeDropdown}
            shortcutKey="u"
            allowSelectNone
            version={v}
            {disabled} /></pngx-filterable-dropdown>
        {/if}
        {#if session.can('view', 'storagepath') && view.paths}
          <pngx-filterable-dropdown class="flex-fill fade show" title="Storage path" icon="folder-fill" filterPlaceholder="Filter storage paths" shortcutKey="i"><FilterableDropdown title={t('common.storage_path')} icon="folder-fill"
          filterPlaceholder={t('list.filter_storage_paths')}
          dropdownState={fe.storagePathDropdown}
          shortcutKey="i"
          allowSelectNone
          version={v}
          {disabled} /></pngx-filterable-dropdown>
        {/if}

        {#if session.can('view', 'customfield') && data.customFields.length > 0}
          <pngx-custom-fields-query-dropdown class="flex-fill fade show" title="Custom fields" icon="ui-radios">{#if CustomFieldsQueryDropdown}<CustomFieldsQueryDropdown title={t('common.custom_fields')} queryState={fe.customFieldsDropdown} version={v} />{/if}</pngx-custom-fields-query-dropdown>
        {/if}
        <pngx-dates-dropdown class="flex-fill fade show"
          title="Dates"
          placement="bottom-end"><DatesDropdown model={fe.dates} title={t('list.dates')} placement="bottom-end" version={v} /></pngx-dates-dropdown>
        <pngx-permissions-filter-dropdown class="flex-fill fade show"
          title="Permissions"><PermissionsFilterDropdown filterState={fe.permissionsFilter} title={t('common.permissions')} version={v} /></pngx-permissions-filter-dropdown>
      </div>
      <div class="d-flex flex-wrap d-none d-sm-inline-block">
        <button class="btn btn-outline-secondary btn-sm" disabled={!view.rulesModified} onclick={() => fe.resetSelected()}>
          <pngx-bs-icon class="ms-n1" name="x">{@html icon('x')}</pngx-bs-icon>{t('common.reset_filters')}
          </button>
        </div>
      </div>
    </div>
  </div>
