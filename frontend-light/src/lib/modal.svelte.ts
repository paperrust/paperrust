// Replica of NgbModal (ng-bootstrap 21): stack of open dialogs, rendered by ModalHost.svelte as
// <ngb-modal-backdrop class="modal-backdrop fade show"> + <ngb-modal-window class="modal fade show d-block">
// (styles: vendor/ngb-modal.css + Bootstrap). Body gets the class modal-open like Bootstrap/ngb.
import type { Component } from 'svelte'

export interface ModalRef<R = unknown> {
  id: number
  component: Component<any>
  props: Record<string, any>
  /** backdrop: 'static' → click outside does not close */
  staticBackdrop: boolean
  /** NgbModalOptions.size → class modal-<size> on .modal-dialog (sm, lg, xl) */
  size?: string
  close: (result?: R) => void
  dismiss: () => void
  result: Promise<R | undefined>
}

class ModalStack {
  version = $state(0)
  stack: ModalRef<any>[] = []
  private seq = 0

  open<R = unknown>(component: Component<any>, props: Record<string, any> = {}, opts: { backdrop?: 'static' | boolean; size?: string } = {}): ModalRef<R> {
    let resolve!: (r: R | undefined) => void
    const result = new Promise<R | undefined>((r) => (resolve = r))
    const ref: ModalRef<R> = {
      id: ++this.seq,
      component,
      props,
      staticBackdrop: opts.backdrop === 'static',
      size: opts.size,
      result,
      close: (r?: R) => this.remove(ref, () => resolve(r)),
      dismiss: () => this.remove(ref, () => resolve(undefined)),
    }
    // Make props reactive so that the dialog can change them (e.g. buttonsEnabled)
    ref.props = props
    this.stack.push(ref)
    this.sync()
    return ref
  }

  private remove(ref: ModalRef<any>, done: () => void) {
    const i = this.stack.indexOf(ref)
    if (i < 0) return
    this.stack.splice(i, 1)
    this.sync()
    done()
  }

  private sync() {
    this.version++
    if (typeof document !== 'undefined') document.body.classList.toggle('modal-open', this.stack.length > 0)
  }

  get open_(): ModalRef<any>[] {
    this.version
    return this.stack
  }
}

export const modals = new ModalStack()

export interface ConfirmOptions {
  title?: string
  messageBold?: string
  message?: string
  btnClass?: string
  btnCaption?: string
  cancelBtnClass?: string
  cancelBtnCaption?: string
  alternativeBtnClass?: string
  alternativeBtnCaption?: string
}
