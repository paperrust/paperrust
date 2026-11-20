<!-- Port of admin/settings/settings.component.html (tabs General, Documents, Permissions,
     Notifications). Host <pngx-settings> in the markup of App.svelte. Tour (lib/tour) and system status dialog
     (common/system-status-dialog) are only loaded on click. -->
<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { formatDate } from '../../../lib/dates'
  import { applyAppearance } from '../../../lib/appearance'
  import { Popover } from '../../../lib/popover.svelte'
  import { modals } from '../../../lib/modal.svelte'
  import { router } from '../../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { sidebarEditing } from '../../../stores/sidebarEditing.svelte'
  import { systemStatus as fetchSystemStatus } from '../../../api/objects'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import InputCheck from '../../common/input/check/InputCheck.svelte'
  import InputColor from '../../common/input/color/InputColor.svelte'
  import InputSelect from '../../common/input/select/InputSelect.svelte'
  import PermissionsUser from '../../common/input/permissions/permissions-user/PermissionsUser.svelte'
  import PermissionsGroup from '../../common/input/permissions/permissions-group/PermissionsGroup.svelte'
  import {
    currentSettings,
    dateLocaleOptions,
    displayLanguageOptions,
    DOCUMENT_DETAIL_FIELD_OPTIONS,
    isDirty,
    saveSettings,
    SIDEBAR_ITEM_OPTIONS,
  } from './settings-form.svelte'

  const NAV = { general: 1, documents: 2, permissions: 3, notifications: 4 } as const
  type NavKey = keyof typeof NAV
  const NAV_KEYS = Object.keys(NAV) as NavKey[]

  const sectionParam = $derived((router.path.split('/')[1] ?? '').toLowerCase())
  const activeNav = $derived<NavKey>(NAV_KEYS.includes(sectionParam as NavKey) ? (sectionParam as NavKey) : 'general')

  let store = $state(currentSettings())
  let form = $state(currentSettings())
  const dirty = $derived(isDirty(form, store))
  let savePending = false

  const currentLocale = locale()
  const today = new Date()
  const computedDateLocale = $derived(form.dateLocale || form.displayLanguage || currentLocale)
  const languages = displayLanguageOptions()
  const dateLocales = dateLocaleOptions()
  const aiEnabled = $derived(session.get(SETTINGS_KEYS.AI_ENABLED))
  const users = $derived([...data.users.values()])
  const canViewSystemStatus = $derived(session.isAdmin || session.can('view', 'systemmonitoring'))
  let systemStatus = $state<any>(undefined)
  // SettingsComponent.showSystemStatus (NgbModal size xl) / TourService.start – both loaded lazily
  function showSystemStatus() {
    import('../../common/system-status-dialog/SystemStatusDialog.svelte').then((m) => modals.open(m.default, { status: systemStatus }, { size: 'xl' }))
  }
  function startTour() {
    // in the PaperRust design a separate tour (design2/tour); the classic tour does not find its anchors there
    if (document.documentElement.classList.contains('d2')) return void dispatchEvent(new Event('pr:start-tour'))
    import('../../../lib/tour/tour.svelte').then((m) => m.tour.start())
  }
  const systemStatusHasErrors = $derived.by(() => {
    const s = systemStatus
    if (!s) return false
    const E = 'ERROR'
    return (
      s.database?.status === E ||
      s.tasks?.redis_status === E ||
      s.tasks?.celery_status === E ||
      s.tasks?.index_status === E ||
      s.tasks?.classifier_status === E ||
      s.tasks?.sanity_check_status === E ||
      s.websocket_connected === E
    )
  })
  $effect(() => {
    if (canViewSystemStatus) untrack(() => fetchSystemStatus().then((s) => (systemStatus = s)).catch(() => {}))
  })

  // "Live" preview of the appearance before saving
  $effect(() => {
    const [a, b, c] = [form.darkModeUseSystem, form.darkModeEnabled, form.themeColor]
    untrack(() => applyAppearance(a, b, c))
  })

  // Sidebar switches only in the "General" tab
  $effect(() => {
    const general = activeNav === 'general'
    untrack(() => (sidebarEditing.hiddenItemsEditing = general ? [...(form.sidebarHiddenItems ?? [])] : null))
  })
  const onSidebarChanged = (items: string[]) => (form.sidebarHiddenItems = items)
  sidebarEditing.listeners.add(onSidebarChanged)

  onDestroy(() => {
    sidebarEditing.listeners.delete(onSidebarChanged)
    sidebarEditing.hiddenItemsEditing = null
    // Reset appearance if not saved
    if (isDirty($state.snapshot(form), $state.snapshot(store)))
      applyAppearance(session.get(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM), session.get(SETTINGS_KEYS.DARK_MODE_ENABLED), session.get(SETTINGS_KEYS.THEME_COLOR))
  })

  function onNavChange(e: Event, key: NavKey) {
    e.preventDefault()
    if (key !== activeNav) router.navigate(`settings/${key}`)
  }

  const isSidebarItemShown = (id: string) => !(form.sidebarHiddenItems || []).includes(id)
  function toggleSidebarItem(id: string, checked: boolean) {
    sidebarEditing.updateVisibility(id, checked)
  }
  const isDocumentDetailFieldShown = (id: string) => !(form.documentDetailsHiddenFields || []).includes(id)
  function toggleDocumentDetailField(id: string, checked: boolean) {
    const hidden = new Set<string>(form.documentDetailsHiddenFields || [])
    if (checked) hidden.delete(id)
    else hidden.add(id)
    form.documentDetailsHiddenFields = Array.from(hidden)
  }

  function reset() {
    form = { ...$state.snapshot(store) }
    if (sidebarEditing.organizing) sidebarEditing.hiddenItemsEditing = [...(form.sidebarHiddenItems ?? [])]
  }
  function clearThemeColor() {
    form.themeColor = ''
  }

  async function submit(e: Event) {
    e.preventDefault()
    savePending = true
    try {
      const reloadRequired = await saveSettings($state.snapshot(form), $state.snapshot(store))
      store = { ...$state.snapshot(form) }
      applyAppearance(session.get(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM), session.get(SETTINGS_KEYS.DARK_MODE_ENABLED), session.get(SETTINGS_KEYS.THEME_COLOR))
      if (reloadRequired)
        toastService.show({
          content: t('admin.settings.settings_saved_successfully_reload'),
          delay: 5000,
          level: 'success',
          persistent: true,
          actionName: t('admin.settings.reload_now'),
          action: () => location.reload(),
        })
      else toastService.show({ content: t('admin.settings.settings_saved_successfully'), delay: 5000, level: 'success', persistent: false })
    } catch (error) {
      toastService.showError(t('admin.settings.error_occurred_while_saving'), error)
    } finally {
      savePending = false
    }
  }

  const updatesPopover = new Popover({ triggers: 'mouseenter:mouseleave', autoClose: true })
