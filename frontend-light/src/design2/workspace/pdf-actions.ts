// PDF tools and further document actions in the PaperRust design (feedback 2026-11-18: were missing compared to the
// classic UI). Same dialogs and API calls as components/document-detail/DocumentDetail.svelte
// (editPdf, openRedact, openShrink, openConvertPdf, reprocess, printDocument, openEmailDocument, openShareLinks)
// and components/document-list/bulk-editor/BulkEditor.svelte (rotateSelected, mergeSelected, mergeSelectedAsVersions).
import { fullLocale, t } from '../../lib/i18n'
import { api } from '../../api/client'
import type { Document } from '../../api/v1'
import { modals } from '../../lib/modal.svelte'
import { toastService } from '../../stores/toasts.svelte'
import { isConvertible, isImage } from '../../components/common/convert-pdf/convert-pdf-api'
import { ensureClassicDialogs } from './classic-dialogs.svelte'
import { data } from '../../stores/data.svelte'

type SetEnabled = (b: boolean) => void

export const isPdf = (d: Pick<Document, 'mime_type'>) => d.mime_type === 'application/pdf'
export const canConvert = (d: Pick<Document, 'mime_type'>) => isConvertible(d.mime_type)

async function open(load: () => Promise<{ default: any }>, props: Record<string, any>, opts: { backdrop?: 'static' | boolean; size?: string } = { backdrop: 'static' }) {
  // Texts of the classic dialogs are not in the core dictionary → wait for the full language file (tools/i18n-split.mjs)
  const [C] = await Promise.all([load().then((m) => m.default), ensureClassicDialogs(), fullLocale()])
  return modals.open(C, props, opts)
}

/** Rotate, delete, reorder, split pages (PdfEditor) */
export async function openPdfEditor(d: Document, done: (deletedOriginal: boolean) => void) {
  const ref = await open(
    () => import('../../components/common/pdf-editor/PdfEditor.svelte'),
    {
      title: t('common.pdf_editor'),
      btnCaption: t('common.proceed'),
      documentID: d.id,
      versionID: d.id,
      onconfirm: async (setEnabled: SetEnabled, r: { operations: { page: number; rotate: number; doc: number }[]; deleteOriginal: boolean; updateDocument: boolean; includeMetadata: boolean }) => {
        setEnabled(false)
        try {
          await api.editPdfDocuments([d.id], {
            operations: r.operations,
            delete_original: r.deleteOriginal,
            update_document: r.updateDocument,
            include_metadata: r.includeMetadata,
            source_mode: 'explicit_selection',
          })
          toastService.showInfo(t('dialog.pdf_editor.started', { PH: d.title }))
          ref.close()
          done(r.deleteOriginal)
        } catch (e) {
          setEnabled(true)
          toastService.showError(t('dialog.pdf_editor.error'), e)
        }
      },
    },
    { backdrop: 'static', size: 'xl' },
  )
}

/** Redact (new version) */
export function openRedact(d: Document) {
  return open(() => import('../../components/document-detail/pdf-tools/RedactDialog.svelte'), { documentId: d.id, src: api.previewUrl(d.id, true) }, { backdrop: 'static', size: 'xl' })
}

/** Shrink (new version) */
export function openShrink(d: Document) {
  return open(() => import('../../components/document-detail/pdf-tools/ShrinkDialog.svelte'), { documentId: d.id })
}

/** Convert to PDF (image/office → new PDF version) */
export function openConvertPdf(docs: Pick<Document, 'id' | 'mime_type'>[], onqueued?: (ids: number[]) => void) {
  return open(() => import('../../components/common/convert-pdf/ConvertPdfDialog.svelte'), {
    documentIds: docs.map((d) => d.id),
    showOcr: docs.some((d) => isImage(d.mime_type)),
    onqueued,
  })
}

/** Reprocess (regenerate archive file and text) */
export async function openReprocess(d: Document) {
  const ref = await open(() => import('../../components/common/confirm-dialog/reprocess-confirm-dialog/ReprocessConfirmDialog.svelte'), {
    title: t('detail.reprocess_confirm'),
    messageBold: t('detail.operation_permanently_recreate_archive'),
    message: t('detail.archive_file_re_generated'),
    btnClass: 'btn-danger',
    btnCaption: t('common.proceed'),
    onconfirm: async (setEnabled: SetEnabled, remoteOcr: boolean, aiOcr: boolean) => {
      setEnabled(false)
      try {
        await api.reprocessDocuments([d.id], remoteOcr, aiOcr)
        toastService.showInfo(t('detail.reprocess_operation_begin_in', { PH: d.title }))
        ref.close()
      } catch (e) {
        setEnabled(true)
        toastService.showError(t('common.error_executing_operation'), e)
      }
    },
  })
}

