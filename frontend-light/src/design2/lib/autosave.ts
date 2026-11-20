// Autosave for inline editing (docs/DESIGN2.md §4.3): collect changes, send debounced, at most one
// request at a time, changes during a running request go into the next. Framework-free (Vitest).
export type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'error'

export interface AutosaveOptions<R> {
  /** sends the collected fields; result = new server state */
  save: (patch: Record<string, unknown>) => Promise<R>
  delay?: number
  onState?: (s: SaveState, error?: unknown) => void
  onSaved?: (result: R, patch: Record<string, unknown>) => void
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (h: unknown) => void
}

export class Autosaver<R = unknown> {
  private pending: Record<string, unknown> = {}
  private inflight: Promise<void> | null = null
  private timer: unknown = null
  private failed: Record<string, unknown> | null = null
  state: SaveState = 'idle'
  lastError: unknown = null

  constructor(private o: AutosaveOptions<R>) {}

  private set(s: SaveState, e?: unknown) {
    this.state = s
    this.o.onState?.(s, e)
  }

  get hasPending() {
    return Object.keys(this.pending).length > 0 || this.inflight !== null
  }

  /** Change field; `immediate` = without debouncing (selection fields) */
  change(field: string, value: unknown, immediate = false) {
    this.pending[field] = value
    this.failed = null
    this.set('pending')
    this.schedule(immediate ? 0 : (this.o.delay ?? 700))
  }

  private schedule(ms: number) {
    const clear = this.o.clearTimer ?? ((h) => clearTimeout(h as number))
    const setT = this.o.setTimer ?? ((fn, t) => setTimeout(fn, t))
    if (this.timer !== null) clear(this.timer)
    this.timer = setT(() => {
      this.timer = null
      void this.flush()
    }, ms)
  }

  /** send immediately (blur, switching documents); waits for all running requests */
  async flush(): Promise<void> {
    if (this.timer !== null) {
      ;(this.o.clearTimer ?? ((h) => clearTimeout(h as number)))(this.timer)
      this.timer = null
    }
    while (this.inflight) await this.inflight
    if (!Object.keys(this.pending).length) return
    const patch = this.pending
    this.pending = {}
    this.set('saving')
    this.inflight = this.o
      .save(patch)
      .then((r) => {
        this.inflight = null
        this.o.onSaved?.(r, patch)
        if (Object.keys(this.pending).length) return this.flush()
        this.set('saved')
      })
      .catch((e) => {
        this.inflight = null
        // keep unsaved fields (newer input wins)
        this.failed = { ...patch, ...this.pending }
        this.pending = {}
        this.lastError = e
        this.set('error', e)
      })
    await this.inflight
  }

  /** retry after error */
  retry() {
    if (!this.failed) return Promise.resolve()
    this.pending = { ...this.failed, ...this.pending }
    this.failed = null
    return this.flush()
  }

  /** discard (e.g. document deleted) */
  reset() {
    if (this.timer !== null) (this.o.clearTimer ?? ((h) => clearTimeout(h as number)))(this.timer)
    this.timer = null
    this.pending = {}
    this.failed = null
    this.set('idle')
  }
}
