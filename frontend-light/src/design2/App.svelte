<!-- PaperRust design: frame with fixed header bar and navigation (docs/DESIGN2.md §3). Nothing scrolls as a whole;
     each area scrolls only in its own panes. Areas other than the workspace are loaded lazily. -->
<script lang="ts">
  import { onMount, type Component } from 'svelte'
  import { t, fullLocale } from '../lib/i18n'
  import { router, APP_BASE } from '../router.svelte'
  import { toastService } from '../stores/toasts.svelte'
  import { consumerStatus } from '../stores/uploads.svelte'
  import Header from './components/Header.svelte'
  import Nav from './components/Nav.svelte'
  import TabBar from './components/TabBar.svelte'
  import Toasts from './components/Toasts.svelte'
  import DropZone from './components/DropZone.svelte'
  import Workspace from './workspace/Workspace.svelte'
  import Icon from './components/Icon.svelte'
  import { ui } from './stores/ui.svelte'
  import { KeyInterpreter, isTypingTarget, type Command } from './lib/keys'
  import { dispatch } from './lib/bus'
  import { findDocumentIdsByAsn, asnTarget } from '../components/document-asn/document-asn'
  import { tour } from './tour/state.svelte'

  type Loader = () => Promise<{ default: Component<any> }>
  const LAZY: Record<string, Loader> = {
    overview: () => import('./timeline/Overview.svelte'),
    assistant: () => import('./assistant/Assistant.svelte'),
    automation: () => import('./automation/AutomationHost.svelte'),
    manage: () => import('./manage/ManageHost.svelte'),
    settings: () => import('./settings/SettingsHost.svelte'),
    palette: () => import('./components/CommandPalette.svelte'),
    help: () => import('./components/ShortcutsHelp.svelte'),
    tray: () => import('./components/UploadTray.svelte'),
    tour: () => import('./tour/Tour.svelte'),
  }
  const MANAGE = /^(attributes(\/[^/]+)?|savedviews|workflows|mail|usersgroups|roles|config|settings\/system(\/[^/]+)?|tasks|logs|trash|tools\/(asn|scan|scanners)|protection|share-links)$/

  /** Route → area */
  function area(p: string): string {
    if (p === 'dashboard' || p === '') return 'overview'
    if (/^(documents(\/\d+(\/[a-z]+)?)?|inbox(\/\d+)?|view\/\d+(\/\d+)?)$/.test(p)) return 'workspace'
    if (p === 'assistant') return 'assistant'
    if (p === 'automation' || p.startsWith('automation/') || p === 'ai/review' || p === 'workflows') return 'automation'
    if (/^asn\/\d+$/.test(p)) return 'workspace'
    if (MANAGE.test(p)) return 'manage'
    if (/^settings(\/.*)?$/.test(p)) return 'settings'
    return 'unknown'
  }
  const current = $derived(area(router.path))
  // Former paths into the "Automation" area (docs/WORKFLOWS-NEU.md, topic 10 B): Manage → Workflows and the
  // AI review list redirect; Manage → Email (accounts) stays reachable.
  $effect(() => {
    const p = router.path
    if (p === 'workflows') router.navigate('automation/workflows', true)
    else if (p === 'ai/review') router.navigate('automation/review', true)
  })

  const loaded = $state<Record<string, Component<any>>>({})
  // Tour: start from the user menu or from the (classic) settings via the event pr:start-tour
  onMount(() => {
    const start = () => tour.start()
    addEventListener('pr:start-tour', start)
    return () => removeEventListener('pr:start-tour', start)
  })
  $effect(() => {
    if (tour.active && !loaded.tour) Promise.all([LAZY.tour(), fullLocale()]).then(([m]) => (loaded.tour = m.default))
  })
  function load(k: string) {
    if (loaded[k] || !LAZY[k]) return
    // lazily loaded areas (classic components) need the full language file
    Promise.all([LAZY[k](), k === 'manage' || k === 'settings' || k === 'automation' ? fullLocale() : null]).then(([m]) => (loaded[k] = m.default))
  }
  $effect(() => {
    if (current !== 'workspace') load(current)
  })
  $effect(() => {
    if (ui.paletteOpen) load('palette')
    if (ui.helpOpen) load('help')
    if (consumerStatus.consumerStatus.length) load('tray')
  })

  // asn/:nr (label QR, links): open the document with this ASN – like the classic UI
  $effect(() => {
    const m = /^asn\/(\d+)$/.exec(router.path)
    if (!m) return
    let cancelled = false
    findDocumentIdsByAsn(m[1])
      .catch(() => [] as number[])
      .then((ids) => { if (!cancelled) router.navigate(asnTarget(ids), true) })
    return () => (cancelled = true)
  })

  // Title of the page
  $effect(() => {
    const names: Record<string, string> = { overview: t('common.dashboard'), workspace: t('common.documents_2'), assistant: t('common.assistant'), automation: t('m.auto.title'), manage: t('common.management'), settings: t('common.settings') }
    document.title = `${names[current] ?? 'PaperRust'} – PaperRust`
  })

  // Keyboard
  const keys = new KeyInterpreter()
  const go = (p: string) => router.navigate(p)
  function run(cmd: Command) {
    if (dispatch(cmd)) return
    switch (cmd) {
      case 'palette':
        ui.paletteOpen ? (ui.paletteOpen = false) : ui.openPalette()
        break
      case 'search':
        ui.openPalette()
        break
      case 'help':
        ui.helpOpen = true
        break
      case 'upload':
        document.getElementById('d2-upload-input')?.click()
        break
      case 'theme':
        ui.cycleTheme()
        break
      case 'go-overview':
        go('dashboard')
        break
      case 'go-inbox':
        go('inbox')
        break
      case 'go-documents':
        go('documents')
        break
      case 'go-assistant':
        go('assistant')
        break
      case 'go-manage':
        go('attributes/tags')
        break
      case 'go-settings':
        go('settings')
        break
    }
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.defaultPrevented || e.isComposing) return
    // open dialogs handle their keys themselves (except Ctrl+K)
    const inDialog = !!(document.activeElement as HTMLElement | null)?.closest?.('[role="dialog"]')
    const cmd = keys.interpret(e, isTypingTarget(document.activeElement))
    if (!cmd || (inDialog && cmd !== 'palette')) return
    if (cmd === 'escape') {
      if (dispatch('escape')) e.preventDefault()
      else if (ui.navOpen) ui.navOpen = false
      return
    }
    e.preventDefault()
    run(cmd)
  }

  // Notifications about the processing status (like the classic UI)
  onMount(() => {
    ui.syncTheme()
    consumerStatus.connect()
    const offs = [
      consumerStatus.onDocumentConsumptionFinished((s) => {
        toastService.show({
          content: t('common.document_added_paperrust', { PH: s.filename }),
          delay: 10000,
          level: 'success',
          actionName: s.documentId ? t('common.open_document') : undefined,
          action: s.documentId ? () => router.navigate('documents/' + s.documentId) : undefined,
        })
      }),
      consumerStatus.onDocumentConsumptionFailed((s) => toastService.showError(t('common.could_not_add', { PH: s.filename, PH_1: s.message }))),
    ]
    return () => {
      offs.forEach((o) => o())
      consumerStatus.disconnect()
    }
  })
