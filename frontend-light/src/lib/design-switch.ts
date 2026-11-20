// Switch classic Light UI ↔ PaperRust design (docs/DESIGN2.md §6). Pure functions + one save helper,
// used by both UIs. Stored per user in ui_settings: settings.paperrust.design.
import { rawValue } from '../stores/settings.svelte'

export const DESIGN_KEY = 'paperrust:design'
export type Design = 'classic' | 'modern'

/** Default is the PaperRust design (owner's decision 2026-11-17); classic only if chosen. */
export function designOf(settings: Record<string, any>): Design {
  return rawValue(settings, DESIGN_KEY) === 'classic' ? 'classic' : 'modern'
}

/** Routes only in the modern UI → next classic page */
const MODERN_ONLY: Record<string, string> = { inbox: 'documents?is_in_inbox=true', assistant: 'dashboard' }

/**
 * Target URL when switching. Both UIs use the same routes (Angular paths), the modern one only under
 * `/m`; own routes of the modern UI are mapped to the next classic page.
 */
export function switchTarget(target: Design, path: string, query = '', appBase = '/rs/app'): string {
  const p = path.replace(/^\/+|\/+$/g, '')
  // never take `design=` along: the router remembers the query at start before takeDesignParam removes it;
  // a carried-along `?design=classic` sent the new UI straight back (click test 2026-11-20)
  query = stripDesignParam(query)
  if (target === 'modern') return `${appBase}/m/${p}${query}`
  const head = p.split('/')[0]
  if (MODERN_ONLY[head] !== undefined) return `${appBase}/${MODERN_ONLY[head]}`
  return `${appBase}/${p}${query}`
}

/** Remove `design=` from a query (`?a=1&design=classic`); empty query → ''. */
export function stripDesignParam(query: string): string {
  if (!query) return ''
  const q = new URLSearchParams(query.startsWith('?') ? query.slice(1) : query)
  q.delete('design')
  const s = q.toString()
  return s ? `?${s}` : ''
}

/** Classic start with the modern UI chosen: only redirect the entry (dashboard), deep links stay. */
export function autoRedirect(design: Design, path: string, appBase = '/rs/app'): string | null {
  return design === 'modern' && (path === '' || path === 'dashboard') ? `${appBase}/m/` : null
}

/** Read `?design=classic|modern` (desktop tray, links) and remove it from the address bar */
export function takeDesignParam(loc: Location = location, hist: History = history): Design | null {
  const q = new URLSearchParams(loc.search)
  const v = q.get('design')
  if (v !== 'classic' && v !== 'modern') return null
  q.delete('design')
  const qs = q.toString()
  hist.replaceState(hist.state, '', loc.pathname + (qs ? `?${qs}` : '') + loc.hash)
  return v
}
