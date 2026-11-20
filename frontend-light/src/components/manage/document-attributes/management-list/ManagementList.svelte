<!-- Port of manage/document-attributes/management-list/management-list.component.html (shared template of
     tag-/correspondent-/document-type-/storage-path-list). Host <pngx-tag-list> etc. in the parent markup; state and
     actions come from the parent component (DocumentAttributes also uses them for the header row). -->
<script lang="ts">
  import '../../../../styles/components/management-list.css'
  import { t, locale } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { Dropdown } from '../../../../lib/dropdown.svelte'
  import { formatDate } from '../../../../lib/dates'
  import { getMatching, type MatchingModel } from '../../../../lib/matching'
  import { quickFilterPath } from '../../../../lib/quick-filter'
  import { router } from '../../../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import Pagination from '../../../common/pagination/Pagination.svelte'
  import { truncatePath, type ManagementListColumn, type ManagementListState } from './management-list.svelte'
  import { openDeleteDialog, openEditDialog, setPageSize, userCanDelete, userCanEdit } from './actions'

  let { list }: { list: ManagementListState } = $props()
  const c = list.config
  const typeName = t(c.typeName)
  const typeNamePlural = t(c.typeNamePlural)
  const permissionType = c.permissionType

  /** CustomDatePipe (format/language from the UI settings) */
  function customDate(v: string) {
    let date = new Date(v)
    // very old dates with seconds in the offset cannot be parsed
    if (date.toString() == 'Invalid Date') date = new Date(v?.toString().replace(/([-+])(\d\d):\d\d:\d\d/gm, `$1$2:00`))
    return formatDate(date, session.get(SETTINGS_KEYS.DATE_FORMAT), session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  }

  const extraColumns: ManagementListColumn[] =
    c.kind === 'tags'
      ? [{ key: 'color', name: t('common.color'), badgeFn: (o: any) => ({ text: o.color, textColor: o.text_color, backgroundColor: o.color }) }]
      : c.kind === 'correspondents'
        ? [{ key: 'last_correspondence', name: t('manage.attributes.last_used'), valueFn: (o: any) => (o.last_correspondence ? customDate(o.last_correspondence) : '') }]
        : c.kind === 'storagepaths'
          ? [{ key: 'path', name: t('common.path'), hideOnMobile: true, monospace: true, valueFn: (o: any) => truncatePath(o.path) }]
          : []

  const menus = new Map<number, Dropdown>()
  const menu = (id: number) => {
    let d = menus.get(id)
    if (!d) menus.set(id, (d = new Dropdown()))
    return d
  }
  let nameFilterInput = $state(list.nameFilter ?? '')
  const pageSize = $derived(list.pageSize)

  function filterDocuments(e: Event, object: MatchingModel) {
    e.preventDefault()
    e.stopPropagation()
    router.navigate(quickFilterPath([{ rule_type: c.filterRuleType, value: object.id.toString() }]))
  }
  const filterHref = (object: MatchingModel) => router.href(quickFilterPath([{ rule_type: c.filterRuleType, value: object.id.toString() }]))
  function onCheckAll(e: Event) {
    const el = e.target as HTMLInputElement
    if (el.checked) list.selectPage()
    else list.clearSelection()
    e.stopPropagation()
  }
</script>

{#snippet objectRow(object: MatchingModel, depth: number)}
  {@const dd = menu(object.id)}
  <tr onclick={(e) => { list.toggleSelected(object); e.stopPropagation() }} class="data-row fade" class:show={list.show}>
    <td>
      <div class="form-check m-0 ms-2 me-n2">
        <input type="checkbox" class="form-check-input" id="{typeName}{object.id}" checked={list.selectedObjects.has(object.id)} onclick={(e) => { list.toggleSelected(object); e.stopPropagation() }}>
        <label class="form-check-label" for="{typeName}{object.id}"></label>
      </div>
    </td>
    <td class="name-cell" style="--depth: {depth}">
      {#if depth > 0}
        <div class="indicator"></div>
      {/if}
      <button class="btn btn-link ms-0 ps-0 text-start" style={"user-select: text;"} disabled={!userCanEdit(object)} onclick={(e) => { if (userCanEdit(object)) openEditDialog(list, object); e.stopPropagation() }}>{object.name}</button>
    </td>
    <td class="d-none d-sm-table-cell">{getMatching(object)}</td>
    <td>{list.getDocumentCount(object)}</td>
    {#each extraColumns as column (column)}
      <td class:d-none={column.hideOnMobile} class:d-sm-table-cell={column.hideOnMobile}>
        {#if column.badgeFn}
          <span
            class="badge"
            style:color={column.badgeFn(object)?.textColor}
            style:background-color={column.badgeFn(object)?.backgroundColor}
          >
            {column.badgeFn(object)?.text}
          </span>
        {:else if column.monospace}
          <span class="font-monospace">{column.valueFn?.(object)}</span>
        {:else}
          {column.valueFn?.(object)}
        {/if}
      </td>
    {/each}
    <td>
      <div class="btn-toolbar gap-2">
        <div class="btn-group d-block d-sm-none">
          <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
            <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
              <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
            </button>
            {#if dd.open}
              <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:dd.menuEl>
                {#if session.can('change', permissionType)}
                  <button onclick={() => openEditDialog(list, object)} class="dropdown-item">{t('common.edit')}</button>
                {/if}
                {#if session.can('delete', permissionType)}
                  <button class="text-danger dropdown-item" onclick={() => openDeleteDialog(list, object)}>{t('common.delete')}</button>
                {/if}
                {#if list.getDocumentCount(object) > 0}
                  {#if session.can('view', 'document')}
                    <a class="dropdown-item" href={filterHref(object)} onclick={(e) => filterDocuments(e, object)}>{t('manage.attributes.filter_documents', { INTERPOLATION: list.getDocumentCount(object) })}</a>
                  {/if}
                {/if}
              </div>
            {/if}
          </div>
        </div>
        <div class="btn-group d-none d-sm-inline-block">
          {#if session.can('change', permissionType)}
            <button class="btn btn-sm btn-outline-secondary" onclick={(e) => { openEditDialog(list, object); e.stopPropagation() }} disabled={!userCanEdit(object)}>
              <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
            </button>
          {/if}
          {#if session.can('delete', permissionType)}
            <button class="btn btn-sm btn-outline-danger" onclick={(e) => { openDeleteDialog(list, object); e.stopPropagation() }} disabled={!userCanDelete(object)}>
              <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
            </button>
          {/if}
        </div>
        {#if list.getDocumentCount(object) > 0}
          <div class="btn-group d-none d-sm-inline-block">
            {#if session.can('view', 'document')}
              <a
                class="btn btn-sm btn-outline-secondary"
                href={filterHref(object)}
                onclick={(e) => filterDocuments(e, object)}
              >
                <pngx-bs-icon width="1em" height="1em" name="filter" class="me-1">{@html icon('filter', '1em', '1em')}</pngx-bs-icon>{t('common.documents_2')}<span class="badge bg-light text-secondary ms-2">{list.getDocumentCount(object)}</span>
              </a>
            {/if}
          </div>
        {/if}
      </div>
    </td>
  </tr>

  {#if object.children && object.children.length > 0}
    {#each object.children as child (child)}
      {@render objectRow(list.getOriginalObject(child), depth + 1)}
    {/each}
  {/if}
{/snippet}

<div class="row mb-3">
  <div class="col mb-2 mb-xl-0">
    <div class="form-inline d-flex align-items-center">
      <label class="me-2 mb-0" for="managementNameFilter">{t('manage.attributes.filter_label')}</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input id="managementNameFilter" class="form-control form-control-sm flex-fill w-auto" type="text" autofocus bind:value={nameFilterInput} oninput={() => list.setNameFilter(nameFilterInput)} onkeyup={(e) => { if (e.code == 'Escape') list.setNameFilter(null) }} placeholder={t('common.name')}>
    </div>
  </div>

  <div class="col-auto mb-2 mb-xl-0">
    <div class="form-inline d-flex align-items-center">
      <div class="input-group input-group-sm w-auto d-none d-md-flex">
        <label class="input-group-text bg-transparent border-0" for="managementPageSize">{t('manage.attributes.show_label')}</label>
      </div>
      <div class="input-group input-group-sm w-auto">
        <select id="managementPageSize" class="form-select form-select-sm small" value={pageSize} onchange={(e) => setPageSize(list, +(e.target as HTMLSelectElement).value)}>
          <option value={25}>25</option>
          <option value={50}>50</option>
          <option value={100}>100</option>
        </select>
        <span class="input-group-text text-muted d-none d-md-flex">{t('manage.attributes.per_page')}</span>
      </div>
      {#if list.collectionSize > pageSize}
        <ngb-pagination class="ms-3" size="sm" aria-label="Pagination" role="navigation"><Pagination pageSize={pageSize} collectionSize={list.collectionSize} page={list.page} maxSize={5} onpage={(p) => list.setPage(p)} /></ngb-pagination>
      {/if}
    </div>
  </div>

</div>

<div class="card border table-responsive mb-3">
  <table class="table table-striped align-middle shadow-sm mb-0">
    <thead>
      <tr>
        <th>
          <div class="form-check m-0 ms-2 me-n2">
            <input type="checkbox" class="form-check-input" id="all-objects" checked={list.toggleAll} disabled={list.data.length === 0} onchange={onCheckAll}>
            <label class="form-check-label" for="all-objects"></label>
          </div>
        </th>
        <th class="fw-normal {list.sortClass('name')}" pngxSortable="name" onclick={() => list.rotateSort('name')}>{t('common.name')}</th>
        <th class="fw-normal d-none d-sm-table-cell {list.sortClass('matching_algorithm')}" pngxSortable="matching_algorithm" onclick={() => list.rotateSort('matching_algorithm')}>{t('manage.attributes.matching')}</th>
        <th class="fw-normal {list.sortClass('document_count')}" pngxSortable="document_count" onclick={() => list.rotateSort('document_count')}>{t('manage.attributes.document_count')}</th>
        {#each extraColumns as column (column)}
          <th class="fw-normal {list.sortClass(column.key)}" class:d-none={column.hideOnMobile} class:d-sm-table-cell={column.hideOnMobile} onclick={() => list.rotateSort(column.key)}>{column.name}</th>
        {/each}
        <th class="fw-normal">{t('common.actions')}</th>
      </tr>
    </thead>
    <tbody>
      {#if list.loading && list.data.length === 0}
        <tr>
          <td colspan="5">
            <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
          </td>
        </tr>
      {/if}
      {#each list.data as object (object)}
        {@render objectRow(object, 0)}
      {/each}
    </tbody>
  </table>
</div>

{#if !list.loading || list.data.length > 0}
  <div class="d-flex mb-2">
    {#if list.displayCollectionSize > 0}
      <div>{t('manage.attributes.total', { VAR_PLURAL: list.displayCollectionSize, INTERPOLATION: typeName, INTERPOLATION_1: list.displayCollectionSize || 0, INTERPOLATION_2: typeNamePlural })}{#if list.hasSelection}
          &nbsp;({list.selectedCount} selected)
        {/if}
      </div>
    {/if}
    {#if list.collectionSize > pageSize}
      <ngb-pagination class="ms-auto" size="sm" aria-label="Pagination" role="navigation"><Pagination pageSize={pageSize} collectionSize={list.collectionSize} page={list.page} maxSize={5} onpage={(p) => list.setPage(p)} /></ngb-pagination>
    {/if}
  </div>
{/if}
