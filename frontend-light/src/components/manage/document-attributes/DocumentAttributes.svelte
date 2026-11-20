<!-- Port of manage/document-attributes/document-attributes.component.html (route attributes/:section).
     Host <pngx-document-attributes> in the markup of App.svelte. -->
<script lang="ts">
  import { untrack } from 'svelte'
  import { t, msg } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { router } from '../../../router.svelte'
  import { session } from '../../../stores/settings.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ClearableBadge from '../../common/clearable-badge/ClearableBadge.svelte'
  import ManagementList from './management-list/ManagementList.svelte'
  import CustomFields from './custom-fields/CustomFields.svelte'
  import { CustomFieldsState } from './custom-fields/custom-fields.svelte'
  import type { ListKind, ManagementListState } from './management-list/management-list.svelte'
  import { bulkDelete, createListState, openCreateDialog, setPermissions, userCanBulkEdit } from './management-list/actions'

  interface Section {
    id: number
    path: string
    label: string
    icon: string
    infoLink: string
    permissionType: string
    kind: 'managementList' | 'customFields'
    host: string
  }
  const SECTIONS: Section[] = [
    { id: 1, path: 'tags', label: msg('common.tags'), icon: icon('tags'), infoLink: 'usage/#terms-and-definitions', permissionType: 'tag', kind: 'managementList', host: 'pngx-tag-list' },
    { id: 2, path: 'correspondents', label: msg('common.correspondents'), icon: icon('person'), infoLink: 'usage/#terms-and-definitions', permissionType: 'correspondent', kind: 'managementList', host: 'pngx-correspondent-list' },
    { id: 3, path: 'documenttypes', label: msg('common.document_types'), icon: icon('hash'), infoLink: 'usage/#terms-and-definitions', permissionType: 'documenttype', kind: 'managementList', host: 'pngx-document-type-list' },
    { id: 4, path: 'storagepaths', label: msg('common.storage_paths'), icon: icon('folder'), infoLink: 'usage/#terms-and-definitions', permissionType: 'storagepath', kind: 'managementList', host: 'pngx-storage-path-list' },
    { id: 5, path: 'customfields', label: msg('common.custom_fields'), icon: icon('ui-radios'), infoLink: 'usage/#custom-fields', permissionType: 'customfield', kind: 'customFields', host: 'pngx-custom-fields' },
  ]
  const visibleSections = $derived(SECTIONS.filter((s) => session.can('view', s.permissionType)))
  const sectionParam = $derived(router.path.split('/')[1]?.toLowerCase() ?? '')
  const activeSection = $derived(visibleSections.find((s) => s.path === sectionParam) ?? null)

  // paramMap: without/invalid section → first visible section (replaceUrl), without permissions → dashboard
  $effect(() => {
    if (!router.path.startsWith('attributes')) return
    if (!session.data) return
    if (!activeSection) {
      const def = visibleSections[0]
      untrack(() => router.navigate(def ? `attributes/${def.path}` : 'dashboard', true))
    }
  })

  // Section change = new component (ngComponentOutlet) → new state, load immediately
  let list = $state<ManagementListState | null>(null)
  let cf = $state<CustomFieldsState | null>(null)
  $effect(() => {
    const s = activeSection
    untrack(() => {
      list = null
      cf = null
      if (s?.kind === 'managementList') {
        list = createListState(s.path as ListKind)
        list.reloadData()
      } else if (s?.kind === 'customFields') {
        cf = new CustomFieldsState()
        cf.reload()
      }
    })
  })

  const headerLoading = $derived(list ? list.loading : cf ? cf.loading : false)
  const ddSelect = new Dropdown()

  function onNavChange(e: Event, s: Section) {
    e.preventDefault()
    if (s.id === activeSection?.id) return
    router.navigate(`attributes/${s.path}`)
  }
</script>

