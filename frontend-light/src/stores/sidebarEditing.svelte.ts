// SettingsService.sidebarHiddenItemsEditing / organizingSidebarItems / sidebarItemIsHidden / updateSidebarItemVisibility:
// As long as the "General" tab of the settings is open, the sidebar shows a switch per hideable entry.
import { session, SETTINGS_KEYS } from './settings.svelte'

/** HideableSidebarItemID (data/ui-settings.ts) */
export const HIDEABLE_SIDEBAR_ITEM_IDS = ['dashboard', 'saved_views', 'share_links', 'workflows', 'mail', 'documentation'] as const
export type HideableSidebarItemID = (typeof HIDEABLE_SIDEBAR_ITEM_IDS)[number]

class SidebarEditing {
  /** null = not in edit mode */
  hiddenItemsEditing = $state<string[] | null>(null)
  /** sidebarHiddenItemsEditingChanged */
  listeners = new Set<(items: string[]) => void>()

  get organizing() {
    return this.hiddenItemsEditing !== null
  }
  hiddenItems(): string[] {
    return this.hiddenItemsEditing ?? session.get(SETTINGS_KEYS.SIDEBAR_HIDDEN_ITEMS) ?? []
  }
  isHidden(item: string) {
    return this.hiddenItems().includes(item)
  }
  updateVisibility(item: string, visible: boolean) {
    const s = new Set(this.hiddenItems())
    if (visible) s.delete(item)
    else s.add(item)
    const updated = [...s]
    this.hiddenItemsEditing = updated
    for (const l of this.listeners) l(updated)
  }
}

export const sidebarEditing = new SidebarEditing()
