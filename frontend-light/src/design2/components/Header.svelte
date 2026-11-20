<!-- Fixed header bar: logo, search/command palette (Ctrl+K; with AI on also "Ask question"), upload (with AI
     text recognition on: switch for the next uploads, field ai_ocr), notifications, user menu. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { isLocalDesktopWindow } from '../../lib/desktop-local'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { router } from '../../router.svelte'
  import { ui } from '../stores/ui.svelte'
  import Icon from './Icon.svelte'
  import Menu from './Menu.svelte'
  import { aiStatus } from '../ai/status.svelte'

  let fileInput: HTMLInputElement
  let userOpen = $state(false)
  // Desktop app, local window: no "Log out" (automatic login, docs/DESKTOP.md §25)
  const localDesktop = isLocalDesktopWindow()
  let bellOpen = $state(false)
  const unread = $derived(toastService.toasts.filter((x) => !x.read).length)
  const busy = $derived(consumerStatus.getConsumerStatusNotCompleted().length)
  const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)
  const aiOn = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const themeLabel = $derived(ui.theme === 'light' ? t('common.light') : ui.theme === 'dark' ? t('common.dark') : t('common.system'))

  function onFiles(e: Event) {
    const input = e.target as HTMLInputElement
    const files = Array.from(input.files ?? [])
    input.value = ''
    if (files.length) consumerStatus.uploadFiles(files)
  }

  async function switchClassic() {
    const m = await import('../../lib/design-apply')
    await m.applyDesign('classic')
  }
</script>

