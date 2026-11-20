import { setSetting, storeSettings } from './settingsSave.svelte'
import { SETTINGS_KEYS } from './settings.svelte'
import { toastService } from './toasts.svelte'
import { t } from '../lib/i18n'

// SettingsService.organizingSidebarSavedViews: as long as the "Saved views" page is open (and the user may
// save UI settings), the sidebar shows handles for reordering the views.
class SavedViewsOrganizing {
  active = $state(false)
}
export const savedViewsOrganizing = new SavedViewsOrganizing()

/** AppFrameComponent.onDrop → SettingsService.updateSidebarViewsSort (toast like Angular) */
export async function dropSidebarView(ids: number[], previousIndex: number, currentIndex: number) {
  const next = [...ids]
  const [m] = next.splice(previousIndex, 1)
  next.splice(currentIndex, 0, m)
  setSetting(SETTINGS_KEYS.SIDEBAR_VIEWS_SORT_ORDER, [...new Set(next)])
  try {
    await storeSettings()
    toastService.showSuccess(t('app.sidebar_views_updated'))
  } catch (e) {
    toastService.showError(t('app.error_updating_sidebar_views'), e)
  }
}
