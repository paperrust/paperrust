<!-- Port of app-frame/app-frame.component.html (markup/classes 1:1, only reading parts in F0). -->
<script lang="ts">
  import '../../styles/components/app-frame.css'
  import type { Snippet } from 'svelte'
  import { t, tRich } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { isLocalDesktopWindow } from '../../lib/desktop-local'
  // Desktop app, local window: no "Log out" (automatic login, docs/DESKTOP.md §25)
  const localDesktop = isLocalDesktopWindow()
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { router } from '../../router.svelte'
  import { openDocuments } from '../../stores/openDocuments.svelte'
  import type { Document } from '../../api/v1'
  import Logo from '../common/logo/Logo.svelte'
  import BrandMark from '../common/logo/BrandMark.svelte'
  import GlobalSearch from './global-search/GlobalSearch.svelte'
  import ToastsDropdown from './toasts-dropdown/ToastsDropdown.svelte'
  import { LazyUploadStatusTray } from '../common/toast/lazy.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { savedViewsOrganizing, dropSidebarView } from '../../stores/savedViewsOrganizing.svelte'
  import { sidebarEditing } from '../../stores/sidebarEditing.svelte'
  import InputSwitch from '../common/input/switch/InputSwitch.svelte'
  import { modals } from '../../lib/modal.svelte'
  import { ignoreHotkeyEvent, matchAppHotkey, activeHotkeys } from '../common/hotkey-dialog/hotkeys'
  import { registeredHotkeys } from '../../lib/hotkeys-registry'
  // Ransomware protection: banner in read-only mode (PaperRust extension), loaded lazily (budget FRONTEND-LIGHT §1)
  import { Lazy } from '../common/toast/lazy.svelte'
  const LazyReadOnlyBanner = new Lazy(() => import('../tools/protection/ReadOnlyBanner.svelte'))

  let { children }: { children: Snippet } = $props()

  // Upload via the navbar (AppFrameComponent.onFileSelected / uploadsInProgress / showUploadStatus)
  let fileUpload: HTMLInputElement | undefined = $state()
  const UploadStatusTray = $derived(LazyUploadStatusTray.get(consumerStatus.consumerStatus.length > 0))
  const ReadOnlyBanner = $derived(LazyReadOnlyBanner.get(!!session.data))
  const uploadsInProgress = $derived(consumerStatus.getConsumerStatusNotCompleted().length)
  function onFileSelected(event: Event) {
    const input = event.target as HTMLInputElement
    const files = Array.from(input.files ?? [])
    input.value = ''
    if (!files.length) return
    consumerStatus.uploadFiles(files)
  }

  const DOCS_URL = '/rs/docs/README.md' // built-in docs (offline, matching version)
  const REPO_URL = 'https://github.com/paperrust/paperrust'
  const UPSTREAM_URL = 'https://github.com/paperless-ngx/paperless-ngx'
  const LICENSE_URL = 'https://www.gnu.org/licenses/gpl-3.0.html'
  const DEFAULT_APP_TITLE = 'PaperRust'

  let menuCollapsed = $state(true)
  let userMenuOpen = $state(false)
  let slimLocal = $state<boolean | null>(null)
  let attrsCollapsedLocal = $state<boolean | null>(null)

  const slim = $derived(slimLocal ?? session.get(SETTINGS_KEYS.SLIM_SIDEBAR))
  const attributesSectionsCollapsed = $derived(attrsCollapsedLocal ?? (session.get(SETTINGS_KEYS.ATTRIBUTES_SECTIONS_COLLAPSED) ?? []).includes('attributes'))
  let sidebarDragIndex: number | null = null
  const canSaveSettings = $derived(session.can('change', 'uisettings') && session.can('add', 'uisettings'))
  const canManageAttributes = $derived(['tag', 'correspondent', 'documenttype', 'storagepath', 'customfield'].some((x) => session.can('view', x)))
  const canManageShareLinks = $derived(session.can('view', 'sharelink') || session.can('view', 'sharelinkbundle'))
  const appTitleSetting = $derived(session.get(SETTINGS_KEYS.APP_TITLE))
  const customAppTitle = $derived(appTitleSetting === DEFAULT_APP_TITLE ? null : appTitleSetting)
  const appLogo = $derived(session.get(SETTINGS_KEYS.APP_LOGO))
  const customLogo = $derived(appLogo?.length && !appLogo.includes('__rs/brand/') ? appLogo : null)
  const hasCustomBranding = $derived(!!(customAppTitle?.length || customLogo))
  const appTitle = $derived(appTitleSetting || DEFAULT_APP_TITLE)
  const versionString = $derived(`PaperRust v${session.get(SETTINGS_KEYS.VERSION)}`)
  // SettingsService.sidebarItemIsHidden / organizingSidebarItems: in the "General" tab of the settings the
  // unsaved selection applies; hidden entries then stay visible (opacity-50) and get a switch.
  const isHidden = (id: string) => sidebarEditing.isHidden(id)
  const organizing = $derived(sidebarEditing.organizing)
  const showSidebarCounts = $derived(session.get(SETTINGS_KEYS.SIDEBAR_VIEWS_SHOW_COUNT))
  const updateCheckingIsSet = $derived(session.isSet(SETTINGS_KEYS.UPDATE_CHECKING_ENABLED))
  const attribution = $derived(tRich('common.paperrust_based_paperless_ngx'))

  const href = (p: string) => router.href(p)
  const active = (p: string, exact = false) => router.isActive(p, exact)
  const docTitle = (v: string) => v || t('common.no_title')
  // AppFrameComponent.closeDocument/closeAll: leave the open view of the closed document
  async function closeDocument(d: Document) {
    if (!(await openDocuments.closeDocument(d))) return
    if (router.path.startsWith(`documents/${d.id}`)) router.navigate('')
  }
  async function closeAll() {
    if (!(await openDocuments.closeAll())) return
    if (/^documents\/\d+/.test(router.path)) router.navigate('')
  }
  function closeMenu() {
    menuCollapsed = true
  }
  function toggleSlimSidebar() {
    slimLocal = !slim
  }
  function toggleAttributesSections(e: Event) {
    e.preventDefault()
    e.stopPropagation()
    attrsCollapsedLocal = !attributesSectionsCollapsed
  }
  function onUserMenu(e: Event) {
    e.stopPropagation()
    userMenuOpen = !userMenuOpen
  }
  // AppFrameComponent.editProfile: NgbModal backdrop static, size xl (dialog loaded lazily)
  function editProfile() {
    userMenuOpen = false
    import('../common/profile-edit-dialog/ProfileEditDialog.svelte').then((m) => modals.open(m.default, {}, { backdrop: 'static', size: 'xl' }))
    closeMenu()
  }
  // AI chat only with ai_enabled (AppFrameComponent.aiEnabled), load the component only then
  const aiEnabled = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  // HotKeyService (shift.? → HotkeyDialogComponent) + AppComponent shortcuts h/d/s
  function onHotkey(e: KeyboardEvent) {
    const action = matchAppHotkey(e)
    if (!action || ignoreHotkeyEvent(e)) return
    if (action === 'documents' && !session.can('view', 'document')) return
    if (action === 'settings' && !session.can('change', 'uisettings')) return
    // HotKeyService: modalService.dismissAll() before every action
    for (const ref of [...modals.open_]) ref.dismiss()
    e.preventDefault()
    if (action === 'dashboard') router.navigate('dashboard')
    else if (action === 'documents') router.navigate('documents')
    else if (action === 'settings') router.navigate('settings')
    else {
      const base = activeHotkeys(router.path, { viewDocuments: session.can('view', 'document'), changeSettings: session.can('change', 'uisettings') })
      // plus the shortcuts of the filter dropdowns (lib/hotkeys-registry), duplicate keys only once
      const seen = new Set(base.map(([k]) => k))
      const hotkeys = [...base, ...registeredHotkeys().filter(([k]) => !seen.has(k) && (seen.add(k), true))]
      import('../common/hotkey-dialog/HotkeyDialog.svelte').then((m) => modals.open(m.default, { hotkeys }))
    }
  }
