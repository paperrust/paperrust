// Replica of NgbPopover (ng-bootstrap 21): window <ngb-popover-window class="popover bs-popover-…"> directly after the
// trigger (like ViewContainerRef), Popper like ngbPositioning (placement 'auto' → order top, bottom, start, end, …;
// flip/preventOverflow/arrow, offset [0, 8], classes bs-popover-<side>), trigger 'click' or 'mouseenter:mouseleave',
// autoClose (true/'inside'/'outside'/false) and Escape.
import { createPopperLite, flip, preventOverflow, arrow, offset, type Instance, type Placement, type Modifier } from '@popperjs/core'

const ALLOWED = ['top', 'bottom', 'start', 'end', 'top-start', 'top-end', 'bottom-start', 'bottom-end', 'start-top', 'start-bottom', 'end-top', 'end-bottom']
const TO_POPPER: Record<string, Placement> = {
  top: 'top', bottom: 'bottom', start: 'left', end: 'right', left: 'left', right: 'right',
  'top-start': 'top-start', 'top-end': 'top-end', 'bottom-start': 'bottom-start', 'bottom-end': 'bottom-end',
  'start-top': 'left-start', 'start-bottom': 'left-end', 'end-top': 'right-start', 'end-bottom': 'right-end',
  'left-top': 'left-start', 'left-bottom': 'left-end', 'right-top': 'right-start', 'right-bottom': 'right-end',
}

/** getPopperOptions: placement list with resolved 'auto' */
export function resolvePlacements(placement: string | string[]): Placement[] {
  const vals = Array.isArray(placement) ? [...placement] : placement.split(/\s+/)
  let auto = vals.findIndex((v) => v === 'auto')
  if (auto >= 0) {
    for (const p of ALLOWED) if (vals.find((v) => v.search('^' + p) !== -1) == null) vals.splice(auto++, 1, p)
  }
  return vals.map((v) => TO_POPPER[v] ?? (v as Placement))
}

/** getBootstrapBaseClassPlacement('bs-popover', 'left-start') → 'bs-popover-start bs-popover-start-top' */
export function bsPlacementClass(base: string, placement: string): string {
  const [primary, secondary] = placement.split('-')
  const p = primary.replace(/^left/, 'start').replace(/^right/, 'end')
  const out = [p]
  if (secondary) {
    let s = secondary
    if (primary === 'left' || primary === 'right') s = s.replace(/^start/, 'top').replace(/^end/, 'bottom')
    out.push(`${p}-${s}`)
  }
  return out.map((c) => `${base}-${c}`).join(' ')
}

let seq = 0

export interface PopoverOptions {
  placement?: string | string[]
  autoClose?: boolean | 'inside' | 'outside'
  triggers?: string
}

export class Popover {
  open = $state(false)
  readonly id = `ngb-popover-${seq++}`
  private trigger?: HTMLElement
  private win?: HTMLElement
  private popper?: Instance
  private cleanup?: () => void

  constructor(private opts: PopoverOptions = {}) {}

  /** Action on the trigger (click or mouse events like triggers) */
  triggerEl = (node: HTMLElement) => {
    this.trigger = node
    const triggers = (this.opts.triggers ?? 'click').trim().split(/\s+/)
    const offs: (() => void)[] = []
    for (const tr of triggers) {
      const [openEv, closeEv] = tr.split(':')
      if (!closeEv || openEv === closeEv) {
        const h = () => this.toggle()
        node.addEventListener(openEv, h)
        offs.push(() => node.removeEventListener(openEv, h))
      } else {
        const o = () => this.show()
        const c = () => this.hide()
        node.addEventListener(openEv, o)
        node.addEventListener(closeEv, c)
        offs.push(() => (node.removeEventListener(openEv, o), node.removeEventListener(closeEv, c)))
      }
    }
    return { destroy: () => offs.forEach((f) => f()) }
  }

  /** Action on the window <ngb-popover-window> */
  windowEl = (node: HTMLElement) => {
    this.win = node
    // via CSSOM (CSP without 'unsafe-inline' forbids style attributes in the markup, A-331)
    node.style.position = 'absolute'
    this.trigger?.setAttribute('aria-describedby', this.id)
    const [main, ...fallback] = resolvePlacements(this.opts.placement ?? 'auto')
    const bsModifier: Partial<Modifier<'bootstrapClasses', any>> = {
      name: 'bootstrapClasses',
      enabled: true,
      phase: 'write',
      fn({ state }) {
        const el = state.elements.popper
        el.className = (el.className.replace(/bs-popover(-[a-z]+)*/gi, '') + ' ' + bsPlacementClass('bs-popover', state.placement)).trim().replace(/\s+/g, ' ')
      },
    }
    if (this.trigger) {
      this.popper = createPopperLite(this.trigger, node, {
        placement: main,
        modifiers: [
          bsModifier as any,
          flip,
          preventOverflow,
          arrow,
          { enabled: true, name: 'flip', options: { fallbackPlacements: fallback } },
          offset,
          { name: 'offset', options: { offset: () => [0, 8] } },
        ],
      })
      Promise.resolve().then(() => this.popper?.update())
    }
    this.setCloseHandlers()
    return {
      destroy: () => {
        this.popper?.destroy()
        this.popper = undefined
        this.win = undefined
        this.trigger?.removeAttribute('aria-describedby')
        this.cleanup?.()
        this.cleanup = undefined
      },
    }
  }

  show() {
    this.open = true
  }
  hide() {
    this.open = false
  }
  toggle() {
    this.open = !this.open
  }

  private setCloseHandlers() {
    const type = this.opts.autoClose ?? true
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') this.hide()
    }
    document.addEventListener('keydown', key)
    let down: ((e: MouseEvent) => void) | undefined
    let up: ((e: MouseEvent) => void) | undefined
    let shouldClose = false
    let closeTimer: ReturnType<typeof setTimeout> | undefined
    const t = setTimeout(() => {
      if (!type) return
      down = (e) => {
        const el = e.target as Node
        if (this.trigger?.contains(el)) return void (shouldClose = false)
        const inside = !!this.win?.contains(el)
        shouldClose = type === 'inside' ? inside : type === 'outside' ? !inside : true
      }
      // ngbAutoClose: mouseup → delay(0) → close. Without the delay the window disappears between mouseup
      // and click, a button in the popover (e.g. "Yes" in ConfirmButton) would never get the click (test log A-017/A-087).
      up = () => {
        if (shouldClose) {
          if (closeTimer) clearTimeout(closeTimer)
          closeTimer = setTimeout(() => {
            closeTimer = undefined
            this.hide()
          }, 0)
        }
        shouldClose = false
      }
      document.addEventListener('mousedown', down)
      document.addEventListener('mouseup', up)
    }, 0)
    this.cleanup = () => {
      clearTimeout(t)
      if (closeTimer) clearTimeout(closeTimer)
      document.removeEventListener('keydown', key)
      if (down) document.removeEventListener('mousedown', down)
      if (up) document.removeEventListener('mouseup', up)
    }
  }
}
