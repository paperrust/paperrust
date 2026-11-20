// Vitest: pure logic of the PaperRust design (docs/DESIGN2.md §9).
import { describe, expect, it, vi } from 'vitest'
import { Autosaver, type SaveState } from './autosave'
import { KeyInterpreter, isTypingTarget } from './keys'
import { fuzzyScore, rank } from './palette'
import { getObjects, getText, otherRules, queryFromState, sameRules, setDate, setText, stateFromQuery, toggleObject, QUICK_FILTERS, INBOX_RULES } from './filters'
import { months, place, position, upcomingFrom, urgency, yearRange, daysBetween, addDays } from './timeline'
import { nextTheme, themeMode } from './theme'
import { autoRedirect, designOf, switchTarget, takeDesignParam } from '../../lib/design-switch'
import { parseRoute, ctxPath } from '../stores/workspace.svelte'
import { toV2Patch, type IdMaps } from '../api/v2docs'
import { messageToEvents, normalizeCitation, normalizeTimeline, parseSse, mockEnabled } from '../api/assist'
import { FILTER_CORRESPONDENT, FILTER_HAS_CORRESPONDENT_ANY, FILTER_HAS_TAGS_ALL, FILTER_SIMPLE_TEXT, FILTER_CREATED_FROM } from '../../lib/filter-rule-type'

describe('Autosaver', () => {
  const timers = () => {
    const q: (() => void)[] = []
    return { setTimer: (fn: () => void) => (q.push(fn), q.length - 1), clearTimer: () => {}, fire: () => q.splice(0).forEach((f) => f()) }
  }
  it('sammelt Änderungen und sendet nur geänderte Felder, eine Anfrage gleichzeitig', async () => {
    const calls: Record<string, unknown>[] = []
    let release!: () => void
    const save = vi.fn((p: Record<string, unknown>) => {
      calls.push(p)
      return new Promise<string>((r) => (release = () => r('ok')))
    })
    const states: SaveState[] = []
    const tm = timers()
    const s = new Autosaver({ save, onState: (x) => states.push(x), ...tm })
    s.change('title', 'a')
    s.change('title', 'ab')
    s.change('tags', [1])
    const f = s.flush()
    expect(calls).toEqual([{ title: 'ab', tags: [1] }])
    s.change('title', 'abc') // during the request
    release()
    await Promise.resolve()
    await new Promise((r) => setTimeout(r, 0))
    release()
    await f
    expect(calls).toEqual([{ title: 'ab', tags: [1] }, { title: 'abc' }])
    expect(states.at(-1)).toBe('saved')
  })
  it('Fehler behält Felder, retry sendet sie erneut', async () => {
    let fail = true
    const save = vi.fn(async (p: Record<string, unknown>) => {
      if (fail) throw new Error('500')
      return p
    })
    const s = new Autosaver({ save, ...timers() })
    s.change('title', 'x')
    await s.flush()
    expect(s.state).toBe('error')
    fail = false
    await s.retry()
    expect(save).toHaveBeenLastCalledWith({ title: 'x' })
    expect(s.state).toBe('saved')
  })
  it('entprellt Text, Auswahl sofort', () => {
    const tm = { setTimer: vi.fn(() => 1), clearTimer: vi.fn() }
    const s = new Autosaver({ save: async () => 0, delay: 700, ...tm })
    s.change('title', 'a')
    expect(tm.setTimer).toHaveBeenLastCalledWith(expect.any(Function), 700)
    s.change('correspondent', 3, true)
    expect(tm.setTimer).toHaveBeenLastCalledWith(expect.any(Function), 0)
  })
})

