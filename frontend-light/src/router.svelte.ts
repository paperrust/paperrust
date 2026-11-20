// Own history router (~1 KB), base /rs/app/, same paths as app-routing.module.ts.
/** Base of the app (/rs/app) */
export const APP_BASE = (import.meta.env?.BASE_URL ?? '/rs/app/').replace(/\/$/, '')
/** Second UI "PaperRust design" (docs/DESIGN2.md): the same routes under /rs/app/m/… */
export const MODERN_PREFIX = '/m'
export const IS_MODERN = typeof location !== 'undefined' && (location.pathname === APP_BASE + MODERN_PREFIX || location.pathname.startsWith(APP_BASE + MODERN_PREFIX + '/'))
/** Route base of the running UI (classic /rs/app, modern /rs/app/m) */
export const BASE = APP_BASE + (IS_MODERN ? MODERN_PREFIX : '')

/** Angular redirects (app-routing.module.ts) */
const REDIRECTS: Record<string, string> = {
  '': 'dashboard',
  documentproperties: 'attributes',
  tags: 'attributes/tags',
  correspondents: 'attributes/correspondents',
  documenttypes: 'attributes/documenttypes',
  storagepaths: 'attributes/storagepaths',
  customfields: 'attributes/customfields',
  'settings/mail': 'mail',
  'settings/usersgroups': 'usersgroups',
  'settings/savedviews': 'savedviews',
}

export function stripBase(pathname: string, base = BASE): string {
  let p = pathname.startsWith(base) ? pathname.slice(base.length) : pathname
  p = p.replace(/^\/+|\/+$/g, '')
  if (p.startsWith('documentproperties/')) return 'attributes/' + p.slice('documentproperties/'.length)
  return REDIRECTS[p] ?? p
}

class Router {
  path = $state(typeof location !== 'undefined' ? stripBase(location.pathname) : '')
  query = $state(typeof location !== 'undefined' ? location.search : '')

  constructor() {
    if (typeof window === 'undefined') return
    window.addEventListener('popstate', () => this.sync())
    this.sync(true)
  }

  private sync(replace = false) {
    const p = stripBase(location.pathname)
    if (replace && `${BASE}/${p}` !== location.pathname) history.replaceState(null, '', `${BASE}/${p}${location.search}`)
    this.path = p
    this.query = location.search
  }

  href(path: string) {
    return `${BASE}/${path.replace(/^\//, '')}`
  }

  navigate(path: string, replace = false) {
    const url = this.href(path)
    if (replace) history.replaceState(null, '', url)
    else history.pushState(null, '', url)
    this.sync()
  }

  /** routerLinkActive (prefix, like Angular default without exact) */
  isActive(path: string, exact = false) {
    return exact ? this.path === path : this.path === path || this.path.startsWith(path + '/')
  }
}

export const router = new Router()

/** Intercept clicks on internal links (instead of the routerLink directive). Alt+click counts as a normal click
 *  (the browser would otherwise download the page as a file); Ctrl/Cmd/Shift still open in a new tab. */
export function interceptLinks(root: HTMLElement) {
  root.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return
    const a = (e.target as Element).closest?.('a[href]') as HTMLAnchorElement | null
    if (!a || a.target === '_blank' || a.origin !== location.origin || !a.pathname.startsWith(BASE + '/')) return
    e.preventDefault()
    router.navigate(stripBase(a.pathname))
  })
}