</script>

{#snippet navLink(key: NavKey, label: string)}
  <a href="" class="nav-link" class:active={activeNav === key} role="tab" id="ngb-nav-{NAV[key] - 1}" aria-controls={activeNav === key ? `ngb-nav-${NAV[key] - 1}-panel` : null} aria-selected={activeNav === key} aria-disabled="false" onclick={(e) => onNavChange(e, key)}>{label}</a>
{/snippet}

<pngx-page-header>
<PageHeader
  title={t('common.settings')}
  info={t('admin.settings.options_customize_appearance_notifications')}
  >
  <button class="btn btn-sm btn-outline-primary" onclick={startTour}>
    <pngx-bs-icon class="me-2" name="airplane">{@html icon('airplane')}</pngx-bs-icon>{t('admin.settings.start_tour')}
  </button>
  {#if canViewSystemStatus}
    <button class="btn btn-sm btn-outline-primary position-relative ms-md-5 me-1" onclick={showSystemStatus}
      disabled={!systemStatus}>
      {#if !systemStatus}
        <div class="spinner-border spinner-border-sm me-2 h-75" role="status"></div>
      {:else}
        <pngx-bs-icon class="me-2" name="card-checklist">{@html icon('card-checklist')}</pngx-bs-icon>
        {#if systemStatusHasErrors}
          <span class="badge bg-body position-absolute top-0 start-100 translate-middle rounded-pill p-0">
            <pngx-bs-icon name="exclamation-circle-fill" class="text-danger" width="1.75em" height="1.75em">{@html icon('exclamation-circle-fill', '1.75em', '1.75em')}</pngx-bs-icon>
          </span>
        {:else}
          <span class="badge bg-body position-absolute top-0 start-100 translate-middle rounded-pill p-0">
            <pngx-bs-icon name="check-circle-fill" class="text-success" width="1.75em" height="1.75em">{@html icon('check-circle-fill', '1.75em', '1.75em')}</pngx-bs-icon>
          </span>
        {/if}
      {/if}{t('admin.settings.system_status')}
    </button>
  {/if}
</PageHeader>
</pngx-page-header>

<form onsubmit={submit}>

  <ul class="nav-tabs nav" role="tablist">
    <li class="nav-item" role="presentation">
      {@render navLink('general', t('common.general'))}
    </li>

    <li class="nav-item" role="presentation">
      {@render navLink('documents', t('common.documents_2'))}
    </li>

    <li class="nav-item" role="presentation">
      {@render navLink('permissions', t('common.permissions'))}
    </li>

    <li class="nav-item" role="presentation">
      {@render navLink('notifications', t('common.notifications'))}
    </li>

    {#if session.data?.user?.is_superuser || session.can('view', 'applicationconfiguration')}
      <!-- PaperRust: "System" tab (docs/SETTINGS.md §4.1), own route settings/system -->
      <li class="nav-item" role="presentation">
        <a href="" class="nav-link" role="tab" onclick={(e) => { e.preventDefault(); router.navigate('settings/system') }}>{t('common.system')}</a>
      </li>
    {/if}
  </ul>

  <div class="border-start border-end border-bottom p-3 mb-3 shadow-sm">
    <div class="tab-pane fade show active" role="tabpanel" id="ngb-nav-{NAV[activeNav] - 1}-panel" aria-labelledby="ngb-nav-{NAV[activeNav] - 1}">
      {#if activeNav === 'general'}
        <div class="row">
          <div class="col-xl-6 pe-xl-5">
            <h5>{t('common.appearance')}</h5>
            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.display_language')}</span>
              </div>
              <div class="col">

                <select class="form-select" bind:value={form.displayLanguage}>
                  {#each languages as lang (lang)}
                    <option value={lang.code}>{lang.name}{#if lang.code && currentLocale !== 'en-US'} - {lang.englishName}{/if}</option>
                  {/each}
                </select>

                {#if form.displayLanguage != store.displayLanguage}
                  <small class="form-text text-primary">{t('admin.settings.need_reload_page_after')}</small>
                {/if}

              </div>
            </div>

            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.date_display')}</span>
              </div>
              <div class="col">

                <select class="form-select" bind:value={form.dateLocale}>
                  {#each dateLocales as lang (lang)}
                    <option value={lang.code}>{lang.name}{#if lang.code} - {formatDate(today, 'shortDate', lang.code)}{/if}</option>
                  {/each}
                </select>

              </div>
            </div>

            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.date_format')}</span>
              </div>
              <div class="col">

                <div class="form-check">
                  <input type="radio" id="dateFormatShort" name="dateFormat" class="form-check-input" bind:group={form.dateFormat} value="shortDate">
                  <label class="form-check-label" for="dateFormatShort">{t('admin.settings.short', { INTERPOLATION: formatDate(today, 'shortDate', computedDateLocale) })}</label>
                </div>
                <div class="form-check">
                  <input type="radio" id="dateFormatMedium" name="dateFormat" class="form-check-input" bind:group={form.dateFormat} value="mediumDate">
                  <label class="form-check-label" for="dateFormatMedium">{t('admin.settings.medium', { INTERPOLATION: formatDate(today, 'mediumDate', computedDateLocale) })}</label>
                </div>
                <div class="form-check">
                  <input type="radio" id="dateFormatLong" name="dateFormat" class="form-check-input" bind:group={form.dateFormat} value="longDate">
                  <label class="form-check-label" for="dateFormatLong">{t('admin.settings.long', { INTERPOLATION: formatDate(today, 'longDate', computedDateLocale) })}</label>
                </div>

              </div>
            </div>

            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.sidebar')}</span>
              </div>
              <div class="col">

                <pngx-input-check><InputCheck title={t("admin.settings.use_slim_sidebar_icons")} bind:value={form.slimSidebarEnabled} /></pngx-input-check>

                <p class="mb-2 mt-3">{t('admin.settings.sidebar_items_show_label')}</p>
                {#each SIDEBAR_ITEM_OPTIONS as option (option.id)}
                  <div class="form-check">
                    <input
                      class="form-check-input"
                      type="checkbox"
                      id={'sidebar-item-setting-' + option.id}
                      checked={isSidebarItemShown(option.id)}
                      onchange={(e) => toggleSidebarItem(option.id, (e.target as HTMLInputElement).checked)}
                    />
                    <label class="form-check-label" for={'sidebar-item-setting-' + option.id}>
                      {t(option.label)}
                    </label>
                  </div>
                {/each}

              </div>
            </div>

            <div class="row">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.dark_mode')}</span>
              </div>
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.use_system_settings')} bind:value={form.darkModeUseSystem} /></pngx-input-check>
                <pngx-input-check hidden={form.darkModeUseSystem || undefined}><InputCheck title={t('admin.settings.enable_dark_mode')} bind:value={form.darkModeEnabled} /></pngx-input-check>
                <pngx-input-check><InputCheck title={t('admin.settings.invert_thumbnails_in_dark')} bind:value={form.darkModeInvertThumbs} /></pngx-input-check>
              </div>
            </div>

            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.theme_color')}</span>
              </div>
              <div class="col col-md-4">
                <pngx-input-color><InputColor bind:value={form.themeColor} /></pngx-input-color>
              </div>
              <div class="col-2">
                <button type="button" class="btn btn-link btn-sm pt-2 ps-0" disabled={!form.themeColor} onclick={clearThemeColor}>
                  <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon>{t('common.reset')}
                </button>
              </div>
            </div>
          </div>
          <div class="col-xl-6 ps-xl-5">
            <h5 class="mt-3 mt-md-0">{t('admin.settings.global_search')}</h5>
            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.not_include_advanced_search')} bind:value={form.searchDbOnly} /></pngx-input-check>
              </div>
            </div>

            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <label for="searchLink">{t('admin.settings.full_search_links')}</label>
              </div>
              <div class="col mb-3">
                <select id="searchLink" class="form-select" bind:value={form.searchLink}>
                  <option value="title-content">{t('admin.settings.title_content_search')}</option>
                  <option value="advanced">{t('common.advanced_search')}</option>
                </select>
              </div>
            </div>

            <h5 class="mt-3 mt-md-0" id="update-checking">{t('admin.settings.update_checking')}</h5>
            <div class="row mb-3">
              <div class="col d-flex flex-row align-items-start">
                <pngx-input-check><InputCheck title={t('admin.settings.enable_update_checking')} bind:value={form.updateCheckingEnabled} /></pngx-input-check>
                <button class="btn btn-sm btn-link text-muted me-auto p-0 ms-2" title={t("common.whats")} type="button" use:updatesPopover.triggerEl>
                  <pngx-bs-icon name="question-circle">{@html icon('question-circle')}</pngx-bs-icon>
                </button>
                {#if updatesPopover.open}
                  <ngb-popover-window role="tooltip" class="popover" id={updatesPopover.id} use:updatesPopover.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
                    <p>
                      {t('admin.settings.update_checking_works_pinging')}
                    </p>
                    <p class="mb-0">
                      <em>{t('admin.settings.no_tracking_data_collected')}</em>
                    </p>
                  </div></ngb-popover-window>
                {/if}
              </div>
            </div>

            <h5 class="mt-3">{t('common.saved_views')}</h5>
            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.show_warning_closing_saved')} bind:value={form.savedViewsWarnOnUnsavedChange} /></pngx-input-check>
                <pngx-input-check><InputCheck title={t('admin.settings.show_document_counts_in')} bind:value={form.sidebarViewsShowCount} /></pngx-input-check>
              </div>
            </div>
          </div>
        </div>

      {:else if activeNav === 'documents'}
        <div class="row">
          <div class="col-xl-6 pe-xl-5">
            <h5>{t('common.documents_2')}</h5>
            <div class="row mb-3">
              <div class="col-md-3 col-form-label pt-0">
                <label for="documentListItemPerPage">{t('admin.settings.items_per_page')}</label>
              </div>
              <div class="col">
                <select id="documentListItemPerPage" class="form-select" bind:value={form.documentListItemPerPage}>
                  <option value={10}>10</option>
                  <option value={25}>25</option>
                  <option value={50}>50</option>
                  <option value={100}>100</option>
                </select>
              </div>
            </div>

            <h5 class="mt-3">{t('admin.settings.document_editing')}</h5>
            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.use_pdf_viewer_provided')} hint={t('admin.settings.usually_faster_displaying_large')} bind:value={form.useNativePdfViewer} /></pngx-input-check>
              </div>
            </div>

            <div class="row">
              <div class="col-md-3 col-form-label pt-0">
                <span>{t('admin.settings.default_zoom')}</span>
              </div>
              <div class="col">
                <select class="form-select" bind:value={form.pdfViewerDefaultZoom}>
                  <option value="page-width">{t('common.fit_width')}</option>
                  <option value="page-fit">{t('common.fit_page')}</option>
                </select>
                <p class="small text-muted mt-1">{t('admin.settings.only_applies_paperrust_pdf')}</p>
              </div>
            </div>

            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.automatically_remove_inbox_tag')} bind:value={form.documentEditingRemoveInboxTags} /></pngx-input-check>
              </div>
            </div>

            {#if aiEnabled}
              <div class="row mb-3">
                <div class="col-md-3 col-form-label pt-0">
                  <span>{t('admin.settings.suggestions_default')}</span>
                </div>
                <div class="col">
                  <fieldset class="btn-group btn-group-sm">
                    <input type="radio" class="btn-check" id="suggestionSourceBoth" value="both" bind:group={form.documentEditingSuggestionSource}>
                    <label class="btn btn-outline-primary" for="suggestionSourceBoth">{t('admin.settings.both')}</label>
                    <input type="radio" class="btn-check" id="suggestionSourceML" value="ml" bind:group={form.documentEditingSuggestionSource}>
                    <label class="btn btn-outline-primary" for="suggestionSourceML"><pngx-bs-icon class="me-1" name="cpu">{@html icon('cpu')}</pngx-bs-icon>{t('admin.settings.ml_only')}</label>
                    <input type="radio" class="btn-check" id="suggestionSourceAI" value="ai" bind:group={form.documentEditingSuggestionSource}>
                    <label class="btn btn-outline-primary" for="suggestionSourceAI"><pngx-bs-icon class="me-1" name="stars">{@html icon('stars')}</pngx-bs-icon>{t('admin.settings.ai_only')}</label>
                  </fieldset>
                </div>
              </div>
            {/if}

            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.automatically_request_suggestions_inbox')} hint={t('admin.settings.un_checked_suggestions_must')} bind:value={form.documentEditingAutoSuggest} /></pngx-input-check>
              </div>
            </div>

            <div class="row">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.show_document_thumbnail_during')} bind:value={form.documentEditingOverlayThumbnail} /></pngx-input-check>
              </div>
            </div>

            <div class="row mb-3">
              <div class="col">
                <p class="mb-2">{t('admin.settings.built_in_fields_show_label')}</p>
                {#each DOCUMENT_DETAIL_FIELD_OPTIONS as option (option.id)}
                  <div class="form-check ms-3">
                    <input class="form-check-input" type="checkbox"
                      id={'documentDetailField-' + option.id}
                      checked={isDocumentDetailFieldShown(option.id)}
                      onchange={(e) => toggleDocumentDetailField(option.id, (e.target as HTMLInputElement).checked)} />
                    <label class="form-check-label" for={'documentDetailField-' + option.id}>
                      {t(option.label)}
                    </label>
                  </div>
                {/each}
                <p class="small text-muted mt-1">{t('admin.settings.uncheck_fields_hide_them')}</p>
              </div>
            </div>
          </div>

          <div class="col-xl-6 ps-xl-5">
            <h5 class="mt-3">{t('admin.settings.bulk_editing')}</h5>
            <div class="row mb-3">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.show_confirmation_dialogs')} bind:value={form.bulkEditConfirmationDialogs} /></pngx-input-check>
                <pngx-input-check><InputCheck title={t('admin.settings.apply_close')} bind:value={form.bulkEditApplyOnClose} /></pngx-input-check>
              </div>
            </div>

            <h5 class="mt-3">{t('common.pdf_editor')}</h5>
            <div class="row">
              <div class="col-md-3 col-form-label pt-0">
                <label for="pdfEditorDefaultEditMode">{t('admin.settings.default_editing_mode')}</label>
              </div>
              <div class="col">
                <select id="pdfEditorDefaultEditMode" class="form-select" bind:value={form.pdfEditorDefaultEditMode}>
                  <option value="create">{t('admin.settings.create_new_document_s')}</option>
                  <option value="update">{t('admin.settings.add_document_version')}</option>
                </select>
              </div>
            </div>

            <h5 class="mt-3">{t('common.notes')}</h5>
            <div class="row mb-3">
              <div class="col">
                <pngx-input-check><InputCheck title={t('admin.settings.enable_notes')} bind:value={form.notesEnabled} /></pngx-input-check>
              </div>
            </div>
          </div>
        </div>

      {:else if activeNav === 'permissions'}

        <h5>{t('admin.settings.default_permissions')}</h5>

        <div class="row mb-3">
          <div class="col">
            <p>
              {t('admin.settings.settings_apply_user_account')}
            </p>
          </div>
        </div>
        <div class="row mb-3">
          <div class="col-md-3 col-form-label pt-0">
            <span>{t('admin.settings.default_owner')}</span>
          </div>
          <div class="col-md-5">
            <pngx-input-select><InputSelect items={users} bindLabel="username" bind:value={form.defaultPermsOwner} allowNull={true} /></pngx-input-select>
            <small class="form-text text-muted text-end d-block mt-n2">{t('common.objects_without_owner_viewed')}</small>
          </div>
        </div>
        <div class="row mb-3">
          <div class="col-md-3 col-form-label pt-0">
            <span>{t('admin.settings.default_view_permissions')}</span>
          </div>
          <div class="col-md-5">
            <div class="row">
              <div class="col-3">
                <span class="d-block pt-1">{t('common.users_label')}</span>
              </div>
              <div class="col">
                {#if session.can('view', 'user')}
                  <pngx-permissions-user type="view"><PermissionsUser bind:value={form.defaultPermsViewUsers} /></pngx-permissions-user>
                {/if}
              </div>
            </div>
            <div class="row">
              <div class="col-3">
                <span class="d-block pt-1">{t('common.groups_label')}</span>
              </div>
              <div class="col">
                {#if session.can('view', 'group')}
                  <pngx-permissions-group type="view"><PermissionsGroup bind:value={form.defaultPermsViewGroups} /></pngx-permissions-group>
                {/if}
              </div>
            </div>
          </div>
        </div>
        <div class="row">
          <div class="col-md-3 col-form-label pt-0">
            <span>{t('admin.settings.default_edit_permissions')}</span>
          </div>
          <div class="col-md-5">
            <div class="row">
              <div class="col-3">
                <span class="d-block pt-1">{t('common.users_label')}</span>
              </div>
              <div class="col">
                {#if session.can('view', 'user')}
                  <pngx-permissions-user type="view"><PermissionsUser bind:value={form.defaultPermsEditUsers} /></pngx-permissions-user>
                {/if}
              </div>
            </div>
            <div class="row">
              <div class="col-3">
                <span class="d-block pt-1">{t('common.groups_label')}</span>
              </div>
              <div class="col">
                {#if session.can('view', 'group')}
                  <pngx-permissions-group type="view"><PermissionsGroup bind:value={form.defaultPermsEditGroups} /></pngx-permissions-group>
                {/if}
              </div>
            </div>
            <div class="row">
              <small class="form-text text-muted text-end d-block">{t('common.edit_permissions_also_grant')}</small>
            </div>
          </div>
        </div>

      {:else}

        <h5>{t('admin.settings.document_processing')}</h5>

        <div class="row">
          <div class="col">
            <pngx-input-check><InputCheck title={t('admin.settings.add_notifications_document_processing')} bind:value={form.notificationsConsumerSuccess} hint={t('admin.settings.added_notifications_menu_without')} /></pngx-input-check>
            <pngx-input-check><InputCheck title={t('admin.settings.show_notifications_document_processing')} bind:value={form.notificationsConsumerFailed} /></pngx-input-check>
          </div>
        </div>

      {/if}
    </div>
  </div>

  <button type="button" onclick={reset} class="btn btn-outline-secondary mb-2" disabled={!dirty}>{t('common.cancel')}</button>
  <button type="submit" class="btn btn-primary ms-2 mb-2" disabled={!dirty}>{t('common.save')}</button>
</form>
