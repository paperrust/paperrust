// Minimal replacement for the RxJS `Subject`s used by the Angular filter editor.
// Framework-free: a Svelte component subscribes with `onChange(fn)` and calls the
// returned function on destroy.

export type Listener<T> = (value: T) => void

export class Emitter<T> {
  private listeners = new Set<Listener<T>>()
  private completed = false

  /** Subscribe; returns an unsubscribe function. No-op after `complete()`. */
  subscribe(fn: Listener<T>): () => void {
    if (this.completed) return () => {}
    this.listeners.add(fn)
    return () => {
      this.listeners.delete(fn)
    }
  }

  /** Like `Subject.next()`; ignored after `complete()`. */
  next(value: T): void {
    if (this.completed) return
    for (const fn of [...this.listeners]) fn(value)
  }

  /** Like `Subject.complete()`: drops all listeners, further `next()` calls are ignored. */
  complete(): void {
    this.completed = true
    this.listeners.clear()
  }

  get isCompleted(): boolean {
    return this.completed
  }
}

/** Translator signature, compatible with `t` from `../i18n`. */
export type Translator = (key: string, params?: Record<string, string | number | null | undefined>) => string

/** Default translator: identity on the English source text, substituting `{$NAME}` placeholders. */
export const identityTranslator: Translator = (key, params) =>
  key.replace(/\{\$([A-Za-z0-9_]+)\}/g, (_, name) => String(params?.[name] ?? ''))

/** Same behaviour as `title.replace(/\s/g, '_').toLowerCase()` in the Angular dropdowns. */
export function sanitizeName(title: string): string {
  return title ? title.replace(/\s/g, '_').toLowerCase() : null
}
