// Logic of ShareLinksDialogComponent + SHARE_LINK_EXPIRATION_OPTIONS (data/share-link.ts) and the PaperRust
// additions from frontend/patches/09-desktop-share-links.patch (paperrust.ts paperrustShareMode/-ShareUrl).
import { t } from '../../../lib/i18n'
import type { DocumentVersionInfo } from '../../../api/v1'

export const expirationOptions = () => [
  { label: t('ui.1_day'), value: 1 },
  { label: t('ui.7_days'), value: 7 },
  { label: t('ui.30_days'), value: 30 },
  { label: t('ui.never'), value: null },
]

/** getDaysRemaining */
export function daysRemainingLabel(expiration: string, now = Date.now()): string {
  const days = Math.round((Date.parse(expiration) - now) / (1000 * 60 * 60 * 24))
  return days === 1 ? t('ui.1_day') : t('ui.days', { PH: days })
}

function meta(name: string, doc: Document = document): string {
  return doc.querySelector(`meta[name="${name}"]`)?.getAttribute('content') ?? ''
}

/** '' (server, like upstream), 'local' (desktop, only this computer) or 'lan' (share in the local network) */
export function shareMode(doc: Document = document): '' | 'local' | 'lan' {
  const mode = meta('paperrust_share_mode', doc)
  return mode === 'local' || mode === 'lan' ? mode : ''
}

/**
 * Share link for other devices: in 'lan' mode the beginning of the address under which the UI runs
 * (Angular: document.baseURI = server root; Light UI: origin + '/') is replaced by the LAN address.
 */
export function shareUrl(url: string, doc: Document = document, app = `${location.origin}/`): string {
  const base = meta('paperrust_share_url', doc)
  if (shareMode(doc) !== 'lan' || !/^https?:\/\/[^\s"'<>]+$/.test(base)) return url
  return url.startsWith(app) ? base.replace(/\/*$/, '/') + url.slice(app.length) : url
}

/**
 * PaperRust (docs/contract/pl-pdftools.md §4.1): versions for the share dialog, oldest first, with
 * number (version 1 = original). A share link delivers exactly the version it points to; the
 * link to the main document shows version 1 – so not the redacted version.
 */
export interface ShareVersion {
  id: number
  number: number
  label: string
  redacted: boolean
  isRoot: boolean
}

export function shareVersions(versions: DocumentVersionInfo[] | undefined, rootId: number): ShareVersion[] {
  const list = [...(versions ?? [])].reverse() // API: newest first
  if (!list.some((v) => v.id === rootId)) list.unshift({ id: rootId, is_root: true })
  return list.map((v, i) => ({
    id: v.id,
    number: i + 1,
    label: (v.version_label ?? '').trim(),
    redacted: !!v.redacted,
    isRoot: v.id === rootId,
  }))
}

/** Preselection: the newest redacted version, otherwise the main document (like paperless-ngx). */
export function defaultShareVersion(list: ShareVersion[], rootId: number): number {
  const redacted = list.filter((v) => v.redacted)
  return redacted.length ? redacted[redacted.length - 1].id : rootId
}

/** Note "what is visible where" – only if there is at least one redacted version. */
export function redactionNotice(list: ShareVersion[]): { redacted: number[]; readable: number[] } | null {
  if (!list.some((v) => v.redacted)) return null
  return {
    redacted: list.filter((v) => v.redacted).map((v) => v.number),
    readable: list.filter((v) => !v.redacted).map((v) => v.number),
  }
}

/** Display name of a version, e.g. "Version 3 – Redacted". */
export function versionName(v: ShareVersion): string {
  const base = t('ui.share_version_n', { PH: v.number })
  return v.label ? `${base} – ${v.label}` : base
}
