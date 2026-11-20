<!-- Port of document-list/document-list.component.html/.ts (table, small/large cards, filter, selection,
     saved views under /view/<id> incl. save / save as). -->
<script lang="ts">
  import '../../styles/components/document-list.css'
  import { untrack } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { formatDate } from '../../lib/dates'
  import { icon } from '../common/bootstrap-icon/icon'
  import type { Document } from '../../api/v1'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { router } from '../../router.svelte'
  import { list, DEFAULT_DISPLAY_FIELDS, DOCUMENT_SORT_FIELDS, LIST_DEFAULT_DISPLAY_FIELDS, type DisplayMode, type SavedViewConfig } from '../../stores/listView.svelte'
  import { api } from '../../api/client'
  import { modals } from '../../lib/modal.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { savedViewIsModified as viewModified } from './save-view-config-dialog/save-view'
  import type { SaveViewConfigValue } from './save-view-config-dialog/SaveViewConfigDialog.svelte'
  import { isFullTextFilterRule } from '../../lib/filter-rules'
  import { Dropdown } from '../../lib/dropdown.svelte'
  import { ignoreHotkeyEvent, matchListHotkey } from '../common/hotkey-dialog/hotkeys'
  import PageHeader from '../common/page-header/PageHeader.svelte'
  import FilterEditor from './filter-editor/FilterEditor.svelte'
  import Tag from '../common/tag/Tag.svelte'
  import Pagination from '../common/pagination/Pagination.svelte'
  import PreviewPopup from '../common/preview-popup/PreviewPopup.svelte'
  import ClearableBadge from '../common/clearable-badge/ClearableBadge.svelte'
  import CustomFieldDisplay from '../common/custom-field-display/CustomFieldDisplay.svelte'
  import DocumentCardSmall from './document-card-small/DocumentCardSmall.svelte'
  // Budget first call: load bulk edit and large cards only on demand (own chunks)
  const loadBulkEditor = () => import('./bulk-editor/BulkEditor.svelte')
  const loadCardLarge = () => import('./document-card-large/DocumentCardLarge.svelte')
  let bulkEditorNeeded = $state(false)
  $effect(() => {
    if (list.hasSelection) bulkEditorNeeded = true
  })

  let filterEditor: FilterEditor | undefined = $state()
  const ddSelect = new Dropdown()
  const ddShow = new Dropdown()
  const ddSort = new Dropdown()
  const ddViews = new Dropdown()

  const notesEnabled = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED))
  const dateFormat = $derived(session.get(SETTINGS_KEYS.DATE_FORMAT))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())

  /** SettingsService.allDisplayFields (permissions, notes, custom fields) */
  const allDisplayFields = $derived.by(() => {
    const perm: Record<string, string> = { tag: 'tag', correspondent: 'correspondent', documenttype: 'documenttype', storagepath: 'storagepath', owner: 'user', note: 'note' }
    const base = DEFAULT_DISPLAY_FIELDS.filter((f) => {
      if (f.id === 'note' && !notesEnabled) return false
      if (['title', 'created', 'added', 'asn', 'pagecount', 'shared'].includes(f.id)) return true
      return session.can('view', perm[f.id])
    }).map((f) => ({ id: f.id, name: t(f.name) }))
    const cfs = session.can('view', 'customfield') ? data.customFields.map((c) => ({ id: `custom_field_${c.id}`, name: c.name })) : []
    return [...base, ...cfs]
  })
  const fields = $derived(list.displayFields)
  const has = (f: string) => fields.includes(f)
  const activeDisplayCustomFields = $derived(fields.filter((f) => f.startsWith('custom_field_')))
  const customFieldTitle = (f: string) => allDisplayFields.find((x) => x.id === f)?.name
  const sortFields = $derived.by(() => {
    const base = DOCUMENT_SORT_FIELDS.map((f) => ({ field: f.field, name: t(f.name) }))
    const cfs = data.customFields.map((c) => ({ field: `custom_field_${c.id}`, name: c.name }))
    const all = [...base, ...cfs]
    return isFullTextFilterRule(list.filterRules) ? [{ field: 'score', name: t('list.search_score') }, ...all] : all
  })
  const isBulkEditing = $derived(list.hasSelection)

  // ---- Saved views (DocumentListComponent: activeSavedView, unmodifiedSavedView) ------------------
  type ActiveView = SavedViewConfig & { owner?: number | null; user_can_change?: boolean; permissions?: any; icon?: string | null }
  let activeSavedView = $state<ActiveView | null>(null)
  let unmodifiedSavedView = $state<ActiveView | null>(null)
  const activeSavedViewCanChange = $derived(!!activeSavedView && session.hasObjectPermissions('change', activeSavedView))
  const savedViewIsModified = $derived(
    !!list.activeSavedViewId &&
      !!unmodifiedSavedView &&
      activeSavedViewCanChange &&
      viewModified(
        unmodifiedSavedView,
        { filterRules: list.filterRules, sortField: list.sortField, sortReverse: list.sortReverse, pageSize: list.pageSize, displayMode: list.mode, displayFields: fields },
        LIST_DEFAULT_DISPLAY_FIELDS,
      ),
  )
  /** getTitle(): name of the view (+ "*" on changes) or "Documents" */
  const title = $derived(list.activeSavedViewTitle ? list.activeSavedViewTitle + (savedViewIsModified ? '*' : '') : t('common.documents_2'))

  // Routes documents and view/<id> use the same component (stays alive when switching) → react to the path
  // instead of only onMount (Angular: route.paramMap / route.queryParamMap).
  let handled = ''
  $effect(() => {
    const path = router.path
    const query = router.query
    const ready = !data.savedViewsLoading
    untrack(() => onRoute(path, query, ready))
  })
  function onRoute(path: string, query: string, ready: boolean) {
    const m = /^view\/(\d+)$/.exec(path)
    if (m) {
      if (!ready || handled === path) return
      handled = path
      void openSavedView(+m[1], query)
    } else if (path === 'documents') {
      const key = path + query
      if (handled === key) return
      handled = key
      const viewId = new URLSearchParams(query).get('view')
      if (viewId && /^\d+$/.test(viewId)) {
        // loadViewConfig (?view=<id>): Angular activates the view and switches to /view/<id>
        router.navigate(`view/${viewId}`, true)
        return
      }
      activeSavedView = null
      unmodifiedSavedView = null
      list.activateSavedView(null) // Default view
      list.loadFromQuery(query)
    }
  }
  /** route.paramMap → savedViewService.getCached(id); unknown → /404 */
  async function openSavedView(id: number, query: string) {
    let view = data.savedViews.find((v) => v.id === id) as ActiveView | undefined
    if (!view) view = await api.getSavedView<ActiveView>(id).catch(() => undefined)
    if (handled !== `view/${id}`) return // navigated on in the meantime
    if (!view) {
      activeSavedView = null
      router.navigate('404', true)
      return
    }
    activeSavedView = view
    unmodifiedSavedView = view
    list.activateSavedViewWithQueryParams(view, query)
    await list.reload()
    // savedViewService.setDocumentCount
    if (list.activeSavedViewId === id && list.collectionSize != null) data.viewCounts = { ...data.viewCounts, [id]: list.collectionSize }
  }

  /** SavedViewService.reload after create/patch */
  async function refreshSavedViews() {
    try {
      data.savedViews = (await api.listAll<any>('saved_views')).results
    } catch {}
  }

  /** saveViewConfig(): overwrite the active view with the current list state (PATCH) */
  async function saveViewConfig() {
    ddViews.close()
    if (list.activeSavedViewId == null || !activeSavedViewCanChange) return
    const viewTitle = list.activeSavedViewTitle
    try {
      const view = await api.patchSavedView<ActiveView>({
        id: list.activeSavedViewId,
        filter_rules: list.filterRules,
        sort_field: list.sortField,
        sort_reverse: list.sortReverse,
        display_mode: list.displayMode,
        display_fields: fields,
      })
      activeSavedView = view
      unmodifiedSavedView = view
      toastService.showSuccess(t('list.view_saved', { PH: view.name ?? viewTitle }))
      data.savedViews = data.savedViews.map((v) => (v.id === view.id ? (view as any) : v))
    } catch (e) {
      toastService.showError(t('list.view_save_error', { PH: viewTitle }), e)
    }
  }

  /** saveViewConfigAs(): dialog, POST /api/saved_views/, afterwards visibility in the UI settings */
  async function saveViewConfigAs() {
    ddViews.close()
    const [{ default: Dialog }, sv, ui, settingsSave] = await Promise.all([
      import('./save-view-config-dialog/SaveViewConfigDialog.svelte'),
      import('./save-view-config-dialog/save-view'),
      import('../manage/saved-views/saved-views.svelte'),
      import('../../stores/settingsSave.svelte'),
    ])
    const defaultName = sv.generateFilterName(
      list.filterRules,
      {
        correspondent: (id) => data.correspondents.get(id),
        documentType: (id) => data.documentTypes.get(id),
        storagePath: (id) => data.storagePaths.get(id),
        tag: (id) => data.tags.get(id),
      },
      t,
    )
    const ref = modals.open(
      Dialog,
      {
        defaultName,
        onsave: async (formValue: SaveViewConfigValue) => {
          const savedView: Record<string, any> = {
            name: formValue.name,
            icon: formValue.icon,
            filter_rules: list.filterRules,
            sort_reverse: list.sortReverse,
            sort_field: list.sortField,
            display_mode: list.displayMode,
            display_fields: fields,
          }
          const permissions = formValue.permissions_form
          if (permissions) {
            if (permissions.owner !== null && permissions.owner !== undefined) savedView.owner = permissions.owner
            if (permissions.set_permissions) savedView.set_permissions = permissions.set_permissions
          }
          let created: { id: number }
          try {
            created = await api.createSavedView<{ id: number }>(savedView)
          } catch (httpError: any) {
            const body = httpError?.body
            const error: Record<string, any> = body && typeof body === 'object' ? { ...body } : { name: String(body ?? httpError) }
            if (Array.isArray(error.filter_rules)) error.filter_rules = error.filter_rules.map((r: any) => r?.value ?? r)
            return error
          }
          await refreshSavedViews()
          // saveCreatedViewVisibility (dashboardViews/sidebarViews: existing views only)
          const existing = new Set(data.savedViews.map((v) => v.id))
          const { dashboard, sidebar } = sv.visibilityAfterCreate(
            ui.visibleIds(ui.DASHBOARD_VIEWS_VISIBLE_IDS).filter((id) => existing.has(id)),
            ui.visibleIds(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS).filter((id) => existing.has(id)),
            created.id,
            formValue.showOnDashboard,
            formValue.showInSideBar,
          )
          try {
            settingsSave.setSetting(ui.DASHBOARD_VIEWS_VISIBLE_IDS, [...new Set(dashboard)])
            settingsSave.setSetting(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS, [...new Set(sidebar)])
            await settingsSave.storeSettings()
            ref.close()
            toastService.showSuccess(t('list.view_created', { PH: savedView.name }))
          } catch (error) {
            ref.close()
            toastService.showError(t('list.view_created_visibility_error', { PH: savedView.name }), error)
          }
          data.refreshViewCounts()
        },
      },
      { backdrop: 'static' },
    )
  }

  function onSort(field: string) {
    // SortableDirective: same column → reverse direction, otherwise ascending
    if (list.sortField === field) list.setSort(field, !list.sortReverse)
    else list.setSort(field, false)
  }
  function sortClass(field: string) {
    return list.sortField === field ? (list.sortReverse ? 'des' : 'asc') : ''
  }
  function toggleSelected(d: Document, e: MouseEvent) {
    e.stopPropagation()
    if (e.shiftKey) list.selectRangeTo(d)
    else list.toggleSelected(d)
  }
  function openDocument(d: Document | number) {
    router.navigate(`documents/${typeof d === 'number' ? d : d.id}`)
  }
  // Keyboard shortcuts (DocumentListComponent.ngOnInit, rules of the HotKeyService)
  function hotkey(e: KeyboardEvent) {
    const k = matchListHotkey(e)
    if (!k || ignoreHotkeyEvent(e)) return
    const busy = modals.open_.length > 0 || document.getElementsByClassName('dropdown-menu show').length > 0
    for (const ref of [...modals.open_]) ref.dismiss()
    if (k === 'reset' && busy) return
    e.preventDefault()
    if (k === 'reset') {
      if (list.hasSelection) list.selectNone()
      else if (filterEditor?.isModified()) filterEditor.resetSelected()
    } else if (k === 'selectAll') list.selectAll()
    else if (k === 'selectPage') list.selectPage()
    else if (k === 'open') {
      const first = list.hasSelection ? [...list.selected][0] : list.documents[0]
      if (list.documents.length && first != null) openDocument(first)
    } else if (k === 'previousPage') {
      if (list.currentPage > 1) list.setPage(list.currentPage - 1)
    } else if (list.currentPage < list.lastPage) list.setPage(list.currentPage + 1)
  }
  function toggleDisplayField(id: string) {
    list.displayFields = fields.includes(id) ? fields.filter((f) => f !== id) : [...fields, id]
  }
  function setMode(m: DisplayMode) {
    list.mode = m
  }
  function clickTag(id: number) {
    list.selectNone()
    filterEditor?.toggleTag(id)
  }
  function clickCorrespondent(id: number) {
    list.selectNone()
    filterEditor?.toggleCorrespondent(id)
  }
  function clickDocumentType(id: number) {
    list.selectNone()
    filterEditor?.toggleDocumentType(id)
  }
  function clickStoragePath(id: number) {
    list.selectNone()
    filterEditor?.toggleStoragePath(id)
  }
  function clickMoreLike(id: number) {
    filterEditor?.moreLike(id)
  }
  const docTitle = (title: string) => title || t('common.no_title')
  const stop = (fn: () => void) => (e: Event) => {
    fn()
    e.stopPropagation()
  }
