// UI settings + permissions like SettingsService/PermissionsService (Angular), read-only in F0.
import type { UiSettingsResponse } from '../api/v1'

export const SETTINGS_KEYS = {
  VERSION: 'version',
  LANGUAGE: 'language',
  APP_LOGO: 'app_logo',
  APP_TITLE: 'app_title',
  DOCUMENT_LIST_SIZE: 'general-settings:documentListSize',
  DARK_MODE_USE_SYSTEM: 'general-settings:dark-mode:use-system',
  DARK_MODE_ENABLED: 'general-settings:dark-mode:enabled',
  DARK_MODE_THUMB_INVERTED: 'general-settings:dark-mode:thumb-inverted',
  EMAIL_ENABLED: 'email_enabled',
  BULK_EDIT_APPLY_ON_CLOSE: 'general-settings:bulk-edit:apply-on-close',
  BULK_EDIT_CONFIRMATION_DIALOGS: 'general-settings:bulk-edit:confirmation-dialogs',
  THEME_COLOR: 'general-settings:theme:color',
  DATE_LOCALE: 'general-settings:date-display:date-locale',
  DATE_FORMAT: 'general-settings:date-display:date-format',
  NOTES_ENABLED: 'general-settings:notes-enabled',
  SLIM_SIDEBAR: 'general-settings:slim-sidebar',
  SIDEBAR_HIDDEN_ITEMS: 'general-settings:sidebar:hidden-items',
  ATTRIBUTES_SECTIONS_COLLAPSED: 'general-settings:attributes-sections-collapsed',
  UPDATE_CHECKING_ENABLED: 'general-settings:update-checking:enabled',
  SIDEBAR_VIEWS_VISIBLE_IDS: 'general-settings:saved-views:sidebar-views-visible-ids',
  SIDEBAR_VIEWS_SORT_ORDER: 'general-settings:saved-views:sidebar-views-sort-order',
  SIDEBAR_VIEWS_SHOW_COUNT: 'general-settings:saved-views:sidebar-views-show-count',
  AI_ENABLED: 'ai_enabled',
  USE_NATIVE_PDF_VIEWER: 'general-settings:document-details:native-pdf-viewer',
  PDF_VIEWER_ZOOM_SETTING: 'general-settings:document-details:pdf-viewer-zoom-setting',
  AUDITLOG_ENABLED: 'general-settings:auditlog-enabled',
  DOCUMENT_EDITING_REMOVE_INBOX_TAGS: 'general-settings:document-editing:remove-inbox-tags',
  DOCUMENT_EDITING_OVERLAY_THUMBNAIL: 'general-settings:document-editing:overlay-thumbnail',
  DOCUMENT_EDITING_AUTO_SUGGEST: 'general-settings:document-editing:auto-suggest',
  DOCUMENT_EDITING_SUGGESTION_SOURCE: 'general-settings:document-editing:suggestion-source',
  DOCUMENT_DETAILS_HIDDEN_FIELDS: 'general-settings:document-details:hidden-fields',
} as const

type Kind = 'string' | 'boolean' | 'number' | 'array' | 'object'
// Excerpt from data/ui-settings.ts (type + default value)
const SETTINGS: Record<string, [Kind, unknown]> = {
  [SETTINGS_KEYS.VERSION]: ['string', ''],
  [SETTINGS_KEYS.LANGUAGE]: ['string', ''],
  [SETTINGS_KEYS.APP_LOGO]: ['string', ''],
  [SETTINGS_KEYS.APP_TITLE]: ['string', ''],
  [SETTINGS_KEYS.DOCUMENT_LIST_SIZE]: ['number', 50],
  [SETTINGS_KEYS.DARK_MODE_USE_SYSTEM]: ['boolean', true],
  [SETTINGS_KEYS.DARK_MODE_ENABLED]: ['boolean', false],
  [SETTINGS_KEYS.DARK_MODE_THUMB_INVERTED]: ['boolean', true],
  [SETTINGS_KEYS.EMAIL_ENABLED]: ['boolean', false],
  [SETTINGS_KEYS.BULK_EDIT_APPLY_ON_CLOSE]: ['boolean', false],
  [SETTINGS_KEYS.BULK_EDIT_CONFIRMATION_DIALOGS]: ['boolean', true],
  [SETTINGS_KEYS.THEME_COLOR]: ['string', ''],
  [SETTINGS_KEYS.DATE_LOCALE]: ['string', ''],
  [SETTINGS_KEYS.DATE_FORMAT]: ['string', 'mediumDate'],
  [SETTINGS_KEYS.NOTES_ENABLED]: ['boolean', true],
  [SETTINGS_KEYS.SLIM_SIDEBAR]: ['boolean', false],
  [SETTINGS_KEYS.SIDEBAR_HIDDEN_ITEMS]: ['array', []],
  [SETTINGS_KEYS.ATTRIBUTES_SECTIONS_COLLAPSED]: ['array', []],
  [SETTINGS_KEYS.UPDATE_CHECKING_ENABLED]: ['boolean', false],
  [SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS]: ['array', []],
  [SETTINGS_KEYS.SIDEBAR_VIEWS_SORT_ORDER]: ['array', []],
  [SETTINGS_KEYS.SIDEBAR_VIEWS_SHOW_COUNT]: ['boolean', true],
  [SETTINGS_KEYS.AI_ENABLED]: ['boolean', false],
  [SETTINGS_KEYS.USE_NATIVE_PDF_VIEWER]: ['boolean', false],
  [SETTINGS_KEYS.PDF_VIEWER_ZOOM_SETTING]: ['string', 'page-width'],
  [SETTINGS_KEYS.AUDITLOG_ENABLED]: ['boolean', true],
  [SETTINGS_KEYS.DOCUMENT_EDITING_REMOVE_INBOX_TAGS]: ['boolean', false],
  [SETTINGS_KEYS.DOCUMENT_EDITING_OVERLAY_THUMBNAIL]: ['boolean', true],
  [SETTINGS_KEYS.DOCUMENT_EDITING_AUTO_SUGGEST]: ['boolean', true],
  [SETTINGS_KEYS.DOCUMENT_EDITING_SUGGESTION_SOURCE]: ['string', 'ai'],
  [SETTINGS_KEYS.DOCUMENT_DETAILS_HIDDEN_FIELDS]: ['array', []],
}

