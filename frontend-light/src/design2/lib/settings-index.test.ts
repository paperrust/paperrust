// Vitest: settings in the global search (settings-index.ts)
import { describe, expect, it } from 'vitest'
import { searchSettings, type SettingEntry } from './settings-index'

const E = (id: string, label: string, keywords = '', where = 'Einstellungen › System'): SettingEntry => ({ id, label, keywords, where, route: 'settings/system' })
const entries = [
  E('a', 'Sprache der Texterkennung', 'PAPERRUST_OCR_LANGUAGE ocr'),
  E('b', 'Rotation threshold', 'PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD ocr'),
  E('c', 'Date format', 'datumsformat', 'Settings › General'),
  E('d', 'Allowed schemes', 'PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES'),
]

describe('searchSettings', () => {
  it('verlangt alle Wörter und bevorzugt Treffer im Namen', () => {
    expect(searchSettings(entries, 'ocr sprache').map((e) => e.id)).toEqual(['a'])
    expect(searchSettings(entries, 'ocr').map((e) => e.id).sort()).toEqual(['a', 'b'])
    expect(searchSettings(entries, 'texterkennung')[0].id).toBe('a')
  })
  it('findet über Stichwörter und Variablennamen', () => {
    expect(searchSettings(entries, 'datumsformat').map((e) => e.id)).toEqual(['c'])
    expect(searchSettings(entries, 'webhooks_allowed').map((e) => e.id)).toEqual(['d'])
  })
  it('liefert bei Unsinn nichts Zufälliges', () => {
    expect(searchSettings(entries, 'qqqzzz')).toEqual([])
  })
})
// findTarget/flashSetting need a DOM (checked in the browser click test)
