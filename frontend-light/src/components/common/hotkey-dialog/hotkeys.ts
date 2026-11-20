// Port of services/hot-key.service.ts (shortcut matching, ignore rules) and the shortcut list that
// HotkeyDialogComponent shows. Upstream builds the list from live addShortcut() registrations; the light UI
// registers its shortcuts in fixed places (AppFrame: h/d/s/?; DocumentDetail: control.*/escape; DocumentList:
// escape/a/p/o/control.arrow*), so the list
// is derived from the current route and permissions instead. Keep in sync with those handlers.
import { t } from '../../../lib/i18n'

/** HotkeyDialogComponent SYMBOLS (HTML entities there, the same characters here) */
const SYMBOLS: Record<string, string> = {
  meta: '⌘', // ⌘
  control: '⌃', // ⌃
  shift: '⇧', // ⇧
  left: '←',
  right: '→',
  up: '↑',
  down: '↓',
  arrowleft: '←',
  arrowright: '→',
}

/** HotkeyDialogComponent.formatKey */
export function formatKey(key: string, macOS = false): string {
  if (macOS) key = key.replace('control', 'meta')
  return key
    .split('.')
    .map((k) => SYMBOLS[k] || k)
    .join(' ')
}

/**
 * HotKeyService handler: key events from inputs/textareas without a modifier key (or with an arrow key) are
 * ignored, so typing and cursor navigation keep working.
 */
export function ignoreHotkeyEvent(e: Pick<KeyboardEvent, 'altKey' | 'metaKey' | 'ctrlKey' | 'key' | 'target'>): boolean {
  const target = e.target as any
  const isInput = typeof HTMLElement !== 'undefined' && (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
  return (!(e.altKey || e.metaKey || e.ctrlKey) || e.key.toLowerCase().includes('arrow')) && isInput
}

export type AppHotkey = 'help' | 'dashboard' | 'documents' | 'settings'

/**
 * Global shortcuts registered by AppFrame: `shift.?` (HotKeyService constructor) and h/d/s (AppComponent.ngOnInit).
 * Returns the action for a keydown event, or null.
 */
export function matchAppHotkey(e: Pick<KeyboardEvent, 'altKey' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'key'>): AppHotkey | null {
  if (e.altKey || e.metaKey || e.ctrlKey) return null
  // '?' regardless of shift: some keyboard layouts produce '?' without it
  if (e.key === '?') return 'help'
  if (e.shiftKey) return null
  if (e.key === 'h') return 'dashboard'
  if (e.key === 'd') return 'documents'
  if (e.key === 's') return 'settings'
  return null
}

export type ListHotkey = 'reset' | 'selectAll' | 'selectPage' | 'open' | 'previousPage' | 'nextPage'

/**
 * Shortcuts of DocumentListComponent.ngOnInit: escape, a, p, o (no modifier), control.arrowleft/right (control or
 * meta, like DocumentDetail). Returns the action for a keydown event, or null.
 */
export function matchListHotkey(e: Pick<KeyboardEvent, 'altKey' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'key'>): ListHotkey | null {
  if (e.altKey || e.shiftKey) return null
  if (e.ctrlKey || e.metaKey) {
    if (e.key === 'ArrowLeft') return 'previousPage'
    if (e.key === 'ArrowRight') return 'nextPage'
    return null
  }
  if (e.key === 'Escape') return 'reset'
  if (e.key === 'a') return 'selectAll'
  if (e.key === 'p') return 'selectPage'
  if (e.key === 'o') return 'open'
  return null
}

/**
 * Shortcuts active on `path` (router path without base), in upstream registration order (AppComponent, then
 * DocumentListComponent / DocumentDetailComponent). `can` = permission check of the current user.
 */
export function activeHotkeys(path: string, can: { viewDocuments: boolean; changeSettings: boolean }): [string, string][] {
  const out: [string, string][] = [['h', t('common.dashboard')]]
  if (can.viewDocuments) out.push(['d', t('common.documents_2')])
  if (can.changeSettings) out.push(['s', t('common.settings')])
  if (path === 'documents' || /^view\/\d+$/.test(path)) {
    out.push(
      ['escape', t('app.hotkeys.reset_filters_selection')],
      ['a', t('common.select_all')],
      ['p', t('common.select_page')],
      ['o', t('app.hotkeys.open_first_document')],
      ['control.arrowleft', t('app.hotkeys.previous_page')],
      ['control.arrowright', t('app.hotkeys.next_page')],
    )
  }
  if (/^documents\/\d+(\/|$)/.test(path)) {
    out.push(
      ['control.arrowright', t('m.app.next_document')],
      ['control.arrowleft', t('m.app.previous_document')],
      ['escape', t('app.close_document')],
      ['control.s', t('app.hotkeys.save_document')],
      ['control.shift.s', t('app.hotkeys.save_close_next')],
    )
  }
  return out
}
