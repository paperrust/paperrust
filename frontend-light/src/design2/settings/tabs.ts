// Tabs of the settings in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §6): route settings/<section> → tab.
// The classic sections documents/permissions/general are together under "Documents".
import { msg } from '../../lib/i18n'

export type SettingsTab = 'display' | 'documents' | 'notifications' | 'account' | 'ai'

export function settingsTab(section: string): SettingsTab {
  switch (section.toLowerCase()) {
    case 'documents':
    case 'permissions':
    case 'general':
      return 'documents'
    case 'notifications':
      return 'notifications'
    case 'account':
    case 'profile':
      return 'account'
    case 'ai':
      return 'ai'
    default:
      // '' | interface | display | unknown
      return 'display'
  }
}

/** Sub-tabs of "Documents" = sections of the classic settings component */
export const DOC_SUBTABS = [
  { k: 'documents', label: msg('m.settings.doc_display_editing') },
  { k: 'permissions', label: msg('admin.settings.default_permissions') },
  { k: 'general', label: msg('common.general') },
] as const
