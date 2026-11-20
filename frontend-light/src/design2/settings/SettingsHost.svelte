<!-- Settings in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §6): four tabs
     Appearance (own: design, light/dark, density, language, customize home page) · Documents (classic
     settings component: display/editing, default permissions, general) · Notifications (classic) ·
     Account (profile dialog: password, two-factor, token, email). Technical things are under Manage → System →
     Configuration (route settings/system is displayed there). -->
<script lang="ts">
  import 'bootstrap/dist/css/bootstrap.css'
  import '../../styles/global.scss'
  import '../manage/classic.css'
  import type { Component } from 'svelte'
  import { t, msg } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import ModalHost from '../../components/common/confirm-dialog/ModalHost.svelte'
  import Icon from '../components/Icon.svelte'
  import { ui } from '../stores/ui.svelte'
  import { SYMBOL } from '../lib/icons'
  import type { Density, ThemeMode } from '../lib/theme'
  import { DESIGN_KEY, designOf } from '../../lib/design-switch'
  import { settingsTab, DOC_SUBTABS, type SettingsTab } from './tabs'


  const sec = $derived(router.path.split('/')[1] ?? '')
  const active = $derived<SettingsTab>(settingsTab(sec))
  // Route settings/system[/group] is shown by App.svelte in Manage → System → Configuration (Server)

  let Classic = $state<Component<any> | null>(null)
  // "Customize home page" (design2/home/HomeCustomize.svelte), loaded lazily only in the Appearance tab
  let HomeCustomize = $state<Component<{ idPrefix?: string }> | null>(null)
  // "AI" tab (design2/settings/ai), loaded lazily only there
  let AiSettings = $state<Component<any> | null>(null)
  $effect(() => {
    if ((active === 'documents' || active === 'notifications') && !Classic) import('../../components/admin/settings/Settings.svelte').then((m) => (Classic = m.default))
    if (active === 'display' && !HomeCustomize) import('../home/HomeCustomize.svelte').then((m) => (HomeCustomize = m.default))
    if (active === 'ai' && !AiSettings) import('./ai/AiSettings.svelte').then((m) => (AiSettings = m.default))
  })

  const design = $derived(designOf(session.settings))
  const canSave = $derived(session.can('change', 'uisettings'))
  const canConfig = $derived(!!(session.data?.user?.is_superuser || session.data?.user?.is_staff) && session.can('view', 'applicationconfiguration'))
  let savingDesign = $state(false)
  let savingLang = $state(false)

  async function setDefault(d: 'classic' | 'modern') {
    savingDesign = true
    const m = await import('../../lib/design-apply')
    await m.applyDesign(d, d === 'classic')
    savingDesign = false
  }
  async function setDensity(d: Density) {
    try {
      await ui.setDensity(d)
    } catch {
      toastService.showError(t('m.settings.could_not_save'))
    }
  }

  // Language: same list as the classic "General" tab; takes effect after reload (as there)
  let languages = $state<{ code: string; name: string }[]>([])
  $effect(() => {
    if (active === 'display' && !languages.length) import('../../components/admin/settings/settings-form.svelte').then((m) => (languages = m.displayLanguageOptions()))
  })
  const language = $derived((session.get(SETTINGS_KEYS.LANGUAGE) as string) ?? '')
  async function setLanguage(code: string) {
    savingLang = true
    try {
      const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
      setSetting(SETTINGS_KEYS.LANGUAGE, code)
      await storeSettings()
      location.reload()
    } catch {
      savingLang = false
      toastService.showError(t('m.settings.could_not_save'))
    }
  }

  async function openProfile() {
    const [{ modals }, m] = await Promise.all([import('../../lib/modal.svelte'), import('../../components/common/profile-edit-dialog/ProfileEditDialog.svelte')])
    modals.open(m.default, {}, { backdrop: 'static', size: 'xl' })
  }

  const THEMES: { k: ThemeMode; label: string; icon: string }[] = [
    { k: 'light', label: msg('common.light'), icon: 'sun' },
    { k: 'dark', label: msg('common.dark'), icon: 'moon' },
    { k: 'auto', label: msg('common.system'), icon: 'circle-half' },
  ]
  const DENSITIES: { k: Density; label: string }[] = [
    { k: 'normal', label: msg('m.settings.density_normal') },
    { k: 'compact', label: msg('m.settings.density_compact') },
  ]
  const TABS: { k: SettingsTab; path: string; label: string }[] = [
    { k: 'display', path: 'settings', label: msg('m.settings.display') },
    { k: 'documents', path: 'settings/documents', label: msg('common.documents_2') },
    { k: 'notifications', path: 'settings/notifications', label: msg('common.notifications') },
    { k: 'account', path: 'settings/account', label: msg('common.account') },
    { k: 'ai', path: 'settings/ai', label: msg('m.ai_settings.tab') },
  ]
  const ACCOUNT_ITEMS = [msg('common.password'), msg('dialog.user.two_factor_authentication'), msg('m.settings.devices_tokens'), msg('common.email')]
  const classicPane = $derived(active === 'documents' || active === 'notifications')
