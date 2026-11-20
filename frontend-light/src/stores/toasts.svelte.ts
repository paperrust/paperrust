// Port of services/toast.service.ts (+ popup list from common/toasts/toasts.component.ts):
// notifications in the bell menu (persistent) and popups at the top center (at most 3).

export type ToastLevel = 'info' | 'success' | 'error'

export interface Toast {
  id?: string
  content: string
  delay: number
  action?: () => void
  actionName?: string
  /** Standard 'info' */
  level?: ToastLevel
  error?: any
  /** Stays in the notifications menu after the popup (default true) */
  persistent?: boolean
  /** Menu only, no popup (default false) */
  silent?: boolean
  timestamp?: Date
  read?: boolean
}

/** At most this many popups at the same time (MAX_VISIBLE_TOASTS) */
export const MAX_VISIBLE_TOASTS = 3

let seq = 0
const newId = () => (globalThis.crypto?.randomUUID?.() ?? `t${Date.now().toString(36)}${(seq++).toString(36)}`)

export class ToastState {
  /** Notifications (newest first) */
  toasts = $state<Toast[]>([])
  /** visible popups (ToastsComponent.toasts) */
  popups = $state<Toast[]>([])
  private suppress = false

  /** Bell menu open → no popups (ToastService.suppressPopupToasts) */
  set suppressPopupToasts(v: boolean) {
    this.suppress = v
    this.popups = []
  }

  show(toast: Toast) {
    if (!toast.id) toast.id = newId()
    toast.timestamp ??= new Date()
    if (typeof toast.error === 'string') {
      try {
        toast.error = JSON.parse(toast.error)
      } catch {}
    }
    const persistent = toast.persistent !== false
    if (persistent) {
      toast.read = !!this.suppress
      this.toasts = [toast, ...this.toasts]
    }
    if (!toast.silent && (!this.suppress || !persistent)) {
      this.popups = [toast, ...this.popups.filter((t) => t.id !== toast.id)].slice(0, MAX_VISIBLE_TOASTS)
    }
  }

  showError(content: string, error: any = null, delay = 10000) {
    this.show({ content, delay, level: 'error', error })
  }

  showInfo(content: string, delay = 5000) {
    this.show({ content, delay, persistent: false })
  }

  /** Confirmation of a user action */
  showSuccess(content: string, delay = 5000) {
    this.show({ content, delay, level: 'success', persistent: false })
  }

  markAllRead() {
    this.toasts = this.toasts.map((t) => (t.read ? t : { ...t, read: true }))
  }

  closeToast(toast: Toast) {
    this.toasts = this.toasts.filter((t) => t.id !== toast.id)
  }

  clearToasts() {
    this.toasts = []
    this.popups = []
  }

  /** ToastsComponent.removePopup */
  removePopup(toast: Toast) {
    this.popups = this.popups.filter((t) => t.id !== toast.id)
  }
}

export const toastService = new ToastState()
