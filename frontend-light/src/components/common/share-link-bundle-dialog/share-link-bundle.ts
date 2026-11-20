// Logic from data/share-link-bundle.ts (labels), share-link-bundle-dialog.component.ts (payload) and the
// getShareUrl methods of the share components (server root + /share/<slug>, in desktop LAN mode via shareUrl()).
import { t } from '../../../lib/i18n'
import type { Document, ShareLinkBundle } from '../../../api/v1'
import { shareUrl } from '../share-links-dialog/share-links'

export type FileVersion = 'archive' | 'original'

/** SHARE_LINK_BUNDLE_STATUS_LABELS */
export function statusLabel(status: string): string {
  const labels: Record<string, string> = { pending: t('dialog.share_bundle.status_pending'), processing: t('dialog.share_bundle.status_processing'), ready: t('dialog.share_bundle.status_ready'), failed: t('dialog.share_bundle.status_failed') }
  return labels[status] ?? status
}

/** SHARE_LINK_BUNDLE_FILE_VERSION_LABELS */
export function fileVersionLabel(version: string): string {
  const labels: Record<string, string> = { archive: t('manage.views.archive'), original: t('dialog.share_bundle.version_original') }
  return labels[version] ?? version
}

/** getShareUrl (share links and bundles) */
export function slugShareUrl(slug: string, origin = typeof location !== 'undefined' ? location.origin : ''): string {
  return shareUrl(`${origin}/share/${slug}`)
}

/** ShareLinkBundleDialogComponent.submit → ShareLinkBundleCreatePayload */
export function bundlePayload(documentIds: number[], shareArchiveVersion: boolean, expirationDays: number | null) {
  return {
    document_ids: documentIds,
    file_version: (shareArchiveVersion ? 'archive' : 'original') as FileVersion,
    expiration_days: expirationDays,
  }
}

/** isExpired */
export const isExpired = (expiration?: string | null, now = Date.now()) => !!expiration && Date.parse(expiration) <= now

/** replaceBundle (bundle list after "Create new") */
export function replaceBundle(bundles: ShareLinkBundle[], updated: ShareLinkBundle): ShareLinkBundle[] {
  const index = bundles.findIndex((b) => b.id === updated.id)
  if (index >= 0) return [...bundles.slice(0, index), updated, ...bundles.slice(index + 1)]
  return [updated, ...bundles]
}

/** Clipboard.copy: true on success */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

/** DocumentTitlePipe */
export const docTitle = (v: string | null | undefined) => v || t('common.no_title')

/**
 * PaperRust (pl-pdftools §4.1): per document the newest redacted version (root id → version id).
 * A package with the root id contains version 1 – so not the redacted version.
 */
export function redactedVersionMap(docs: Pick<Document, 'id' | 'versions'>[]): Map<number, number> {
  const out = new Map<number, number>()
  for (const d of docs) {
    // API: newest first → the first redacted entry is the newest
    const v = (d.versions ?? []).find((x) => x.redacted)
    if (v) out.set(d.id, v.id)
  }
  return out
}

/** Ids for the package: with `useRedacted` the redacted version instead of the main document. */
export function bundleIds(ids: number[], redacted: Map<number, number>, useRedacted: boolean): number[] {
  return ids.map((id) => (useRedacted ? (redacted.get(id) ?? id) : id))
}