</script>

<!-- Capture: before ModalHost (closes dialogs on Escape), so that "dialog open" is still detected -->
<svelte:window onkeydowncapture={hotkey} />

<pngx-page-header>
<PageHeader {title}>
  <div class="btn-group flex-fill d-sm-none {ddSelect.placementClass}" class:show={ddSelect.open} use:ddSelect.hostEl>
    <button class="dropdown-toggle btn btn-sm btn-outline-primary" id="dropdownSelectMobile" aria-label={t('common.select')} aria-expanded={ddSelect.open} use:ddSelect.anchorEl onclick={ddSelect.toggle}>
      <pngx-bs-icon name="text-indent-left">{@html icon('text-indent-left')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.select')}</div>
      {#if list.hasSelection}
        <pngx-clearable-badge><ClearableBadge selected={list.hasSelection} number={list.selectedCount} oncleared={() => list.selectNone()} /></pngx-clearable-badge><span class="visually-hidden">selected</span>
      {/if}
    </button>
    {#if ddSelect.open}
      <div aria-labelledby="dropdownSelectMobile" class="dropdown-menu shadow show" use:ddSelect.menuEl>
        <button class="dropdown-item" tabindex="0" onclick={() => list.selectNone()}>{t('common.select_none')}</button>
        <button class="dropdown-item" tabindex="0" onclick={() => list.selectPage()}>{t('common.select_page')}</button>
        <button class="dropdown-item" tabindex="0" onclick={() => list.selectAll()}>{t('common.select_all')}</button>
      </div>
    {/if}
  </div>
  <div class="d-none d-sm-flex flex-fill me-3">
    <div class="input-group input-group-sm">
      <span class="input-group-text bg-transparent border-0">{t('common.select_label')}</span>
    </div>
    <div class="btn-group btn-group-sm flex-nowrap">
      {#if list.hasSelection}
        <button class="btn btn-sm btn-outline-secondary" onclick={() => list.selectNone()}>
          <pngx-bs-icon name="slash-circle" class="me-1">{@html icon('slash-circle')}</pngx-bs-icon>{t('common.none')}
        </button>
      {/if}
      <button class="btn btn-sm btn-outline-primary" onclick={() => list.selectPage()}>
        <pngx-bs-icon name="file-earmark-check" class="me-1">{@html icon('file-earmark-check')}</pngx-bs-icon>{t('common.page')}
      </button>
      <button class="btn btn-sm btn-outline-primary" onclick={() => list.selectAll()}>
        <pngx-bs-icon name="check-all" class="me-1">{@html icon('check-all')}</pngx-bs-icon>{t('common.all')}
      </button>
    </div>
  </div>
  <div class="btn-group flex-fill {ddShow.placementClass}" class:show={ddShow.open} use:ddShow.hostEl>
    <button class="dropdown-toggle btn btn-sm btn-outline-primary" id="dropdownDisplayFields" aria-label={t('common.show')} aria-expanded={ddShow.open} use:ddShow.anchorEl onclick={ddShow.toggle}>
      <pngx-bs-icon name="card-heading">{@html icon('card-heading')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.show')}</div>
    </button>
    {#if ddShow.open}
      <div aria-labelledby="dropdownDisplayFields" class="dropdown-menu shadow show" use:ddShow.menuEl>
        <div class="px-3">
          {#each allDisplayFields as field (field.id)}
            <div class="form-check my-1">
              <input class="form-check-input mt-1" type="checkbox" id="displayField{field.id}" checked={fields.includes(field.id)} onchange={() => toggleDisplayField(field.id)}>
              <label class="form-check-label" for="displayField{field.id}">{field.name}</label>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
  <div class="btn-group flex-fill" role="group">
    <input type="radio" class="btn-check" checked={list.mode === 'table'} onchange={() => setMode('table')} value="table" id="displayModeDetails" name="displayModeDetails">
    <label for="displayModeDetails" class="btn btn-outline-primary btn-sm">
      <pngx-bs-icon name="list-ul">{@html icon('list-ul')}</pngx-bs-icon>
    </label>
    <input type="radio" class="btn-check" checked={list.mode === 'smallCards'} onchange={() => setMode('smallCards')} value="smallCards" id="displayModeSmall" name="displayModeSmall">
    <label for="displayModeSmall" class="btn btn-outline-primary btn-sm">
      <pngx-bs-icon name="grid">{@html icon('grid')}</pngx-bs-icon>
    </label>
    <input type="radio" class="btn-check" checked={list.mode === 'largeCards'} onchange={() => setMode('largeCards')} value="largeCards" id="displayModeLarge" name="displayModeLarge">
    <label for="displayModeLarge" class="btn btn-outline-primary btn-sm">
      <pngx-bs-icon name="hdd-stack">{@html icon('hdd-stack')}</pngx-bs-icon>
    </label>
  </div>

  <div class="btn-group flex-fill {ddSort.placementClass}" class:show={ddSort.open} use:ddSort.hostEl>
    <button class="dropdown-toggle btn btn-outline-primary btn-sm" id="dropdownBasic1" aria-label={t('common.sort')} aria-expanded={ddSort.open} use:ddSort.anchorEl onclick={ddSort.toggle}>
      <pngx-bs-icon name="arrow-down-up">{@html icon('arrow-down-up')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.sort')}</div>
    </button>
    {#if ddSort.open}
      <div aria-labelledby="dropdownBasic1" class="dropdown-menu shadow dropdown-menu-right show" use:ddSort.menuEl>
        <div class="w-100 d-flex pb-2 mb-1 border-bottom">
          <input type="radio" class="btn-check" checked={!list.sortReverse} onchange={() => list.setSort(list.sortField, false)} id="listSortReverseFalse">
          <label class="btn btn-outline-primary btn-sm mx-2 flex-fill" for="listSortReverseFalse">
            <pngx-bs-icon name="sort-alpha-down">{@html icon('sort-alpha-down')}</pngx-bs-icon>
          </label>
          <input type="radio" class="btn-check" checked={list.sortReverse} onchange={() => list.setSort(list.sortField, true)} id="listSortReverseTrue">
          <label class="btn btn-outline-primary btn-sm me-2 flex-fill" for="listSortReverseTrue">
            <pngx-bs-icon name="sort-alpha-up-alt">{@html icon('sort-alpha-up-alt')}</pngx-bs-icon>
          </label>
        </div>
        <div>
          {#each sortFields as f (f.field)}
            <button class="dropdown-item" tabindex="0" onclick={() => list.setSort(f.field, list.sortReverse)}
              class:active={list.sortField === f.field}>{f.name}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  {#if session.can('view', 'savedview')}
    <div class="btn-group flex-fill {ddViews.placementClass}" class:show={ddViews.open} role="group" use:ddViews.hostEl>
      <button class="btn btn-sm btn-outline-primary dropdown-toggle flex-fill" tourAnchor="tour.documents-views" aria-label={t('common.views')} aria-expanded={ddViews.open} use:ddViews.anchorEl onclick={ddViews.toggle}>
        <pngx-bs-icon name="window-stack">{@html icon('window-stack')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.views')}</div>
        {#if savedViewIsModified}
          <div class="position-absolute top-0 start-100 p-2 translate-middle badge bg-secondary border border-light rounded-circle">
            <span class="visually-hidden">selected</span>
          </div>
        {/if}
      </button>
      {#if ddViews.open}
        <div class="dropdown-menu shadow dropdown-menu-right show" use:ddViews.menuEl>
          {#if !list.activeSavedViewId && data.savedViews.length > 0}
            <div class="views-list overflow-y-auto">
              {#each data.savedViews as view (view.id)}
                <button class="dropdown-item" tabindex="0" onclick={() => { ddViews.close(); router.navigate(`view/${view.id}`) }}>
                  <pngx-bs-icon class="me-2">{@html icon(view.icon || 'funnel')}</pngx-bs-icon>{view.name}
                </button>
              {/each}
            </div>
            <div class="dropdown-divider"></div>
          {/if}

          {#if list.activeSavedViewId && activeSavedViewCanChange}
            <button class="dropdown-item" tabindex="0" onclick={saveViewConfig} disabled={!savedViewIsModified}>{t('list.save_view', { INTERPOLATION: list.activeSavedViewTitle })}</button>
          {/if}
          {#if session.can('add', 'savedview')}
            <button class="dropdown-item" tabindex="0" onclick={saveViewConfigAs}>{t('list.save')}</button>
          {/if}
          <a class="dropdown-item" tabindex="0" href={router.href('savedviews')} onclick={() => ddViews.close()}>{t('list.all_saved_views')}</a>
        </div>
      {/if}
    </div>
  {/if}

</PageHeader>
</pngx-page-header>

<div class="row sticky-top py-3 mt-n2 mt-md-n3 bg-body rounded shadow-sm">
  <pngx-filter-editor hidden={isBulkEditing || undefined}><FilterEditor bind:this={filterEditor} disabled={isBulkEditing} /></pngx-filter-editor>
  <pngx-bulk-editor hidden={!isBulkEditing || undefined}>{#if bulkEditorNeeded}{#await loadBulkEditor() then M}<M.default disabled={!isBulkEditing} />{/await}{/if}</pngx-bulk-editor>
</div>


{#snippet pagination()}
  <div class="d-flex flex-wrap gap-3 justify-content-between align-items-center mb-3">
    <div class="d-flex align-items-center">
      {#if list.isReloading}
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>
        {t('common.loading')}
      {/if}
      {#if list.hasSelection}
        <span>{t('list.selected_documents', { VAR_PLURAL: list.collectionSize, INTERPOLATION: list.selectedCount, INTERPOLATION_1: list.collectionSize || 0 })}</span>
      {/if}
      {#if !list.isReloading}
        {#if !list.hasSelection}
          <span>{t('list.documents', { VAR_PLURAL: list.collectionSize, INTERPOLATION: list.collectionSize || 0 })}</span>
        {/if}
        {#if filterEditor?.isModified()}
            {' '}<span>{t('list.filtered')}</span>
        {/if}
      {/if}
      {#if !list.isReloading && filterEditor?.isModified()}
        <button class="btn btn-link py-0" onclick={() => filterEditor?.resetSelected()}>
          <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon><small>{t('common.reset_filters')}</small>
        </button>
      {/if}
      {#if !list.isReloading && list.hasSelection}
        <button class="btn btn-link py-0" onclick={() => list.selectNone()}>
          <pngx-bs-icon width="1em" height="1em" name="slash-circle" class="me-1">{@html icon('slash-circle', '1em', '1em')}</pngx-bs-icon><small>{t('common.clear_selection')}</small>
        </button>
      {/if}
      </div>
      {#if list.collectionSize}
        <ngb-pagination role="navigation" aria-label="Default pagination" size="sm"><Pagination pageSize={list.pageSize} collectionSize={list.collectionSize} page={list.currentPage} onpage={(p) => list.setPage(p)} maxSize={5} rotate /></ngb-pagination>
      {/if}
    </div>
{/snippet}

  <div class="mt-3 mb-n2" tourAnchor="tour.documents">
    {@render pagination()}
  </div>

  {#if list.error}
    <div class="alert alert-danger" role="alert">{t('common.error_while_loading_documents')}: {list.error}</div>
  {:else}
    {#if list.mode === 'largeCards'}
      {#await loadCardLarge() then { default: DocumentCardLarge }}
      <div>
        {#each list.documents as d, i (d.id)}
          <pngx-document-card-large><DocumentCardLarge
            document={d}
            selected={list.isSelected(d)}
            priority={i < 2}
            displayFields={fields}
            {dateLocale}
            ontoggle={(e) => toggleSelected(d, e)}
            onopen={() => openDocument(d)}
            onclicktag={clickTag}
            onclickcorrespondent={clickCorrespondent}
            onclickdocumenttype={clickDocumentType}
            onclickstoragepath={clickStoragePath}
            onclickmorelike={() => clickMoreLike(d.id)} />
          </pngx-document-card-large>
        {/each}
      </div>
      {/await}
    {/if}
    {#if list.mode === 'table'}
      <div class="table-responsive">
        <table class="table table-sm align-middle border shadow-sm">
          <thead>
            <tr>
              <th></th>
              {#if has('asn')}
                <th class="cursor-pointer {sortClass('archive_serial_number')}" pngxSortable="archive_serial_number"
                  title={t('list.sort_asn')}
                  onclick={() => onSort('archive_serial_number')}
                  >{t('common.asn')}</th>
              {/if}
              {#if has('correspondent') && session.can('view', 'correspondent')}
                <th class="cursor-pointer {sortClass('correspondent__name')}" pngxSortable="correspondent__name"
                  title={t('list.sort_correspondent')}
                  onclick={() => onSort('correspondent__name')}
                  >{t('common.correspondent')}</th>
              {/if}
              {#if has('title')}
                <th class="pr-w-2rem"></th>
                <th class="cursor-pointer pr-minw-150 {sortClass('title')}" pngxSortable="title"
                  title={t('list.sort_title')}
                  onclick={() => onSort('title')}
                  >{t('common.title')}</th>
              {/if}
              {#if has('tag') && !has('title')}
                <th>{t('common.tags')}</th>
              {/if}
              {#if has('owner') && session.can('view', 'user')}
                <th class="cursor-pointer {sortClass('owner')}" pngxSortable="owner"
                  title={t('list.sort_owner')}
                  onclick={() => onSort('owner')}
                  >{t('common.owner')}</th>
              {/if}
              {#if has('note') && notesEnabled}
                <th class="cursor-pointer {sortClass('num_notes')}" pngxSortable="num_notes"
                  title={t('list.sort_notes')}
                  onclick={() => onSort('num_notes')}
                  >{t('common.notes')}</th>
              {/if}
              {#if has('documenttype') && session.can('view', 'documenttype')}
                <th class="cursor-pointer {sortClass('document_type__name')}" pngxSortable="document_type__name"
                  title={t('list.sort_document_type')}
                  onclick={() => onSort('document_type__name')}
                  >{t('common.document_type')}</th>
              {/if}
              {#if has('storagepath') && session.can('view', 'storagepath')}
                <th class="cursor-pointer {sortClass('storage_path__name')}" pngxSortable="storage_path__name"
                  title={t('list.sort_storage_path')}
                  onclick={() => onSort('storage_path__name')}
                  >{t('common.storage_path')}</th>
              {/if}
              {#if has('created')}
                <th class="cursor-pointer {sortClass('created')}" pngxSortable="created"
                  title={t('list.sort_created_date')}
                  onclick={() => onSort('created')}
                  >{t('common.created')}</th>
              {/if}
              {#if has('added')}
                <th class="cursor-pointer {sortClass('added')}" pngxSortable="added"
                  title={t('list.sort_added_date')}
                  onclick={() => onSort('added')}
                  >{t('common.added')}</th>
              {/if}
              {#if has('pagecount')}
                  <th class="cursor-pointer {sortClass('page_count')}" pngxSortable="page_count"
                    title={t('list.sort_number_pages')}
                    onclick={() => onSort('page_count')}
                    >{t('common.pages')}</th>
                {/if}
              {#if has('shared')}
                <th>{t('common.shared')}</th>
              {/if}
              {#each activeDisplayCustomFields as fieldId (fieldId)}
                <th class="cursor-pointer {sortClass(fieldId)}" pngxSortable={fieldId}
                  title={t('list.sort', { INTERPOLATION: customFieldTitle(fieldId) })}
                  onclick={() => onSort(fieldId)}>
                  {customFieldTitle(fieldId)}
                </th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each list.documents as d (d.id)}
              <tr onclick={(e) => toggleSelected(d, e)} ondblclick={() => openDocument(d)} class={list.isSelected(d) ? 'table-row-selected' : ''}>
                <td>
                  <div class="form-check">
                    <input type="checkbox" class="form-check-input" id="docCheck{d.id}" checked={list.isSelected(d)} onclick={(e) => toggleSelected(d, e)}>
                    <label class="form-check-label" for="docCheck{d.id}"></label>
                  </div>
                </td>
                {#if has('asn')}
                  <td class="">
                    {d.archive_serial_number ?? ''}
                  </td>
                {/if}
                {#if has('correspondent') && session.can('view', 'correspondent')}
                  <td class="">
                    {#if d.correspondent}
                      <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                      <a onclick={stop(() => clickCorrespondent(d.correspondent!))} title={t('common.filter_correspondent')}>{data.correspondents.get(d.correspondent)?.name ?? ''}</a>
                    {/if}
                  </td>
                {/if}
                {#if has('title')}
                  <td class="pe-0 text-end align-baseline">
                    <pngx-preview-popup><PreviewPopup id={d.id} linkClasses="btn btn-sm btn-link text-secondary px-1" linkTitle={t('list.preview_document')}>
                      <pngx-bs-icon name="eye">{@html icon('eye')}</pngx-bs-icon>
                    </PreviewPopup></pngx-preview-popup>
                  </td>
                {/if}
                {#if has('title') || has('tag')}
                  <td width="30%" class="align-baseline">
                    {#if has('title')}
                      <a href={router.href(`documents/${d.id}`)} title={t('list.edit_document')} class="pr-wrap-anywhere">{docTitle(d.title)}</a>
                    {/if}
                    {#if has('tag')}
                      {#each d.tags as tagID (tagID)}
                        <pngx-tag class="ms-1 fs-6"><Tag tag={data.tags.get(tagID)} clickable linkTitle={t('common.filter_tag')} onclick={stop(() => clickTag(tagID))} /></pngx-tag>
                      {/each}
                    {/if}
                  </td>
                {/if}
                {#if has('owner') && session.can('view', 'user')}
                  <td>
                    {data.username(d.owner)}
                  </td>
                {/if}
                {#if has('note') && notesEnabled}
                  <td class="">
                    {#if d.notes.length}
                      <a href={router.href(`documents/${d.id}/notes`)} class="btn btn-sm p-0">
                        <span class="badge rounded-pill bg-light border text-primary">
                          <pngx-bs-icon width="1.2em" height="1.2em" class="ms-1 me-1" name="chat-left-text">{@html icon('chat-left-text', '1.2em', '1.2em')}</pngx-bs-icon>
                        {d.notes.length}</span>
                      </a>
                    {/if}
                  </td>
                {/if}
                {#if has('documenttype') && session.can('view', 'documenttype')}
                  <td class="">
                    {#if d.document_type}
                      <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                      <a onclick={stop(() => clickDocumentType(d.document_type!))} title={t('common.filter_document_type')}>{data.documentTypes.get(d.document_type)?.name ?? ''}</a>
                    {/if}
                  </td>
                {/if}
                {#if has('storagepath') && session.can('view', 'storagepath')}
                  <td class="">
                    {#if d.storage_path}
                      <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                      <a onclick={stop(() => clickStoragePath(d.storage_path!))} title={t('common.filter_storage_path')}>{data.storagePaths.get(d.storage_path)?.name ?? ''}</a>
                    {/if}
                  </td>
                {/if}
                {#if has('created')}
                  <td>
                    {formatDate(d.created, dateFormat, dateLocale)}
                  </td>
                {/if}
                {#if has('added')}
                  <td>
                    {formatDate(d.added, dateFormat, dateLocale)}
                  </td>
                {/if}
                {#if has('pagecount')}
                  <td>
                      {d.page_count ?? ''}
                  </td>
                {/if}
                {#if has('shared')}
                  <td>
                    {#if d.is_shared_by_requester}{' '}{t('common.yes')}{' '}{:else}{' '}{t('common.no')}{' '}{/if}
                  </td>
                {/if}
                {#each activeDisplayCustomFields as fieldId (fieldId)}
                  <td class="">
                    <pngx-custom-field-display><CustomFieldDisplay document={d} fieldId={+fieldId.slice(13)} /></pngx-custom-field-display>
                  </td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if list.mode === 'smallCards'}
      <div class="row row-cols-paperless-cards">
        {#each list.documents as d, i (d.id)}
          <pngx-document-card-small class="p-0"><DocumentCardSmall
            document={d}
            selected={list.isSelected(d)}
            priority={i < 6}
            displayFields={fields}
            {dateFormat}
            {dateLocale}
            ontoggle={(e) => toggleSelected(d, e)}
            onopen={() => openDocument(d)}
            onclicktag={clickTag}
            onclickcorrespondent={clickCorrespondent}
            onclickdocumenttype={clickDocumentType}
            onclickstoragepath={clickStoragePath} />
          </pngx-document-card-small>
        {/each}
      </div>
    {/if}
    {#if list.documents.length > 15}
      <div class="mt-3">
        {@render pagination()}
      </div>
    {/if}
  {/if}
