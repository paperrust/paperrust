// UI state of the frame (palette, help, mobile navigation, theme).
import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
import { applyDensity, applyTheme, densityOf, DENSITY_KEY, nextTheme, themeMode, type Density, type ThemeMode } from '../lib/theme'

class UiState {
  paletteOpen = $state(false)
  paletteQuery = $state('')
  helpOpen = $state(false)
  navOpen = $state(false)
  /** < 768 px: panes as stack/tabs */
  mobile = $state(typeof matchMedia !== 'undefined' && matchMedia('(max-width: 767.98px)').matches)
  /** < 1100 px: sidebar as icon bar */
  compact = $state(typeof matchMedia !== 'undefined' && matchMedia('(max-width: 1099.98px)').matches)
  theme = $state<ThemeMode>('auto')
  density = $state<Density>('normal')

  constructor() {
    if (typeof matchMedia === 'undefined') return
    const m = matchMedia('(max-width: 767.98px)')
    m.addEventListener('change', (e) => (this.mobile = e.matches))
    const c = matchMedia('(max-width: 1099.98px)')
    c.addEventListener('change', (e) => (this.compact = e.matches))
  }

  syncTheme() {
    this.theme = themeMode(session.get(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM), session.get(SETTINGS_KEYS.DARK_MODE_ENABLED))
    this.density = densityOf(session.settings)
    applyDensity(this.density)
  }

  /** Set density (compact/normal) and store in ui_settings */
  async setDensity(d: Density) {
    this.density = d
    applyDensity(d)
    const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
    setSetting(DENSITY_KEY, d)
    return storeSettings()
  }

  /** Set theme and store in ui_settings (like the classic UI) */
  async setTheme(mode: ThemeMode) {
    this.theme = mode
    applyTheme(mode)
    const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
    setSetting(SETTINGS_KEYS.DARK_MODE_USE_SYSTEM, mode === 'auto')
    setSetting(SETTINGS_KEYS.DARK_MODE_ENABLED, mode === 'dark')
    storeSettings().catch(() => {})
  }

  cycleTheme() {
    return this.setTheme(nextTheme(this.theme))
  }

  openPalette(q = '') {
    this.paletteQuery = q
    this.paletteOpen = true
  }
}

export const ui = new UiState()
