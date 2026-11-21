// Sidebar collapsed to the icon rail by the user (desktop ≥ 1100 px; below that the rail applies anyway).
// Stored per browser; storage may be missing or throw (private mode, blocked site data) – then not collapsed.

export const NAV_COLLAPSED_KEY = 'paperrust.design2.navCollapsed'

type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

function defaultStore(): Store | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage : null
  } catch {
    return null
  }
}

export function readNavCollapsed(store: Store | null = defaultStore()): boolean {
  try {
    return store?.getItem(NAV_COLLAPSED_KEY) === '1'
  } catch {
    return false
  }
}

export function writeNavCollapsed(collapsed: boolean, store: Store | null = defaultStore()): void {
  try {
    if (collapsed) store?.setItem(NAV_COLLAPSED_KEY, '1')
    else store?.removeItem(NAV_COLLAPSED_KEY)
  } catch {}
}

/** Sidebar shown as icon rail: tablet width, or collapsed by the user; never on the phone (drawer) */
export function isRail(mobile: boolean, compact: boolean, collapsed: boolean): boolean {
  return !mobile && (compact || collapsed)
}
