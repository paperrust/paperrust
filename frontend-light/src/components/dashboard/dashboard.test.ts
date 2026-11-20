// Logic of Dashboard, statistics widget, upload status and notifications (cases adapted from
// dashboard.component.spec.ts, statistics-widget.component.spec.ts, websocket-status.service.spec.ts,
// upload-status-tray.component.spec.ts and toast.service.spec.ts).
import { describe, expect, it } from 'vitest'
import { dashboardViews, displayFieldIds, truthy } from './dashboard'
import { fileTypeExtension, fileTypeName, groupFileTypes } from './widgets/statistics-widget/mime-names'
import { ConsumerStatusState, FileStatus, FileStatusPhase } from '../../stores/uploads.svelte'
import { ToastState, MAX_VISIBLE_TOASTS } from '../../stores/toasts.svelte'

describe('dashboardViews', () => {
  const views = [{ id: 1 }, { id: 2 }, { id: 3 }, { id: 4 }]
  it('zeigt nur sichtbare Ansichten', () => {
    expect(dashboardViews(views, [1, 3], []).map((v) => v.id)).toEqual([1, 3])
    expect(dashboardViews(views, undefined, undefined)).toEqual([])
  })
  it('sortiert nach Einstellung und hängt den Rest an', () => {
    expect(dashboardViews(views, [1, 2, 3], [3, 1]).map((v) => v.id)).toEqual([3, 1, 2])
    expect(dashboardViews(views, [1, 2], [9, 2]).map((v) => v.id)).toEqual([2, 1])
  })
})

describe('displayFieldIds', () => {
  it('filtert nach Rechten, Notizen und ergänzt Custom Fields', () => {
    const all = displayFieldIds(() => true, true, [7])
    expect(all).toContain('note')
    expect(all).toContain('custom_field_7')
    const none = displayFieldIds(() => false, false, [7])
    expect(none).toEqual(['title', 'created', 'added', 'shared', 'asn', 'pagecount'])
  })
  it('truthy liest gespeicherte Zeichenketten', () => {
    expect(truthy('false')).toBe(false)
    expect(truthy('true')).toBe(true)
    expect(truthy(undefined)).toBe(false)
  })
})

describe('Statistik', () => {
  it('fasst mehr als 5 Dateitypen zu "Other" zusammen', () => {
    const s = groupFileTypes(
      { documents_total: 21, document_file_type_counts: [1, 2, 3, 4, 5, 6].map((n) => ({ mime_type: `x/${n}`, mime_type_count: n })) },
      'Other',
    )
    expect(s.document_file_type_counts!.length).toBe(6)
    expect(s.document_file_type_counts![5]).toEqual({ mime_type: 'Other', is_other: true, mime_type_count: 6 })
  })
  it('Dateityp-Namen wie mime-names', () => {
    expect(fileTypeExtension('application/pdf')).toBe('PDF')
    expect(fileTypeName('text/plain')).toBe('Text File')
    expect(fileTypeExtension('foo/bar')).toBe('foo/bar')
  })
})

describe('Upload-/Verarbeitungsstatus', () => {
  it('berechnet den Fortschritt je Phase', () => {
    const s = new FileStatus()
    expect(s.getProgress()).toBe(0)
    s.updateProgress(FileStatusPhase.UPLOADING, 50, 100)
    expect(s.getProgress()).toBeCloseTo(0.1)
    s.updateProgress(FileStatusPhase.WORKING, 50, 100)
    expect(s.getProgress()).toBeCloseTo(0.6)
    // A step back to an earlier phase is ignored
    s.updateProgress(FileStatusPhase.UPLOADING, 1, 100)
    expect(s.phase).toBe(FileStatusPhase.WORKING)
    s.phase = FileStatusPhase.SUCCESS
    expect(s.getProgress()).toBe(1)
  })

  it('verarbeitet Websocket-Meldungen und meldet Erfolg/Fehler', () => {
    const svc = new ConsumerStatusState()
    const finished: string[] = []
    const failed: string[] = []
    svc.onDocumentConsumptionFinished((s) => finished.push(s.filename))
    svc.onDocumentConsumptionFailed((s) => failed.push(s.message))
    const up = svc.newFileUpload('a.pdf')
    expect(svc.getConsumerStatusNotCompleted().length).toBe(1)
    // A message without task_id is assigned to the upload with the same name
    svc.handleProgressUpdate({ filename: 'a.pdf', task_id: 't1', current_progress: 20, max_progress: 100, status: 'WORKING', message: 'parsing_document' })
    expect(svc.consumerStatus.length).toBe(1)
    expect(up.taskId).toBe('t1')
    expect(up.message).toBe('Processing document…')
    svc.handleProgressUpdate({ filename: 'a.pdf', task_id: 't1', current_progress: 100, max_progress: 100, status: 'SUCCESS', message: 'finished', document_id: 5 })
    expect(finished).toEqual(['a.pdf'])
    expect(up.documentId).toBe(5)
    svc.handleProgressUpdate({ filename: 'b.pdf', task_id: 't2', current_progress: 100, max_progress: 100, status: 'FAILED', message: 'document_already_exists' })
    expect(failed).toEqual(['This document is already there.'])
    expect(svc.getConsumerStatusCompleted().length).toBe(2)
    svc.dismiss(up)
    expect(svc.consumerStatus.map((s) => s.filename)).toEqual(['b.pdf'])
    svc.dismissCompleted()
    expect(svc.consumerStatus).toEqual([])
  })
})

describe('Benachrichtigungen', () => {
  it('zeigt höchstens 3 Popups, behält persistente im Menü', () => {
    const ts = new ToastState()
    for (let i = 0; i < 5; i++) ts.showError(`e${i}`)
    expect(ts.popups.length).toBe(MAX_VISIBLE_TOASTS)
    expect(ts.popups[0].content).toBe('e4')
    expect(ts.toasts.length).toBe(5)
    ts.showSuccess('ok')
    expect(ts.toasts.length).toBe(5) // not persistent
    expect(ts.toasts.every((t) => !t.read)).toBe(true)
    ts.markAllRead()
    expect(ts.toasts.every((t) => t.read)).toBe(true)
  })
  it('unterdrückt Popups bei offenem Menü, silent nur ins Menü', () => {
    const ts = new ToastState()
    ts.suppressPopupToasts = true
    ts.showError('x')
    expect(ts.popups.length).toBe(0)
    expect(ts.toasts[0].read).toBe(true)
    ts.suppressPopupToasts = false
    ts.show({ content: 's', delay: 1, silent: true })
    expect(ts.popups.length).toBe(0)
    expect(ts.toasts.length).toBe(2)
    ts.closeToast(ts.toasts[0])
    expect(ts.toasts.length).toBe(1)
    ts.clearToasts()
    expect(ts.toasts.length).toBe(0)
  })
})