describe('Tastatur', () => {
  it('Ctrl+K immer, Ein-Tasten-Kürzel nicht beim Tippen', () => {
    const k = new KeyInterpreter()
    expect(k.interpret({ key: 'k', ctrlKey: true }, true)).toBe('palette')
    expect(k.interpret({ key: 'K', metaKey: true }, false)).toBe('palette')
    expect(k.interpret({ key: 'j' }, true)).toBeNull()
    expect(k.interpret({ key: 'j' }, false)).toBe('next')
    expect(k.interpret({ key: 'k' }, false)).toBe('prev')
    expect(k.interpret({ key: 'Escape' }, true)).toBe('escape')
    expect(k.interpret({ key: '?' }, false)).toBe('help')
  })
  it('g-Folgen mit Zeitfenster', () => {
    let now = 0
    const k = new KeyInterpreter(() => now)
    expect(k.interpret({ key: 'g' }, false)).toBeNull()
    expect(k.interpret({ key: 'i' }, false)).toBe('go-inbox')
    k.interpret({ key: 'g' }, false)
    now = 5000
    expect(k.interpret({ key: 'd' }, false)).toBeNull()
  })
  it('Eingabeelemente', () => {
    expect(isTypingTarget({ tagName: 'INPUT' } as Element)).toBe(true)
    expect(isTypingTarget({ tagName: 'DIV', isContentEditable: false } as unknown as Element)).toBe(false)
  })
})

describe('Befehlspalette', () => {
  it('Teilstring und Wortanfang vor unscharfen Treffern', () => {
    expect(fuzzyScore('inb', 'Inbox')).toBeGreaterThan(fuzzyScore('inb', 'Find in bulk'))
    expect(fuzzyScore('xyz', 'Inbox')).toBe(0)
    const r = rank(
      [
        { id: '1', label: 'Settings', group: 'g' },
        { id: '2', label: 'Inbox', group: 'g' },
        { id: '3', label: 'All documents', group: 'g', keywords: 'inbox list' },
      ],
      'inbox',
    )
    expect(r.map((x) => x.id)).toEqual(['2', '3'])
  })
})

describe('Filter (klassische Regeln/URL)', () => {
  it('Text, Objekte, nicht zugewiesen, Datum ↔ Query', () => {
    let rules = setText([], 'rechnung')
    rules = toggleObject(rules, 'tags', 4)
    rules = toggleObject(rules, 'correspondent', 7)
    rules = setDate(rules, 'from', '2025-01-01')
    expect(getText(rules)).toBe('rechnung')
    expect(getObjects(rules, 'tags')).toEqual([4])
    const q = queryFromState({ rules, sortField: 'created', sortReverse: true })
    expect(q).toContain('text=rechnung')
    expect(q).toContain('tags__id__all=4')
    expect(q).toContain('correspondent__id__in=7')
    expect(q).toContain('created__date__gte=2025-01-01')
    const back = stateFromQuery(q)
    expect(sameRules(back.rules, rules)).toBe(true)
    expect(back.sortReverse).toBe(true)
    // "not assigned" replaces concrete values
    const none = toggleObject(rules, 'correspondent', null)
    expect(getObjects(none, 'correspondent')).toEqual([null])
    expect(queryFromState({ rules: none, sortField: 'created', sortReverse: false })).toContain('correspondent__isnull=1')
    expect(none.some((r) => r.rule_type === FILTER_HAS_CORRESPONDENT_ANY)).toBe(false)
    expect(none.some((r) => r.rule_type === FILTER_CORRESPONDENT)).toBe(true)
  })
  it('Schnellfilter und übrige Regeln', () => {
    const q = QUICK_FILTERS.find((x) => x.id === 'month')!.rules('2026-11-10')
    expect(q).toEqual([{ rule_type: 46, value: '2026-11-01' }])
    expect(otherRules([...q, { rule_type: FILTER_SIMPLE_TEXT, value: 'x' }, { rule_type: FILTER_HAS_TAGS_ALL, value: '1' }, { rule_type: FILTER_CREATED_FROM, value: '2020-01-01' }])).toEqual(q)
    expect(INBOX_RULES[0].rule_type).toBe(5)
  })
})

