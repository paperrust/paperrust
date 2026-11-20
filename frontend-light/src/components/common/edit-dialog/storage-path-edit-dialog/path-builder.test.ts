import { describe, expect, it } from 'vitest'
import { buildTemplate, examplePath, parseTemplate, PRESETS } from './path-builder'

describe('Speicherpfad-Baukasten', () => {
  it('baut Vorlagen und ergänzt den Titel als Dateinamen', () => {
    expect(buildTemplate(['year', 'correspondent'])).toBe('{{ created_year }}/{{ correspondent }}/{{ title }}')
    expect(buildTemplate(['correspondent', 'asn'])).toBe('{{ correspondent }}/{{ asn }}')
    expect(buildTemplate([])).toBe('')
    expect(buildTemplate(['year', 'year', 'title'])).toBe('{{ created_year }}/{{ title }}')
  })
  it('liest einfache Vorlagen zurück, freie nicht', () => {
    expect(parseTemplate('{{ created_year }}/{{ correspondent }}/{{ title }}')).toEqual(['year', 'correspondent', 'title'])
    expect(parseTemplate('{{created_year}}/{{title}}')).toEqual(['year', 'title'])
    expect(parseTemplate('Archiv/{{ title }}')).toBeNull()
    expect(parseTemplate('{{ title|lower }}')).toBeNull()
    expect(parseTemplate('')).toEqual([])
  })
  it('jede Vorlage → Teile → Vorlage verlustfrei', () => {
    for (const p of PRESETS) expect(parseTemplate(buildTemplate(p.parts))).toEqual(p.parts)
  })
  it('Beispielpfad', () => {
    expect(examplePath(['year', 'correspondent'])).toBe('2026/Swisscom/Rechnung März.pdf')
  })
})
