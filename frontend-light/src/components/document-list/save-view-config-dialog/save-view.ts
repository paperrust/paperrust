// Logic for "Save view": FilterEditorComponent.generateFilterName (suggestion for the name),
// DocumentListComponent.savedViewIsModified and saveCreatedViewVisibility (visibility in the UI settings).
import {
  FILTER_ASN,
  FILTER_CORRESPONDENT,
  FILTER_CUSTOM_FIELDS_QUERY,
  FILTER_DOCUMENT_TYPE,
  FILTER_HAS_ANY_TAG,
  FILTER_HAS_CORRESPONDENT_ANY,
  FILTER_HAS_DOCUMENT_TYPE_ANY,
  FILTER_HAS_DUPLICATES,
  FILTER_HAS_STORAGE_PATH_ANY,
  FILTER_HAS_TAGS_ALL,
  FILTER_OWNER,
  FILTER_OWNER_DOES_NOT_INCLUDE,
  FILTER_OWNER_ISNULL,
  FILTER_SIMPLE_TEXT,
  FILTER_SIMPLE_TITLE,
  FILTER_STORAGE_PATH,
  FILTER_TITLE,
  FILTER_TITLE_CONTENT,
} from '../../../lib/filter-rule-type'
import { filterRulesDiffer, type FilterRule } from '../../../lib/filter-rules'
import { FILTER_NAME_KEYS } from '../../../lib/filter-editor/filter-editor-state'

type Named = { name: string } | undefined
export interface NameLookup {
  correspondent: (id: number) => Named
  documentType: (id: number) => Named
  storagePath: (id: number) => Named
  tag: (id: number) => Named
}
type T = (key: string, params?: Record<string, unknown>) => string

/** FilterEditorComponent.generateFilterName: a suggestion only with exactly one rule, otherwise '' */
export function generateFilterName(rules: { rule_type: number; value: string | null }[], names: NameLookup, t: T): string {
  if (rules.length != 1) return ''
  const rule = rules[0]
  const v = rule.value
  switch (rule.rule_type) {
    case FILTER_CORRESPONDENT:
    case FILTER_HAS_CORRESPONDENT_ANY:
      return v ? t(FILTER_NAME_KEYS.correspondent, { PH: names.correspondent(+v)?.name }) : t(FILTER_NAME_KEYS.withoutCorrespondent)
    case FILTER_DOCUMENT_TYPE:
    case FILTER_HAS_DOCUMENT_TYPE_ANY:
      return v ? t(FILTER_NAME_KEYS.documentType, { PH: names.documentType(+v)?.name }) : t(FILTER_NAME_KEYS.withoutDocumentType)
    case FILTER_STORAGE_PATH:
    case FILTER_HAS_STORAGE_PATH_ANY:
      return v ? t(FILTER_NAME_KEYS.storagePath, { PH: names.storagePath(+v)?.name }) : t(FILTER_NAME_KEYS.withoutStoragePath)
    case FILTER_HAS_TAGS_ALL:
      return t(FILTER_NAME_KEYS.tag, { PH: v == null ? undefined : names.tag(+v)?.name })
    case FILTER_HAS_ANY_TAG:
      return v == 'false' ? t(FILTER_NAME_KEYS.withoutAnyTag) : ''
    case FILTER_HAS_DUPLICATES:
      return v == 'false' ? t(FILTER_NAME_KEYS.withoutDuplicates) : t(FILTER_NAME_KEYS.withDuplicates)
    case FILTER_CUSTOM_FIELDS_QUERY:
      return t(FILTER_NAME_KEYS.customFieldsQuery)
    case FILTER_TITLE:
    case FILTER_SIMPLE_TITLE:
      return t(FILTER_NAME_KEYS.title, { PH: v })
    case FILTER_TITLE_CONTENT:
    case FILTER_SIMPLE_TEXT:
      return t(FILTER_NAME_KEYS.titleContent, { PH: v })
    case FILTER_ASN:
      return t(FILTER_NAME_KEYS.asn, { PH: v })
    case FILTER_OWNER:
      return t(FILTER_NAME_KEYS.owner, { PH: v })
    case FILTER_OWNER_DOES_NOT_INCLUDE:
      return t(FILTER_NAME_KEYS.ownerNotIn, { PH: v })
    case FILTER_OWNER_ISNULL:
      return t(FILTER_NAME_KEYS.withoutOwner)
  }
  return ''
}

export interface ViewSnapshot {
  filter_rules: { rule_type: number; value: string | null }[]
  sort_field: string | null
  sort_reverse: boolean
  page_size?: number | null
  display_mode?: string | null
  display_fields?: string[] | null
}
export interface ListSnapshot {
  filterRules: FilterRule[]
  sortField: string
  sortReverse: boolean
  pageSize: number
  displayMode: string | undefined
  displayFields: string[]
}

/** DocumentListComponent.savedViewIsModified (without the permission/active check) */
export function savedViewIsModified(view: ViewSnapshot, list: ListSnapshot, defaultDisplayFields: string[]): boolean {
  return (
    // without a sort field the list sorts by "created" (listView.applySavedViewConfig)
    (view.sort_field || 'created') !== (list.sortField || 'created') ||
    view.sort_reverse !== list.sortReverse ||
    (!!view.page_size && view.page_size !== list.pageSize) ||
    (!!view.display_mode && view.display_mode !== list.displayMode) ||
    // without a display mode "small cards" applies
    (!view.display_mode && list.displayMode !== 'smallCards') ||
    (!!view.display_fields && view.display_fields.join(',') !== list.displayFields.join(',')) ||
    (!view.display_fields && list.displayFields.join(',') !== defaultDisplayFields.join(',')) ||
    filterRulesDiffer(view.filter_rules as FilterRule[], list.filterRules)
  )
}

/** saveCreatedViewVisibility: append the new view to the visible IDs for dashboard/sidebar */
export function visibilityAfterCreate(dashboardIds: number[], sidebarIds: number[], id: number, showOnDashboard: boolean, showInSideBar: boolean) {
  const dashboard = [...dashboardIds]
  const sidebar = [...sidebarIds]
  if (showOnDashboard) dashboard.push(id)
  if (showInSideBar) sidebar.push(id)
  return { dashboard, sidebar }
}
