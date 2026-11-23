// Port of services/websocket-status.service.ts + services/upload-documents.service.ts:
// upload via POST /api/documents/post_document/ (progress via XHR upload event like Angular
// HttpEventType.UploadProgress), processing status via the websocket /ws/status/.
import { api } from '../api/client'
import { msg, t } from '../lib/i18n'
import { session } from './settings.svelte'

// see ProgressStatusOptions in src/documents/plugins/helpers.py
export enum FileStatusPhase {
  STARTED = 0,
  UPLOADING = 1,
  WORKING = 2,
  SUCCESS = 3,
  FAILED = 4,
}

export const FILE_STATUS_MESSAGES: Record<string, string> = {
  document_already_exists: msg('app.document_already_exists'),
  document_already_exists_in_trash: msg('app.document_already_exists_note'),
  asn_already_exists: msg('app.document_asn_already_exists'),
  asn_already_exists_in_trash: msg('app.document_asn_already_exists_2'),
  file_not_found: msg('app.file_not_found'),
  pre_consume_script_not_found: msg('app.pre_consume_script_not'),
  pre_consume_script_error: msg('app.error_while_executing_pre'),
  post_consume_script_not_found: msg('app.post_consume_script_not'),
  post_consume_script_error: msg('app.error_while_executing_post'),
  new_file: msg('app.received_new_file'),
  unsupported_type: msg('app.file_type_not_supported'),
  parsing_document: msg('app.processing_document'),
  generating_thumbnail: msg('app.generating_thumbnail'),
  parse_date: msg('app.retrieving_date_document'),
  save_document: msg('app.saving_document'),
  finished: msg('app.finished'),
}

export class FileStatus {
  filename = $state('')
  taskId = $state<string | null>(null)
  phase = $state(FileStatusPhase.STARTED)
  currentPhaseProgress = $state(0)
  currentPhaseMaxProgress = $state(0)
  message = $state('')
  documentId = $state<number | null>(null)
  ownerId = $state<number | null>(null)

  getProgress(): number {
    switch (this.phase) {
      case FileStatusPhase.STARTED:
        return 0.0
      case FileStatusPhase.UPLOADING:
        return (this.currentPhaseProgress / this.currentPhaseMaxProgress) * 0.2
      case FileStatusPhase.WORKING:
        return (this.currentPhaseProgress / this.currentPhaseMaxProgress) * 0.8 + 0.2
      case FileStatusPhase.SUCCESS:
      case FileStatusPhase.FAILED:
        return 1.0
    }
  }

  updateProgress(status: FileStatusPhase, currentProgress?: number | null, maxProgress?: number | null) {
    if (status >= this.phase) {
      this.phase = status
      if (currentProgress != null) this.currentPhaseProgress = currentProgress
      if (maxProgress != null) this.currentPhaseMaxProgress = maxProgress
    }
  }
}

export interface WebsocketProgressMessage {
  filename?: string
  task_id?: string
  current_progress?: number
  max_progress?: number
  status?: string
  message?: string
  document_id?: number
  owner_id?: number
  users_can_view?: number[]
  groups_can_view?: number[]
}

type Listener = (s: FileStatus) => void

export class ConsumerStatusState {
  consumerStatus = $state<FileStatus[]>([])
  /** UploadStatusTrayComponent.collapsed (shared so that the navbar button can expand the list) */
  trayCollapsed = $state(false)
  /** PaperRust "AI text recognition" for the next uploads (field ai_ocr; switch in the upload area) */
  aiOcr = $state(false)
  /** PaperRust folder view: folder that is open while uploading (field `folder`; set by the design2 workspace) */
  folder = $state<{ id: number; name: string } | null>(null)
  private ws: WebSocket | null = null
  private listeners = { detected: new Set<Listener>(), finished: new Set<Listener>(), failed: new Set<Listener>(), started: new Set<() => void>(), updated: new Set<(m: any) => void>() }

