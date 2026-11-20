import { msg } from '../../lib/i18n'
// Keyboard mapping of the PaperRust design (docs/DESIGN2.md §5). Pure logic: event → command.
export type Command =
  | 'palette'
  | 'help'
  | 'upload'
  | 'search'
  | 'next'
  | 'prev'
  | 'open'
  | 'toggle-select'
  | 'select-all'
  | 'escape'
  | 'edit-title'
  | 'go-overview'
  | 'go-inbox'
  | 'go-documents'
  | 'go-assistant'
  | 'go-manage'
  | 'go-settings'
  | 'theme'
  | 'focus-list'
  | 'focus-detail'

export interface KeyLike {
  key: string
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
  shiftKey?: boolean
}

/** Mapping for help dialog and docs (key, command, description as source text for t()) */
export const KEYMAP: { keys: string[]; cmd: Command; label: string }[] = [
  { keys: ['Ctrl', 'K'], cmd: 'palette', label: msg('m.app.command_palette_search') },
  { keys: ['/'], cmd: 'search', label: msg('m.app.search_documents') },
  { keys: ['?'], cmd: 'help', label: msg('common.keyboard_shortcuts') },
  { keys: ['j'], cmd: 'next', label: msg('m.app.next_document') },
  { keys: ['k'], cmd: 'prev', label: msg('m.app.previous_document') },
  { keys: ['↑', '↓'], cmd: 'next', label: msg('m.app.move_in_list_focused') },
  { keys: ['x'], cmd: 'toggle-select', label: msg('m.app.select_document') },
  { keys: ['Ctrl', 'A'], cmd: 'select-all', label: msg('common.select_all') },
  { keys: ['e'], cmd: 'edit-title', label: msg('m.app.edit_title') },
  { keys: ['u'], cmd: 'upload', label: msg('common.upload_documents') },
  { keys: ['g', 'o'], cmd: 'go-overview', label: msg('m.app.go_overview') },
  { keys: ['g', 'i'], cmd: 'go-inbox', label: msg('common.go_inbox') },
  { keys: ['g', 'd'], cmd: 'go-documents', label: msg('m.app.go_all_documents') },
  { keys: ['g', 'a'], cmd: 'go-assistant', label: msg('m.app.go_assistant') },
  { keys: ['g', 'm'], cmd: 'go-manage', label: msg('m.app.go_management') },
  { keys: ['g', 's'], cmd: 'go-settings', label: msg('m.app.go_settings') },
  { keys: ['t'], cmd: 'theme', label: msg('common.switch_light_dark') },
  { keys: ['Esc'], cmd: 'escape', label: msg('m.app.close_clear_selection') },
]

const GO: Record<string, Command> = { o: 'go-overview', i: 'go-inbox', d: 'go-documents', a: 'go-assistant', m: 'go-manage', s: 'go-settings' }

/**
 * Stateful translator (for "g then x"). `typing` = focus in input field: then only Ctrl+K and Esc.
 */
export class KeyInterpreter {
  private pendingG: number | null = null
  constructor(private now: () => number = () => Date.now()) {}

  interpret(e: KeyLike, typing: boolean): Command | null {
    const mod = e.ctrlKey || e.metaKey
    if (mod && !e.altKey && e.key.toLowerCase() === 'k') return 'palette'
    if (e.key === 'Escape') return 'escape'
    if (typing) return null
    if (mod && e.key.toLowerCase() === 'a') return 'select-all'
    if (mod || e.altKey) return null
    if (this.pendingG !== null && this.now() - this.pendingG < 1200) {
      this.pendingG = null
      return GO[e.key.toLowerCase()] ?? null
    }
    this.pendingG = null
    switch (e.key) {
      case 'g':
        this.pendingG = this.now()
        return null
      case '?':
        return 'help'
      case '/':
        return 'search'
      case 'j':
        return 'next'
      case 'k':
        return 'prev'
      case 'x':
        return 'toggle-select'
      case 'e':
        return 'edit-title'
      case 'u':
        return 'upload'
      case 't':
        return 'theme'
    }
    return null
  }
}

/** Focus is in an input element (then no single-key shortcuts) */
export function isTypingTarget(el: Element | null): boolean {
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || (el as HTMLElement).isContentEditable === true
}
