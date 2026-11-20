<!-- Port of the result menu of app-frame/global-search/global-search.component.html (resultItemTemplate,
     ngbDropdownMenu) including primaryAction/secondaryAction/keyboard control. Loaded only on the first search
     (own chunk); input field, debouncing and full-text search stay in GlobalSearch.svelte. -->
<script lang="ts">
  import '../../../styles/components/global-search.css'
  import type { Component } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { registerIcons, ricon } from '../../../lib/route-icons'
  import { formatDate } from '../../../lib/dates'
  import { locale } from '../../../lib/i18n'
  import { api } from '../../../api/client'
  import { router } from '../../../router.svelte'
  import { modals } from '../../../lib/modal.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { list } from '../../../stores/listView.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import {
    SECTIONS,
    EDIT_PRIMARY,
    FILTER_RULE_FOR,
    disablePrimary,
    disableSecondary,
    editDialogSize,
    filterDocumentsPath,
    hasSecondary,
    type GlobalSearchResult,
    type SectionKey,
  } from './global-search'
  import iTag from 'bootstrap-icons/icons/tag.svg?raw'
  import iPersonSquare from 'bootstrap-icons/icons/person-square.svg?raw'
  import iEnvelopeAt from 'bootstrap-icons/icons/envelope-at.svg?raw'

  registerIcons({ "tag": iTag, "person-square": iPersonSquare, "envelope-at": iEnvelopeAt })

  let {
    results,
    menuEl,
    onreset,
    onfocusinput,
  }: {
    results: GlobalSearchResult
    /** Popper action of the dropdown (Dropdown.menuEl) */
    menuEl: (node: HTMLElement) => any
    /** reset(true): clear search and close menu */
    onreset: () => void
    onfocusinput: () => void
  } = $props()

  const dateFormat = $derived(session.get(SETTINGS_KEYS.DATE_FORMAT))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())

  let container: HTMLElement | undefined = $state()
  let currentItemIndex = -1
  let domIndex = -1

  const canChange = (o: any) => session.hasObjectPermissions('change', o)

  const items = () => [...(container?.querySelectorAll<HTMLElement>('.dropdown-item') ?? [])]
  /** setCurrentItem: focus the main button of the current entry */
  function setCurrentItem() {
    const all = items()
    domIndex = currentItemIndex
    all[currentItemIndex]?.querySelector<HTMLButtonElement>('button[data-primary]')?.focus()
  }
  export function focusFirst() {
    currentItemIndex = 0
    setCurrentItem()
  }
  export function focusLast() {
    currentItemIndex = (results?.total ?? 0) - 1
    setCurrentItem()
  }
  /** Enter with exactly one hit: trigger the main button */
  export function clickFirstPrimary() {
    container?.querySelector<HTMLButtonElement>('button[data-primary]')?.click()
  }
  function onItemHover(e: MouseEvent) {
    currentItemIndex = items().indexOf(e.currentTarget as HTMLElement)
    setCurrentItem()
  }
  function onButtonHover(e: MouseEvent) {
    ;(e.currentTarget as HTMLElement).focus()
  }
  function dropdownKeyDown(e: KeyboardEvent) {
    if (!results?.total || document.activeElement === null) return
    const total = results.total
    const stop = () => {
      e.preventDefault()
      e.stopImmediatePropagation()
    }
    if (e.key === 'ArrowDown') {
      stop()
      if (currentItemIndex < total - 1) {
        currentItemIndex++
        setCurrentItem()
      } else {
        onfocusinput()
        currentItemIndex = -1
      }
    } else if (e.key === 'ArrowUp') {
      stop()
      if (currentItemIndex > 0) {
        currentItemIndex--
        setCurrentItem()
      } else {
        onfocusinput()
        currentItemIndex = -1
      }
    } else if (e.key === 'ArrowRight') {
      stop()
      items()[domIndex]?.querySelector<HTMLButtonElement>('button[data-secondary]')?.focus()
    } else if (e.key === 'ArrowLeft') {
      stop()
      items()[domIndex]?.querySelector<HTMLButtonElement>('button[data-primary]')?.focus()
    } else if (e.key === 'Escape') {
      stop()
      onreset()
      onfocusinput()
    }
  }
  function onButtonKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault()
      e.target?.dispatchEvent(new MouseEvent('click', { ctrlKey: true, bubbles: true, cancelable: true }))
    }
  }

  function navigateOrOpenInNewWindow(path: string, newWindow: boolean) {
    if (newWindow) window.open(router.href(path), '_blank')
    else router.navigate(path)
  }

  const DIALOGS: Partial<Record<SectionKey, () => Promise<{ default: Component<any> }>>> = {
    tags: () => import('../../common/edit-dialog/tag-edit-dialog/TagEditDialog.svelte'),
    correspondents: () => import('../../common/edit-dialog/correspondent-edit-dialog/CorrespondentEditDialog.svelte'),
    document_types: () => import('../../common/edit-dialog/document-type-edit-dialog/DocumentTypeEditDialog.svelte'),
    storage_paths: () => import('../../common/edit-dialog/storage-path-edit-dialog/StoragePathEditDialog.svelte'),
    users: () => import('../../common/edit-dialog/user-edit-dialog/UserEditDialog.svelte'),
    groups: () => import('../../common/edit-dialog/group-edit-dialog/GroupEditDialog.svelte'),
    mail_accounts: () => import('../../common/edit-dialog/mail-account-edit-dialog/MailAccountEditDialog.svelte'),
    mail_rules: () => import('../../common/edit-dialog/mail-rule-edit-dialog/MailRuleEditDialog.svelte'),
    custom_fields: () => import('../../common/edit-dialog/custom-field-edit-dialog/CustomFieldEditDialog.svelte'),
    workflows: () => import('../../common/edit-dialog/workflow-edit-dialog/WorkflowEditDialog.svelte'),
  }
  /** Refresh master data cache after editing (Angular: services reload on changes) */
  const REFRESH: Partial<Record<SectionKey, () => Promise<void>>> = {
    tags: async () => void (data.tags = new Map((await api.listAll<any>('tags')).results.map((x) => [x.id, x]))),
    correspondents: async () => void (data.correspondents = new Map((await api.listAll<any>('correspondents')).results.map((x) => [x.id, x]))),
    document_types: async () => void (data.documentTypes = new Map((await api.listAll<any>('document_types')).results.map((x) => [x.id, x]))),
    storage_paths: async () => void (data.storagePaths = new Map((await api.listAll<any>('storage_paths')).results.map((x) => [x.id, x]))),
    custom_fields: async () => void (data.customFields = (await api.listAll<any>('custom_fields')).results),
  }

  async function openEditDialog(type: SectionKey, object: any) {
    const loader = DIALOGS[type]
    if (!loader) return
    const C = (await loader()).default
    modals.open(
      C,
      {
        dialogMode: 'edit',
        object,
        onsucceeded: () => {
          toastService.showSuccess(t('search.object_updated'))
          REFRESH[type]?.().catch(() => {})
        },
        onfailed: (e: any) => toastService.showError(t('search.object_save_error'), e),
      },
      { size: editDialogSize(type) },
    )
  }

  function primaryAction(type: SectionKey, object: any, event: MouseEvent | null = null) {
    const newWindow = !!(event?.metaKey || event?.ctrlKey)
    onreset()
    if (type === 'documents') return navigateOrOpenInNewWindow(`documents/${object.id}`, newWindow)
    if (type === 'saved_views') return navigateOrOpenInNewWindow(`view/${object.id}`, newWindow)
    const ruleType = FILTER_RULE_FOR[type]
    if (ruleType != null) return navigateOrOpenInNewWindow(filterDocumentsPath(ruleType, object.id, list.sortField, list.sortReverse), newWindow)
    if (EDIT_PRIMARY.includes(type)) void openEditDialog(type, object)
  }

  function secondaryAction(type: SectionKey, object: any) {
    onreset()
    if (type === 'documents') window.open(api.downloadUrl(object.id))
    else if (FILTER_RULE_FOR[type] != null) void openEditDialog(type, object)
  }
