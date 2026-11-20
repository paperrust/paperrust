// Vitest: home page components (docs/DESIGN2-ENTWIRREN.md §4) and lanes of the year bar.
import { describe, expect, it } from 'vitest'
import { defaultWidgets, homeViews, moveWidget, moveWidgetTo, normalizeWidgets, serializeWidgets, toggleWidget, visibleWidgets, WIDGET_IDS } from './widgets'
import { groupItems, NO_GROUP } from '../lib/timeline'
import type { TimelineItem } from '../api/assist'

describe('Startseite: Bausteine', () => {
  it('Vorgabe für neue Benutzer: Posteingang, Fristen, Zuletzt hinzugefügt, Ansichten (+ Frage mit KI)', () => {
    expect(visibleWidgets(defaultWidgets(false), false)).toEqual(['inbox', 'deadlines', 'recent', 'views'])
    expect(visibleWidgets(defaultWidgets(true), true)).toEqual(['ask', 'inbox', 'deadlines', 'recent', 'views', 'ai'])
    expect(defaultWidgets(true).map((w) => w.id)).toEqual(WIDGET_IDS)
    // nothing stored / unreadable → default
    expect(normalizeWidgets(undefined, false)).toEqual(defaultWidgets(false))
    expect(normalizeWidgets(null, true)).toEqual(defaultWidgets(true))
    expect(normalizeWidgets('x', false)).toEqual(defaultWidgets(false))
  })
  it('gespeicherte Auswahl: Reihenfolge bleibt, Unbekanntes/Doppeltes fällt weg, Neues ausgeschaltet angehängt', () => {
    const l = normalizeWidgets([{ id: 'year', on: true }, { id: 'nope', on: true }, { id: 'inbox', on: false }, { id: 'year', on: false }, 'stats'], false)
    expect(l.slice(0, 3)).toEqual([
      { id: 'year', on: true },
      { id: 'inbox', on: false },
      { id: 'stats', on: true },
    ])
    expect(l).toHaveLength(WIDGET_IDS.length)
    expect(l.slice(3).every((w) => !w.on)).toBe(true)
    expect(visibleWidgets(l, false)).toEqual(['year', 'stats'])
  })
  it('„Frage ans Archiv“ nur sichtbar, wenn KI eingeschaltet ist', () => {
    const l = normalizeWidgets([{ id: 'ask', on: true }, { id: 'inbox', on: true }], false)
    expect(visibleWidgets(l, false)).toEqual(['inbox'])
    expect(visibleWidgets(l, true)).toEqual(['ask', 'inbox'])
  })
  it('verschieben (geklemmt), ein-/ausschalten, Speicherform', () => {
    const l = defaultWidgets(false)
    expect(moveWidget(l, 1, -1).map((w) => w.id).slice(0, 2)).toEqual(['inbox', 'ask'])
    expect(moveWidget(l, 0, -1)).toEqual(l)
    expect(moveWidget(l, l.length - 1, 1)).toEqual(l)
    expect(moveWidgetTo(l, 0, 99).at(-1)?.id).toBe('ask')
    expect(moveWidgetTo(l, 5, 1).map((w) => w.id).slice(0, 3)).toEqual(['ask', 'year', 'inbox'])
    const t = toggleWidget(l, 'stats', true)
    expect(t.find((w) => w.id === 'stats')?.on).toBe(true)
    expect(l.find((w) => w.id === 'stats')?.on).toBe(false) // unchanged
    expect(serializeWidgets([{ id: 'inbox', on: true, extra: 1 } as any])).toEqual([{ id: 'inbox', on: true }])
    expect(normalizeWidgets(JSON.parse(JSON.stringify(serializeWidgets(t))), false)).toEqual(t)
  })
  it('gespeicherte Ansichten auf der Startseite: sichtbar + Sortierung, Rest angehängt', () => {
    const views = [{ id: 1 }, { id: 2 }, { id: 3 }, { id: 4 }]
    expect(homeViews(views, [1, 3, 4], [4, 9])).toEqual([{ id: 4 }, { id: 1 }, { id: 3 }])
    expect(homeViews(views, undefined, undefined)).toEqual([])
    expect(homeViews(views, ['2'], null)).toEqual([{ id: 2 }])
  })
})

describe('Jahresleiste: Bahnen nach Dokumenttyp oder Tag', () => {
  const it_ = (id: string, doc: number, type: string | null): TimelineItem => ({ id, kind: 'due', date: '2026-05-01', title: id, document: { id: doc, title: id, documentType: type } })
  const items = [it_('a', 1, 'Rechnung'), it_('b', 2, 'Vertrag'), it_('c', 3, null), it_('d', 4, 'rechnung')]
  it('nach Dokumenttyp (Name, ohne Groß/Klein), „ohne“ zuletzt', () => {
    const l = groupItems(items, 'type', 'Ohne Dokumenttyp')
    expect(l.map((x) => [x.name, x.items.map((i) => i.id)])).toEqual([
      ['Rechnung', ['a', 'd']],
      ['Vertrag', ['b']],
      ['Ohne Dokumenttyp', ['c']],
    ])
    expect(l.at(-1)?.key).toBe(NO_GROUP)
  })
  it('nach Tag: Dokument in jeder seiner Tag-Bahnen, unbekannte Tags ignoriert, Farbe vom Tag', () => {
    const tags: Record<number, number[]> = { 1: [10, 11], 2: [11], 3: [99], 4: [] }
    const info: Record<number, { name: string; color: string }> = { 10: { name: 'Steuer', color: '#a00' }, 11: { name: 'Auto', color: '#0a0' } }
    const l = groupItems(items, 'tag', 'Ohne Tag', (d) => tags[d] ?? [], (id) => info[id])
    expect(l.map((x) => [x.name, x.color, x.items.map((i) => i.id)])).toEqual([
      ['Auto', '#0a0', ['a', 'b']],
      ['Steuer', '#a00', ['a']],
      ['Ohne Tag', null, ['c', 'd']],
    ])
  })
  it('ohne Einträge ohne Gruppe keine „ohne“-Bahn', () => {
    expect(groupItems([it_('a', 1, 'X')], 'type', 'Ohne').map((x) => x.key)).toEqual(['t:x'])
    expect(groupItems([], 'tag', 'Ohne')).toEqual([])
  })
})