  // ---- Events (instead of RxJS subjects); return = unsubscribe -------------------------------
  onDocumentDetected(fn: Listener) {
    this.listeners.detected.add(fn)
    return () => this.listeners.detected.delete(fn)
  }
  onDocumentConsumptionFinished(fn: Listener) {
    this.listeners.finished.add(fn)
    return () => this.listeners.finished.delete(fn)
  }
  onDocumentConsumptionFailed(fn: Listener) {
    this.listeners.failed.add(fn)
    return () => this.listeners.failed.delete(fn)
  }
  /** UploadDocumentsService.onUploadsStarted */
  onUploadsStarted(fn: () => void) {
    this.listeners.started.add(fn)
    return () => this.listeners.started.delete(fn)
  }
  onDocumentUpdated(fn: (m: any) => void) {
    this.listeners.updated.add(fn)
    return () => this.listeners.updated.delete(fn)
  }
  private emit(kind: 'detected' | 'finished' | 'failed', s: FileStatus) {
    for (const fn of this.listeners[kind]) fn(s)
  }

  private get(taskId: string | undefined, filename?: string) {
    let status = this.consumerStatus.find((e) => e.taskId == taskId) || this.consumerStatus.find((e) => e.filename == filename && e.taskId == null)
    let created = false
    if (!status) {
      status = new FileStatus()
      this.consumerStatus = [...this.consumerStatus, status]
      created = true
    }
    status.taskId = taskId ?? null
    status.filename = filename ?? ''
    return { status, created }
  }

  newFileUpload(filename: string): FileStatus {
    const status = new FileStatus()
    status.filename = filename
    this.consumerStatus = [...this.consumerStatus, status]
    return status
  }

  getConsumerStatus(phase?: FileStatusPhase) {
    return phase != null ? this.consumerStatus.filter((s) => s.phase == phase) : this.consumerStatus
  }

  getConsumerStatusNotCompleted() {
    return this.consumerStatus.filter((s) => s.phase < FileStatusPhase.SUCCESS)
  }

  getConsumerStatusCompleted() {
    return this.consumerStatus.filter((s) => s.phase == FileStatusPhase.FAILED || s.phase == FileStatusPhase.SUCCESS)
  }

  /** Desktop app mode 2b (Light UI via `prapi`): custom schemes cannot do WebSockets –
   *  the client core relays `/ws/status/` via 127.0.0.1 (address from `/local/ws`). */
  static inDesktopApp() {
    return typeof location !== 'undefined' && (location.hostname === 'prapi.localhost' || location.protocol === 'prapi:')
  }
  private retry: ReturnType<typeof setTimeout> | null = null
  private retryDelay = 5000
  private wanted = false

  connect() {
    this.disconnect()
    if (typeof WebSocket === 'undefined') return
    this.wanted = true
    if (ConsumerStatusState.inDesktopApp()) {
      // offline/server gone: silently retry (5 s … 60 s), no error message
      fetch('/local/ws', { headers: { Accept: 'application/json' } })
        .then((r) => (r.ok ? r.json() : null))
        .then((j: { url?: string | null } | null) => {
          if (!this.wanted || this.ws || !j?.url) return
          const ws = this.open(j.url)
          ws.onopen = () => (this.retryDelay = 5000)
          ws.onclose = () => {
            if (this.ws !== ws || !this.wanted) return
            this.ws = null
            this.scheduleRetry()
          }
        })
        .catch(() => this.scheduleRetry())
      return
    }
    const proto = location.protocol == 'https:' ? 'wss:' : 'ws:'
    this.open(`${proto}//${location.host}/ws/status/`)
  }

  private scheduleRetry() {
    if (!this.wanted || this.retry) return
    this.retry = setTimeout(() => {
      this.retry = null
      if (this.wanted && !this.ws) this.connect()
    }, this.retryDelay)
    this.retryDelay = Math.min(this.retryDelay * 2, 60000)
  }

  private open(url: string): WebSocket {
    const ws = new WebSocket(url)
    ws.onmessage = (ev: MessageEvent) => {
      let parsed: { type: string; data: any }
      try {
        parsed = JSON.parse(ev.data)
      } catch {
        return
      }
      switch (parsed.type) {
        case 'status_update':
          this.handleProgressUpdate(parsed.data)
          break
        case 'document_updated':
          if (this.canViewMessage(parsed.data)) for (const fn of this.listeners.updated) fn(parsed.data)
          break
        // documents_deleted / heartbeat: nothing to do
      }
    }
    this.ws = ws
    return ws
  }

