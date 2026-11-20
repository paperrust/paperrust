// Replica of the tour (ngx-ui-tour-core 17 + ngx-ui-tour-ng-bootstrap 19) with the steps from app.component.ts and the
// configuration from main.ts (provideUiTour: backdrop on, distance 10, Prev/Next/End, isOptional, useLegacyTitle).
// Loaded only on start (own chunk). Anchor = elements with attribute tourAnchor="…" (like the Angular
// directive, which also leaves the static attribute in the DOM). Popover markup like NgbPopoverWindow with
// <tour-step-template> from app.component.html; backdrop = 4 .ngx-ui-tour_backdrop areas like TourBackdropService.
import { createPopperLite, flip, preventOverflow, arrow, offset, type Instance, type Modifier } from '@popperjs/core'
import { t } from '../i18n'
import { resolvePlacements, bsPlacementClass } from '../popover.svelte'
import { router, stripBase } from '../../router.svelte'
import { completeTour } from './complete'

interface Step {
  anchorId: string
  content: string
  title?: string
  route: string
  placement?: string
  delayAfterNavigation?: number
  isOptional?: boolean
  /** backdropConfig.offset (default 10 from main.ts) */
  offset?: number
}

const steps = (): Step[] => [
  {
    anchorId: 'tour.dashboard',
    content: t("app.dashboard_used_show_saved"),
    route: '/dashboard',
    delayAfterNavigation: 500,
    isOptional: false,
  },
  {
    anchorId: 'tour.upload-widget',
    content: t('app.drag_drop_documents_here'),
    route: '/dashboard',
  },
  {
    anchorId: 'tour.documents',
    content: t('app.documents_list_shows_all'),
    route: '/documents?sort=created&reverse=1&page=1',
    delayAfterNavigation: 500,
    placement: 'bottom',
  },
  {
    anchorId: 'tour.documents-filter-editor',
    content: t('app.filtering_tools_allow_quickly'),
    route: '/documents?sort=created&reverse=1&page=1',
    placement: 'bottom',
  },
  {
    anchorId: 'tour.documents-views',
    content: t("app.any_combination_filters_saved"),
    route: '/documents?sort=created&reverse=1&page=1',
  },
  {
    anchorId: 'tour.tags',
    content: t('app.attributes_like_tags_correspondents'),
    route: '/attributes/tags',
    offset: 0,
  },
  { anchorId: 'tour.mail', content: t('common.manage_e_mail_accounts'), route: '/mail', offset: 0 },
  { anchorId: 'tour.workflows', content: t('app.workflows_give_more_control'), route: '/workflows', offset: 0 },
  { anchorId: 'tour.file-tasks', content: t('app.tasks_helps_track_background'), route: '/tasks', offset: 0 },
  { anchorId: 'tour.settings', content: t('app.check_out_settings_various'), route: '/settings', offset: 0 },
  {
    anchorId: 'tour.outro',
    title: t('app.thank'),
    content:
      t("app.em_tons_em_more") +
      '<br/><br/>' +
      t('app.lastly_behalf_every_contributor'),
    route: '/dashboard',
    isOptional: false,
    offset: 0,
  },
]

const anchorEl = (id: string) => document.querySelector<HTMLElement>(`[tourAnchor="${id}"]`)
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

// ---- ScrollingService.ensureVisible (center: true, like centerAnchorOnScroll) ----
function scrollableParent(node: Node | null): HTMLElement | null {
  if (!(node instanceof HTMLElement)) return null
  const s = getComputedStyle(node)
  if (node.scrollHeight > node.clientHeight && ['scroll', 'auto'].includes(s.overflowY)) return node
  return scrollableParent(node.parentNode)
}
function ensureVisible(el: HTMLElement, smooth: boolean): Promise<void> {
  const behavior: ScrollBehavior = smooth ? 'smooth' : 'auto'
  const container = scrollableParent(el) ?? document.documentElement
  if (container.getBoundingClientRect().height < el.getBoundingClientRect().height) el.scrollIntoView({ block: 'start', inline: 'start', behavior })
  else el.scrollIntoView({ block: 'center', inline: 'center', behavior })
  if (!smooth) return Promise.resolve()
  // waitForScrollFinish$: end 50 ms after the last scroll event, without scrolling after 75 ms
  const target: EventTarget = scrollableParent(el) ?? document
  return new Promise((resolve) => {
    let debounce: ReturnType<typeof setTimeout> | undefined
    const done = () => {
      clearTimeout(debounce)
      clearTimeout(idle)
      target.removeEventListener('scroll', onScroll)
      resolve()
    }
    let idle = setTimeout(done, 75)
    const onScroll = () => {
      clearTimeout(idle)
      clearTimeout(debounce)
      idle = setTimeout(done, 75)
      debounce = setTimeout(done, 50)
    }
    target.addEventListener('scroll', onScroll)
  })
}

