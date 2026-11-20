// Start of the PaperRust design (docs/DESIGN2.md): own entry m.html under /rs/app/m/…
// Shared with the classic Light UI: API client, session/stores, i18n, router (base /rs/app/m).
import '@fontsource-variable/inter/wght.css'
import './styles/tokens.css'
import './styles/base.css'
import { mount } from 'svelte'
import App from './App.svelte'
import { api, loginUrl } from '../api/client'
import { ApiError } from '../api/v1'
import { fullLocale, loadLocale, resolveLocale, type Dict } from '../lib/i18n'
import { session, SETTINGS_KEYS } from '../stores/settings.svelte'
import { data } from '../stores/data.svelte'
import { interceptLinks } from '../router.svelte'
import { applyDensity, applyTheme, densityOf, themeMode } from './lib/theme'
import { designOf, takeDesignParam } from '../lib/design-switch'
import { APP_BASE } from '../router.svelte'

/** Core dictionary of the modern UI (tools/i18n-split.mjs → i18n/core-m/) */
const coreM = import.meta.glob<Dict>('../../i18n/core-m/[a-z][a-z]*-*.json', { import: 'default' })

async function showNoPermission() {
  const loc = resolveLocale(null, navigator.languages)
  const [m] = await Promise.all([import('../components/no-permission/no-permission'), loadLocale(loc, undefined, (l) => coreM[`../../i18n/core-m/${l}.json`]?.() ?? null)])
  await fullLocale()
  document.documentElement.lang = loc
  m.renderNoPermission(document.getElementById('d2-root') as HTMLElement, APP_BASE)
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
  // Desktop tray/links with ?design=…: store the choice per user (docs/DESIGN2.md §6)
  const wanted = takeDesignParam()
  if (wanted === 'classic') {
    import('../lib/design-apply').then((m) => m.applyDesign('classic'))
    return
  }
  if (wanted === 'modern' && designOf(ui.settings ?? {}) !== 'modern') import('../lib/design-apply').then((m) => m.applyDesign('modern', false))
  const loc = resolveLocale(session.get(SETTINGS_KEYS.LANGUAGE), navigator.languages)
  await loadLocale(loc, undefined, (l) => coreM[`../../i18n/core-m/${l}.json`]?.() ?? null)
  document.documentElement.lang = loc
  document.documentElement.classList.add('d2')
  applyTheme(themeMode(session.get(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM), session.get(SETTINGS_KEYS.DARK_MODE_ENABLED)))
  applyDensity(densityOf(session.settings))
  const root = document.getElementById('d2-root') as HTMLElement
  interceptLinks(root)
  data.loadAll()
  mount(App, { target: root })
}

boot()
