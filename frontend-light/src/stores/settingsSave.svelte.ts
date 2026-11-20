// SettingsService: further keys (settings, administration) + set()/storeSettings() on the session state.
import { session, rawValue, getSetting, SETTINGS_KEYS } from './settings.svelte'
import { cloneSettings, setSettingValue } from '../lib/settings-write'
import { storeUiSettings } from '../api/objects'

export const EXTRA_KEYS = {
  OBJECT_LIST_SIZES: 'general-settings:object-list-sizes',
  DEFAULT_PERMS_OWNER: 'general-settings:permissions:default-owner',
  DEFAULT_PERMS_VIEW_USERS: 'general-settings:permissions:default-view-users',
  DEFAULT_PERMS_VIEW_GROUPS: 'general-settings:permissions:default-view-groups',
  DEFAULT_PERMS_EDIT_USERS: 'general-settings:permissions:default-edit-users',
  DEFAULT_PERMS_EDIT_GROUPS: 'general-settings:permissions:default-edit-groups',
  NOTIFICATIONS_CONSUMER_SUCCESS: 'general-settings:notifications:consumer-success',
  NOTIFICATIONS_CONSUMER_FAILED: 'general-settings:notifications:consumer-failed',
  SAVED_VIEWS_WARN_ON_UNSAVED_CHANGE: 'general-settings:saved-views:warn-on-unsaved-change',
  SEARCH_DB_ONLY: 'general-settings:search:db-only',
  SEARCH_FULL_TYPE: 'general-settings:search:more-link',
  PDF_EDITOR_DEFAULT_EDIT_MODE: 'general-settings:document-editing:default-edit-mode',
} as const

type Kind = 'string' | 'boolean' | 'number' | 'array' | 'object'
// Excerpt from data/ui-settings.ts (type + default value)
const EXTRA: Record<string, [Kind, unknown]> = {
  [EXTRA_KEYS.OBJECT_LIST_SIZES]: ['object', { correspondents: 25, document_types: 25, tags: 25, storage_paths: 25, share_links: 25, share_link_bundles: 25 }],
  [EXTRA_KEYS.DEFAULT_PERMS_OWNER]: ['number', undefined],
  [EXTRA_KEYS.DEFAULT_PERMS_VIEW_USERS]: ['array', []],
  [EXTRA_KEYS.DEFAULT_PERMS_VIEW_GROUPS]: ['array', []],
  [EXTRA_KEYS.DEFAULT_PERMS_EDIT_USERS]: ['array', []],
  [EXTRA_KEYS.DEFAULT_PERMS_EDIT_GROUPS]: ['array', []],
  [EXTRA_KEYS.NOTIFICATIONS_CONSUMER_SUCCESS]: ['boolean', true],
  [EXTRA_KEYS.NOTIFICATIONS_CONSUMER_FAILED]: ['boolean', true],
  [EXTRA_KEYS.SAVED_VIEWS_WARN_ON_UNSAVED_CHANGE]: ['boolean', true],
  [EXTRA_KEYS.SEARCH_DB_ONLY]: ['boolean', false],
  [EXTRA_KEYS.SEARCH_FULL_TYPE]: ['string', 'title-content'],
  [EXTRA_KEYS.PDF_EDITOR_DEFAULT_EDIT_MODE]: ['string', 'create'],
}

/** SettingsService.get for all keys (incl. special case DEFAULT_PERMS_OWNER → current user) */
export function setting(key: string): any {
  const def = EXTRA[key]
  if (!def) return session.get(key)
  const v = rawValue(session.settings, key)
  if (key === EXTRA_KEYS.DEFAULT_PERMS_OWNER && v === undefined) return session.data?.user?.id
  if (v === undefined) return def[1] && typeof def[1] === 'object' ? cloneSettings(def[1]) : def[1]
  if (v === null) return null
  if (def[0] === 'boolean') return typeof v === 'string' ? JSON.parse(v) : v
  if (def[0] === 'number') return +(v as number)
  return v
}

/** SettingsService.set (state is replaced → reactive) */
export function setSetting(key: string, value: unknown) {
  const d = session.data
  if (!d) return
  const s = cloneSettings(d.settings)
  setSettingValue(s, key, value)
  session.data = { ...d, settings: s }
}

/** SettingsService.storeSettings */
export async function storeSettings() {
  return storeUiSettings(session.settings)
}

export { getSetting, SETTINGS_KEYS }
