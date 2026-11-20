// DocumentService.searchQuery (services/rest/document.service.ts): the current text/full-text search term. Upstream
// appends it to the preview URL as `#search="<term>"` (getPreviewUrl), so the browser's PDF viewer highlights it.
// Set by the document list from its text filter rules (upstream: FilterEditorComponent / GlobalSearch.runFullSearch).
import type { FilterRule } from '../lib/filter-rules'
import { FILTER_FULLTEXT_QUERY, FILTER_SIMPLE_TEXT, FILTER_SIMPLE_TITLE, FILTER_TITLE, FILTER_TITLE_CONTENT } from '../lib/filter-rule-type'

let query = ''

export function setSearchQuery(q: string | null | undefined) {
  query = (q ?? '').trim()
}
export function getSearchQuery(): string {
  return query
}

const TEXT_RULES = [FILTER_FULLTEXT_QUERY, FILTER_SIMPLE_TEXT, FILTER_SIMPLE_TITLE, FILTER_TITLE_CONTENT, FILTER_TITLE]

/** Search term of the text filter in `rules` ('' if there is none). */
export function searchQueryFromRules(rules: FilterRule[]): string {
  return rules.find((r) => TEXT_RULES.includes(r.rule_type) && r.value)?.value ?? ''
}

/** getPreviewUrl: `url.hash = '#search="<term>"'` when a term is set */
export function withSearchHash(url: string | undefined): string | undefined {
  if (!url || !query) return url
  try {
    const u = new URL(url)
    u.hash = `#search="${query}"`
    return u.toString()
  } catch {
    return url
  }
}
