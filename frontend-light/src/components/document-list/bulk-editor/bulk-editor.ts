// Logic from document-list/bulk-editor/bulk-editor.component.ts (without DOM): selection query, confirmation texts
// (_localizeList, setTags, setCustomFields, …). Texts go into ConfirmDialog.message, which is rendered as HTML –
// therefore the *Message functions already return escaped text (names come from users).
import { t } from '../../../lib/i18n'
import { queryParamsFromFilterRules } from '../../../lib/query-params'
import type { DocumentSelectionQuery } from '../../../api/v1'
import type { FilterRule } from '../../../lib/filter-rules'

export interface Named {
  id: number
  name: string
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
}

/** getSelectionQuery */
export function selectionQuery(l: { allSelected: boolean; filterRules: FilterRule[]; excluded: Set<number>; selected: Set<number> }): DocumentSelectionQuery {
  if (l.allSelected) {
    return { all: true, filters: queryParamsFromFilterRules(l.filterRules), excluded_documents: Array.from(l.excluded) }
  }
  return { documents: Array.from(l.selected) }
}

/** _localizeList */
export function localizeList(items: { name: string }[]): string {
  if (items.length == 0) return ''
  if (items.length == 1) return t('bulk.list_one', { PH: items[0].name })
  if (items.length == 2) return t('bulk.list_two', { PH: items[0].name, PH_1: items[1].name })
  // Separator ", ": the i18n key is trimmed (",")
  const sep = t('ui.text').trimEnd() + ' '
  const list = items
    .slice(0, items.length - 1)
    .map((i) => t('bulk.list_one', { PH: i.name }))
    .join(sep)
  return t('bulk.list_last', { PH: list, PH_1: items[items.length - 1].name })
}

/** Text from setTags (escaped) */
export function tagsMessage(add: Named[], remove: Named[], count: number): string {
  let m: string
  if (add.length == 1 && remove.length == 0) m = t('bulk.tags_add_one', { PH: add[0].name, PH_1: count })
  else if (add.length > 1 && remove.length == 0) m = t('bulk.tags_add_many', { PH: localizeList(add), PH_1: count })
  else if (add.length == 0 && remove.length == 1) m = t('bulk.tags_remove_one', { PH: remove[0].name, PH_1: count })
  else if (add.length == 0 && remove.length > 1) m = t('bulk.tags_remove_many', { PH: localizeList(remove), PH_1: count })
  else
    m = t('bulk.tags_add_remove', {
      PH: localizeList(add),
      PH_1: localizeList(remove),
      PH_2: count,
    })
  return escapeHtml(m)
}

/** Text from setCustomFields (escaped) */
export function customFieldsMessage(add: Named[], remove: Named[], count: number): string {
  let m: string
  if (add.length == 1 && remove.length == 0) m = t('bulk.custom_fields_add_one', { PH: add[0].name, PH_1: count })
  else if (add.length > 1 && remove.length == 0) m = t('bulk.custom_fields_add_many', { PH: localizeList(add), PH_1: count })
  else if (add.length == 0 && remove.length == 1) m = t('bulk.custom_fields_remove_one', { PH: remove[0].name, PH_1: count })
  else if (add.length == 0 && remove.length > 1) m = t('bulk.custom_fields_remove_many', { PH: localizeList(remove), PH_1: count })
  else
    m = t('bulk.custom_fields_add_remove', {
      PH: localizeList(add),
      PH_1: localizeList(remove),
      PH_2: count,
    })
  return escapeHtml(m)
}

export type SingleKind = 'correspondent' | 'document_type' | 'storage_path'

/** Title + text from setCorrespondents / setDocumentTypes / setStoragePaths (text escaped) */
export function singleMessage(kind: SingleKind, item: Named | null, count: number): { title: string; message: string } {
  let title: string
  let m: string
  if (kind === 'correspondent') {
    title = t('bulk.confirm_correspondent')
    m = item
      ? t('bulk.correspondent_set', { PH: item.name, PH_1: count })
      : t('bulk.correspondent_remove', { PH: count })
  } else if (kind === 'document_type') {
    title = t('bulk.confirm_document_type')
    m = item
      ? t('bulk.document_type_set', { PH: item.name, PH_1: count })
      : t('bulk.document_type_remove', { PH: count })
  } else {
    title = t('bulk.confirm_storage_path')
    m = item
      ? t('bulk.storage_path_set', { PH: item.name, PH_1: count })
      : t('bulk.storage_path_remove', { PH: count })
  }
  return { title, message: escapeHtml(m) }
}

/** RotateConfirmDialogComponent.degrees */
export function rotationDegrees(rotation: number): number {
  let degrees = rotation % 360
  if (degrees < 0) degrees += 360
  return degrees
}