  disconnect() {
    this.wanted = false
    if (this.retry) clearTimeout(this.retry)
    this.retry = null
    this.ws?.close()
    this.ws = null
  }

  isConnected() {
    return this.ws?.readyState === WebSocket.OPEN
  }

  /** see paperless.consumers.StatusConsumer._can_view */
  canViewMessage(m: { owner_id?: number; users_can_view?: number[]; groups_can_view?: number[] }): boolean {
    const user: any = session.data?.user ?? {}
    return !!(
      !m.owner_id ||
      user.is_superuser ||
      (m.owner_id && m.owner_id === user.id) ||
      m.users_can_view?.includes(user.id) ||
      m.groups_can_view?.some((g) => user.groups?.includes(g))
    )
  }

  handleProgressUpdate(m: WebsocketProgressMessage) {
    if (!this.canViewMessage(m)) return
    const { status, created } = this.get(m.task_id, m.filename)
    status.updateProgress(FileStatusPhase.WORKING, m.current_progress, m.max_progress)
    if (m.message && m.message in FILE_STATUS_MESSAGES) status.message = t(FILE_STATUS_MESSAGES[m.message])
    else if (m.message) status.message = m.message
    status.documentId = m.document_id ?? null
    if (m.status && m.status in FileStatusPhase && isNaN(+m.status)) status.phase = FileStatusPhase[m.status as keyof typeof FileStatusPhase]
    switch (status.phase) {
      case FileStatusPhase.STARTED:
        if (created) this.emit('detected', status)
        break
      case FileStatusPhase.SUCCESS:
        this.emit('finished', status)
        break
      case FileStatusPhase.FAILED:
        this.emit('failed', status)
        break
    }
  }

  fail(status: FileStatus, message: string) {
    status.message = message
    status.phase = FileStatusPhase.FAILED
    this.emit('failed', status)
  }

  dismiss(status: FileStatus) {
    const index =
      status.taskId != null ? this.consumerStatus.findIndex((s) => s.taskId == status.taskId) : this.consumerStatus.findIndex((s) => s.filename == status.filename)
    if (index > -1) this.consumerStatus = this.consumerStatus.filter((_, i) => i !== index)
    // everything hidden → next uploads expanded again
    if (!this.consumerStatus.length) this.trayCollapsed = false
  }

  dismissCompleted() {
    this.consumerStatus = this.consumerStatus.filter((s) => ![FileStatusPhase.SUCCESS, FileStatusPhase.FAILED].includes(s.phase))
    if (!this.consumerStatus.length) this.trayCollapsed = false
  }

  // ---- UploadDocumentsService --------------------------------------------------------------
  uploadFiles(files: Iterable<File>) {
    let n = 0
    for (const file of files) {
      this.uploadFile(file)
      n++
    }
    if (n) {
      this.trayCollapsed = false
      for (const fn of this.listeners.started) fn()
    }
  }

  uploadFile(file: File) {
    const formData = new FormData()
    formData.append('document', file, file.name)
    formData.append('from_webui', 'true')
    if (this.aiOcr) formData.append('ai_ocr', 'true')
    if (this.folder) formData.append('folder', String(this.folder.id))
    const status = this.newFileUpload(file.name)
    status.message = t('app.connecting')
    api.uploadDocument(formData, {
      progress: (loaded, total) => {
        status.updateProgress(FileStatusPhase.UPLOADING, loaded, total)
        status.message = t('common.uploading')
      },
      done: (body: any) => {
        status.taskId = body?.['task_id'] ?? String(body)
        status.message = t('app.upload_complete_waiting')
      },
      error: (httpStatus, statusText, body: any) => {
        // Angular shows error.error.document (array → "a,b" like toString)
        // PaperRust: a refused upload folder answers {"folder": [...]}
        if (httpStatus === 400) this.fail(status, body?.document != null ? String(body.document) : body?.folder != null ? String(body.folder) : '')
        else this.fail(status, t('app.http_error', { PH: httpStatus, PH_1: statusText }))
      },
    })
  }
}

export const consumerStatus = new ConsumerStatusState()
