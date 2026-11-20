// Selection of the home page components of the logged-in user (ui_settings `paperrust.home.widgets`).
// Read from the session state (reactive), write like the other settings (settingsSave → POST /api/ui_settings/).
import { session, rawValue, SETTINGS_KEYS } from '../../stores/settings.svelte'
import { HOME_KEY, normalizeWidgets, serializeWidgets, visibleWidgets, type WidgetEntry, type WidgetId } from './widgets'

class HomeState {
  get aiOn(): boolean {
    return !!session.get(SETTINGS_KEYS.AI_ENABLED)
  }
  get list(): WidgetEntry[] {
    return normalizeWidgets(rawValue(session.settings, HOME_KEY), this.aiOn)
  }
  get visible(): WidgetId[] {
    return visibleWidgets(this.list, this.aiOn)
  }
  /** Set immediately in the session state (home page follows), then save; errors go to the caller */
  async save(list: WidgetEntry[] | null) {
    const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
    setSetting(HOME_KEY, list ? serializeWidgets(list) : null)
    await storeSettings()
  }
}

export const home = new HomeState()
