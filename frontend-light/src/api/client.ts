import { V1Client } from './v1'
import { BASE } from '../router.svelte'

/** Without session → existing login page with return target (docs/ARCHITECTURE-V2.md §Anmeldung). */
export function loginUrl(loc: Location = location): string {
  const next = loc.pathname.startsWith(BASE) ? loc.pathname + loc.search : `${BASE}/`
  return `/accounts/login/?next=${encodeURIComponent(next)}`
}

export const api = new V1Client({
  onUnauthorized: (status) => {
    if (status === 401) location.href = loginUrl()
  },
})