const UNSAFE = new Set(['__proto__', 'constructor', 'prototype'])

/** Raw value like SettingsService.getSettingRawValue ("general-settings:" dropped, "-" → "_"). */
export function rawValue(settings: Record<string, any>, key: string): unknown {
  let value: unknown = undefined
  let obj: any = settings
  const parts = key.replace('general-settings:', '').split(':')
  parts.forEach((p, i) => {
    p = p.replace(/-/g, '_')
    if (UNSAFE.has(p) || obj == null || typeof obj !== 'object' || !Object.prototype.hasOwnProperty.call(obj, p)) return
    if (i === parts.length - 1) value = obj[p]
    else obj = obj[p]
  })
  return value
}

/**
 * Read boolean setting robustly: older data (upstream/snapshot) store e.g. dark_mode.enabled as
 * string "false"/"true" (A-279/A-280). Unknown values → default instead of exception.
 */
export function toBool(v: unknown, fallback: boolean): boolean {
  if (typeof v == 'string') v = ({ true: true, 1: true, false: false, 0: false, '': false } as Record<string, boolean>)[v.trim().toLowerCase()]
  return typeof v == 'boolean' ? v : typeof v == 'number' ? v != 0 : fallback
}

export function getSetting(settings: Record<string, any>, key: string): any {
  const def = SETTINGS[key]
  if (!def) return undefined
  const v = rawValue(settings, key)
  if (v === undefined) return def[1]
  if (v === null) return null
  if (def[0] === 'boolean') return toBool(v, def[1] as boolean)
  if (def[0] === 'number') return +(v as number)
  return v
}

export function settingIsSet(settings: Record<string, any>, key: string) {
  return rawValue(settings, key) !== undefined
}

export type PermissionAction = 'add' | 'view' | 'change' | 'delete'

class SessionState {
  data = $state<UiSettingsResponse | null>(null)

  get settings() {
    return this.data?.settings ?? {}
  }
  get(key: string) {
    return getSetting(this.settings, key)
  }
  isSet(key: string) {
    return settingIsSet(this.settings, key)
  }
  get displayName(): string {
    const u = this.data?.user
    return (u?.first_name ?? u?.username ?? '').trim()
  }
  /** PermissionsService.currentUserCan(action, '%s_type') */
  can(action: PermissionAction, type: string): boolean {
    const u = this.data?.user
    return !!(u?.is_superuser || this.data?.permissions?.includes(`${action}_${type}`))
  }
  /** PermissionsService.currentUserOwnsObject */
  ownsObject(o: { owner?: number | null } | null | undefined): boolean {
    const u = this.data?.user
    return !o || !o.owner || !!u?.is_superuser || o.owner === u?.id
  }
  /** PermissionsService.currentUserHasObjectPermissions */
  hasObjectPermissions(action: 'view' | 'change', o: any): boolean {
    const u: any = this.data?.user
    const groups: number[] = u?.groups ?? []
    const p = o?.permissions?.[action]
    return (
      this.ownsObject(o) ||
      (action === 'change' && !!o?.user_can_change) ||
      !!p?.users?.includes(u?.id) ||
      (p?.groups?.filter((g: number) => groups.includes(g)).length ?? 0) > 0
    )
  }
  get isAdmin() {
    return !!this.data?.user?.is_staff
  }
}

export const session = new SessionState()
