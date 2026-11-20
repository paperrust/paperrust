import { describe, expect, it } from 'vitest'
import { dropItem, selectedFromValue, unselected } from './drag-drop'

const items = ['a', 'b', 'c', 'd'].map((id) => ({ id, name: id.toUpperCase() }))

describe('DragDropSelectComponent.drop', () => {
  it('writeValue: IDs → Einträge, unbekannte entfallen', () => {
    expect(selectedFromValue(items, ['c', 'x', 'a']).map((i) => i.id)).toEqual(['c', 'a'])
    expect(selectedFromValue(items, null)).toEqual([])
  })
  it('innerhalb der Auswahl verschieben (moveItemInArray)', () => {
    const sel = selectedFromValue(items, ['a', 'b', 'c'])
    expect(dropItem(items, sel, 'selected', 'selected', 0, 2).map((i) => i.id)).toEqual(['b', 'c', 'a'])
  })
  it('aus „nicht ausgewählt“ an Position einfügen', () => {
    const sel = selectedFromValue(items, ['a', 'c'])
    expect(unselected(items, sel).map((i) => i.id)).toEqual(['b', 'd'])
    expect(dropItem(items, sel, 'unselected', 'selected', 1, 1).map((i) => i.id)).toEqual(['a', 'd', 'c'])
  })
  it('zurück in „nicht ausgewählt“ entfernt; Verschieben innerhalb „nicht ausgewählt“ ändert nichts', () => {
    const sel = selectedFromValue(items, ['a', 'c'])
    expect(dropItem(items, sel, 'selected', 'unselected', 0, 0).map((i) => i.id)).toEqual(['c'])
    expect(dropItem(items, sel, 'unselected', 'unselected', 0, 1).map((i) => i.id)).toEqual(['a', 'c'])
  })
})
