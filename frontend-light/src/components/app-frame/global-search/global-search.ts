// Logic of app-frame/global-search/global-search.component.ts without DOM: result groups (order, icon,
// name field as in the template), target of the primary/secondary action, locking of the buttons, path for "Filter documents".
import {
  FILTER_FULLTEXT_QUERY,
  FILTER_HAS_CORRESPONDENT_ANY,
  FILTER_HAS_DOCUMENT_TYPE_ANY,
  FILTER_HAS_STORAGE_PATH_ANY,
  FILTER_HAS_TAGS_ALL,
  FILTER_SIMPLE_TEXT,
} from '../../../lib/filter-rule-type'
import { msg } from '../../../lib/i18n'
import { paramsFromViewState, paramsToURLSearchParams } from '../../../lib/query-params'

/** SearchService.GlobalSearchResult */
export interface GlobalSearchResult {
  total: number
  documents: any[]
  saved_views: any[]
  correspondents: any[]
  document_types: any[]
  storage_paths: any[]
  tags: any[]
  users: any[]
  groups: any[]
  mail_accounts: any[]
  mail_rules: any[]
  custom_fields: any[]
  workflows: any[]
}

export type SectionKey = Exclude<keyof GlobalSearchResult, 'total'>

export interface Section {
  key: SectionKey
  header: string
  nameProp: string
  icon: string
  date?: boolean
}

/** Groups in the order of the template */
export const SECTIONS: Section[] = [
  { key: 'documents', header: msg('common.documents_2'), nameProp: 'title', icon: 'file-text', date: true },
  { key: 'saved_views', header: msg('common.saved_views'), nameProp: 'name', icon: 'funnel' },
  { key: 'tags', header: msg('common.tags'), nameProp: 'name', icon: 'tag' },
  { key: 'correspondents', header: msg('common.correspondents'), nameProp: 'name', icon: 'person' },
  { key: 'document_types', header: msg('common.document_types'), nameProp: 'name', icon: 'file-earmark' },
  { key: 'storage_paths', header: msg('common.storage_paths'), nameProp: 'name', icon: 'folder' },
  { key: 'users', header: msg('common.users'), nameProp: 'username', icon: 'person-square' },
  { key: 'groups', header: msg('common.groups'), nameProp: 'name', icon: 'people' },
  { key: 'custom_fields', header: msg('common.custom_fields'), nameProp: 'name', icon: 'ui-radios' },
  { key: 'mail_accounts', header: msg('manage.mail.mail_accounts'), nameProp: 'name', icon: 'envelope-at' },
  { key: 'mail_rules', header: msg('manage.mail.mail_rules'), nameProp: 'name', icon: 'envelope' },
  { key: 'workflows', header: msg('common.workflows'), nameProp: 'name', icon: 'boxes' },
]

/** Types whose primary action is an edit dialog (template: icon "pencil", no secondary button) */
export const EDIT_PRIMARY: SectionKey[] = ['users', 'groups', 'mail_accounts', 'mail_rules', 'custom_fields', 'workflows']
/** Types whose primary action is "Filter documents" (secondary action: edit dialog) */
export const FILTER_RULE_FOR: Partial<Record<SectionKey, number>> = {
  correspondents: FILTER_HAS_CORRESPONDENT_ANY,
  document_types: FILTER_HAS_DOCUMENT_TYPE_ANY,
  storage_paths: FILTER_HAS_STORAGE_PATH_ANY,
  tags: FILTER_HAS_TAGS_ALL,
}

/** Does the row have a secondary button? (not for views and the EDIT_PRIMARY types) */
export function hasSecondary(type: SectionKey): boolean {
  return type !== 'saved_views' && !EDIT_PRIMARY.includes(type)
}

/** disablePrimaryButton */
export function disablePrimary(type: SectionKey, canChange: boolean): boolean {
  return ['workflows', 'custom_fields', 'groups', 'users'].includes(type) ? !canChange : false
}

/** disableSecondaryButton */
export function disableSecondary(type: SectionKey, canChange: boolean): boolean {
  return type === 'documents' ? false : !canChange
}

/** Size of the edit dialog (NgbModal size) */
export function editDialogSize(type: SectionKey): string | undefined {
  if (type === 'users' || type === 'groups') return 'lg'
  if (type === 'mail_accounts' || type === 'mail_rules' || type === 'workflows') return 'xl'
  return undefined
}

/** primaryAction for correspondent/document type/storage path/tag: /documents with a filter rule (page 1) */
export function filterDocumentsPath(ruleType: number, id: number, sortField: string | null | undefined, sortReverse: boolean): string {
  const params = paramsFromViewState({
    filterRules: [{ rule_type: ruleType, value: id.toString() }],
    currentPage: 1,
    sortField: sortField ?? 'created',
    sortReverse,
  } as any)
  const qs = paramsToURLSearchParams(params).toString()
  return `documents${qs ? '?' + qs : ''}`
}

/** runFullSearch: rule type depending on the setting "general-settings:search:more-link" */
export function fullSearchRuleType(fullType: unknown): number {
  return fullType === 'advanced' ? FILTER_FULLTEXT_QUERY : FILTER_SIMPLE_TEXT
}

/** Number of entries (in template order) */
export function flatten(r: GlobalSearchResult | null): { type: SectionKey; item: any }[] {
  if (!r) return []
  return SECTIONS.flatMap((s) => (r[s.key] ?? []).map((item: any) => ({ type: s.key, item })))
}
