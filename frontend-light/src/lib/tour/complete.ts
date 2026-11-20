// SettingsService.completeTour (end of the tour or closing the welcome widget)
import { t } from '../i18n'
import { session, rawValue } from '../../stores/settings.svelte'
import { setSetting, storeSettings } from '../../stores/settingsSave.svelte'
import { toastService } from '../../stores/toasts.svelte'

export const TOUR_COMPLETE = 'general-settings:tour-complete'

/** Like getSetting: boolean values may be stored as a string */
const truthy = (v: unknown) => (typeof v === 'string' ? v === 'true' || v === '1' : !!v)

export async function completeTour(): Promise<void> {
  if (truthy(rawValue(session.settings, TOUR_COMPLETE)) || !session.can('change', 'uisettings')) return
  setSetting(TOUR_COMPLETE, true)
  try {
    await storeSettings()
    toastService.show({ content: t('app.restart_tour_settings_page'), delay: 5000 })
  } catch {}
}