/** Send by email */
export function openEmail(ids: number[], hasArchiveVersion: boolean) {
  return open(() => import('../../components/common/email-document-dialog/EmailDocumentDialog.svelte'), { documentIds: ids, hasArchiveVersion })
}

/** Share links of the document */
export function openShareLinks(d: Document) {
  return open(() => import('../../components/common/share-links-dialog/ShareLinksDialog.svelte'), { documentId: d.id, hasArchiveVersion: !!d.archived_file_name }, {})
}

/** Print via an invisible iframe (like printDocument of the classic UI) */
export async function printDocument(d: Document) {
  const r = await fetch(api.downloadUrlFor(d.id, false), { credentials: 'same-origin' })
  if (!r.ok) return toastService.showError(t('detail.error_loading_document_printing'))
  const url = URL.createObjectURL(await r.blob())
  const iframe = document.createElement('iframe')
  Object.assign(iframe.style, { position: 'fixed', right: '0', bottom: '0', width: '0', height: '0', border: '0', visibility: 'hidden' })
  iframe.src = url
  document.body.appendChild(iframe)
  const cleanup = () => {
    iframe.remove()
    URL.revokeObjectURL(url)
  }
  iframe.onload = () =>
    setTimeout(() => {
      try {
        iframe.contentWindow?.focus()
        iframe.contentWindow?.print()
        iframe.contentWindow?.addEventListener('afterprint', () => setTimeout(cleanup, 500), { once: true })
      } catch (e) {
        toastService.showError(t('detail.print_failed'), e)
        cleanup()
      }
    }, 100)
}

// ---- Multi-selection ---------------------------------------------------------------------------------------

async function runBulk(ref: { close: () => void }, setEnabled: SetEnabled, fn: () => Promise<unknown>, after: () => void, success?: string) {
  setEnabled(false)
  try {
    await fn()
    if (success) toastService.showSuccess(success)
    ref.close()
    after()
  } catch (e) {
    setEnabled(true)
    toastService.showError(t('common.error_executing_bulk_operation'), e)
  }
}

/** Rotate selected documents */
export async function openRotate(ids: number[], after: () => void) {
  const ref = await open(
    () => import('../../components/common/confirm-dialog/rotate-confirm-dialog/RotateConfirmDialog.svelte'),
    {
      title: t('bulk.rotate_title'),
      messageBold: t('bulk.rotate_confirm', { PH: ids.length }),
      btnClass: 'btn-danger',
      btnCaption: t('common.proceed'),
      documentID: ids[0],
      onconfirm: (setEnabled: SetEnabled, degrees: number) => runBulk(ref, setEnabled, () => api.rotateDocuments({ documents: ids }, degrees), after),
    },
    { backdrop: 'static', size: 'lg' },
  )
}

/** Merge selected documents into a new one */
export async function openMerge(ids: number[], after: (deletedOriginals: boolean) => void) {
  const ref = await open(() => import('../../components/common/confirm-dialog/merge-confirm-dialog/MergeConfirmDialog.svelte'), {
    title: t('bulk.merge_title'),
    messageBold: t('bulk.merge_confirm', { PH: ids.length }),
    btnCaption: t('common.proceed'),
    documentIDs: ids,
    onconfirm: (setEnabled: SetEnabled, r: { documentIDs: number[]; metadataDocumentID: number; archiveFallback: boolean; deleteOriginals: boolean }) => {
      const args: { metadata_document_id?: number; delete_originals?: boolean; archive_fallback?: boolean } = {}
      if (r.metadataDocumentID > -1) args.metadata_document_id = r.metadataDocumentID
      if (r.deleteOriginals) args.delete_originals = true
      if (r.archiveFallback) args.archive_fallback = true
      void runBulk(ref, setEnabled, () => api.mergeDocuments(r.documentIDs, args), () => after(!!args.delete_originals))
      toastService.showInfo(t('bulk.merge_queued'))
    },
  })
}

