// Command palette (Ctrl+K): fuzzy search over commands/targets. Pure logic (Vitest).

/** Hit score: 0 = no hit; word starts and contiguous hits count more. */
export function fuzzyScore(query: string, text: string): number {
  const q = query.trim().toLowerCase()
  if (!q) return 1
  const s = text.toLowerCase()
  const idx = s.indexOf(q)
  if (idx >= 0) return 100 - Math.min(idx, 50) + (idx === 0 || /\W/.test(s[idx - 1]) ? 50 : 0)
  let score = 0
  let i = 0
  let prev = -2
  for (const c of q) {
    if (c === ' ') continue
    const j = s.indexOf(c, i)
    if (j < 0) return 0
    score += j === prev + 1 ? 3 : 1
    if (j === 0 || /\W/.test(s[j - 1])) score += 2
    prev = j
    i = j + 1
  }
  return score
}

export interface Entry {
  id: string
  label: string
  /** further search terms */
  keywords?: string
  group: string
}

export function rank<T extends Entry>(entries: T[], query: string, limit = 50): T[] {
  return entries
    .map((e) => ({ e, s: Math.max(fuzzyScore(query, e.label), fuzzyScore(query, e.keywords ?? '') * 0.8) }))
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s)
    .slice(0, limit)
    .map((x) => x.e)
}

/** Rows for typed text (docs/DESIGN2-ENTWIRREN.md §5): full-text search always, "Ask question: …" only with AI on. */
export type TextAction = { kind: 'fulltext' | 'ask'; query: string }
export function textActions(query: string, aiEnabled: boolean): TextAction[] {
  const q = query.trim()
  if (!q) return []
  return aiEnabled ? [{ kind: 'fulltext', query: q }, { kind: 'ask', query: q }] : [{ kind: 'fulltext', query: q }]
}

/** Target of the answer view for a question (route `assistant`, the question is asked there immediately) */
export function askPath(query: string): string {
  return 'assistant?q=' + encodeURIComponent(query.trim())
}
