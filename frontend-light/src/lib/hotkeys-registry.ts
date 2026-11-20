// Live directory of keyboard shortcuts that components register at runtime – port of HotKeyService.addShortcut
// (services/hot-key.service.ts): keydown listener on document, ignore input without modifier, entry
// (keys → description) for the shortcut dialog. Difference from upstream: with an open dialog the shortcut is
// ignored (upstream closes all dialogs via dismissAll and executes it).
// The shortcut dialog can read `registeredHotkeys()` (and subscribe to `onHotkeysChange`).
import { ignoreHotkeyEvent } from '../components/common/hotkey-dialog/hotkeys'
import { modals } from './modal.svelte'

export interface HotkeyOptions {
  /** Angular notation like upstream, e.g. 't', 'control.s', 'shift.?' */
  keys: string
  /** Text in the shortcut dialog (without description: not listed) */
  description?: string
  handler: (e: KeyboardEvent) => void
  /** false → shortcut has no effect and is not listed (upstream: filter(() => !this.disabled)) */
  enabled?: () => boolean
}

const entries: HotkeyOptions[] = []
const listeners = new Set<() => void>()
const notify = () => listeners.forEach((f) => f())

/** Angular KeyEventsPlugin: key and modifier must match exactly; 'control' also applies to meta (upstream). */
export function matchesKeys(e: Pick<KeyboardEvent, 'key' | 'altKey' | 'ctrlKey' | 'metaKey' | 'shiftKey'>, keys: string): boolean {
  const parts = keys.toLowerCase().split('.')
  const key = parts.pop()
  const mods = new Set(parts)
  const k = (e.key ?? '').toLowerCase()
  if (k !== key && !(key === 'escape' && k === 'esc')) return false
  const ctrl = mods.has('control')
  if (e.altKey !== mods.has('alt') || e.shiftKey !== mods.has('shift')) return false
  if (ctrl) return (e.ctrlKey || e.metaKey) && !(e.ctrlKey && e.metaKey)
  return e.ctrlKey === false && e.metaKey === mods.has('meta')
}

/** keydown handler on document (exported for tests) */
export function handleHotkeyKeydown(e: KeyboardEvent): void {
  if (ignoreHotkeyEvent(e)) return
  if (modals.stack.length > 0) return
  for (const h of [...entries]) {
    if ((h.enabled?.() ?? true) && matchesKeys(e, h.keys)) {
      e.preventDefault()
      h.handler(e)
      return
    }
  }
}

/** Register shortcut; the return value unregisters it (call on onDestroy). */
export function registerHotkey(opts: HotkeyOptions): () => void {
  if (entries.length === 0 && typeof document !== 'undefined') document.addEventListener('keydown', handleHotkeyKeydown)
  entries.push(opts)
  notify()
  return () => {
    const i = entries.indexOf(opts)
    if (i >= 0) entries.splice(i, 1)
    if (entries.length === 0 && typeof document !== 'undefined') document.removeEventListener('keydown', handleHotkeyKeydown)
    notify()
  }
}

/** Active, described shortcuts as [keys, description] in registration order, once per key sequence. */
export function registeredHotkeys(): [string, string][] {
  const out = new Map<string, string>()
  for (const h of entries) if (h.description && (h.enabled?.() ?? true) && !out.has(h.keys)) out.set(h.keys, h.description)
  return [...out]
}

/** Notification on registration/unregistration (e.g. for an open shortcut dialog). */
export function onHotkeysChange(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}
