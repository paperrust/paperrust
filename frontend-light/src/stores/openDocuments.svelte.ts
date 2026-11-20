// Port of services/open-documents.service.ts: open documents (max. 5) in sessionStorage under the
// same key as Angular, "unsaved" marker, closing with confirmation (confirmation dialog).
import type { Document } from '../api/v1'
import { api } from '../api/client'
import { t } from '../lib/i18n'
import { modals } from '../lib/modal.svelte'
import ConfirmDialog from '../components/common/confirm-dialog/ConfirmDialog.svelte'

export const OPEN_DOCUMENTS_KEY = 'open-documents-service:openDocuments'
const MAX_OPEN_DOCUMENTS = 5

export class OpenDocumentsState {
  version = $state(0)
  private openDocuments: Document[] = []
  private dirtyDocuments = new Set<number>()

  constructor(private storage: Storage | null = typeof sessionStorage !== 'undefined' ? sessionStorage : null) {
    this.load()
  }

  load() {
    const raw = this.storage?.getItem(OPEN_DOCUMENTS_KEY)
    if (!raw) return
    try {
      this.openDocuments = JSON.parse(raw)
    } catch {
      this.storage?.removeItem(OPEN_DOCUMENTS_KEY)
      this.openDocuments = []
    }
  }

  get documents(): Document[] {
    this.version
    return this.openDocuments
  }

  getOpenDocument(id: number): Document | undefined {
    this.version
    return this.openDocuments.find((d) => d.id == id)
  }

  /** Open; with 5 open, the oldest is closed (with confirmation). */
  async openDocument(doc: Document): Promise<boolean> {
    if (this.openDocuments.find((d) => d.id == doc.id)) return true
    if (this.openDocuments.length == MAX_OPEN_DOCUMENTS) {
      const closed = await this.closeDocument(this.openDocuments[MAX_OPEN_DOCUMENTS - 1])
      if (closed) this.finishOpen(doc)
      return closed
    }
    this.finishOpen(doc)
    return true
  }

  private finishOpen(doc: Document) {
    this.openDocuments.unshift(doc)
    this.dirtyDocuments.delete(doc.id)
    this.save()
  }

  setDirty(doc: Document, dirty: boolean, changedFields: object = {}) {
    const existing = this.openDocuments.find((d) => d.id == doc.id)
    if (!existing) return
    if (dirty) {
      this.dirtyDocuments.add(doc.id)
      existing.__changedFields = Object.keys(changedFields).filter((k) => k !== 'id')
    } else {
      this.dirtyDocuments.delete(doc.id)
      existing.__changedFields = []
    }
    this.save()
  }

  hasDirty() {
    this.version
    return this.dirtyDocuments.size > 0
  }

  isDirty(doc: { id: number } | undefined | null) {
    this.version
    return !!doc && this.dirtyDocuments.has(doc.id)
  }

  /** Take over server state, keep unsaved fields */
  refreshDocument(id: number) {
    const index = this.openDocuments.findIndex((d) => d.id == id)
    if (index < 0) return
    api.getDocument(id).then(
      (doc) => {
        const open = this.openDocuments.find((d) => d.id == id)
        if (!open) return
        const unsaved = Object.fromEntries((open.__changedFields ?? []).map((f) => [f, (open as any)[f]]))
        Object.assign(open, doc, unsaved)
        this.save()
      },
      () => {
        this.openDocuments.splice(index, 1)
        this.save()
      },
    )
  }

  async closeDocument(doc: Document): Promise<boolean> {
    const index = this.openDocuments.findIndex((d) => d.id == doc.id)
    if (index == -1) return true
    if (!this.dirtyDocuments.has(doc.id)) {
      this.openDocuments.splice(index, 1)
      this.save()
      return true
    }
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('common.unsaved_changes'),
        messageBold: t('app.unsaved_changes_document') + ' "' + doc.title + '".',
        message: t('app.sure_want_close_document'),
        btnClass: 'btn-warning',
        btnCaption: t('app.close_document'),
      },
      { backdrop: 'static' },
    )
    const r = await ref.result
    if (r !== 'confirm') return false
    const i = this.openDocuments.findIndex((d) => d.id == doc.id)
    if (i > -1) this.openDocuments.splice(i, 1)
    this.dirtyDocuments.delete(doc.id)
    this.save()
    return true
  }

  async closeAll(): Promise<boolean> {
    if (this.dirtyDocuments.size) {
      const ref = modals.open<string>(
        ConfirmDialog,
        {
          title: t('common.unsaved_changes'),
          messageBold: t('app.unsaved_changes'),
          message: t('app.sure_want_close_all'),
          btnClass: 'btn-warning',
          btnCaption: t('app.close_documents'),
        },
        { backdrop: 'static' },
      )
      if ((await ref.result) !== 'confirm') return false
    }
    this.openDocuments.splice(0, this.openDocuments.length)
    this.dirtyDocuments.clear()
    this.save()
    return true
  }

  save() {
    this.version++
    try {
      this.storage?.setItem(OPEN_DOCUMENTS_KEY, JSON.stringify(this.openDocuments))
    } catch (e) {
      console.error('Error saving open documents to session storage', e)
    }
  }
}

export const openDocuments = new OpenDocumentsState()