// ---- TourBackdropService ----
class Backdrop {
  private els: HTMLElement[] | null = null
  private target: HTMLElement | null = null
  private step: Step | null = null
  private closed = false
  private ro: ResizeObserver | null = null
  private timer: ReturnType<typeof setTimeout> | undefined
  private onResize = () => {
    clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      if (!this.els || !this.target) return
      this.position()
      ensureVisible(this.target, false)
    }, 10)
  }

  show(target: HTMLElement, step: Step) {
    if (this.target) this.ro?.unobserve(this.target)
    this.target = target
    this.step = step
    this.ro ??= new ResizeObserver(this.onResize)
    this.ro.observe(target)
    if (!this.els) {
      this.els = Array.from({ length: 4 }, () => {
        const d = document.createElement('div')
        d.classList.add('ngx-ui-tour_backdrop')
        document.body.appendChild(d)
        return d
      })
      window.addEventListener('resize', this.onResize)
    }
    this.closed = false
    this.position()
  }

  closeSpotlight() {
    if (!this.els || !this.target) return
    const r = this.target.getBoundingClientRect()
    const x = r.left + r.width / 2
    const y = r.top + r.height / 2
    this.closed = true
    this.position(new DOMRect(x, y, 0, 0))
  }

  private position(rect: DOMRect | null = null) {
    const docEl = document.documentElement
    const container = scrollableParent(this.target) ?? docEl
    const el = rect ?? this.target!.getBoundingClientRect()
    const cr = container.getBoundingClientRect()
    // OverflowUtils.getVisibleSection
    const vs =
      cr.height < el.height
        ? new DOMRect(Math.max(el.left, cr.left), Math.max(el.top, cr.top), Math.min(el.right, cr.right) - Math.max(el.left, cr.left), Math.min(el.bottom, cr.bottom) - Math.max(el.top, cr.top))
        : el
    const sh = docEl.scrollHeight
    const sw = docEl.scrollWidth
    const sx = window.scrollX
    const sy = window.scrollY
    const off = this.closed ? 0 : (this.step?.offset ?? 10)
    const rects = [
      { width: vs.left + sx - off, height: sh, top: 0, left: 0 },
      { width: vs.width + off * 2, height: vs.top + sy - off, top: 0, left: vs.left + sx - off },
      { width: vs.width + off * 2, height: sh - (vs.bottom + sy) - off, top: vs.bottom + sy + off, left: vs.left + sx - off },
      { width: sw - (vs.right + sx) - off, height: sh, top: 0, left: vs.right + sx + off },
    ]
    rects.forEach((r, i) => {
      const s = this.els![i].style
      s.position = 'absolute'
      s.width = `${Math.max(r.width, 0)}px`
      s.height = `${Math.max(r.height, 0)}px`
      s.top = `${r.top}px`
      s.left = `${r.left}px`
      s.backgroundColor = 'rgba(0, 0, 0, 0.7)'
      s.zIndex = '101'
    })
  }

  close() {
    if (!this.els) return
    if (this.target) this.ro?.unobserve(this.target)
    this.els.forEach((e) => e.remove())
    this.els = null
    window.removeEventListener('resize', this.onResize)
  }

  disconnect() {
    this.ro?.disconnect()
    this.ro = null
  }
}

// ---- NgbPopover (container 'body', class tour-step) ----
let popoverSeq = 0
class StepPopover {
  win: HTMLElement | null = null
  private popper: Instance | null = null
  private anchor: HTMLElement | null = null
  private readonly id = `ngb-popover-tour-${popoverSeq++}`

