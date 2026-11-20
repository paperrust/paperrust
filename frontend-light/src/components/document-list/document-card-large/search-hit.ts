// Full-text search hits (`__search_hit__`) as DocumentCardLargeComponent uses them (document-card-large.component.ts:
// searchScoreClass, searchNoteHighlights, hasSearchHighlights, shouldShowContentFallback, contentTrimmed).
// Upstream renders the highlights with [innerHtml] (Angular sanitizer). The server output is tantivy
// `Snippet.to_html()` (text already HTML-escaped, matches in <b>…</b>; whoosh used <span class="match">), so only
// those two highlight tags are kept and every other `<`/`>` is escaped: no other markup can reach the DOM.

export interface SearchHit {
  score?: number | null
  highlights?: string | null
  note_highlights?: string | null
  rank?: number
}

const TAG = /<b>|<\/b>|<span class="match">|<\/span>/g
const esc = (s: string) => s.replace(/</g, '&lt;').replace(/>/g, '&gt;')

/** Highlight HTML → safe HTML: only <b> and <span class="match"> (balanced), everything else escaped. */
export function sanitizeHighlight(html: string | null | undefined): string {
  if (!html) return ''
  let out = ''
  let last = 0
  const open: string[] = []
  for (const m of html.matchAll(TAG)) {
    out += esc(html.slice(last, m.index))
    last = m.index! + m[0].length
    const tag = m[0]
    if (!tag.startsWith('</')) {
      open.push(tag === '<b>' ? 'b' : 'span')
      out += tag
    } else if (open[open.length - 1] === (tag === '</b>' ? 'b' : 'span')) {
      // only close what is open (innermost); stray closing tags are dropped
      open.pop()
      out += tag
    }
  }
  out += esc(html.slice(last))
  while (open.length) out += `</${open.pop()}>`
  return out
}

type WithHit = { __search_hit__?: SearchHit | null; content?: string | null }

/** searchScoreClass: progress bar colour */
export function searchScoreClass(d: WithHit): string | undefined {
  const hit = d.__search_hit__
  if (!hit) return undefined
  const score = hit.score ?? 0
  return score > 0.7 ? 'success' : score > 0.3 ? 'warning' : 'danger'
}

/**
 * searchNoteHighlights: note highlights split at ',', only notes with a match. Upstream filters on '<span' (whoosh
 * markup) — with tantivy's <b> that would drop every note, so <b> counts as a match here too.
 */
export function searchNoteHighlights(d: WithHit): string[] {
  const n = d.__search_hit__?.note_highlights
  if (!n) return []
  return n.split(',').filter((h) => h.includes('<span') || h.includes('<b>'))
}

/** hasSearchHighlights */
export function hasSearchHighlights(d: WithHit): boolean {
  return Boolean(d.__search_hit__?.highlights?.trim()?.length)
}

/** shouldShowContentFallback */
export function shouldShowContentFallback(d: WithHit): boolean {
  return d.__search_hit__?.score == null || (!hasSearchHighlights(d) && searchNoteHighlights(d).length === 0)
}
