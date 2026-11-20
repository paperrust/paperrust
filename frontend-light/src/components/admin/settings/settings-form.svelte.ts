// Logic of admin/settings/settings.component.ts: form ↔ UI settings (getCurrentSettings/saveSettings),
// change detection (dirtyCheck), live preview of the appearance, language/date lists.
import { AVAILABLE_LOCALES, msg, t } from '../../../lib/i18n'
import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
import { EXTRA_KEYS, setSetting, setting, storeSettings } from '../../../stores/settingsSave.svelte'

export interface LanguageOption {
  code: string
  name: string
  englishName?: string
  dateInputFormat?: string
}

// settings.service.ts LANGUAGE_OPTIONS (order of the source; sorted at runtime by translated name)
const LANGUAGES: LanguageOption[] = [
  { code: 'en-us', name: msg('admin.settings.english'), englishName: 'English (US)', dateInputFormat: 'mm/dd/yyyy' },
  { code: 'af-za', name: msg('admin.settings.afrikaans'), englishName: 'Afrikaans', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'ar-ar', name: msg('admin.settings.arabic'), englishName: 'Arabic', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'be-by', name: msg('admin.settings.belarusian'), englishName: 'Belarusian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'bg-bg', name: msg('admin.settings.bulgarian'), englishName: 'Bulgarian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'ca-es', name: msg('admin.settings.catalan'), englishName: 'Catalan', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'cs-cz', name: msg('admin.settings.czech'), englishName: 'Czech', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'da-dk', name: msg('admin.settings.danish'), englishName: 'Danish', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'de-de', name: msg('admin.settings.german'), englishName: 'German', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'el-gr', name: msg('admin.settings.greek'), englishName: 'Greek', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'en-gb', name: msg('admin.settings.english_gb'), englishName: 'English (GB)', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'es-es', name: msg('admin.settings.spanish'), englishName: 'Spanish', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'fi-fi', name: msg('admin.settings.finnish'), englishName: 'Finnish', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'fr-fr', name: msg('admin.settings.french'), englishName: 'French', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'hu-hu', name: msg('admin.settings.hungarian'), englishName: 'Hungarian', dateInputFormat: 'yyyy.mm.dd' },
  { code: 'id-id', name: msg('admin.settings.indonesian'), englishName: 'Indonesian', dateInputFormat: 'dd-mm-yyyy' },
  { code: 'it-it', name: msg('admin.settings.italian'), englishName: 'Italian', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'ja-jp', name: msg('admin.settings.japanese'), englishName: 'Japanese', dateInputFormat: 'yyyy/mm/dd' },
  { code: 'ko-kr', name: msg('admin.settings.korean'), englishName: 'Korean', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'lb-lu', name: msg('admin.settings.luxembourgish'), englishName: 'Luxembourgish', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'nl-nl', name: msg('admin.settings.dutch'), englishName: 'Dutch', dateInputFormat: 'dd-mm-yyyy' },
  { code: 'no-no', name: msg('admin.settings.norwegian'), englishName: 'Norwegian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'fa-ir', name: msg('admin.settings.persian'), englishName: 'Persian', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'pl-pl', name: msg('admin.settings.polish'), englishName: 'Polish', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'pt-br', name: msg('admin.settings.portuguese_brazil'), englishName: 'Portuguese (Brazil)', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'pt-pt', name: msg('admin.settings.portuguese'), englishName: 'Portuguese', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'ro-ro', name: msg('admin.settings.romanian'), englishName: 'Romanian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'ru-ru', name: msg('admin.settings.russian'), englishName: 'Russian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'sk-sk', name: msg('admin.settings.slovak'), englishName: 'Slovak', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'sl-si', name: msg('admin.settings.slovenian'), englishName: 'Slovenian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'sr-cs', name: msg('admin.settings.serbian'), englishName: 'Serbian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'sv-se', name: msg('admin.settings.swedish'), englishName: 'Swedish', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'tr-tr', name: msg('admin.settings.turkish'), englishName: 'Turkish', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'uk-ua', name: msg('admin.settings.ukrainian'), englishName: 'Ukrainian', dateInputFormat: 'dd.mm.yyyy' },
  { code: 'vi-vn', name: msg('admin.settings.vietnamese'), englishName: 'Vietnamese', dateInputFormat: 'dd/mm/yyyy' },
  { code: 'zh-cn', name: msg('admin.settings.chinese_simplified'), englishName: 'Chinese Simplified', dateInputFormat: 'yyyy-mm-dd' },
  { code: 'zh-tw', name: msg('admin.settings.chinese_traditional'), englishName: 'Chinese Traditional', dateInputFormat: 'yyyy/mm/dd' },
]

/** SettingsService.getLanguageOptions: translated, sorted with a.name < b.name ? -1 : 1 */
export function languageOptions(): LanguageOption[] {
  return LANGUAGES.map((l) => ({ ...l, name: t(l.name) })).sort((a, b) => (a.name < b.name ? -1 : 1))
}
/** Display language: only languages with their own catalog (+ the saved one, if a different one was selected) */
export function displayLanguageOptions(current: string = session.get(SETTINGS_KEYS.LANGUAGE) ?? ''): LanguageOption[] {
  const own = (code: string) => AVAILABLE_LOCALES.some((l) => l.toLowerCase() === code) || (!!current && code === current)
  return [{ code: '', name: t('admin.settings.use_system_language') }, ...languageOptions().filter((l) => own(l.code))]
}
export function dateLocaleOptions(): LanguageOption[] {
  return [{ code: '', name: t('admin.settings.use_date_format_display') }, { code: 'iso-8601', name: t('admin.settings.iso_8601'), dateInputFormat: 'yyyy-mm-dd' }, ...languageOptions()]
}

export const DOCUMENT_DETAIL_FIELD_OPTIONS = [
  { id: 'archive_serial_number', label: msg('common.archive_serial_number') },
  { id: 'correspondent', label: msg('common.correspondent') },
  { id: 'document_type', label: msg('common.document_type') },
  { id: 'storage_path', label: msg('common.storage_path') },
  { id: 'tags', label: msg('common.tags') },
]

export const SIDEBAR_ITEM_OPTIONS = [
  { id: 'dashboard', label: msg('common.dashboard') },
  { id: 'saved_views', label: msg('common.saved_views') },
  { id: 'share_links', label: msg('common.share_links') },
  { id: 'workflows', label: msg('common.workflows') },
  { id: 'mail', label: msg('common.mail') },
  { id: 'documentation', label: msg('common.documentation') },
]

/** Form field → UI setting (order like saveSettings) */
export const FIELDS: [string, string][] = [
  ['bulkEditApplyOnClose', SETTINGS_KEYS.BULK_EDIT_APPLY_ON_CLOSE],
  ['bulkEditConfirmationDialogs', SETTINGS_KEYS.BULK_EDIT_CONFIRMATION_DIALOGS],
  ['documentListItemPerPage', SETTINGS_KEYS.DOCUMENT_LIST_SIZE],
  ['slimSidebarEnabled', SETTINGS_KEYS.SLIM_SIDEBAR],
  ['sidebarHiddenItems', SETTINGS_KEYS.SIDEBAR_HIDDEN_ITEMS],
  ['darkModeUseSystem', SETTINGS_KEYS.DARK_MODE_USE_SYSTEM],
  ['darkModeEnabled', SETTINGS_KEYS.DARK_MODE_ENABLED],
  ['darkModeInvertThumbs', SETTINGS_KEYS.DARK_MODE_THUMB_INVERTED],
  ['themeColor', SETTINGS_KEYS.THEME_COLOR],
  ['useNativePdfViewer', SETTINGS_KEYS.USE_NATIVE_PDF_VIEWER],
  ['pdfViewerDefaultZoom', SETTINGS_KEYS.PDF_VIEWER_ZOOM_SETTING],
  ['pdfEditorDefaultEditMode', EXTRA_KEYS.PDF_EDITOR_DEFAULT_EDIT_MODE],
  ['dateLocale', SETTINGS_KEYS.DATE_LOCALE],
  ['dateFormat', SETTINGS_KEYS.DATE_FORMAT],
  ['notificationsConsumerSuccess', EXTRA_KEYS.NOTIFICATIONS_CONSUMER_SUCCESS],
  ['notificationsConsumerFailed', EXTRA_KEYS.NOTIFICATIONS_CONSUMER_FAILED],
  ['notesEnabled', SETTINGS_KEYS.NOTES_ENABLED],
  ['updateCheckingEnabled', SETTINGS_KEYS.UPDATE_CHECKING_ENABLED],
  ['savedViewsWarnOnUnsavedChange', EXTRA_KEYS.SAVED_VIEWS_WARN_ON_UNSAVED_CHANGE],
  ['sidebarViewsShowCount', SETTINGS_KEYS.SIDEBAR_VIEWS_SHOW_COUNT],
  ['defaultPermsOwner', EXTRA_KEYS.DEFAULT_PERMS_OWNER],
  ['defaultPermsViewUsers', EXTRA_KEYS.DEFAULT_PERMS_VIEW_USERS],
  ['defaultPermsViewGroups', EXTRA_KEYS.DEFAULT_PERMS_VIEW_GROUPS],
  ['defaultPermsEditUsers', EXTRA_KEYS.DEFAULT_PERMS_EDIT_USERS],
  ['defaultPermsEditGroups', EXTRA_KEYS.DEFAULT_PERMS_EDIT_GROUPS],
  ['documentEditingRemoveInboxTags', SETTINGS_KEYS.DOCUMENT_EDITING_REMOVE_INBOX_TAGS],
  ['documentEditingOverlayThumbnail', SETTINGS_KEYS.DOCUMENT_EDITING_OVERLAY_THUMBNAIL],
  ['documentEditingAutoSuggest', SETTINGS_KEYS.DOCUMENT_EDITING_AUTO_SUGGEST],
  ['documentEditingSuggestionSource', SETTINGS_KEYS.DOCUMENT_EDITING_SUGGESTION_SOURCE],
  ['documentDetailsHiddenFields', SETTINGS_KEYS.DOCUMENT_DETAILS_HIDDEN_FIELDS],
  ['searchDbOnly', EXTRA_KEYS.SEARCH_DB_ONLY],
  ['searchLink', EXTRA_KEYS.SEARCH_FULL_TYPE],
]

export type SettingsFormValue = Record<string, any>

/** getCurrentSettings */
export function currentSettings(): SettingsFormValue {
  const v: SettingsFormValue = {}
  for (const [k, key] of FIELDS) v[k] = setting(key)
  v.displayLanguage = session.get(SETTINGS_KEYS.LANGUAGE)
  return v
}

/** dirtyCheck: form value ≠ saved state */
export const isDirty = (form: SettingsFormValue, store: SettingsFormValue) => JSON.stringify(normalize(form)) !== JSON.stringify(normalize(store))
function normalize(v: SettingsFormValue) {
  return Object.keys(v)
    .sort()
    .map((k) => [k, v[k] === undefined ? null : v[k]])
}

function languageCookie(language: string) {
  const prefix = document.querySelector('meta[name=cookie_prefix]')?.getAttribute('content') ?? ''
  const name = `${prefix}django_language`
  document.cookie = language?.length ? `${name}=${encodeURIComponent(language)}; path=/` : `${name}=; expires=Thu, 01 Jan 1970 00:00:01 GMT; path=/`
}

/** saveSettings: set values, language + cookie, save. Returns whether a reload is needed. */
export async function saveSettings(form: SettingsFormValue, store: SettingsFormValue): Promise<boolean> {
  const reloadRequired = form.displayLanguage != store.displayLanguage || (form.updateCheckingEnabled != store.updateCheckingEnabled && !!form.updateCheckingEnabled)
  for (const [k, key] of FIELDS) {
    let value = form[k]
    if (k === 'darkModeEnabled' || k === 'darkModeInvertThumbs') value = (value == true).toString()
    setSetting(key, value)
  }
  setSetting(SETTINGS_KEYS.LANGUAGE, form.displayLanguage)
  languageCookie(form.displayLanguage)
  await storeSettings()
  return reloadRequired
}