  open(anchor: HTMLElement, step: Step, tour: Tour) {
    this.anchor = anchor
    const all = tour.steps
    const win = document.createElement('ngb-popover-window')
    win.setAttribute('role', 'tooltip')
    win.className = 'popover tour-step fade'
    win.id = this.id
    win.style.position = 'absolute'
    const el = (tag: string, cls = '', attrs: Record<string, string> = {}) => {
      const e = document.createElement(tag)
      if (cls) e.className = cls
      for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v)
      return e
    }
    const arrowEl = el('div', 'popover-arrow', { 'data-popper-arrow': '' })
    win.append(arrowEl)
    if (step.title) {
      const h = el('h3', 'popover-header')
      h.textContent = step.title
      win.append(h)
    }
    const body = el('div', 'popover-body')
    const p = el('p', 'tour-step-content')
    p.innerHTML = step.content
    const row = el('div', 'd-flex justify-content-between align-items-center')
    const badge = el('span', 'badge bg-light text-dark')
    badge.textContent = `${all.indexOf(step) + 1} / ${all.length}`
    const nav = el('div', 'tour-step-navigation btn-toolbar', { role: 'toolbar', 'aria-label': 'Controls' })
    const g1 = el('div', 'btn-group btn-group-sm me-2', { role: 'group', 'aria-label': 'Dismiss' })
    const btn = (cls: string, text: string, onclick: () => void) => {
      const b = el('button', cls)
      b.textContent = ` ${text} `
      b.addEventListener('click', onclick)
      return b
    }
    g1.append(btn('btn btn-outline-danger', t('app.end'), () => tour.end()))
    const g2 = el('div', 'btn-group btn-group-sm align-self-end', { role: 'group', 'aria-label': 'Previous / Next' })
    if (tour.hasPrev(step)) g2.append(btn('btn btn-outline-primary', `« ${t('app.prev')}`, () => tour.prev()))
    if (tour.hasNext(step)) g2.append(btn('btn btn-outline-primary', `${t('common.next')} »`, () => tour.next()))
    nav.append(g1, g2)
    row.append(badge, nav)
    body.append(p, el('hr'), row)
    win.append(body)
    anchor.setAttribute('aria-describedby', this.id)
    document.body.appendChild(win)
    this.win = win

    // ngbPositioning: Popper like getPopperOptions + addPopperOffset([0, 8]); tour distance → [0, offset + 10]
    const [main, ...fallback] = resolvePlacements(step.placement || 'auto')
    const off = step.offset ?? 10
    const bsModifier: Partial<Modifier<'bootstrapClasses', any>> = {
      name: 'bootstrapClasses',
      enabled: true,
      phase: 'write',
      fn({ state }) {
        const e = state.elements.popper
        e.className = (e.className.replace(/bs-popover(-[a-z]+)*/gi, '') + ' ' + bsPlacementClass('bs-popover', state.placement)).trim().replace(/\s+/g, ' ')
      },
    }
    this.popper = createPopperLite(anchor, win, {
      placement: main,
      modifiers: [
        bsModifier as any,
        flip,
        preventOverflow,
        arrow,
        { enabled: true, name: 'flip', options: { fallbackPlacements: fallback } },
        offset,
        { name: 'offset', options: { offset: off ? [0, off + 10] : () => [0, 8] } },
      ],
    })
    Promise.resolve().then(() => this.popper?.update())
    // ngbRunTransition: fade → show
    void win.offsetHeight
    win.classList.add('show')
  }

  close() {
    const win = this.win
    if (!win) return
    this.anchor?.removeAttribute('aria-describedby')
    this.win = null
    win.classList.remove('show')
    const popper = this.popper
    this.popper = null
    const ms = parseFloat(getComputedStyle(win).transitionDuration) * 1000 || 0
    setTimeout(() => {
      popper?.destroy()
      win.remove()
    }, ms)
  }
}

// ---- TourService ----
class Tour {
  readonly steps = steps()
  status: 'off' | 'on' = 'off'
  current: Step | undefined
  private forwards = true
  private waitingForScroll = false
  private inProgress = false
  private navigating = false
  private popover = new StepPopover()
  private anchor: HTMLElement | null = null
  private backdrop = new Backdrop()
  private scrollBlocked = false
  private stopRouterWatch: (() => void) | null = null

  private onKey = (e: KeyboardEvent) => {
    if (this.status !== 'on') return
    if (e.key === 'Escape') this.end()
    else if (e.key === 'ArrowRight' && this.current && this.hasNext(this.current)) this.next()
    else if (e.key === 'ArrowLeft' && this.current && this.hasPrev(this.current)) this.prev()
  }

  start() {
    if (this.status === 'on') return
    this.status = 'on'
    this.watchNavigation()
    window.addEventListener('keydown', this.onKey)
    void this.goToStep(this.steps[0])
    document.body.classList.add('tour-active')
  }