</script>

{#snippet resultItem(item: any, type: SectionKey, nameProp: string, iconName: string, date: string | undefined)}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_mouse_events_have_key_events -->
  <div class="py-2 d-flex align-items-center focus-ring border-0 cursor-pointer dropdown-item" tabindex="-1"
    onclick={(e) => primaryAction(type, item, e)}
    onmouseenter={onItemHover}>
    <pngx-bs-icon width="1.2em" height="1.2em" name={iconName} class="me-2 text-muted">{@html ricon(iconName, '1.2em', '1.2em')}</pngx-bs-icon>
    <div class="text-truncate">
      {item[nameProp]}
      {#if date}
        <small class="small text-muted">{formatDate(date, dateFormat, dateLocale)}</small>
      {/if}
    </div>
    <div class="btn-group ms-auto">
      <button data-primary type="button" class="btn btn-sm btn-outline-primary d-flex"
        onclick={(e) => { primaryAction(type, item, e); e.stopImmediatePropagation() }}
        onkeydown={onButtonKeyDown}
        disabled={disablePrimary(type, canChange(item))}
        onmouseenter={onButtonHover}>
        {#if type === 'documents'}
          <pngx-bs-icon width="1em" height="1em" name="file-earmark-richtext" class="me-1">{@html icon('file-earmark-richtext', '1em', '1em')}</pngx-bs-icon><span>{t('common.open')}</span>
        {:else if type === 'saved_views'}
          <pngx-bs-icon width="1em" height="1em" name="eye" class="me-1">{@html icon('eye', '1em', '1em')}</pngx-bs-icon><span>{t('common.open')}</span>
        {:else if EDIT_PRIMARY.includes(type)}
          <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon><span>{t('common.open')}</span>
        {:else}
          <pngx-bs-icon width="1em" height="1em" name="filter" class="me-1">{@html icon('filter', '1em', '1em')}</pngx-bs-icon><span>{t('search.filter_documents')}</span>
        {/if}
      </button>
      {#if hasSecondary(type)}
        <button data-secondary type="button" class="btn btn-sm btn-outline-primary d-flex"
          onclick={(e) => { secondaryAction(type, item); e.stopImmediatePropagation() }}
          onkeydown={onButtonKeyDown}
          disabled={disableSecondary(type, canChange(item))}
          onmouseenter={onButtonHover}>
          {#if type === 'documents'}
            <pngx-bs-icon width="1em" height="1em" name="download" class="me-1">{@html icon('download', '1em', '1em')}</pngx-bs-icon><span>{t('common.download')}</span>
          {:else}
            <pngx-bs-icon width="1em" height="1em" name="file-earmark-richtext" class="me-1">{@html icon('file-earmark-richtext', '1em', '1em')}</pngx-bs-icon><span>{t('common.open')}</span>
          {/if}
        </button>
      {/if}
    </div>
  </div>
{/snippet}

<div class="dropdown-menu w-100 mh-75 overflow-y-scroll shadow-lg show" use:menuEl>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div bind:this={container} onkeydown={dropdownKeyDown}>
    {#if results?.total === 0}
      <h6 class="dropdown-header">{t('m.ui.no_results')}</h6>
    {:else}
      {#each SECTIONS as s (s.key)}
        {#if results?.[s.key]?.length}
          <h6 class="dropdown-header">{t(s.header)}</h6>
          {#each results[s.key] as item (item.id)}
            {@render resultItem(item, s.key, s.nameProp, s.icon, s.date ? item.created : undefined)}
          {/each}
        {/if}
      {/each}
    {/if}
  </div>
</div>