</script>

<svelte:window onclick={() => (userMenuOpen = false)} onkeydown={onHotkey} />

<nav class="navbar navbar-dark fixed-top bg-primary flex-md-nowrap p-0 shadow-sm">
  <button class="navbar-toggler d-md-none collapsed border-0" type="button" data-toggle="collapse"
    data-target="#sidebarMenu" aria-controls="sidebarMenu" aria-expanded={!menuCollapsed} aria-label="Toggle navigation"
    onclick={() => (menuCollapsed = !menuCollapsed)}>
    <pngx-bs-icon width="1.5em" height="1.5em" name="list">{@html icon('list', '1.5em', '1.5em')}</pngx-bs-icon>
  </button>
  <div class="navbar-brand-container order-sm-0">
    <a class="navbar-brand d-flex align-items-center me-0 ps-md-3 py-0" class:slim href={href('dashboard')}>
      {#if !hasCustomBranding}
        <pngx-logo extra_classes="navbar-official-logo px-1" height="2.4rem"><Logo extraClasses="navbar-official-logo px-1" height="2.4rem" /></pngx-logo>
        <pngx-brand-mark class="brand-mark brand-mark-slim d-none"><BrandMark /></pngx-brand-mark>
      {:else}
        {#if customLogo}
          <img class="brand-logo" src={customLogo} alt="" />
        {:else}
          <pngx-brand-mark class="brand-mark"><BrandMark /></pngx-brand-mark>
        {/if}
        <div class="brand-copy ms-2 text-truncate" class:d-md-none={slim}>
          <span class="brand-title text-truncate">{appTitle}</span>
          {#if customAppTitle}
            <span class="byline text-uppercase font-monospace">{t('frame.paperrust')}</span>
          {/if}
        </div>
      {/if}
    </a>
  </div>
  <div class="search-container flex-grow-1 py-2 pb-3 pb-sm-2 me-sm-auto order-3 order-sm-1">
    <div class="col-12 header-search mx-auto d-flex align-items-stretch gap-3">
      <pngx-global-search class="flex-grow-1"><GlobalSearch /></pngx-global-search>
      {#if session.can('add', 'document')}
        <div class="btn-group header-upload-group flex-shrink-0">
          <button class="btn btn-sm header-upload border-0 d-flex align-items-center justify-content-center py-0 px-2" type="button"
            onclick={() => fileUpload?.click()} aria-label={t('common.upload_documents')}>
            <pngx-bs-icon class="d-flex" width="1.1em" height="1.1em" name="file-earmark-arrow-up">{@html icon('file-earmark-arrow-up', '1.1em', '1.1em')}</pngx-bs-icon><span class="ms-2 d-sm-none d-md-inline">{t('common.upload')}</span>
          </button>
          {#if uploadsInProgress > 0}
            <button class="btn btn-sm header-upload border-0 d-flex align-items-center py-0 px-2" type="button"
              onclick={() => (consumerStatus.trayCollapsed = false)} aria-label={t('frame.show_document_processing_status')}>
              <span class="spinner-border spinner-border-sm" aria-hidden="true"></span><span class="ms-2 small">{uploadsInProgress}</span>
            </button>
          {/if}
        </div>
      {/if}
    </div>
  </div>
  <ul class="nav order-sm-3" role="tablist">
    {#if aiEnabled}
      {#await import('../chat/Chat.svelte') then m}
        <pngx-chat><m.default /></pngx-chat>
      {/await}
    {/if}
    <pngx-toasts-dropdown><ToastsDropdown /></pngx-toasts-dropdown>
    <li class="nav-item dropdown" class:show={userMenuOpen}>
      <button class="dropdown-toggle btn navbar-action border-0 me-1 me-md-2" id="userDropdown" aria-label={t('common.user_menu')} aria-expanded={userMenuOpen} onclick={onUserMenu}>
        <pngx-bs-icon width="1.3em" height="1.3em" name="person-circle">{@html icon('person-circle', '1.3em', '1.3em')}</pngx-bs-icon>
        <span class="small ms-2 d-none d-sm-inline">
          {session.displayName}
        </span>
      </button>
      {#if userMenuOpen}
        <div class="dropdown-menu dropdown-menu-end shadow me-2 show" aria-labelledby="userDropdown" data-bs-popper="static">
          <div class="d-sm-none">
            <p class="small mb-0 px-3 text-muted">{t('common.logged_in', { INTERPOLATION: session.displayName })}</p>
            <div class="dropdown-divider"></div>
          </div>
          <button type="button" class="dropdown-item nav-link" onclick={editProfile}>
            <pngx-bs-icon class="me-2" name="person">{@html icon('person')}</pngx-bs-icon>{t('frame.my_profile')}
          </button>
          {#if session.can('change', 'uisettings')}
            <a class="dropdown-item nav-link" href={href('settings')}>
              <pngx-bs-icon class="me-2" name="gear">{@html icon('gear')}</pngx-bs-icon>{t('common.settings')}
            </a>
          {/if}
          <!-- PaperRust: switch to the second UI (docs/DESIGN2.md §6) -->
          <button type="button" class="dropdown-item nav-link" id="d2-switch-modern" onclick={() => import('../../lib/design-apply').then((m) => m.applyDesign('modern'))}>
            <pngx-bs-icon class="me-2" name="stars">{@html icon('stars')}</pngx-bs-icon>{t('frame.switch_paperrust_design')}
          </button>
          {#if !localDesktop}
            <a class="dropdown-item nav-link d-flex" href="/accounts/logout/">
              <pngx-bs-icon class="me-2" name="door-open">{@html icon('door-open')}</pngx-bs-icon>{t('common.logout')}
            </a>
          {/if}
          <div class="dropdown-divider"></div>
          <a class="dropdown-item nav-link" target="_blank" rel="noopener noreferrer" href={DOCS_URL}>
            <pngx-bs-icon class="me-2" name="question-circle">{@html icon('question-circle')}</pngx-bs-icon>{t('common.documentation')}
          </a>
        </div>
      {/if}
    </li>
  </ul>
</nav>

<input type="file" class="visually-hidden" id="navbar-file-upload" onchange={onFileSelected} multiple bind:this={fileUpload} tabindex="-1" aria-hidden="true">
<label for="navbar-file-upload" class="visually-hidden">{t('frame.upload_files')}</label>

<!-- @if (settingsService.organizingSidebarItems()) { <pngx-input-switch … [compact]="true"> } for each hideable entry -->
{#snippet itemSwitch(id: string, title: string)}
  {#if organizing}
    <pngx-input-switch class="position-absolute top-50 end-0 translate-middle-y me-1" class:d-none={slim}>
      <InputSwitch compact={true} {title} value={!isHidden(id)} onchange={(v) => sidebarEditing.updateVisibility(id, v)} />
    </pngx-input-switch>
  {/if}
{/snippet}
<div class="container-fluid">
  <div class="row">
    <nav id="sidebarMenu" class="d-md-block bg-light sidebar collapse {slim ? 'slim' : 'expanded'}" class:show={!menuCollapsed}>
      {#if canSaveSettings}
        <button class="btn btn-sm btn-dark sidebar-slim-toggler" onclick={toggleSlimSidebar} aria-label={slim ? 'Expand sidebar' : 'Collapse sidebar'}>
          {#if slim}
            <pngx-bs-icon width="0.9em" height="0.9em" name="chevron-double-right">{@html icon('chevron-double-right', '0.9em', '0.9em')}</pngx-bs-icon>
          {:else}
            <pngx-bs-icon width="0.9em" height="0.9em" name="chevron-double-left">{@html icon('chevron-double-left', '0.9em', '0.9em')}</pngx-bs-icon>
          {/if}
        </button>
      {/if}
      <div class="sidebar-sticky pt-3 pb-1 d-flex flex-column justify-space-around">
        <ul class="nav flex-column">
          <li class="nav-item app-link position-relative" class:d-none={isHidden('dashboard') && !organizing}>
            <a class="nav-link" class:opacity-50={isHidden('dashboard')} class:pe-5={organizing && !slim} class:active={active('dashboard')} href={href('dashboard')} onclick={closeMenu}>
              <pngx-bs-icon class="me-2" name="house">{@html icon('house')}</pngx-bs-icon><span class="nav-link-label">{t('common.dashboard')}</span>
            </a>
            {@render itemSwitch('dashboard', t('common.dashboard'))}
          </li>
          {#if session.can('view', 'document')}
            <li class="nav-item app-link">
              <a class="nav-link" class:active={active('documents', true)} href={href('documents')} onclick={closeMenu}>
                <pngx-bs-icon class="me-2" name="files">{@html icon('files')}</pngx-bs-icon><span class="nav-link-label">{t('common.documents_2')}</span>
              </a>
            </li>
          {/if}
        </ul>

        {#if session.can('view', 'savedview')}
          <div class="nav-group mt-3 mb-1">
            {#if data.sidebarViews.length > 0}
              <h6 class="sidebar-heading px-3 text-muted">
                <span>{t('common.saved_views')}</span>
              </h6>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <ul class="nav flex-column mb-2 cdk-drop-list" ondragover={(e) => e.preventDefault()} ondrop={(e) => {
                e.preventDefault()
                if (sidebarDragIndex === null) return
                const lis = [...(e.currentTarget as HTMLElement).children]
                const to = lis.findIndex((li) => e.clientY < li.getBoundingClientRect().bottom)
                dropSidebarView(data.sidebarViews.map((v) => v.id), sidebarDragIndex, to === -1 ? lis.length - 1 : to)
                sidebarDragIndex = null
              }}>
                {#each data.sidebarViews as view, viewIndex (view.id)}
                  <li class="nav-item app-link cdk-drag" class:cdk-drag-disabled={!savedViewsOrganizing.active || !canSaveSettings} draggable={savedViewsOrganizing.active && canSaveSettings} ondragstart={() => (sidebarDragIndex = viewIndex)}>
                    <a class="nav-link" class:active={active(`view/${view.id}`)} href={href(`view/${view.id}`)} onclick={closeMenu}>
                      <pngx-bs-icon class="me-2">{@html icon(view.icon || 'funnel')}</pngx-bs-icon><span><div class="d-inline-flex view-name"><span class="overflow-hidden" class:text-wrap={!slim}>{view.name}</span></div>
                          {#if showSidebarCounts && !slim && data.viewCounts[view.id] !== undefined}
                            <span class="badge bg-info text-dark ms-2 d-inline">{data.viewCounts[view.id]}</span>
                          {/if}
                        </span>
                    </a>
                    {#if savedViewsOrganizing.active && canSaveSettings}
                      <div class="position-absolute end-0 top-0 px-1 py-2" class:me-n2={slim}>
                        <pngx-bs-icon name="grip-vertical">{@html icon('grip-vertical')}</pngx-bs-icon>
                      </div>
                    {/if}
                  </li>
                {/each}
              </ul>
            {:else if data.savedViewsLoading}
              <h6 class="sidebar-heading px-3 text-muted">
                <span>{t('common.saved_views')}</span>
                <div class="spinner-border spinner-border-sm fw-normal ms-2" role="status"></div>
              </h6>
            {/if}
          </div>
        {/if}

        {#if session.can('view', 'document')}
          <div class="nav-group mt-3 mb-1">
            {#if openDocuments.documents.length > 0}
              <h6 class="sidebar-heading px-3 text-muted">
                <span>{t('frame.open_documents')}</span>
              </h6>
            {/if}
            <ul class="nav flex-column mb-2">
              {#each openDocuments.documents as d (d)}
                <li class="nav-item w-100 app-link">
                  <a class="nav-link app-link" class:text-truncate={!slim} class:active={active(`documents/${d.id}`)} href={href(`documents/${d.id}`)} onclick={closeMenu}>
                    <pngx-bs-icon class="me-2" name="file-text">{@html icon('file-text')}</pngx-bs-icon><span>{docTitle(d.title)}</span>
                    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                    <span class="close flex-column justify-content-center"
                      onclick={(e) => { e.preventDefault(); e.stopPropagation(); closeDocument(d) }}>
                      <pngx-bs-icon name="x">{@html icon('x')}</pngx-bs-icon>
                    </span>
                  </a>
                </li>
              {/each}
              {#if openDocuments.documents.length >= 1}
                <li class="nav-item w-100 app-link">
                  <button type="button" class="nav-link nav-link-action app-link w-100 text-start"
                    class:text-truncate={!slim} onclick={closeAll}>
                    <pngx-bs-icon class="me-2" name="x">{@html icon('x')}</pngx-bs-icon><span class="nav-link-label">{t('frame.close_all')}</span>
                  </button>
                </li>
              {/if}
            </ul>
          </div>
        {/if}

        <div class="nav-group mt-3 mb-1">
          <h6 class="sidebar-heading px-3 text-muted">
            <span>{t('frame.manage')}</span>
          </h6>
          <ul class="nav flex-column mb-2">
            {#if canManageAttributes}
              <li class="nav-item app-link" tourAnchor="tour.tags">
                <div class="d-flex align-items-center attributes-row">
                  <a class="nav-link flex-fill" class:active={active('attributes', !(slim || attributesSectionsCollapsed))} href={href('attributes')} onclick={closeMenu}>
                    <pngx-bs-icon name="stack">{@html icon('stack')}</pngx-bs-icon><span class="nav-link-label ms-2">{t('common.attributes')}</span>
                  </a>
                  {#if !slim && canSaveSettings}
                    <button
                      type="button"
                      class="btn btn-link btn-sm text-muted p-0 me-3 attributes-expand-btn"
                      onclick={toggleAttributesSections}
                      aria-label={attributesSectionsCollapsed ? 'Expand attributes sections' : 'Collapse attributes sections'}
                    >
                      <pngx-bs-icon>{@html icon(attributesSectionsCollapsed ? 'plus-circle' : 'dash-circle')}</pngx-bs-icon>
                    </button>
                  {/if}
                </div>
                <div class="attributes-submenu ms-3 collapse" class:show={!(slim || attributesSectionsCollapsed)}>
                  <ul class="nav flex-column">
                    {#if session.can('view', 'tag')}
                      <li class="nav-item app-link">
                        <a class="nav-link" class:active={active('attributes/tags')} href={href('attributes/tags')} onclick={closeMenu}>
                          <pngx-bs-icon class="me-2" name="tags">{@html icon('tags')}</pngx-bs-icon><span class="nav-link-label">{t('common.tags')}</span>
                        </a>
                      </li>
                    {/if}
                    {#if session.can('view', 'correspondent')}
                      <li class="nav-item app-link">
                        <a class="nav-link" class:active={active('attributes/correspondents')} href={href('attributes/correspondents')} onclick={closeMenu}>
                          <pngx-bs-icon class="me-2" name="person">{@html icon('person')}</pngx-bs-icon><span class="nav-link-label">{t('common.correspondents')}</span>
                        </a>
                      </li>
                    {/if}
                    {#if session.can('view', 'documenttype')}
                      <li class="nav-item app-link">
                        <a class="nav-link" class:active={active('attributes/documenttypes')} href={href('attributes/documenttypes')} onclick={closeMenu}>
                          <pngx-bs-icon class="me-2" name="hash">{@html icon('hash')}</pngx-bs-icon><span class="nav-link-label">{t('common.document_types')}</span>
                        </a>
                      </li>
                    {/if}
                    {#if session.can('view', 'storagepath')}
                      <li class="nav-item app-link">
                        <a class="nav-link" class:active={active('attributes/storagepaths')} href={href('attributes/storagepaths')} onclick={closeMenu}>
                          <pngx-bs-icon class="me-2" name="folder">{@html icon('folder')}</pngx-bs-icon><span class="nav-link-label">{t('common.storage_paths')}</span>
                        </a>
                      </li>
                    {/if}
                    {#if session.can('view', 'customfield')}
                      <li class="nav-item app-link">
                        <a class="nav-link" class:active={active('attributes/customfields')} href={href('attributes/customfields')} onclick={closeMenu}>
                          <pngx-bs-icon class="me-2" name="ui-radios">{@html icon('ui-radios')}</pngx-bs-icon><span class="nav-link-label">{t('common.custom_fields')}</span>
                        </a>
                      </li>
                    {/if}
                  </ul>
                </div>
              </li>
            {/if}
            {#if session.can('view', 'savedview')}
              <li class="nav-item app-link position-relative" class:d-none={isHidden('saved_views') && !organizing}>
                <a class="nav-link" class:opacity-50={isHidden('saved_views')} class:pe-5={organizing && !slim} class:active={active('savedviews')} href={href('savedviews')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="window-stack">{@html icon('window-stack')}</pngx-bs-icon><span class="nav-link-label">{t('common.saved_views')}</span>
                </a>
                {@render itemSwitch('saved_views', t('common.saved_views'))}
              </li>
            {/if}
            {#if canManageShareLinks}
              <li class="nav-item app-link position-relative" class:d-none={isHidden('share_links') && !organizing}>
                <a class="nav-link" class:opacity-50={isHidden('share_links')} class:pe-5={organizing && !slim} class:active={active('share-links')} href={href('share-links')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="link">{@html icon('link')}</pngx-bs-icon><span class="nav-link-label">{t('common.share_links')}</span>
                </a>
                {@render itemSwitch('share_links', t('common.share_links'))}
              </li>
            {/if}
            {#if session.can('view', 'workflow')}
              <li class="nav-item app-link position-relative" class:d-none={isHidden('workflows') && !organizing} tourAnchor="tour.workflows">
                <a class="nav-link" class:opacity-50={isHidden('workflows')} class:pe-5={organizing && !slim} class:active={active('workflows')} href={href('workflows')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="boxes">{@html icon('boxes')}</pngx-bs-icon><span class="nav-link-label">{t('common.workflows')}</span>
                </a>
                {@render itemSwitch('workflows', t('common.workflows'))}
              </li>
            {/if}
            {#if session.can('view', 'mailaccount')}
              <li class="nav-item app-link position-relative" class:d-none={isHidden('mail') && !organizing} tourAnchor="tour.mail">
                <a class="nav-link" class:opacity-50={isHidden('mail')} class:pe-5={organizing && !slim} class:active={active('mail')} href={href('mail')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="envelope">{@html icon('envelope')}</pngx-bs-icon><span class="nav-link-label">{t('common.mail')}</span>
                </a>
                {@render itemSwitch('mail', t('common.mail'))}
              </li>
            {/if}
            {#if session.can('delete', 'document')}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('trash')} href={href('trash')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="trash">{@html icon('trash')}</pngx-bs-icon><span class="nav-link-label">{t('common.trash')}</span>
                </a>
              </li>
            {/if}
            {#if session.can('change', 'document')}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('tools/asn')} href={href('tools/asn')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="upc-scan">{@html icon('upc-scan')}</pngx-bs-icon><span class="nav-link-label">{t('common.asn_labels')}</span>
                </a>
              </li>
            {/if}
            {#if session.can('add', 'document')}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('tools/scan')} href={href('tools/scan')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="printer">{@html icon('printer')}</pngx-bs-icon><span class="nav-link-label">{t('scan.title')}</span>
                </a>
              </li>
            {/if}
          </ul>
        </div>

        <div class="nav-group mt-auto mb-1">
          <h6 class="sidebar-heading px-3 pt-4 text-muted">
            <span>{t('common.administration')}</span>
          </h6>
          <ul class="nav flex-column mb-2">
            {#if session.can('change', 'uisettings')}
              <li class="nav-item app-link" tourAnchor="tour.settings">
                <a class="nav-link" class:active={active('settings')} href={href('settings')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="gear">{@html icon('gear')}</pngx-bs-icon><span class="nav-link-label">{t('common.settings')}</span>
                </a>
              </li>
            {/if}
            {#if session.can('change', 'applicationconfiguration')}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('config')} href={href('config')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="sliders2-vertical">{@html icon('sliders2-vertical')}</pngx-bs-icon><span class="nav-link-label">{t('common.configuration')}</span>
                </a>
              </li>
            {/if}
            {#if session.data?.user?.is_superuser}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('tools/scanners')} href={href('tools/scanners')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="hdd-stack">{@html icon('hdd-stack')}</pngx-bs-icon><span class="nav-link-label">{t('scanners.title')}</span>
                </a>
              </li>
            {/if}
            {#if session.data?.user?.is_superuser}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('protection')} href={href('protection')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="shield-check">{@html icon('shield-check')}</pngx-bs-icon><span class="nav-link-label">{t('protection.title')}</span>
                </a>
              </li>
            {/if}
            {#if session.can('view', 'user')}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('usersgroups')} href={href('usersgroups')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="people">{@html icon('people')}</pngx-bs-icon><span class="nav-link-label">{t('common.users_groups')}</span>
                </a>
              </li>
            {/if}
            {#if session.can('view', 'paperlesstask')}
              <li class="nav-item app-link" tourAnchor="tour.file-tasks">
                <a class="nav-link" class:active={active('tasks')} href={href('tasks')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="list-task">{@html icon('list-task')}</pngx-bs-icon><span class="nav-link-label">{t('common.tasks')}</span>
                  {#if data.needsAttentionTasks > 0 && !slim}
                    <span class="badge bg-danger ms-2 d-inline flex-shrink-0">{data.needsAttentionTasks}</span>
                  {/if}
                  {#if data.needsAttentionTasks > 0 && slim}
                    <span class="badge bg-danger position-absolute top-0 end-0 d-none d-md-block">{data.needsAttentionTasks}</span>
                  {/if}
                </a>
              </li>
            {/if}
            {#if session.isAdmin}
              <li class="nav-item app-link">
                <a class="nav-link" class:active={active('logs')} href={href('logs')} onclick={closeMenu}>
                  <pngx-bs-icon class="me-2" name="text-left">{@html icon('text-left')}</pngx-bs-icon><span class="nav-link-label">{t('common.logs')}</span>
                </a>
              </li>
            {/if}
            <li class="nav-item mt-2 position-relative" class:d-none={isHidden('documentation') && !organizing} tourAnchor="tour.outro">
              <a class="text-muted small d-flex align-items-center flex-wrap text-decoration-none nav-anchor" class:opacity-50={isHidden('documentation')} class:pe-5={organizing && !slim}
                target="_blank" rel="noopener noreferrer" href={DOCS_URL}>
                <pngx-bs-icon class="d-flex me-2" name="question-circle">{@html icon('question-circle')}</pngx-bs-icon><span>{t('common.documentation')}</span>
              </a>
              {@render itemSwitch('documentation', t('common.documentation'))}
            </li>
            <li class="nav-item" class:visually-hidden={slim}>
              <div class="text-muted small d-flex align-items-center flex-wrap nav-label">
                <div class="me-2">
                  <a class="text-muted text-decoration-none" target="_blank" rel="noopener noreferrer" href={REPO_URL}>
                    {versionString}
                  </a>
                </div>
                {#if !updateCheckingIsSet}
                  <div class="version-check d-flex align-items-center">
                    {#if session.can('change', 'uisettings')}
                      <a class="small text-decoration-none" href={href('settings') + '#update-checking'} aria-label={t('frame.configure_update_checking')}>
                        <pngx-bs-icon width="1.2em" height="1.2em" name="info-circle">{@html icon('info-circle', '1.2em', '1.2em')}</pngx-bs-icon>
                      </a>
                    {/if}
                  </div>
                {/if}
              </div>
              <div class="text-muted small paperrust-attribution">{#each attribution as s}{#if s.tag === 'START_LINK'}<a class="text-muted" href={UPSTREAM_URL} target="_blank" rel="noopener noreferrer">{s.text}</a>{:else if s.tag === 'START_LINK_1'}<a class="text-muted" href={LICENSE_URL} target="_blank" rel="noopener noreferrer">{s.text}</a>{:else}{s.text}{/if}{/each}</div>
            </li>
          </ul>
        </div>
      </div>
    </nav>

    <main role="main" class="ms-sm-auto px-md-4 {slim ? 'col-slim' : 'col-sidebar-expanded'}">
      <pngx-read-only-banner>{#if ReadOnlyBanner}<ReadOnlyBanner />{/if}</pngx-read-only-banner>
      {@render children()}
    </main>
    <pngx-upload-status-tray>{#if UploadStatusTray}<UploadStatusTray />{/if}</pngx-upload-status-tray>
  </div>
</div>
