// Replica of the CDK overlay that ng-select 24 uses for its selection field (FlexibleConnectedPositionStrategy):
//   body > .cdk-overlay-container > .cdk-overlay-popover.cdk-overlay-connected-position-bounding-box[popover=manual]
//        > #cdk-overlay-N.cdk-overlay-pane (width = field width, top/left in viewport coordinates) > <ng-dropdown-panel>
// The wrapper sits in the top layer via the popover API (above dialogs), as in CDK. Below, otherwise above if there is
// no room below (class ng-select-bottom/-top on the panel and on the field is set by the caller via onplacement).
let seq = 0

function container(): HTMLElement {
  let c = document.querySelector<HTMLElement>('body > .cdk-overlay-container')
  if (!c) {
    c = document.createElement('div')
    c.className = 'cdk-overlay-container'
    document.body.appendChild(c)
  }
  return c
}

export interface OverlayOptions {
  /** Reference element (the ng-select field) */
  anchor: HTMLElement
  onplacement?: (p: 'bottom' | 'top') => void
}

/** Svelte action on the panel: hangs it into an overlay wrapper and keeps the position up to date */
export function cdkOverlay(node: HTMLElement, opts: OverlayOptions) {
  const box = document.createElement('div')
  box.setAttribute('popover', 'manual')
  box.className = 'cdk-overlay-popover cdk-overlay-connected-position-bounding-box'
  box.setAttribute('dir', 'ltr')
  box.style.cssText = 'inset: 0px auto auto 0px; height: 100%; width: 100%;'
  const pane = document.createElement('div')
  pane.id = `cdk-overlay-${seq++}`
  pane.className = 'cdk-overlay-pane'
  box.appendChild(pane)
  container().appendChild(box)
  pane.appendChild(node)
  try {
    ;(box as any).showPopover?.()
  } catch {}

  let placement: 'bottom' | 'top' | null = null
  const update = () => {
    const r = opts.anchor.getBoundingClientRect()
    pane.style.width = `${r.width}px`
    const h = node.offsetHeight
    const below = r.bottom + h <= innerHeight || r.top - h < 0
    const p = below ? 'bottom' : 'top'
    if (p === 'bottom') {
      pane.style.top = `${r.bottom}px`
      pane.style.bottom = ''
    } else {
      pane.style.top = ''
      pane.style.bottom = `${innerHeight - r.top}px`
    }
    pane.style.left = `${r.left}px`
    if (p !== placement) {
      placement = p
      opts.onplacement?.(p)
    }
  }
  update()
  addEventListener('scroll', update, true)
  addEventListener('resize', update)
  return {
    update(o: OverlayOptions) {
      opts = o
      update()
    },
    destroy() {
      removeEventListener('scroll', update, true)
      removeEventListener('resize', update)
      box.remove()
    },
  }
}
