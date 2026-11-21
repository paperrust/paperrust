// Conversation state of the assistant (module level, survives unmount): a running answer keeps streaming when the
// user leaves the page (e.g. opens a source) and is shown live or finished on return. Only an explicit stop,
// "New conversation" or opening another conversation aborts the stream.
import { t } from '../../lib/i18n'
import {
  ask as apiAsk,
  conversation as apiConversation,
  conversations as apiConversations,
  mockEnabled,
  Unavailable,
  type AskAction,
  type AskEvent,
  type Citation,
  type Conversation,
} from '../api/assist'

export interface Msg {
  role: 'user' | 'assistant'
  text: string
  citations: Citation[]
  actions: AskAction[]
  notes: string[]
  error?: string | null
  streaming?: boolean
  /** AI off / no LLM: only weighted hits (contract: answer null, notes) */
  sourcesOnly?: boolean
}

/**
 * Progress of the running answer: `searching` until the `meta` event, `writing` (passages found, the AI writes)
 * until the first `delta`, then `streaming`; `idle` when nothing runs.
 */
export type Phase = 'idle' | 'searching' | 'writing' | 'streaming'

export interface AssistantDeps {
  ask: typeof apiAsk
  conversation: typeof apiConversation
  conversations: typeof apiConversations
  mockEnabled: () => boolean
  now: () => number
}

const DEFAULT_DEPS: AssistantDeps = { ask: apiAsk, conversation: apiConversation, conversations: apiConversations, mockEnabled: () => mockEnabled(), now: () => Date.now() }

export class AssistantStore {
  msgs = $state<Msg[]>([])
  conversation = $state<string | null>(null)
  convs = $state<Conversation[]>([])
  busy = $state(false)
  unavailable = $state(false)
  phase = $state<Phase>('idle')
  /** time (ms) the running question was sent */
  startedAt = $state(0)
  private ctrl: AbortController | null = null
  private listeners = new Set<(e: AskEvent | null, index: number) => void>()

  constructor(private deps: AssistantDeps = DEFAULT_DEPS) {}

  /** Event listener of the mounted view (scrolling); `null` = stream ended. Returns the unsubscribe function. */
  listen(fn: (e: AskEvent | null, index: number) => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }
  private emit(e: AskEvent | null, i: number) {
    for (const fn of this.listeners) {
      try {
        fn(e, i)
      } catch {}
    }
  }

  refreshConvs(): Promise<void> {
    return this.deps
      .conversations()
      .then((c) => {
        this.convs = c
      })
      .catch((e) => {
        if (e instanceof Unavailable) this.unavailable = !this.deps.mockEnabled()
      })
  }

  async send(text: string): Promise<void> {
    const question = text.trim()
    if (!question || this.busy) return
    this.msgs = [...this.msgs, { role: 'user', text: question, citations: [], actions: [], notes: [] }, { role: 'assistant', text: '', citations: [], actions: [], notes: [], streaming: true }]
    const i = this.msgs.length - 1
    const msgs = this.msgs
    // updates only the message of this question (a later question, "New conversation" or another
    // conversation replaces the array)
    const upd = (f: (m: Msg) => void) => {
      if (this.msgs !== msgs) return
      const m = { ...this.msgs[i] }
      f(m)
      this.msgs[i] = m
    }
    const ctrl = new AbortController()
    this.ctrl = ctrl
    this.busy = true
    this.phase = 'searching'
    this.startedAt = this.deps.now()
    try {
      await this.deps.ask(
        question,
        this.conversation,
        (e) => {
          if (ctrl.signal.aborted) return
          if (e.type === 'meta') {
            if (e.conversation) this.conversation = e.conversation
            if (e.notes?.length) upd((m) => (m.notes = e.notes!))
            if (this.phase === 'searching') this.phase = 'writing'
          } else if (e.type === 'delta') {
            upd((m) => (m.text += e.text))
            this.phase = 'streaming'
          } else if (e.type === 'citations') upd((m) => (m.citations = e.citations))
          else if (e.type === 'actions') upd((m) => (m.actions = e.actions))
          else if (e.type === 'error') upd((m) => (m.error = e.message))
          else if (e.type === 'done')
            upd((m) => {
              if (!m.text && e.answer) m.text = e.answer
              if (e.answerError) m.error = e.answerError
              if (e.notes?.length) m.notes = e.notes
              if (!m.text && !e.answer && !e.answerError && !m.error) m.sourcesOnly = true
            })
          this.emit(e, i)
        },
        ctrl.signal,
      )
    } catch (e: any) {
      if (e instanceof Unavailable) {
        this.unavailable = true
        upd((m) => (m.error = t('m.assistant.assistant_not_available_server')))
      } else if (e?.name !== 'AbortError') upd((m) => (m.error = e?.message ?? String(e)))
    } finally {
      upd((m) => (m.streaming = false))
      // a newer question may already run (stop + send again): only reset what belongs to this one
      if (this.ctrl === ctrl) {
        this.ctrl = null
        this.busy = false
        this.phase = 'idle'
      }
      this.emit(null, i)
      this.refreshConvs()
    }
  }

  /** Abort the running answer (keeps what has arrived so far) */
  stop() {
    const c = this.ctrl
    if (!c) return
    this.ctrl = null
    c.abort()
    this.busy = false
    this.phase = 'idle'
    const last = this.msgs.length - 1
    if (last >= 0 && this.msgs[last].streaming) this.msgs[last] = { ...this.msgs[last], streaming: false }
  }

  newChat() {
    this.stop()
    this.msgs = []
    this.conversation = null
  }

  /** Load a stored conversation; throws when loading fails (the view shows a toast). */
  async openConversation(uuid: string) {
    this.stop()
    const ms = await this.deps.conversation(uuid)
    this.conversation = uuid
    this.msgs = ms.map((m) => ({
      role: m.role,
      text: m.content,
      citations: m.citations,
      actions: m.actions,
      notes: m.notes,
      error: m.answerError,
      sourcesOnly: m.role === 'assistant' && !m.content && !m.answerError,
    }))
  }

  /** seconds since the running question was sent (0 when idle) */
  elapsed(now = this.deps.now()): number {
    return this.busy ? Math.max(0, Math.floor((now - this.startedAt) / 1000)) : 0
  }
}

export const assistant = new AssistantStore()
