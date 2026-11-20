// Desktop app mode 2b: `/ws/status/` via the relay of the client core (address from `/local/ws`),
// silently retry when offline; without the desktop app unchanged `ws://<host>/ws/status/`.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('../api/client', () => ({ api: {} }))
vi.mock('./settings.svelte', () => ({ session: { data: { user: { id: 1, is_superuser: true } } } }))

const { consumerStatus } = await import('./uploads.svelte')

class FakeSocket {
  static all: FakeSocket[] = []
  static OPEN = 1
  readyState = 0
  onmessage: ((ev: any) => void) | null = null
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  constructor(public url: string) {
    FakeSocket.all.push(this)
  }
  close() {
    this.readyState = 3
    this.onclose?.()
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

afterEach(() => {
  consumerStatus.disconnect()
  FakeSocket.all = []
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('ConsumerStatusState.connect', () => {
  it('Server-Seite: direkt ws://<host>/ws/status/', () => {
    vi.stubGlobal('WebSocket', FakeSocket)
    vi.stubGlobal('location', { protocol: 'https:', host: 'docs.example.ch', hostname: 'docs.example.ch' })
    consumerStatus.connect()
    expect(FakeSocket.all.map((s) => s.url)).toEqual(['wss://docs.example.ch/ws/status/'])
  })

  it('Desktop-App (prapi): Adresse vom Kern, Meldungen kommen an, offline erneut versuchen', async () => {
    vi.stubGlobal('WebSocket', FakeSocket)
    vi.stubGlobal('location', { protocol: 'http:', host: 'prapi.localhost', hostname: 'prapi.localhost' })
    const url = 'ws://127.0.0.1:4711/ws/status/?k=abc'
    const fetchMock = vi.fn(async () => ({ ok: true, json: async () => ({ url, online: true }) }))
    vi.stubGlobal('fetch', fetchMock)
    consumerStatus.connect()
    await flush()
    await flush()
    expect(fetchMock).toHaveBeenCalledWith('/local/ws', expect.anything())
    expect(FakeSocket.all.map((s) => s.url)).toEqual([url])
    const ws = FakeSocket.all[0]
    ws.onmessage?.({
      data: JSON.stringify({ type: 'status_update', data: { filename: 'x.pdf', task_id: 't9', current_progress: 5, max_progress: 100, status: 'WORKING', message: 'x' } }),
    })
    expect(consumerStatus.consumerStatus.some((s) => s.taskId === 't9')).toBe(true)

    // Connection drops (server offline) → new attempt after 5 s
    vi.useFakeTimers()
    ws.close()
    expect(FakeSocket.all.length).toBe(1)
    await vi.advanceTimersByTimeAsync(5000)
    vi.useRealTimers()
    await flush()
    await flush()
    expect(fetchMock).toHaveBeenCalledTimes(2)
    expect(FakeSocket.all.length).toBe(2)
  })

  it('Desktop-App ohne Vermittler (paperless-ngx): keine Verbindung, kein Fehler', async () => {
    vi.stubGlobal('WebSocket', FakeSocket)
    vi.stubGlobal('location', { protocol: 'http:', host: 'prapi.localhost', hostname: 'prapi.localhost' })
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ url: null, online: true }) })))
    consumerStatus.connect()
    await flush()
    await flush()
    expect(FakeSocket.all.length).toBe(0)
  })
})
