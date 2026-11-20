import { describe, expect, it, beforeEach } from 'vitest'
import { format, formatParts, parse, resolveLocale, segments, setDictionary, t, tRich, loadLocale, locale, AVAILABLE_LOCALES, fullLocale, catalogFor, catalogLocale } from './i18n'
import { formatDate, parseDate } from './dates'

const DOCS = '{VAR_PLURAL, plural, =1 {One document} other {{$INTERPOLATION} documents}}'

describe('ICU/Platzhalter-Formatierer', () => {
  it('ersetzt Platzhalter', () => {
    expect(format('Logged in as {$INTERPOLATION}', { INTERPOLATION: 'Admin' })).toBe('Logged in as Admin')
    expect(format('Save "{$INTERPOLATION}"', {})).toBe('Save ""')
  })
  it('plural =1 und other (en)', () => {
    expect(format(DOCS, { VAR_PLURAL: 1, INTERPOLATION: 1 })).toBe('One document')
    expect(format(DOCS, { VAR_PLURAL: 195, INTERPOLATION: 195 })).toBe('195 documents')
    expect(format(DOCS, { VAR_PLURAL: 0, INTERPOLATION: 0 })).toBe('0 documents')
  })
  it('plural mit Sprachregeln (one/few/many, Intl.PluralRules)', () => {
    const msg = '{VAR_PLURAL, plural, one {{$N} plik} few {{$N} pliki} many {{$N} plików} other {{$N} pliku}}'
    expect(format(msg, { VAR_PLURAL: 1, N: 1 }, 'pl-PL')).toBe('1 plik')
    expect(format(msg, { VAR_PLURAL: 3, N: 3 }, 'pl-PL')).toBe('3 pliki')
    expect(format(msg, { VAR_PLURAL: 5, N: 5 }, 'pl-PL')).toBe('5 plików')
  })
  it('select und verschachteltes ICU', () => {
    const msg = '{VAR_SELECT, select, true {Ja {VAR_PLURAL, plural, =1 {eins} other {viele}}} other {Nein}}'
    expect(format(msg, { VAR_SELECT: 'true', VAR_PLURAL: 1 })).toBe('Ja eins')
    expect(format(msg, { VAR_SELECT: 'true', VAR_PLURAL: 2 })).toBe('Ja viele')
    expect(format(msg, { VAR_SELECT: 'x' })).toBe('Nein')
  })
  it('behält Leerzeichen innerhalb von ICU-Fällen (Angular rendert sie)', () => {
    const de = '{VAR_PLURAL, plural, =1 { {$INTERPOLATION} von 1 Dokument ausgewählt} other {{$INTERPOLATION} von {$INTERPOLATION_1} Dokumenten ausgewählt}}'
    expect(format(de, { VAR_PLURAL: 1, INTERPOLATION: 1 })).toBe(' 1 von 1 Dokument ausgewählt')
    expect(format(de, { VAR_PLURAL: 7, INTERPOLATION: 2, INTERPOLATION_1: 7 })).toBe('2 von 7 Dokumenten ausgewählt')
  })
  it('Rich-Text: START_/CLOSE_-Platzhalter bleiben als Segmente', () => {
    const msg = 'PaperRust is based on {$START_LINK}paperless-ngx{$CLOSE_LINK} and is licensed under the {$START_LINK_1}GPL-3.0{$CLOSE_LINK}.'
    expect(formatParts(msg)).toEqual(['PaperRust is based on ', { ph: 'START_LINK' }, 'paperless-ngx', { ph: 'CLOSE_LINK' }, ' and is licensed under the ', { ph: 'START_LINK_1' }, 'GPL-3.0', { ph: 'CLOSE_LINK' }, '.'])
    expect(segments(formatParts(msg))).toEqual([
      { text: 'PaperRust is based on ' },
      { text: 'paperless-ngx', tag: 'START_LINK' },
      { text: ' and is licensed under the ' },
      { text: 'GPL-3.0', tag: 'START_LINK_1' },
      { text: '.' },
    ])
  })
  it('lässt geschweifte Klammern ohne ICU stehen und cacht Parsebäume', () => {
    expect(format('a {b} c')).toBe('a {b} c')
    expect(parse(DOCS)).toBe(parse(DOCS))
  })
})