describe('Arbeitsplatz-Routen', () => {
  it('Kontext und aktives Dokument', () => {
    expect(parseRoute('documents')).toEqual({ kind: 'documents', viewId: null, activeId: null })
    expect(parseRoute('documents/12/details')).toEqual({ kind: 'documents', viewId: null, activeId: 12 })
    expect(parseRoute('inbox/3')).toEqual({ kind: 'inbox', viewId: null, activeId: 3 })
    expect(parseRoute('view/5/9')).toEqual({ kind: 'view', viewId: 5, activeId: 9 })
    expect(parseRoute('attributes/tags')).toBeNull()
    expect(ctxPath({ kind: 'view', viewId: 5 }, 9)).toBe('view/5/9')
    expect(ctxPath({ kind: 'inbox', viewId: null }, null)).toBe('inbox')
  })
})

describe('Zeitstrahl', () => {
  it('Achse und Position', () => {
    const { from, to } = yearRange(2026)
    const m = months(from, to)
    expect(m).toHaveLength(12)
    expect(m[0].start).toBe(0)
    expect(m[11].start + m[11].width).toBeCloseTo(1)
    expect(position('2026-07-02', from, to)).toBeCloseTo(182 / 365, 2)
    expect(position('2030-01-01', from, to)).toBe(1)
    expect(daysBetween('2026-11-10', '2026-11-30')).toBe(20)
    expect(addDays('2026-12-31', 1)).toBe('2027-01-01')
  })
  it('Zeilen ohne Überlappung, Spannen', () => {
    const doc = { id: 1, title: 'A' }
    const lanes = [
      {
        key: 'a',
        name: 'A',
        items: [
          { id: '1', kind: 'due', date: '2026-03-01', document: doc, title: 'A' },
          { id: '2', kind: 'due', date: '2026-03-02', document: doc, title: 'B' },
          { id: '3', kind: 'valid_until', start: '2026-01-01', date: '2026-06-30', document: doc, title: 'C' },
          { id: '4', kind: 'created', date: '2026-02-01', document: doc, title: 'D' },
          { id: '5', kind: 'warranty_until', start: '2026-05-01', date: '2027-08-01', document: doc, title: 'E' },
        ],
      },
    ]
    const [p] = place(lanes, '2026-01-01', '2026-12-31', 0.03, ['created'])
    expect(p.items).toHaveLength(4)
    const out = p.items.find((i) => i.item.id === '5')!
    expect(out.row).toBe(-1) // Reference date after year end: bars only, no marker
    expect(out.x + out.w).toBeCloseTo(1)
    const span = p.items.find((i) => i.item.id === '3')!
    expect(span.w).toBeGreaterThan(0.4)
    expect(p.spanRows).toBe(2)
    expect(p.rows).toBe(2) // 1 March and 2 March are too close together, 30 June fits in row 0
    expect(span.row).toBe(0)
  })
  it('Demnächst: Fristen, überfällig, ohne erledigte und ohne Dokumentdaten', () => {
    const doc = { id: 1, title: 'X' }
    const items = [
      { id: 'a', kind: 'due', date: '2026-11-20', document: doc, title: 'a' },
      { id: 'b', kind: 'cancel_by', date: '2026-11-01', document: doc, title: 'b' },
      { id: 'c', kind: 'created', date: '2026-11-12', document: doc, title: 'c' },
      { id: 'd', kind: 'due', date: '2026-11-15', document: doc, title: 'd', done: true },
      { id: 'e', kind: 'due', date: '2027-06-01', document: doc, title: 'e' },
      { id: 'a', kind: 'due', date: '2026-11-20', document: doc, title: 'a' },
    ]
    const u = upcomingFrom(items, '2026-11-10')
    expect(u.map((x) => x.id)).toEqual(['b', 'a'])
    expect(u[0].overdue).toBe(true)
    expect(u[1].daysLeft).toBe(10)
    expect(urgency(3)).toBe('high')
    expect(urgency(20)).toBe('medium')
    expect(urgency(50)).toBe('low')
  })
})

