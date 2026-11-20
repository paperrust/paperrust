// Tour of the PaperRust design (feedback 2026-11-18: the classic tour looks for anchors of the old UI
// and shows nothing in the new design). Own steps on the fixed ids of the shell; steps whose element is currently not
// visible (e.g. navigation in the phone menu, no saved views) are skipped.
import { router } from '../../router.svelte'
import { completeTour } from '../../lib/tour/complete'

export interface TourStep {
  /** CSS selector of the highlighted element; without = card in the center */
  anchor?: string
  /** Route that is opened beforehand */
  route?: string
  title: string
  text: string
}

/** Keys of the steps (texts: m.tour.<k>_title / _text) */
export const STEPS: { k: string; anchor?: string; route?: string }[] = [
  { k: 'welcome', route: 'dashboard' },
  { k: 'home', anchor: '#d2-home', route: 'dashboard' },
  { k: 'upload', anchor: '#d2-upload' },
  { k: 'inbox', anchor: '#d2-nav-inbox' },
  { k: 'filter', anchor: '#d2-filter-text', route: 'documents' },
  { k: 'list', anchor: '#d2-doclist', route: 'documents' },
  { k: 'views', anchor: '#d2-nav-views' },
  { k: 'search', anchor: '#d2-search' },
  { k: 'manage', anchor: '#d2-nav-manage' },
  { k: 'settings', anchor: '#d2-nav-settings' },
  { k: 'user', anchor: '#d2-user' },
  { k: 'outro', route: 'dashboard' },
]

class TourState {
  active = $state(false)
  index = $state(0)

  start() {
    this.index = 0
    this.active = true
    this.go(0)
  }

  /** to step i (switch route if necessary) */
  go(i: number) {
    const s = STEPS[i]
    if (!s) return this.end(true)
    this.index = i
    if (s.route && router.path !== s.route) router.navigate(s.route)
  }

  /** next visible step in direction dir (anchor missing → skip) */
  async step(dir: 1 | -1) {
    let i = this.index + dir
    while (i > 0 && i < STEPS.length) {
      const s = STEPS[i]
      if (s.route && router.path !== s.route) router.navigate(s.route)
      if (!s.anchor || (await visible(s.anchor))) break
      i += dir
    }
    if (i >= STEPS.length) return this.end(true)
    this.go(Math.max(0, i))
  }

  end(done: boolean) {
    this.active = false
    if (done) void completeTour()
  }
}

/** Element visible? (waits briefly after a route change) */
export async function visible(sel: string, tries = 8): Promise<boolean> {
  for (let n = 0; n < tries; n++) {
    const el = document.querySelector<HTMLElement>(sel)
    if (el && el.getClientRects().length && el.offsetParent !== null) return true
    await new Promise((r) => setTimeout(r, 80))
  }
  return false
}

export const tour = new TourState()