<pngx-page-header>
<PageHeader
  title={activeSection ? t(activeSection.label) : ''}
  info={t('manage.attributes.manage_tags_correspondents_document')}
  infoLink={activeSection?.infoLink ?? undefined}
  loading={headerLoading}
  >
  {#if list}
    <div class="btn-group flex-fill d-sm-none {ddSelect.placementClass}" class:show={ddSelect.open} use:ddSelect.hostEl>
      <button class="btn btn-sm btn-outline-primary dropdown-toggle" id="dropdownSelectMobile" aria-label={t('common.select')} aria-expanded={ddSelect.open} use:ddSelect.anchorEl onclick={ddSelect.toggle}>
        <pngx-bs-icon name="text-indent-left">{@html icon('text-indent-left')}</pngx-bs-icon><div class="d-none d-sm-inline ms-1">{t('common.select')}</div>
        {#if list.hasSelection}
          <pngx-clearable-badge><ClearableBadge selected={list.hasSelection} number={list.selectedCount} oncleared={() => list?.selectNone()} /></pngx-clearable-badge><span class="visually-hidden">selected</span>
        {/if}
      </button>
      {#if ddSelect.open}
        <div aria-labelledby="dropdownSelectMobile" class="shadow dropdown-menu show" use:ddSelect.menuEl>
          <button class="dropdown-item" onclick={() => list?.selectNone()}>{t('common.select_none')}</button>
          <button class="dropdown-item" onclick={() => list?.selectPage()}>{t('common.select_page')}</button>
          <button class="dropdown-item" onclick={() => list?.selectAll()}>{t('common.select_all')}</button>
        </div>
      {/if}
    </div>

    <div class="d-none d-sm-flex flex-fill me-3">
      <div class="input-group input-group-sm">
        <span class="input-group-text border-0">{t('common.select_label')}</span>
      </div>
      <div class="btn-group btn-group-sm flex-nowrap">
        {#if list.hasSelection}
          <button class="btn btn-sm btn-outline-secondary" onclick={() => list?.selectNone()}>
            <pngx-bs-icon name="slash-circle" class="me-1">{@html icon('slash-circle')}</pngx-bs-icon>{t('common.none')}
          </button>
        {/if}
        <button class="btn btn-sm btn-outline-primary" onclick={() => list?.selectPage()}>
          <pngx-bs-icon name="file-earmark-check" class="me-1">{@html icon('file-earmark-check')}</pngx-bs-icon>{t('common.page')}
        </button>
        <button class="btn btn-sm btn-outline-primary" onclick={() => list?.selectAll()}>
          <pngx-bs-icon name="check-all" class="me-1">{@html icon('check-all')}</pngx-bs-icon>{t('common.all')}
        </button>
      </div>
    </div>

    <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => list && setPermissions(list)}
      disabled={!userCanBulkEdit(list, 'change') || !list.hasSelection}>
      <pngx-bs-icon name="person-fill-lock" class="me-1">{@html icon('person-fill-lock')}</pngx-bs-icon>{t('common.permissions')}
    </button>
    <button type="button" class="btn btn-sm btn-outline-danger" onclick={() => list && bulkDelete(list)}
      disabled={!userCanBulkEdit(list, 'delete') || !list.hasSelection}>
      <pngx-bs-icon name="trash" class="me-1">{@html icon('trash')}</pngx-bs-icon>{t('common.delete')}
    </button>
    {#if session.can('add', list.config.permissionType)}
      <button type="button" class="btn btn-sm btn-outline-primary ms-md-5" onclick={() => list && openCreateDialog(list)}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('common.create')}
      </button>
    {/if}
  {:else if cf}
    {#if session.can('add', 'customfield')}
      <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => cf?.editField(null)}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('common.add_field')}
      </button>
    {/if}
  {/if}
</PageHeader>
</pngx-page-header>

<ul class="nav-underline nav" role="tablist">
  {#each visibleSections as section (section.id)}
    <li class="nav-item" role="presentation">
      <a href="" class="nav-link" class:active={activeSection?.id === section.id} role="tab" id="ngb-nav-{section.id - 1}" aria-controls={activeSection?.id === section.id ? `ngb-nav-${section.id - 1}-panel` : null} aria-selected={activeSection?.id === section.id} aria-disabled="false" onclick={(e) => onNavChange(e, section)}>
        <pngx-bs-icon class="me-2" name={section.path}>{@html section.icon}</pngx-bs-icon>{t(section.label)}
      </a>
    </li>
  {/each}
</ul>

<div class="my-3">
  {#if list && activeSection}
    {#key list}
      {#if activeSection.host === 'pngx-tag-list'}
        <pngx-tag-list><ManagementList {list} /></pngx-tag-list>
      {:else if activeSection.host === 'pngx-correspondent-list'}
        <pngx-correspondent-list><ManagementList {list} /></pngx-correspondent-list>
      {:else if activeSection.host === 'pngx-document-type-list'}
        <pngx-document-type-list><ManagementList {list} /></pngx-document-type-list>
      {:else}
        <pngx-storage-path-list><ManagementList {list} /></pngx-storage-path-list>
      {/if}
    {/key}
  {:else if cf}
    <pngx-custom-fields><CustomFields {cf} /></pngx-custom-fields>
  {/if}
</div>