describe('Wörterbuch + Sprachwahl', () => {
  beforeEach(() => setDictionary('en-US', {}))
  it('t(): Punkt-Schlüssel, unbekannte Schlüssel (z. B. Namen aus der API) bleiben stehen', () => {
    setDictionary('de-DE', { 'common.documents_2': 'Dokumente', 'list.documents': '{VAR_PLURAL, plural, =1 {Ein Dokument} other {{$INTERPOLATION} Dokumente}}' })
    expect(t('common.documents_2')).toBe('Dokumente')
    expect(t('Mein Ordner')).toBe('Mein Ordner')
    expect(t('list.documents', { VAR_PLURAL: 1, INTERPOLATION: 1 })).toBe('Ein Dokument')
    expect(t('list.documents', { VAR_PLURAL: 2, INTERPOLATION: 2 })).toBe('2 Dokumente')
    expect(tRich('common.documents_2')).toEqual([{ text: 'Dokumente' }])
  })
  it('catalogFor: genau, sonst Grundsprache, sonst Englisch', () => {
    const av = ['en-US', 'de-DE', 'fr-FR', 'pt-BR']
    expect(catalogFor('de-CH', av)).toBe('de-DE')
    expect(catalogFor('de_de', av)).toBe('de-DE')
    expect(catalogFor('pt-PT', av)).toBe('pt-BR')
    expect(catalogFor('ja-JP', av)).toBe('en-US')
    expect(catalogFor(null, av)).toBe('en-US')
  })
  it('resolveLocale: Setting vor Browser, Browser-Sprache mit Katalog bevorzugt, Rückfall en-US', () => {
    const av = ['en-US', 'de-DE', 'fr-FR']
    expect(resolveLocale('de-CH', ['fr-FR'], av)).toBe('de-CH')
    expect(resolveLocale('de_DE', [], av)).toBe('de-DE')
    expect(resolveLocale('', ['ja-JP', 'fr-CA'], av)).toBe('fr-CA')
    expect(resolveLocale(null, ['ja-JP'], av)).toBe('ja-JP')
    expect(resolveLocale(null, [], av)).toBe('en-US')
  })
  it('genau 10 eigene Sprachen', () => {
    expect(AVAILABLE_LOCALES).toEqual(['en-US', 'cs-CZ', 'de-DE', 'es-ES', 'fr-FR', 'it-IT', 'nl-NL', 'pl-PL', 'pt-BR', 'sv-SE'])
  })
  it('loadLocale lädt erst das Kern-Wörterbuch, dann die volle Datei; Sprache ohne Katalog → Englisch, Format bleibt', async () => {
    const calls: string[] = []
    const fake = async (p: string) => {
      calls.push(p)
      return { 'common.documents_2': p.includes('de-DE') ? 'Dokumente' : 'Documents' }
    }
    const core = (l: string) => fake(`core:${l}`)
    await loadLocale('de-CH', fake, core)
    await fullLocale()
    expect(calls).toEqual(['core:de-DE', '../../i18n/de-DE.json'])
    expect(locale()).toBe('de-CH')
    expect(catalogLocale()).toBe('de-DE')
    expect(t('common.documents_2')).toBe('Dokumente')
    await loadLocale('ja-JP', fake, core)
    await fullLocale()
    expect(calls.slice(2)).toEqual(['core:en-US', '../../i18n/en-US.json'])
    expect(locale()).toBe('ja-JP')
    expect(t('common.documents_2')).toBe('Documents')
  })
  it('echte Dateien: en-US und de-DE haben dieselben Schlüssel, Plural funktioniert', async () => {
    await loadLocale('en-US')
    await fullLocale()
    expect(t('common.documents_2')).toBe('Documents')
    expect(t('list.documents', { VAR_PLURAL: 195, INTERPOLATION: 195 })).toBe('195 documents')
    await loadLocale('de-DE')
    await fullLocale()
    expect(t('common.documents_2')).not.toBe('common.documents_2')
    expect(t('list.documents', { VAR_PLURAL: 195, INTERPOLATION: 195 })).toContain('195')
    expect(tRich('common.paperrust_based_paperless_ngx').some((s) => s.tag === 'START_LINK' && s.text === 'paperless-ngx')).toBe(true)
  })
})

describe('Datumsformat wie Angular DatePipe', () => {
  it('mediumDate en-US / de-DE, Datum ohne Zeit = lokal', () => {
    expect(formatDate('2025-11-01', 'mediumDate', 'en-US')).toBe('Nov 1, 2025')
    expect(formatDate('2025-11-01', 'mediumDate', 'de-DE')).toBe('01.11.2025')
    expect(parseDate('2025-01-31')!.getDate()).toBe(31)
  })
  it('iso-8601 und leere Werte', () => {
    expect(formatDate('2025-02-03', 'longDate', 'iso-8601')).toBe('2025-02-03')
    expect(formatDate(null)).toBe('')
    expect(formatDate('kein Datum')).toBe('')
  })
})