  end() {
    if (this.waitingForScroll || this.status === 'off') return
    this.status = 'off'
    this.disable()
    this.current = undefined
    this.forwards = true
    this.stopRouterWatch?.()
    this.stopRouterWatch = null
    window.removeEventListener('keydown', this.onKey)
    void completeTour()
    setTimeout(() => document.body.classList.remove('tour-active'), 500)
  }

  next() {
    if (this.waitingForScroll || this.inProgress) return
    void this.goForward()
  }
  prev() {
    if (this.waitingForScroll || this.inProgress) return
    void this.goBackward()
  }

  hasNext(step: Step): boolean {
    const i = this.steps.indexOf(step)
    if (i >= this.steps.length - 1) return false
    for (let j = i + 1; j < this.steps.length; j++) if (!this.optional(this.steps[j]) || anchorEl(this.steps[j].anchorId)) return true
    return false
  }
  hasPrev(step: Step): boolean {
    const i = this.steps.indexOf(step)
    if (i <= 0) return false
    for (let j = i - 1; j > -1; j--) if (!this.optional(this.steps[j]) || anchorEl(this.steps[j].anchorId)) return true
    return false
  }
  private optional = (s: Step) => s.isOptional ?? true

  private goForward(): Promise<void> {
    this.forwards = true
    if (this.current && this.hasNext(this.current)) return this.goToStep(this.steps[this.steps.indexOf(this.current) + 1])
    return Promise.resolve()
  }
  private goBackward(): Promise<void> {
    this.forwards = false
    if (this.current && this.hasPrev(this.current)) return this.goToStep(this.steps[this.steps.indexOf(this.current) - 1])
    return Promise.resolve()
  }

  private async goToStep(step: Step | undefined) {
    if (!step) return this.end()
    this.inProgress = true
    try {
      if (this.current) {
        this.backdrop.closeSpotlight()
        this.hideStep()
      }
      const [path, query = ''] = step.route.replace(/^\//, '').split('?')
      const want = new URLSearchParams(query)
      const have = new URLSearchParams(router.query)
      const active = stripBase('/' + path, '') === router.path && [...want].every(([k, v]) => have.get(k) === v)
      if (active) {
        await this.setCurrent(step, 0)
      } else {
        this.navigating = true
        router.navigate(step.route.replace(/^\//, ''))
        await sleep(0) // Effects (router observation) run beforehand
        this.navigating = false
        await this.setCurrent(step, step.delayAfterNavigation ?? 100, true)
      }
    } finally {
      this.inProgress = false
    }
  }

  private async setCurrent(step: Step, delay: number, navigated = false) {
    await sleep(delay)
    this.current = step
    await this.showStep(step, navigated)
  }

  private async showStep(step: Step, navigated: boolean) {
    let anchor = anchorEl(step.anchorId)
    // Light UI loads screens lazily: wait up to 2 s for the anchor after a route change (Angular has everything in the bundle)
    for (let i = 0; navigated && !anchor && i < 20; i++) {
      await sleep(100)
      anchor = anchorEl(step.anchorId)
    }
    if (this.status !== 'on') return
    if (!anchor) {
      if (this.optional(step)) return void (await (this.forwards ? this.goForward() : this.goBackward()))
      return this.end()
    }
    this.waitingForScroll = true
    await ensureVisible(anchor, true)
    this.waitingForScroll = false
    if (this.status !== 'on' || this.current !== step) return
    this.anchor = anchor
    anchor.classList.add('touranchor--is-active')
    this.popover.open(anchor, step, this)
    this.backdrop.show(anchor, step)
    if (!this.scrollBlocked) {
      document.documentElement.style.overflow = 'hidden'
      this.scrollBlocked = true
    }
  }

  private hideStep() {
    this.anchor?.classList.remove('touranchor--is-active')
    this.anchor = null
    this.popover.close()
  }

  private disable() {
    this.hideStep()
    this.backdrop.close()
    this.backdrop.disconnect()
    if (this.scrollBlocked) {
      document.documentElement.style.removeProperty('overflow')
      this.scrollBlocked = false
    }
  }

  /** subscribeToNavigationStartEvent: navigation by the user ends the tour */
  private watchNavigation() {
    let first = true
    this.stopRouterWatch = $effect.root(() => {
      $effect(() => {
        void router.path
        void router.query
        if (first) return void (first = false)
        if (!this.navigating && this.current) this.end()
      })
    })
  }
}

export const tour = new Tour()