</script>

<div class="sh">
  <div class="tabsbar" role="tablist" aria-label={t('common.settings')}>
    {#each TABS as x (x.k)}
      <a role="tab" href={router.href(x.path)} aria-selected={active === x.k} class:on={active === x.k} id="d2-set-tab-{x.k}">{t(x.label)}</a>
    {/each}
  </div>
  <div class="pane d2-scroll" class:d2-classic={classicPane}>
    {#if active === 'display'}
      <div class="own">
        <section class="card" aria-labelledby="d2-set-design">
          <h2 id="d2-set-design">{t('m.settings.interface_design')}</h2>
          <p class="d2-muted">{t('m.settings.choose_interface_opens_default')}</p>
          <div class="choices" role="radiogroup" aria-label={t('m.settings.interface_design')}>
            <label class="choice" class:on={design === 'modern'}>
              <input type="radio" name="d2-design" value="modern" checked={design === 'modern'} disabled={!canSave || savingDesign} onchange={() => setDefault('modern')} id="d2-design-modern" />
              <span class="ttl"><Icon name={SYMBOL.designPaperRust} /> {t('m.settings.paperrust_design')}</span>
              <span class="d2-muted">{t('m.settings.fixed_navigation_workspace_preview')}</span>
            </label>
            <label class="choice" class:on={design === 'classic'}>
              <input type="radio" name="d2-design" value="classic" checked={design === 'classic'} disabled={!canSave || savingDesign} onchange={() => setDefault('classic')} id="d2-design-classic" />
              <span class="ttl"><Icon name={SYMBOL.designClassic} /> {t('m.settings.classic_interface')}</span>
              <span class="d2-muted">{t('m.settings.looks_like_paperless_ngx')}</span>
            </label>
          </div>
          <p class="d2-muted small">{t('m.settings.saved_per_user', { PH: DESIGN_KEY })}</p>
        </section>
        <section class="card" aria-labelledby="d2-set-theme">
          <h2 id="d2-set-theme">{t('m.settings.color_scheme')}</h2>
          <div class="seg" role="radiogroup" aria-labelledby="d2-set-theme">
            {#each THEMES as th (th.k)}
              <button role="radio" aria-checked={ui.theme === th.k} class:on={ui.theme === th.k} onclick={() => ui.setTheme(th.k)} id="d2-theme-{th.k}"><Icon name={th.icon} />{t(th.label)}</button>
            {/each}
          </div>
        </section>
        <section class="card" aria-labelledby="d2-set-density">
          <h2 id="d2-set-density">{t('m.settings.density')}</h2>
          <p class="d2-muted">{t('m.settings.density_hint')}</p>
          <div class="seg" role="radiogroup" aria-labelledby="d2-set-density">
            {#each DENSITIES as d (d.k)}
              <button role="radio" aria-checked={ui.density === d.k} class:on={ui.density === d.k} disabled={!canSave} onclick={() => setDensity(d.k)} id="d2-density-{d.k}">{t(d.label)}</button>
            {/each}
          </div>
        </section>
        <section class="card" aria-labelledby="d2-set-lang">
          <h2><label id="d2-set-lang" for="d2-language">{t('common.language')}</label></h2>
          <select id="d2-language" class="d2-select lang" value={language} disabled={!canSave || savingLang || !languages.length} onchange={(e) => setLanguage(e.currentTarget.value)}>
            {#each languages as l (l.code)}<option value={l.code}>{l.name}</option>{/each}
          </select>
          <p class="d2-muted small">{t('m.settings.language_reload')}</p>
        </section>
        <section class="card" aria-labelledby="d2-set-home">
          <h2 id="d2-set-home">{t('m.settings.home_customize')}</h2>
          {#if HomeCustomize}
            <HomeCustomize idPrefix="d2-set-hc" />
          {:else}
            <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
          {/if}
        </section>
        <section class="card">
          <h2>{t('common.keyboard_shortcuts')}</h2>
          <p class="d2-muted">{t('m.settings.press_anywhere_see_all')}</p>
          <button class="d2-btn" onclick={() => (ui.helpOpen = true)}><Icon name={SYMBOL.shortcuts} />{t('m.settings.show_shortcuts')}</button>
        </section>
        {#if canConfig}
          <p class="d2-muted small" id="d2-set-system-moved">{t('m.settings.system_moved')} <a href={router.href('config')}>{t('m.settings.open_configuration')}</a></p>
        {/if}
      </div>
    {:else if active === 'ai'}
      <div class="own">
        {#if AiSettings}{@const A = AiSettings}<A />{:else}<p class="d2-muted" aria-busy="true">{t('common.loading')}</p>{/if}
      </div>
    {:else if active === 'account'}
      <div class="own">
        <section class="card" aria-labelledby="d2-set-account">
          <h2 id="d2-set-account">{t('common.account')}</h2>
          <p class="d2-muted">{t('m.settings.account_hint')}</p>
          <ul class="items">
            {#each ACCOUNT_ITEMS as it (it)}<li>{t(it)}</li>{/each}
          </ul>
          <button class="d2-btn primary" onclick={openProfile} id="d2-set-profile">{t('dialog.profile.title')}</button>
        </section>
      </div>
    {:else}
      {#if active === 'documents'}
        <div class="subtabs" role="tablist" aria-label={t('common.documents_2')}>
          {#each DOC_SUBTABS as s (s.k)}
            <a role="tab" href={router.href('settings/' + s.k)} aria-selected={sec === s.k} class:on={sec === s.k} id="d2-set-sub-{s.k}">{t(s.label)}</a>
          {/each}
        </div>
      {/if}
      {#if Classic}<pngx-settings sh-settings=""><Classic /></pngx-settings>{/if}
    {/if}
  </div>
</div>
<ModalHost />

<style>
  .sh {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .tabsbar {
    display: flex;
    gap: 2px;
    flex: none;
    padding: 0 var(--d2-s4);
    border-bottom: 1px solid var(--d2-border);
    background: var(--d2-surface);
    overflow-x: auto;
  }
  [role='tab'] {
    position: relative;
    display: flex;
    align-items: center;
    height: 44px;
    padding: 0 var(--d2-s3);
    color: var(--d2-text-2);
    font-weight: 500;
    text-decoration: none;
    white-space: nowrap;
  }
  [role='tab'].on {
    color: var(--d2-text);
    font-weight: 600;
  }
  [role='tab'].on::after {
    content: '';
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: -1px;
    height: 2px;
    background: var(--d2-accent);
  }
  .subtabs {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    margin: calc(-1 * var(--d2-s4)) calc(-1 * var(--d2-s5)) var(--d2-s4);
    padding: 0 var(--d2-s5);
    border-bottom: 1px solid var(--d2-border);
  }
  .subtabs [role='tab'] {
    height: 40px;
  }
  .pane {
    flex: 1;
  }
  /* the classic tabs duplicate our tab bars */
  .pane :global(pngx-settings > form > .nav-tabs) {
    display: none;
  }
  .own {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s4);
    max-width: 760px;
    padding: var(--d2-s5);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    padding: var(--d2-s4) var(--d2-s5);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  .card h2 {
    font-size: var(--d2-fs-lg);
  }
  .card .d2-btn {
    align-self: flex-start;
  }
  .choices {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: var(--d2-s3);
  }
  .choice {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: var(--d2-s3);
    border: 1.5px solid var(--d2-border-strong);
    border-radius: var(--d2-r2);
    cursor: pointer;
  }
  .choice.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
  }
  .choice input {
    accent-color: var(--d2-accent);
    width: 16px;
    height: 16px;
  }
  .ttl {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
  .lang {
    max-width: 320px;
  }
  .items {
    margin: 0;
    padding-left: 1.2em;
    color: var(--d2-text-2);
  }
  .seg {
    display: inline-flex;
    align-self: flex-start;
    padding: 3px;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r2);
  }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 var(--d2-s3);
    border: 0;
    border-radius: var(--d2-r1);
    background: none;
    color: var(--d2-text-2);
    font-weight: 500;
    cursor: pointer;
  }
  .seg button.on {
    background: var(--d2-accent);
    color: var(--d2-on-accent);
  }
</style>
