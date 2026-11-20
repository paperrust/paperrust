// Focus rules for dropdown menus and side sheets (design2), without own DOM access → testable in Vitest.
//
// Background (bug "Esc closes the side sheet only on the second press"): when closing an {#if} block, Svelte 5
// first removes the DOM and calls the onMount teardown afterwards. If focus was in the menu (search field of the
// selection list, clicked entry), it is already on <body> at teardown; the old check "focus in the menu?"
// was then always false, focus did not return to the trigger. The next Esc went to the global
// key handling (e.g. clear selection) instead of the side sheet.

interface NodeLike {
  isConnected?: boolean
}
interface ContainerLike {
  contains(other: unknown): boolean
}

/** Focus missing: nothing, <body> or an already removed element */
export function focusLost(active: NodeLike | null | undefined, body: unknown): boolean {
  return !active || active === body || active.isConnected === false
}

/**
 * When closing a menu: target for the focus (the trigger `prev`) if focus was in the menu or got lost;
 * `null` = do not touch focus (user deliberately clicked elsewhere).
 */
export function menuFocusBack<T extends NodeLike>(active: NodeLike | null | undefined, body: unknown, menu: ContainerLike, prev: T | null | undefined): T | null {
  if (!prev || prev.isConnected === false) return null
  return focusLost(active, body) || menu.contains(active) ? prev : null
}

/** Open side sheets in opening order; only the topmost one reacts to Esc without focus. */
export class SheetStack<T> {
  private items: T[] = []
  push(x: T): () => void {
    this.items.push(x)
    return () => {
      const i = this.items.lastIndexOf(x)
      if (i >= 0) this.items.splice(i, 1)
    }
  }
  isTop(x: T): boolean {
    return this.items.length > 0 && this.items[this.items.length - 1] === x
  }
}

/** Esc should close the sheet although focus is not in the sheet (lost, on <body>) */
export function sheetCatchesEscape(key: string, active: NodeLike | null | undefined, body: unknown, isTop: boolean): boolean {
  return key === 'Escape' && isTop && focusLost(active, body)
}

/** shared stack of all design2 side sheets */
export const sheetStack = new SheetStack<object>()