describe('API-Vertrag Design 2 (Normalisierung)', () => {
  it('Zeitstrahl: Bahnen des Servers zusammengeführt, Einträge ohne Doppelte', () => {
    const t = normalizeTimeline(
      {
        from: '2026-01-01',
        to: '2026-12-31',
        today: '2026-11-10',
        lanes: [
          { key: 'u1', area: { uuid: 'u1', name: 'Wohnen', color: '#2F6B4F', icon: 'home' }, items: [{ id: 'd.due.20261130', kind: 'due', date: '2026-11-30', span: { start: '2026-01-01', end: '2026-11-30' }, source: 'custom_field', phrase: 'zahlbar bis', document: { uuid: 'd', legacy_id: 12, title: 'Rechnung', correspondent: { name: 'Swisscom' } }, amount: { value: '89.90', currency: 'CHF' }, state: { done: false } }] },
          { key: 'other', area: null, items: [{ id: 'd.due.20261130', kind: 'due', date: '2026-11-30', document: { uuid: 'd', legacy_id: 12, title: 'Rechnung' } }, { id: 'e.created', kind: 'created', date: '2026-03-01', document: { uuid: 'e', legacy_id: 13, title: 'Brief', document_type: { uuid: 't', name: 'Brief' } } }] },
        ],
      },
      'x',
      'y',
    )
    expect(t.items.map((x) => x.id)).toEqual(['d.due.20261130', 'e.created'])
    const i = t.items[0]
    expect(i).toMatchObject({ kind: 'due', date: '2026-11-30', start: '2026-01-01', amount: 'CHF 89.90', label: 'zahlbar bis' })
    expect(i).not.toHaveProperty('area')
    expect(i.document).toMatchObject({ id: 12, uuid: 'd', correspondent: 'Swisscom' })
    expect(t.items[1].document?.documentType).toBe('Brief')
  })
  it('SSE-Ereignisse (event: + data:)', () => {
    const buf = 'event: meta\ndata: {"conversation":"c1","index_state":"ready","retrieval":"hybrid","notes":[]}\n\n' + 'event: citations\ndata: {"citations":[{"n":1,"document":"uu","legacy_id":12,"title":"T","page":2,"snippet":"s","source":"both","score":0.1}]}\n\n' + 'event: delta\ndata: {"text":"Hallo [1]"}\n\nevent: done\ndata: {"answer":"Hallo [1]","answer_error":null}\n\nevent: delta\ndata: {"te'
    const { events, rest } = parseSse(buf)
    expect(events.map((e) => e.type)).toEqual(['meta', 'citations', 'delta', 'done'])
    expect(events[1]).toMatchObject({ citations: [{ n: 1, document: { id: 12, uuid: 'uu', title: 'T', page: 2 } }] })
    expect(rest).toContain('"te')
  })
  it('JSON-Antwort ohne Streaming', () => {
    const ev = messageToEvents({ conversation: 'c', message: { answer: null, notes: ['No LLM'], citations: [{ n: 1, document: 'u', legacy_id: 3, title: 'A' }], actions: [{ type: 'open_documents', label: 'Öffnen', legacy_ids: [3] }] } })
    expect(ev.map((e) => e.type)).toEqual(['meta', 'citations', 'actions', 'done'])
    expect(ev[2]).toMatchObject({ actions: [{ type: 'open_documents', legacyIds: [3] }] })
    expect(normalizeCitation({ n: 2, document: 'x', legacy_id: 'nope' })).toBeNull()
  })
  it('Demo nur auf ausdrücklichen Wunsch', () => {
    expect(mockEnabled({ search: '' }, { getItem: () => null })).toBe(false)
    expect(mockEnabled({ search: '?mock=1' }, null)).toBe(true)
    expect(mockEnabled({ search: '' }, { getItem: () => '1' })).toBe(true)
  })
})

