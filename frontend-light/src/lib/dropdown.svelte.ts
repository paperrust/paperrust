// Replica of NgbDropdown (ng-bootstrap 21): same Popper setup (createPopperLite + flip/preventOverflow/
// arrow, offset [0, 2], placements bottom-start/-end, top-start/-end), same classes (`show` on host and
// menu, dropdown/dropup), same closing (autoClose: true = click outside or on .dropdown-item/
// .dropdown-divider, Escape; 'outside'/'inside'/false), focus on the toggle when opening.
import { createPopperLite, flip, preventOverflow, arrow, offset, type Instance, type Placement } from '@popperjs/core'

export type AutoClose = boolean | 'inside' | 'outside'
export interface DropdownOptions {
  autoClose?: AutoClose
  placement?: string | string[]
  /** pngxPopperOptions: preventOverflow padding 10 */
  pngxPopper?: boolean
  /** display="static" (e.g. in the navbar): no Popper, data-bs-popper="static" */
  static?: boolean
  onOpenChange?: (open: boolean) => void
}

const BS_TO_POPPER: Record<string, Placement> = {
  top: 'top', bottom: 'bottom', start: 'left', end: 'right', left: 'left', right: 'right',
  'top-start': 'top-start', 'top-end': 'top-end', 'bottom-start': 'bottom-start', 'bottom-end': 'bottom-end',
  'top-left': 'top-start', 'top-right': 'top-end', 'bottom-left': 'bottom-start', 'bottom-right': 'bottom-end',
}

let openDropdowns = new Set<Dropdown>()

export class Dropdown {
  open = $state(false)
  placementClass = $state<'dropdown' | 'dropup'>('dropdown')
  private host?: HTMLElement
  private anchor?: HTMLElement
  private menu?: HTMLElement
  private popper?: Instance
  private cleanup?: () => void

  constructor(private opts: DropdownOptions = {}) {}

  private get placements(): Placement[] {
    const p = this.opts.placement ?? ['bottom-start', 'bottom-end', 'top-start', 'top-end']
    const list = Array.isArray(p) ? p : p.split(/\s+/)
    return list.map((x) => BS_TO_POPPER[x] ?? (x as Placement))
  }

  // ---- Svelte actions -------------------------------------------------------------------
  hostEl = (node: HTMLElement) => {
    this.host = node
  }
  anchorEl = (node: HTMLElement) => {
    this.anchor = node
  }
  /** On the menu element (only rendered while open) */
  menuEl = (node: HTMLElement) => {
    this.menu = node
    if (this.opts.static) node.setAttribute('data-bs-popper', 'static')
    else if (this.anchor) {
      const [main, ...fallback] = this.placements
      const modifiers: any[] = [
        flip,
        preventOverflow,
        arrow,
        { enabled: true, name: 'flip', options: { fallbackPlacements: fallback } },
        offset,
        { name: 'offset', options: { offset: () => [0, 2] } },
      ]
      // like pngxPopperOptions: sets the options on the (shared) preventOverflow modifier – then applies globally
      if (this.opts.pngxPopper) (preventOverflow as any).options = { padding: 10 }
      this.popper = createPopperLite(this.anchor, node, { placement: main, modifiers })
      this.placementClass = main.startsWith('top') ? 'dropup' : 'dropdown'
    }
    return {
      destroy: () => {
        this.popper?.destroy()
        this.popper = undefined
        this.menu = undefined
      },
    }
  }

  update() {
    this.popper?.update()
  }

  // ---- Open/close --------------------------------------------------------------------
  toggle = (e?: Event) => {
    e?.stopPropagation?.()
    if (this.open) this.close()
    else this.openMenu()
  }

  openMenu() {
    if (this.open) return
    this.open = true
    openDropdowns.add(this)
    this.anchor?.focus()
    this.setCloseHandlers()
    this.opts.onOpenChange?.(true)
  }

  close() {
    if (!this.open) return
    this.open = false
    openDropdowns.delete(this)
    this.cleanup?.()
    this.cleanup = undefined
    this.opts.onOpenChange?.(false)
  }

  private setCloseHandlers() {
    const type = this.opts.autoClose ?? true
    if (!type) return
    const contained = (el: Element | null, container?: HTMLElement) => !!(el && container && (el === container || container.contains(el)))
    const matches = (el: Element | null) => !!el?.closest?.('.dropdown-item,.dropdown-divider')
    let shouldClose = false
    const down = (e: MouseEvent) => {
      const el = e.target as Element
      if (e.button === 2 || contained(el, this.anchor)) {
        shouldClose = false
        return
      }
      const inside = contained(el, this.menu)
      if (type === 'inside') shouldClose = inside && matches(el)
      else if (type === 'outside') shouldClose = !inside
      else shouldClose = (inside && matches(el)) || !inside
    }
    const up = () => {
      if (shouldClose) setTimeout(() => this.close(), 0)
    }
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        this.close()
        this.anchor?.focus()
      }
    }
    // register only after the triggering click
    const t = setTimeout(() => {
      document.addEventListener('mousedown', down)
      document.addEventListener('mouseup', up)
    }, 0)
    document.addEventListener('keydown', key)
    this.cleanup = () => {
      clearTimeout(t)
      document.removeEventListener('mousedown', down)
      document.removeEventListener('mouseup', up)
      document.removeEventListener('keydown', key)
    }
  }
}

/** For tests/page changes: close all open menus. */
export function closeAllDropdowns() {
  for (const d of [...openDropdowns]) d.close()
}
