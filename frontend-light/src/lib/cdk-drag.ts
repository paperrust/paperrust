// Replica of @angular/cdk/drag-drop (22.x) for a vertical list: cdkDropList + cdkDrag with cdkDragHandle,
// cdkDragPreviewContainer="parent". Same behavior and same DOM as CDK: pointer (mouse/touch) instead of HTML5 dragging,
// start threshold 5 px, preview = deep copy (.cdk-drag-preview, popover="manual", position fixed, z-index 1000) in the
// parent element, placeholder = deep copy (.cdk-drag-placeholder) in place of the original (original invisible in <body>),
// reordering via transform (SingleAxisSortStrategy), .cdk-drop-list-dragging on the list, dropping with animation
// (.cdk-drag-animating, duration from the CSS transition), auto-scroll of the window at the edge (5 %, 2 px per frame).
// Styles for dragging: styles/parts/drag.css.
import '../styles/parts/drag.css'

type Rect = { top: number; right: number; bottom: number; left: number; width: number; height: number }
type Point = { x: number; y: number }
type ItemPosition = { el: HTMLElement; offset: number; initialTransform: string; clientRect: Rect }

export interface DropListOptions {
  /** Selector of the draggable elements (cdkDrag) within the list */
  item: string
  /** Selector of the handle (cdkDragHandle) within an element */
  handle: string
  /** cdkDropListDisabled */
  disabled?: () => boolean
  /** cdkDropListDropped (previousIndex, currentIndex) */
  dropped: (previousIndex: number, currentIndex: number) => void
}

const DROP_PROXIMITY = 0.05
const SCROLL_PROXIMITY = 0.05
const AUTO_SCROLL_STEP = 2
const START_THRESHOLD = 5
const DIRECTION_THRESHOLD = 5

const rectOf = (el: Element): Rect => {
  const r = el.getBoundingClientRect()
  return { top: r.top, right: r.right, bottom: r.bottom, left: r.left, width: r.width, height: r.height }
}
const adjustRect = (r: Rect, top: number, left: number) => {
  r.top += top
  r.bottom = r.top + r.height
  r.left += left
  r.right = r.left + r.width
}
const inside = (r: Rect, x: number, y: number) => y >= r.top && y <= r.bottom && x >= r.left && x <= r.right
const near = (r: Rect, t: number, x: number, y: number) => {
  const xt = r.width * t
  const yt = r.height * t
  return y > r.top - yt && y < r.bottom + yt && x > r.left - xt && x < r.right + xt
}
const translate = (x: number, y: number) => `translate3d(${Math.round(x)}px, ${Math.round(y)}px, 0)`
const combine = (transform: string, initial?: string) => (initial && initial != 'none' ? transform + ' ' + initial : transform)

function setStyles(el: HTMLElement, styles: Record<string, string>, important?: Set<string>) {
  for (const [k, v] of Object.entries(styles)) {
    if (v) el.style.setProperty(k, v, important?.has(k) ? 'important' : '')
    else el.style.removeProperty(k)
  }
}
/** toggleNativeDragInteractions */
export function nativeDrag(el: HTMLElement, enable: boolean) {
  const us = enable ? '' : 'none'
  setStyles(el, {
    'touch-action': enable ? '' : 'none',
    '-webkit-user-drag': enable ? '' : 'none',
    '-webkit-tap-highlight-color': enable ? '' : 'transparent',
    'user-select': us,
    '-webkit-user-select': us,
  })
}
const IMPORTANT = new Set(['position'])
/** toggleVisibility */
const visible = (el: HTMLElement, enable: boolean) =>
  setStyles(el, { position: enable ? '' : 'fixed', top: enable ? '' : '0', opacity: enable ? '' : '0', left: enable ? '' : '-999em' }, IMPORTANT)

/** deepCloneNode: without IDs, contents of canvas/input taken over */
function deepClone(node: HTMLElement): HTMLElement {
  const clone = node.cloneNode(true) as HTMLElement
  clone.removeAttribute('id')
  clone.querySelectorAll('[id]').forEach((e) => e.removeAttribute('id'))
  const src = node.querySelectorAll('canvas')
  const dst = clone.querySelectorAll('canvas')
  src.forEach((c, i) => {
    try {
      dst[i].getContext('2d')?.drawImage(c, 0, 0)
    } catch {}
  })
  const si = node.querySelectorAll<HTMLInputElement>('input, textarea, select')
  const di = clone.querySelectorAll<HTMLInputElement>('input, textarea, select')
  si.forEach((s, i) => {
    if (di[i].type !== 'file') di[i].value = s.value
  })
  return clone
}

