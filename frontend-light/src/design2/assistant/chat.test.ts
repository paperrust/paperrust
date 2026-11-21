// Vitest: module-level assistant state (chat.svelte.ts) – the stream does not depend on a mounted view,
// progress phases, stop/new conversation abort.
import { describe, expect, it } from 'vitest'
import { AssistantStore, type AssistantDeps } from './chat.svelte'
import type { AskEvent } from '../api/assist'

/** ask() whose events the test releases step by step */
function fakeAsk() {
  let emit: (e: AskEvent) => void = () => {}
  let finish: () => void = () => {}
  let signal: AbortSignal | undefined
  const ask: AssistantDeps['ask'] = (_q, _c, onEvent, s) =>
    new Promise<void>((resolve, reject) => {
      emit = onEvent
      signal = s
      finish = resolve
      s?.addEventListener('abort', () => reject(Object.assign(new Error('aborted'), { name: 'AbortError' })))
    })
  return { ask, emit: (e: AskEvent) => emit(e), finish: () => finish(), aborted: () => !!signal?.aborted }
}

function store(f: ReturnType<typeof fakeAsk>, now = { t: 1000 }) {
  const deps: AssistantDeps = { ask: f.ask, conversation: async () => [], conversations: async () => [], mockEnabled: () => false, now: () => now.t }
  return new AssistantStore(deps)
}
const tick = () => new Promise((r) => setTimeout(r, 0))

describe('Assistent: Zustand außerhalb der Ansicht', () => {
  it('Phasen suchen → schreiben → Text, Antwort läuft ohne Ansicht weiter', async () => {
    const f = fakeAsk()
    const now = { t: 1000 }
    const s = store(f, now)
    const done = s.send('Wann endet mein Vertrag?')
    expect(s.busy).toBe(true)
    expect(s.phase).toBe('searching')
    now.t = 4500
    expect(s.elapsed()).toBe(3)
    f.emit({ type: 'meta', conversation: 'c-1', notes: [] })
    expect(s.phase).toBe('writing')
    expect(s.conversation).toBe('c-1')
    f.emit({ type: 'citations', citations: [{ n: 1, document: { id: 7, title: 'Vertrag' } }] })
    expect(s.phase).toBe('writing')
    f.emit({ type: 'delta', text: 'Am 31.12. [1]' })
    expect(s.phase).toBe('streaming')
    f.emit({ type: 'done', answer: 'Am 31.12. [1]' })
    f.finish()
    await done
    expect(s.busy).toBe(false)
    expect(s.phase).toBe('idle')
    expect(s.elapsed()).toBe(0)
    expect(s.msgs).toHaveLength(2)
    expect(s.msgs[1].text).toBe('Am 31.12. [1]')
    expect(s.msgs[1].citations[0].document.id).toBe(7)
    expect(s.msgs[1].streaming).toBe(false)
  })

  it('Listener abmelden (Ansicht verlassen) bricht nicht ab', async () => {
    const f = fakeAsk()
    const s = store(f)
    const seen: (string | null)[] = []
    const off = s.listen((e) => seen.push(e?.type ?? null))
    const done = s.send('Frage')
    f.emit({ type: 'meta' })
    off()
    f.emit({ type: 'delta', text: 'weiter' })
    expect(f.aborted()).toBe(false)
    f.finish()
    await done
    expect(seen).toEqual(['meta'])
    expect(s.msgs[1].text).toBe('weiter')
  })

  it('Stopp und neue Unterhaltung brechen ab; Ereignisse danach werden ignoriert', async () => {
    const f = fakeAsk()
    const s = store(f)
    const p = s.send('Frage')
    f.emit({ type: 'meta' })
    f.emit({ type: 'delta', text: 'Teil' })
    s.stop()
    expect(f.aborted()).toBe(true)
    expect(s.busy).toBe(false)
    expect(s.msgs[1].streaming).toBe(false)
    f.emit({ type: 'delta', text: ' später' })
    await p
    expect(s.msgs[1].text).toBe('Teil')
    expect(s.msgs[1].error ?? null).toBeNull()

    const g = fakeAsk()
    const s2 = store(g)
    const p2 = s2.send('Frage')
    s2.newChat()
    expect(g.aborted()).toBe(true)
    await p2
    await tick()
    expect(s2.msgs).toEqual([])
    expect(s2.conversation).toBeNull()
  })

  it('leere Frage oder laufende Antwort: nichts senden', async () => {
    const f = fakeAsk()
    const s = store(f)
    await s.send('   ')
    expect(s.msgs).toHaveLength(0)
    const p = s.send('eins')
    await s.send('zwei')
    expect(s.msgs).toHaveLength(2)
    f.finish()
    await p
  })
})