describe('Umschalter und Thema', () => {
  it('ui_settings-Schlüssel und Ziel-URL', () => {
    expect(designOf({})).toBe('modern')
    expect(designOf({ paperrust: { design: 'modern' } })).toBe('modern')
    expect(designOf({ paperrust: { design: 'classic' } })).toBe('classic')
    expect(switchTarget('modern', 'documents/12', '?x=1')).toBe('/rs/app/m/documents/12?x=1')
    // Click test 2026-11-20: a ?design=classic remembered at start must not travel along
    expect(switchTarget('modern', 'dashboard', '?design=classic')).toBe('/rs/app/m/dashboard')
    expect(switchTarget('modern', 'documents', '?x=1&design=classic')).toBe('/rs/app/m/documents?x=1')
    expect(switchTarget('classic', 'documents', '?design=modern&y=2')).toBe('/rs/app/documents?y=2')
    expect(switchTarget('classic', 'attributes/tags', '')).toBe('/rs/app/attributes/tags')
    expect(switchTarget('classic', 'inbox', '')).toBe('/rs/app/documents?is_in_inbox=true')
    expect(switchTarget('classic', 'assistant', '')).toBe('/rs/app/dashboard')
    expect(autoRedirect('modern', 'dashboard')).toBe('/rs/app/m/')
    expect(autoRedirect('modern', 'documents/3')).toBeNull()
    expect(autoRedirect('classic', 'dashboard')).toBeNull()
  })
  it('?design= aus der Adresse übernehmen und entfernen', () => {
    let url = ''
    const hist = { state: null, replaceState: (_s: unknown, _t: string, u: string) => (url = u) } as unknown as History
    expect(takeDesignParam({ search: '?design=modern&x=1', pathname: '/rs/app/documents', hash: '' } as Location, hist)).toBe('modern')
    expect(url).toBe('/rs/app/documents?x=1')
    expect(takeDesignParam({ search: '?design=evil', pathname: '/', hash: '' } as Location, hist)).toBeNull()
  })
  it('Hell → Dunkel → System', () => {
    expect(themeMode(true, false)).toBe('auto')
    expect(themeMode(false, true)).toBe('dark')
    expect(nextTheme('light')).toBe('dark')
    expect(nextTheme('dark')).toBe('auto')
    expect(nextTheme('auto')).toBe('light')
  })
})

describe('Autosave über v2 (If-Match)', () => {
  const m: IdMaps = {
    tags: new Map([[1, 't1'], [2, 't2']]),
    correspondents: new Map([[5, 'c5']]),
    document_types: new Map(),
    storage_paths: new Map(),
    custom_fields: new Map([[9, 'f9']]),
  }
  it('übersetzt v1-Felder in den v2-DocumentPatch (Referenzen als UUID)', () => {
    expect(toV2Patch({ title: 'x', created: '2026-01-02', archive_serial_number: 7, correspondent: 5, tags: [1, 2], custom_fields: [{ field: 9, value: '2026-12-01' }] }, m)).toEqual({
      title: 'x',
      created: '2026-01-02',
      asn: 7,
      correspondent: 'c5',
      tags: ['t1', 't2'],
      custom_fields: [{ field: 'f9', value: '2026-12-01' }],
    })
    expect(toV2Patch({ correspondent: null, archive_serial_number: null }, m)).toEqual({ correspondent: null, asn: null })
  })
  it('nicht abbildbar → null (Aufrufer nimmt v1)', () => {
    expect(toV2Patch({ document_type: 3 }, m)).toBeNull()
    expect(toV2Patch({ tags: [1, 99] }, m)).toBeNull()
    expect(toV2Patch({ custom_fields: [{ field: 9, value: [1, 2] }] }, m)).toBeNull()
    expect(toV2Patch({ owner: 1 }, m)).toBeNull()
  })
})