/** moveItemInArray */
export function moveItemInArray<T>(array: T[], fromIndex: number, toIndex: number): void {
  const clamp = (v: number) => Math.max(0, Math.min(array.length - 1, v))
  const from = clamp(fromIndex)
  const to = clamp(toIndex)
  if (from === to) return
  const target = array[from]
  const delta = to < from ? -1 : 1
  for (let i = from; i !== to; i += delta) array[i] = array[i + delta]
  array[to] = target
}

function transitionMs(el: Element): number {
  const cs = getComputedStyle(el)
  const parse = (n: string) => cs.getPropertyValue(n).split(',').map((p) => p.trim())
  const props = parse('transition-property')
  const prop = props.find((p) => p === 'transform' || p === 'all')
  if (!prop) return 0
  const i = props.indexOf(prop)
  const ms = (v: string) => parseFloat(v) * (v.toLowerCase().includes('ms') ? 1 : 1000)
  return ms(parse('transition-duration')[i]) + ms(parse('transition-delay')[i])
}

const point = (e: MouseEvent | TouchEvent): Point => {
  const p = 'touches' in e ? e.targetTouches[0] || e.changedTouches[0] || { clientX: 0, clientY: 0 } : e
  return { x: p.clientX, y: p.clientY }
}

/** Svelte action for the list (cdkDropList, vertical orientation) */
export function cdkDropList(container: HTMLElement, opts: DropListOptions) {
  let o = opts
  let lastTouch = 0
  // State of a drag operation
  let root: HTMLElement | null = null
  let started = false
  let moved = false
  let pickupInElement: Point = { x: 0, y: 0 }
  let pickupOnPage: Point = { x: 0, y: 0 }
  let lastPointer: Point = { x: 0, y: 0 }
  let delta: Point = { x: 0, y: 0 }
  let lastDirChange: Point = { x: 0, y: 0 }
  let initialRect: Rect | null = null
  let initialTransform = ''
  let preview: HTMLElement | null = null
  let placeholder: HTMLElement | null = null
  let marker: Comment | null = null
  let domRect: Rect | null = null
  let positions: ItemPosition[] = []
  /** registered cdkDrag elements of the list (captured before the dragged element is detached) */
  let draggables: HTMLElement[] = []
  let initialIndex = -1
  let previousSwap: { el: HTMLElement | null; delta: number; overlaps: boolean } = { el: null, delta: 0, overlaps: false }
  let scrollPos = { top: 0, left: 0 }
  let scrollDir = 0
  let scrollRaf = 0
  let cleanupListeners: (() => void) | null = null

  const items = () => [...container.querySelectorAll<HTMLElement>(o.item)].filter((el) => !el.classList.contains('cdk-drag-placeholder'))
  /** getVisibleElement: for the dragged element the placeholder */
  const visibleEl = (el: HTMLElement) => (el === root && started && placeholder ? placeholder : el)

  // Handles like CDK (withHandles → toggleNativeDragInteractions(handle, disabled))
  const syncHandles = () => container.querySelectorAll<HTMLElement>(o.handle).forEach((h) => nativeDrag(h, false))
  const mo = new MutationObserver(syncHandles)
  mo.observe(container, { childList: true, subtree: true })
  syncHandles()

  function onDown(e: MouseEvent | TouchEvent) {
    const isTouch = e.type[0] === 't'
    if (!isTouch && (e as MouseEvent).button !== 0) return
    if (!isTouch && lastTouch && lastTouch + 800 > Date.now()) return
    if (root || o.disabled?.()) return
    const target = e.target as HTMLElement
    const handle = target.closest?.(o.handle) as HTMLElement | null
    const item = handle?.closest(o.item) as HTMLElement | null
    if (!handle || !item || !container.contains(item) || item.classList.contains('cdk-drag-disabled')) return
    if (target.draggable && e.type === 'mousedown') e.preventDefault()
    root = item
    draggables = items()
    started = moved = false
    initialRect = rectOf(item)
    const hr = handle.getBoundingClientRect()
    const p = point(e)
    pickupInElement = { x: hr.left - initialRect.left + (p.x - hr.left), y: hr.top - initialRect.top + (p.y - hr.top) }
    pickupOnPage = lastPointer = p
    delta = { x: 0, y: 0 }
    lastDirChange = { ...p }
    const move = (ev: MouseEvent | TouchEvent) => onMove(ev)
    const up = (ev: MouseEvent | TouchEvent) => onUp(ev)
    const sel = (ev: Event) => started && ev.preventDefault()
    const scroll = (ev: Event) => onScroll(ev)
    const act = { passive: false, capture: true }
    document.addEventListener('mousemove', move, act)
    document.addEventListener('touchmove', move, act)
    document.addEventListener('mouseup', up, true)
    document.addEventListener('touchend', up, true)
    document.addEventListener('touchcancel', up, true)
    document.addEventListener('selectstart', sel, act)
    document.addEventListener('scroll', scroll, true)
    cleanupListeners = () => {
      document.removeEventListener('mousemove', move, act)
      document.removeEventListener('touchmove', move, act)
      document.removeEventListener('mouseup', up, true)
      document.removeEventListener('touchend', up, true)
      document.removeEventListener('touchcancel', up, true)
      document.removeEventListener('selectstart', sel, act)
      document.removeEventListener('scroll', scroll, true)
    }
  }

  function onMove(e: MouseEvent | TouchEvent) {
    const p = point(e)
    if (!started) {
      if (Math.abs(p.x - pickupOnPage.x) + Math.abs(p.y - pickupOnPage.y) >= START_THRESHOLD) {
        if (e.cancelable) e.preventDefault()
        if (e.type[0] === 't') lastTouch = Date.now()
        start()
      }
      return
    }
    if (e.cancelable) e.preventDefault()
    moved = true
    lastPointer = p
    if (Math.abs(p.x - lastDirChange.x) > DIRECTION_THRESHOLD) {
      delta.x = p.x > lastDirChange.x ? 1 : -1
      lastDirChange.x = p.x
    }
    if (Math.abs(p.y - lastDirChange.y) > DIRECTION_THRESHOLD) {
      delta.y = p.y > lastDirChange.y ? 1 : -1
      lastDirChange.y = p.y
    }
    autoScroll(p.y)
    sortItem(p.x, p.y)
    preview!.style.transform = combine(translate(p.x - pickupInElement.x, p.y - pickupInElement.y), initialTransform)
  }

  /** DragRef._startDragSequence + DropListRef.start */
  function start() {
    const el = root!
    started = true
    const parent = el.parentNode as HTMLElement
    placeholder = deepClone(el)
    placeholder.style.pointerEvents = 'none'
    placeholder.classList.add('cdk-drag-placeholder')
    marker = document.createComment('')
    parent.insertBefore(marker, el)
    initialTransform = el.style.transform || ''
    // PreviewRef (cdkDragPreviewContainer="parent")
    const pv = (preview = deepClone(el))
    pv.style.width = `${initialRect!.width}px`
    pv.style.height = `${initialRect!.height}px`
    pv.style.transform = initialTransform || translate(initialRect!.left, initialRect!.top)
    const popover = 'showPopover' in pv
    setStyles(pv, { 'pointer-events': 'none', margin: popover ? '0 auto 0 0' : '0', position: 'fixed', top: '0', left: '0', 'z-index': '1000' }, IMPORTANT)
    nativeDrag(pv, false)
    pv.classList.add('cdk-drag-preview')
    pv.setAttribute('popover', 'manual')
    pv.setAttribute('dir', 'ltr')
    parent.appendChild(pv)
    if (popover) (pv as any).showPopover()
    visible(el, false)
    document.body.appendChild(parent.replaceChild(placeholder, el))
    el.classList.add('cdk-drag-dragging')
    // DropListRef._draggingStarted
    container.classList.add('cdk-drop-list-dragging')
    cachePositions()
    initialIndex = positions.findIndex((p) => p.el === el)
    domRect = rectOf(container)
    scrollPos = { top: window.scrollY, left: window.scrollX }
  }

  function cachePositions() {
    positions = draggables
      .map((el) => {
        const m = visibleEl(el)
        return { el, offset: 0, initialTransform: m.style.transform || '', clientRect: rectOf(m) }
      })
      .sort((a, b) => a.clientRect.top - b.clientRect.top)
  }

  /** DropListRef._sortItem + SingleAxisSortStrategy.sort (vertical) */
  function sortItem(x: number, y: number) {
    if (!domRect || !near(domRect, DROP_PROXIMITY, x, y)) return
    const el = root!
    const newIndex = positions.findIndex(({ el: e, clientRect }) => {
      if (e === el) return false
      if (e === previousSwap.el && previousSwap.overlaps && delta.y === previousSwap.delta) return false
      return y >= Math.floor(clientRect.top) && y < Math.floor(clientRect.bottom)
    })
    if (newIndex === -1) return
    const currentIndex = positions.findIndex((p) => p.el === el)
    const sibling = positions[newIndex]
    const cur = positions[currentIndex].clientRect
    const next = sibling.clientRect
    const d = currentIndex > newIndex ? 1 : -1
    let itemOffset = next.top - cur.top
    if (d === -1) itemOffset += next.height - cur.height
    const imm = positions[currentIndex + d * -1]
    let siblingOffset = cur.height * d
    if (imm) siblingOffset += d === -1 ? -(imm.clientRect.top - cur.bottom) : cur.top - imm.clientRect.bottom
    const old = positions.slice()
    moveItemInArray(positions, currentIndex, newIndex)
    positions.forEach((s, i) => {
      if (old[i] === s) return
      const dragged = s.el === el
      const off = dragged ? itemOffset : siblingOffset
      const target = dragged ? placeholder! : s.el
      s.offset += off
      target.style.transform = combine(`translate3d(0, ${Math.round(s.offset)}px, 0)`, s.initialTransform)
      adjustRect(s.clientRect, off, 0)
    })
    previousSwap = { el: sibling.el, delta: delta.y, overlaps: inside(next, x, y) }
  }

  /** DropListRef._startScrollingIfNecessary (window only) */
  function autoScroll(y: number) {
    const h = window.innerHeight
    const t = h * SCROLL_PROXIMITY
    const dir = y >= -t && y <= t ? -1 : y >= h - t && y <= h + t ? 1 : 0
    if (dir === scrollDir) return
    scrollDir = dir
    cancelAnimationFrame(scrollRaf)
    const step = () => {
      if (!scrollDir) return
      window.scrollBy(0, scrollDir * AUTO_SCROLL_STEP)
      scrollRaf = requestAnimationFrame(step)
    }
    if (dir) scrollRaf = requestAnimationFrame(step)
  }
  const stopScrolling = () => {
    scrollDir = 0
    cancelAnimationFrame(scrollRaf)
  }

  /** ParentPositionTracker.handleScroll (document) + SortStrategy.updateOnScroll */
  function onScroll(e: Event) {
    if (!started || (e.target !== document && e.target !== document.documentElement)) return
    const top = scrollPos.top - window.scrollY
    const left = scrollPos.left - window.scrollX
    scrollPos = { top: window.scrollY, left: window.scrollX }
    if (domRect) adjustRect(domRect, top, left)
    positions.forEach((p) => adjustRect(p.clientRect, top, left))
    pickupOnPage = { x: pickupOnPage.x + left, y: pickupOnPage.y + top }
    sortItem(lastPointer.x, lastPointer.y)
  }

  function onUp(e: MouseEvent | TouchEvent) {
    cleanupListeners?.()
    cleanupListeners = null
    if (!started) {
      root = null
      draggables = []
      return
    }
    stopScrolling()
    const finish = () => cleanup()
    if (!moved) return finish()
    const pr = placeholder!.getBoundingClientRect()
    const pv = preview!
    pv.classList.add('cdk-drag-animating')
    pv.style.transform = combine(translate(pr.left, pr.top), initialTransform)
    const ms = transitionMs(pv)
    if (!ms) return finish()
    let done = false
    const handler = (ev?: TransitionEvent) => {
      if (done || (ev && (ev.target !== pv || ev.propertyName !== 'transform'))) return
      done = true
      pv.removeEventListener('transitionend', handler)
      clearTimeout(timer)
      finish()
    }
    const timer = setTimeout(handler, ms * 1.5)
    pv.addEventListener('transitionend', handler)
    void e
  }

  /** DragRef._cleanupDragArtifacts + DropListRef.drop/_reset */
  function cleanup() {
    const el = root!
    visible(el, true)
    el.classList.remove('cdk-drag-dragging')
    marker!.parentNode!.replaceChild(el, marker!)
    preview?.remove()
    placeholder?.remove()
    preview = placeholder = null
    marker = null
    const currentIndex = positions.findIndex((p) => p.el === el)
    const previousIndex = initialIndex
    container.classList.remove('cdk-drop-list-dragging')
    for (const p of positions) p.el.style.transform = p.initialTransform || ''
    positions = []
    draggables = []
    previousSwap = { el: null, delta: 0, overlaps: false }
    root = null
    started = moved = false
    domRect = null
    o.dropped(previousIndex, currentIndex)
  }

  const down = (e: Event) => onDown(e as MouseEvent)
  const tdown = (e: Event) => {
    lastTouch = Date.now()
    onDown(e as TouchEvent)
  }
  container.addEventListener('mousedown', down)
  container.addEventListener('touchstart', tdown, { passive: true })
  return {
    update(next: DropListOptions) {
      o = next
    },
    destroy() {
      mo.disconnect()
      container.removeEventListener('mousedown', down)
      container.removeEventListener('touchstart', tdown)
      cleanupListeners?.()
      stopScrolling()
      preview?.remove()
      placeholder?.remove()
    },
  }
}
