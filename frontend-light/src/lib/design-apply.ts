// Save the switch and change (loaded lazily so that the first call of the classic UI does not grow).
import { setSetting, storeSettings } from '../stores/settingsSave.svelte'
import { DESIGN_KEY, switchTarget, type Design } from './design-switch'
import { APP_BASE, router } from '../router.svelte'

export async function applyDesign(target: Design, nav = true): Promise<void> {
  setSetting(DESIGN_KEY, target)
  try {
    await storeSettings()
  } catch {
    // Saving failed (e.g. without permission change_uisettings): switch anyway, the URL decides
  }
  if (nav) location.href = switchTarget(target, router.path, router.query, APP_BASE)
}
