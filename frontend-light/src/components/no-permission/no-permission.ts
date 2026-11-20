// Logged in, but without permission for the UI (GET /api/ui_settings/ → 403, test log A-013):
// do not redirect to login (the login page immediately sends a valid session back → endless loop),
// but show an understandable message with a log-out button. Without Svelte, without style attributes (CSP).
import { t } from '../../lib/i18n'

/** Log out and then to login with return to the UI */
export function noPermissionLogoutUrl(base: string): string {
  return `/accounts/logout/?next=${encodeURIComponent(`/accounts/login/?next=${base.replace(/\/$/, '')}/`)}`
}

export function renderNoPermission(target: HTMLElement, base: string): void {
  const wrap = document.createElement('main')
  wrap.className = 'container py-5'
  wrap.setAttribute('role', 'alert')
  const card = document.createElement('div')
  card.className = 'card mx-auto col-12 col-md-8 col-lg-6'
  const body = document.createElement('div')
  body.className = 'card-body text-center p-4'
  const h = document.createElement('h1')
  h.className = 'h4 mb-3'
  h.textContent = t('app.no_permission_title')
  const p = document.createElement('p')
  p.className = 'text-muted mb-4'
  p.textContent = t('app.no_permission_text')
  const a = document.createElement('a')
  a.className = 'btn btn-primary'
  a.href = noPermissionLogoutUrl(base)
  a.textContent = t('common.logout')
  body.append(h, p, a)
  card.append(body)
  wrap.append(card)
  target.replaceChildren(wrap)
}