/** Merge selected documents as versions of one document */
export async function openMergeAsVersions(ids: number[], after: () => void) {
  const ref = await open(() => import('../../components/common/confirm-dialog/merge-as-versions-confirm-dialog/MergeAsVersionsConfirmDialog.svelte'), {
    title: t('list.merge_versions'),
    message: t('bulk.merge_versions_message'),
    btnCaption: t('common.proceed'),
    documentIDs: ids,
    rootDocumentID: ids[0],
    onconfirm: (setEnabled: SetEnabled, sel: number[], rootId: number) => runBulk(ref, setEnabled, () => api.mergeDocumentsAsVersions(sel, rootId), after, t('bulk.merged_as_versions')),
  })
}

/** Reprocess selected documents */
export async function openBulkReprocess(ids: number[], after: () => void) {
  const ref = await open(() => import('../../components/common/confirm-dialog/reprocess-confirm-dialog/ReprocessConfirmDialog.svelte'), {
    title: t('detail.reprocess_confirm'),
    messageBold: t('bulk.reprocess_confirm', { PH: ids.length }),
    message: t('bulk.reprocess_message'),
    btnClass: 'btn-danger',
    btnCaption: t('common.proceed'),
    onconfirm: (setEnabled: SetEnabled, remoteOcr: boolean, aiOcr: boolean) => runBulk(ref, setEnabled, () => api.reprocessSelection({ documents: ids }, remoteOcr, aiOcr), after),
  })
}

/** Set custom field of the selection (value in the dialog) */
export async function openBulkCustomField(ids: number[], fieldId: number, customFields: { id: number | null }[], after: () => void) {
  await open(
    () => import('../../components/document-list/bulk-editor/custom-fields-bulk-edit-dialog/CustomFieldsBulkEditDialog.svelte'),
    {
      customFields: customFields.filter((f) => f.id !== null),
      fieldsToAddIds: [fieldId],
      fieldsToRemoveIds: [],
      selection: { documents: ids },
      selectionCount: ids.length,
      onsucceeded: () => {
        toastService.showSuccess(t('bulk.custom_fields_saved'))
        after()
      },
      onfailed: (e: unknown) => toastService.showError(t('bulk.custom_fields_save_failed'), e),
    },
    { backdrop: 'static', size: 'lg' },
  )
}

/** Create share bundle for the selection */
export function openShareBundle(docs: Document[], onmanage: () => void) {
  return open(() => import('../../components/common/share-link-bundle-dialog/ShareLinkBundleDialog.svelte'), { documents: docs, onmanage }, { backdrop: 'static', size: 'lg' })
}

// ---- Create objects directly from a field (feedback 2026-11-18) -----------------------------------------
// Same dialogs as the classic document view (createTag/-DocumentType/-Correspondent/-StoragePath).

export type CreateKind = 'tag' | 'correspondent' | 'document_type' | 'storage_path'

const CREATE: Record<CreateKind, { load: () => Promise<{ default: any }>; list: string; store: 'tags' | 'correspondents' | 'documentTypes' | 'storagePaths' }> = {
  tag: { load: () => import('../../components/common/edit-dialog/tag-edit-dialog/TagEditDialog.svelte'), list: 'tags', store: 'tags' },
  correspondent: { load: () => import('../../components/common/edit-dialog/correspondent-edit-dialog/CorrespondentEditDialog.svelte'), list: 'correspondents', store: 'correspondents' },
  document_type: { load: () => import('../../components/common/edit-dialog/document-type-edit-dialog/DocumentTypeEditDialog.svelte'), list: 'document_types', store: 'documentTypes' },
  storage_path: { load: () => import('../../components/common/edit-dialog/storage-path-edit-dialog/StoragePathEditDialog.svelte'), list: 'storage_paths', store: 'storagePaths' },
}

/** Open create dialog (name prefilled); afterwards reload the list and report the new id */
export function openCreateObject(kind: CreateKind, name: string, created: (id: number) => void) {
  const c = CREATE[kind]
  return open(c.load, {
    dialogMode: 'create',
    object: name ? { name } : null,
    onsucceeded: async (o: { id: number }) => {
      try {
        const r = await api.listAll<any>(c.list)
        data[c.store] = new Map(r.results.map((x: any) => [x.id, x]))
      } catch {}
      created(o.id)
    },
  })
}