<header class="hdr">
  {#if ui.mobile}
    <button class="d2-btn ghost icon" aria-label={t('m.ui.open_navigation')} aria-expanded={ui.navOpen} onclick={() => (ui.navOpen = !ui.navOpen)}><Icon name="list" size="1.25rem" /></button>
  {/if}
  <a class="brand" href={router.href('dashboard')} aria-label={t('m.ui.paperrust_overview')}>
    <svg viewBox="0 0 120 140" aria-hidden="true">
      <path d="M24 12 H80 L104 36 V122 Q104 128 98 128 H30 Q24 128 24 122 V18 Q24 12 30 12 Z" fill="none" stroke="currentColor" stroke-width="9" stroke-linejoin="round" />
      <path d="M80 12 V30 Q80 36 86 36 H104 Z" fill="currentColor" stroke="currentColor" stroke-width="7" stroke-linejoin="round" />
      <rect x="38" y="56" width="50" height="9" rx="4.5" fill="currentColor" />
      <rect x="38" y="76" width="38" height="9" rx="4.5" fill="currentColor" />
      <rect x="38" y="96" width="46" height="9" rx="4.5" fill="currentColor" />
    </svg>
    {#if !ui.mobile}<span>PaperRust</span>{/if}
  </a>

  <button class="search" id="d2-search" onclick={() => ui.openPalette()} aria-keyshortcuts="Control+K" aria-label={t('m.ui.search_run_command')}>
    <Icon name="search" />
    <span class="ph">{ui.mobile ? t('common.search') : aiOn ? t('m.ui.search_documents_commands_ask') : t('m.ui.search_documents_commands')}</span>
    {#if !ui.mobile}<span class="d2-kbd">{isMac ? '⌘' : 'Ctrl'}</span><span class="d2-kbd">K</span>{/if}
  </button>

  <div class="actions">
    <input id="d2-upload-input" type="file" multiple class="d2-sr" tabindex="-1" aria-hidden="true" bind:this={fileInput} onchange={onFiles} />
    <button class="d2-btn primary" id="d2-upload" onclick={() => fileInput.click()} aria-keyshortcuts="u">
      <Icon name="cloud-arrow-up" />{#if !ui.mobile}<span>{t('common.upload')}</span>{/if}
      {#if busy}<span class="d2-badge" aria-label={t('m.ui.in_progress', { PH: busy })}>{busy}</span>{/if}
    </button>
    {#if aiStatus.vision}
      <button class="d2-btn icon {consumerStatus.aiOcr ? 'primary' : 'ghost'}" id="d2-upload-ai-ocr" aria-pressed={consumerStatus.aiOcr} aria-label={t('ui.use_ai_ocr')} title="{t('ui.use_ai_ocr')}: {t('ui.ai_ocr_upload_hint')} {t('ui.ai_ocr_hint')}" onclick={() => (consumerStatus.aiOcr = !consumerStatus.aiOcr)}><Icon name="stars" /></button>
    {/if}

    <div class="pop">
      <button class="d2-btn ghost icon" id="d2-bell" aria-label={t('common.notifications')} aria-expanded={bellOpen} onclick={() => { bellOpen = !bellOpen; if (bellOpen) toastService.markAllRead() }}>
        <Icon name="bell" size="1.1rem" />
        {#if unread}<span class="dot" aria-hidden="true">{unread}</span>{/if}
      </button>
      {#if bellOpen}
        <Menu onclose={() => (bellOpen = false)} label={t('common.notifications')} width="340px">
          <div class="d2-menu-label">{t('common.notifications')}</div>
          {#each toastService.toasts.slice(0, 8) as n (n.id)}
            <div class="note" class:err={n.level === 'error'}>
              <p>{n.content}</p>
              {#if n.action && n.actionName}
                <button class="d2-btn sm" onclick={() => { n.action?.(); bellOpen = false }}>{n.actionName}</button>
              {/if}
            </div>
          {:else}
            <p class="d2-muted none">{t('common.no_notifications')}</p>
          {/each}
          {#if toastService.toasts.length}
            <div class="d2-menu-sep"></div>
            <button class="d2-menu-item" onclick={() => toastService.clearToasts()}><Icon name="x-circle" />{t('common.dismiss_all')}</button>
          {/if}
        </Menu>
      {/if}
    </div>

    <div class="pop">
      <button class="d2-btn ghost user" id="d2-user" aria-label={t('common.user_menu')} aria-expanded={userOpen} onclick={() => (userOpen = !userOpen)}>
        <span class="avatar" aria-hidden="true">{(session.displayName || '?').slice(0, 1).toUpperCase()}</span>
        {#if !ui.mobile && !ui.compact}<span class="name">{session.displayName}</span>{/if}
        <Icon name="chevron-down" />
      </button>
      {#if userOpen}
        <Menu onclose={() => (userOpen = false)} label={t('common.user_menu')}>
          <div class="d2-menu-label">{t('common.logged_in', { INTERPOLATION: session.displayName })}</div>
          <button class="d2-menu-item" id="d2-theme" onclick={() => ui.cycleTheme()}>
            <Icon name={ui.theme === 'dark' ? 'moon' : ui.theme === 'light' ? 'sun' : 'circle-half'} />{t('common.appearance')}: {themeLabel}
          </button>
          <button class="d2-menu-item" id="d2-switch-classic" onclick={switchClassic}><Icon name="layout-three-columns" />{t('m.ui.switch_classic_interface')}</button>
          <a class="d2-menu-item" href={router.href('settings/account')} onclick={() => (userOpen = false)}><Icon name="person-circle" />{t('frame.my_profile')}</a>
          <a class="d2-menu-item" href="/rs/docs/README.md" target="_blank" rel="noopener" onclick={() => (userOpen = false)}><Icon name="question-circle" />{t('common.documentation')}</a>
          <button class="d2-menu-item" onclick={() => { userOpen = false; ui.helpOpen = true }}><Icon name="keyboard" />{t('common.keyboard_shortcuts')}</button>
          <button class="d2-menu-item" id="d2-start-tour" onclick={() => { userOpen = false; dispatchEvent(new Event('pr:start-tour')) }}><Icon name="signpost" />{t('m.tour.start')}</button>
          <div class="d2-menu-sep"></div>
          {#if session.can('change', 'uisettings')}
            <a class="d2-menu-item" href={router.href('settings')} onclick={() => (userOpen = false)}><Icon name="gear" />{t('common.settings')}</a>
          {/if}
          {#if !localDesktop}
            <a class="d2-menu-item" href="/accounts/logout/"><Icon name="door-open" />{t('common.logout')}</a>
          {/if}
        </Menu>
      {/if}
    </div>
  </div>
</header>

<style>
  .hdr {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    padding: 0 var(--d2-s3);
    background: var(--d2-surface);
    border-bottom: 1px solid var(--d2-border);
    min-width: 0;
    z-index: 20;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    width: calc(var(--d2-nav-w) - var(--d2-s3) * 2);
    flex: none;
    color: var(--d2-text);
    font-size: var(--d2-fs-lg);
    font-weight: 700;
    letter-spacing: -0.01em;
    text-decoration: none;
  }
  :global(.compact) .brand {
    width: auto;
  }
  :global(.mobile) .brand {
    width: auto;
  }
  .brand svg {
    width: 22px;
    height: 26px;
    color: var(--d2-accent);
  }
  .search {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    flex: 1 1 auto;
    max-width: 560px;
    min-width: 0;
    height: 34px;
    padding: 0 var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface-2);
    color: var(--d2-text-3);
    cursor: text;
    text-align: left;
  }
  .search:hover {
    border-color: var(--d2-border-strong);
  }
  .ph {
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    margin-left: auto;
    flex: none;
  }
  .pop {
    position: relative;
  }
  .dot {
    position: absolute;
    top: 2px;
    right: 0;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--d2-text-2);
    color: var(--d2-surface);
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
  }
  .user {
    padding: 0 var(--d2-s2);
  }
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--d2-surface-3);
    color: var(--d2-text);
    font-weight: 700;
    font-size: var(--d2-fs-sm);
  }
  .name {
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .note {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: var(--d2-s2);
    border-radius: var(--d2-r1);
    font-size: var(--d2-fs-sm);
  }
  .note + .note {
    border-top: 1px solid var(--d2-border);
  }
  .note.err p {
    color: var(--d2-danger);
  }
  .none {
    padding: var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
</style>
