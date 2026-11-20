// Start of the Light UI: check session (otherwise login), load language, set appearance, mount app.
import 'bootstrap/dist/css/bootstrap.css'
import './styles/global.scss'
import './styles/paperrust-touch.css'
import { mount } from 'svelte'
import App from './App.svelte'
import { api, loginUrl } from './api/client'
import { ApiError } from './api/v1'
import { fullLocale, loadLocale, resolveLocale } from './lib/i18n'
import { applyAppearance } from './lib/appearance'
import { session, SETTINGS_KEYS } from './stores/settings.svelte'
import { data } from './stores/data.svelte'
import { interceptLinks, router, APP_BASE } from './router.svelte'
import { autoRedirect, designOf, takeDesignParam } from './lib/design-switch'
import { loadExtraIcons } from './components/common/bootstrap-icon/icon-extra.svelte'

async function showNoPermission() {
  const loc = resolveLocale(null, navigator.languages)
  // own chunk + full language file (texts not in the core dictionary → budget first call)
  const [m] = await Promise.all([import('./components/no-permission/no-permission'), loadLocale(loc)])
  await fullLocale()
  document.documentElement.lang = loc
  m.renderNoPermission(document.querySelector('pngx-root') as HTMLElement, APP_BASE)
}

async function boot() {
  let ui
  try {
    ui = await api.uiSettings()
  } catch (e) {
    if (e instanceof ApiError && (e.status === 401)) {
      location.href = loginUrl()
      return
    }
    if (e instanceof ApiError && e.status === 403) {
      // logged in, but without permissions: message instead of redirect (otherwise loop login ↔ app, A-013)
      await showNoPermission()
      return
    }
    throw e
  }
  session.data = ui
  // Desktop tray/links with ?design=classic|modern: take over the choice (docs/DESIGN2.md §6)
  const wanted = takeDesignParam()
  if (wanted === 'modern') {
    import('./lib/design-apply').then((m) => m.applyDesign('modern'))
    return
  }
  if (wanted === 'classic' && designOf(ui.settings ?? {}) === 'modern') import('./lib/design-apply').then((m) => m.applyDesign('classic', false))
  // PaperRust design chosen (docs/DESIGN2.md §6): redirect the entry to the modern UI
  const modern = wanted ? null : autoRedirect(designOf(ui.settings ?? {}), router.path, APP_BASE)
  if (modern) {
    location.replace(modern)
    return
  }
  const loc = resolveLocale(session.get(SETTINGS_KEYS.LANGUAGE), navigator.languages)
  await loadLocale(loc)
  document.documentElement.lang = loc
  document.documentElement.dir = 'ltr'
  applyAppearance(session.get(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM), session.get(SETTINGS_KEYS.DARK_MODE_ENABLED), session.get(SETTINGS_KEYS.THEME_COLOR))
  const root = document.querySelector('pngx-root') as HTMLElement
  interceptLinks(root)
  data.loadAll()
  mount(App, { target: root })
  // preload rare icons (not in the first call, tools/icons.mjs) when idle
  const idle = (window as { requestIdleCallback?: (cb: () => void) => void }).requestIdleCallback ?? ((cb: () => void) => setTimeout(cb, 1500))
  idle(() => void loadExtraIcons())
}

boot()
