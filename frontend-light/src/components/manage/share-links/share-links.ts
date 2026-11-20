// Logic from manage/share-links (share-links.component.ts: start tab; share-link-list/-bundle-list: sorting via
// SortableDirective + onSort, page after deleting).

export type ShareLinksTab = 'links' | 'bundles'

/** ShareLinksNavIDs: values of the query parameter `type` */
export const TAB_LINKS: ShareLinksTab = 'links'
export const TAB_BUNDLES: ShareLinksTab = 'bundles'

/** getInitialNavID */
export function initialTab(type: string | null, canViewLinks: boolean, canViewBundles: boolean): ShareLinksTab {
  if (type === TAB_BUNDLES && canViewBundles) return TAB_BUNDLES
  if (canViewLinks) return TAB_LINKS
  return TAB_BUNDLES
}

/** SortableDirective.rotate + onSort (without column: default "created" descending) */
export function nextSort(currentField: string, currentReverse: boolean, column: string): { sortField: string; sortReverse: boolean } {
  let event: { column: string | null; reverse: boolean }
  if (currentField != column) event = { column, reverse: false }
  else if (!currentReverse) event = { column: currentField, reverse: true }
  else event = { column: null, reverse: false }
  return { sortField: event.column || 'created', sortReverse: event.column ? event.reverse : true }
}

/** HostBinding class.asc / class.des */
export const sortClass = (currentField: string, currentReverse: boolean, column: string) =>
  currentField === column ? (currentReverse ? 'des' : 'asc') : ''

/** delete: last row of a following page deleted → one page back */
export const pageAfterDelete = (rowsOnPage: number, page: number) => (rowsOnPage === 1 && page > 1 ? page - 1 : page)