</script>

<svelte:window {onkeydown} />

<a class="d2-skip" href="#d2-main">{t('m.app.skip_content')}</a>
<div class="d2-app" class:mobile={ui.mobile} class:compact={ui.compact && !ui.mobile}>
  <Header />
  <div class="d2-body">
    <Nav />
    <main id="d2-main" class="d2-main" tabindex="-1">
      {#if current === 'workspace'}
        <Workspace />
      {:else if current === 'unknown'}
        <div class="d2-empty">
          <Icon name="question-circle" size="2rem" />
          <p>{t('m.app.page_not_available_in')}</p>
          <a class="d2-btn" href={`${APP_BASE}/${router.path}${router.query}`}>{t('common.open_in_classic_interface')}</a>
        </div>
      {:else if loaded[current]}
        {@const C = loaded[current]}
        <C />
      {:else}
        <div class="d2-empty" aria-busy="true">{t('common.loading')}</div>
      {/if}
    </main>
  </div>
  {#if ui.mobile}<TabBar />{/if}
</div>
<DropZone />
<Toasts />
{#if loaded.tray && consumerStatus.consumerStatus.length}
  {@const T = loaded.tray}<T />
{/if}
{#if ui.paletteOpen && loaded.palette}
  {@const P = loaded.palette}<P />
{/if}
{#if ui.helpOpen && loaded.help}
  {@const H = loaded.help}<H />
{/if}
{#if tour.active && loaded.tour}
  {@const R = loaded.tour}<R />
{/if}

<style>
  .d2-app {
    display: grid;
    grid-template-rows: var(--d2-header-h) minmax(0, 1fr);
    height: 100vh;
    height: 100dvh;
    overflow: hidden;
  }
  .d2-app.mobile {
    grid-template-rows: var(--d2-header-h) minmax(0, 1fr) var(--d2-tabbar-h);
  }
  .d2-body {
    display: grid;
    grid-template-columns: var(--d2-nav-w) minmax(0, 1fr);
    min-height: 0;
  }
  .compact .d2-body {
    grid-template-columns: var(--d2-rail-w) minmax(0, 1fr);
  }
  .mobile .d2-body {
    grid-template-columns: minmax(0, 1fr);
  }
  .d2-main {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    outline: none;
    display: flex;
    flex-direction: column;
  }
  .d2-main > :global(*) {
    flex: 1 1 auto;
    min-height: 0;
  }
  .d2-skip {
    position: fixed;
    left: 8px;
    top: -60px;
    z-index: 100;
    padding: 8px 12px;
    border-radius: var(--d2-r1);
    background: var(--d2-accent);
    color: var(--d2-on-accent);
  }
  .d2-skip:focus {
    top: 8px;
  }
</style>
