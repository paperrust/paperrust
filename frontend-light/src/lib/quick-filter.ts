// DocumentListViewService.getQuickFilterUrl: default state + current list state + filter rules → /documents?…
import type { FilterRule } from './filter-rules'
import { paramsFromViewState, paramsToURLSearchParams } from './query-params'
import { list } from '../stores/listView.svelte'

export function quickFilterPath(filterRules: FilterRule[]): string {
  const state: any = {
    title: null,
    documents: [],
    currentPage: list.currentPage ?? 1,
    collectionSize: null,
    sortField: list.sortField ?? 'created',
    sortReverse: list.sortReverse ?? true,
    filterRules,
  }
  const qs = paramsToURLSearchParams(paramsFromViewState(state)).toString()
  return `documents${qs ? '?' + qs : ''}`
}
