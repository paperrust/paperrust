// Security review 2026-11-18 (results/release/security/BERICHT.md, finding S-3): Svelte does not check
// link targets (Angular defused them with "unsafe:"). User data like URL custom fields
// (the backend allows any scheme like upstream, e.g. "javascript:") may only be linked as http(s)/mailto;
// everything else is not linked.

const ALLOWED = new Set(['http:', 'https:', 'mailto:'])

/** Link target for user data: the URL itself for http/https/mailto, otherwise `undefined` (no link). */
export function safeHref(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const v = value.trim()
  if (!v) return undefined
  try {
    return ALLOWED.has(new URL(v).protocol) ? v : undefined
  } catch {
    return undefined
  }
}
