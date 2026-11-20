import { describe, expect, it } from 'vitest'
import { customFieldsMessage, escapeHtml, localizeList, rotationDegrees, selectionQuery, singleMessage, tagsMessage } from './bulk-editor'
import { formValue, keepValues } from './custom-fields-bulk-edit-dialog/custom-fields-bulk-edit'

const tag = (id: number, name: string) => ({ id, name })

describe('bulk-editor (BulkEditorComponent)', () => {
  it('getSelectionQuery: einzelne IDs oder alles mit Filter und Ausnahmen', () => {
    expect(selectionQuery({ allSelected: false, filterRules: [], excluded: new Set([9]), selected: new Set([3, 1]) })).toEqual({ documents: [3, 1] })
    expect(selectionQuery({ allSelected: true, filterRules: [{ rule_type: 0, value: 'rent' }], excluded: new Set([7]), selected: new Set([3]) })).toEqual({
      all: true,
      filters: { title_search: 'rent' },
      excluded_documents: [7],
    })
  })

  it('_localizeList wie upstream', () => {
    expect(localizeList([])).toBe('')
    expect(localizeList([tag(1, 'a')])).toBe('"a"')
    expect(localizeList([tag(1, 'a'), tag(2, 'b')])).toBe('"a" and "b"')
    expect(localizeList([tag(1, 'a'), tag(2, 'b'), tag(3, 'c')])).toBe('"a", "b" and "c"')
  })

  it('setTags: Texte für alle Fälle, Namen maskiert', () => {
    expect(tagsMessage([tag(1, 'a')], [], 4)).toBe('The tag &quot;a&quot; will be added to 4 selected document(s).')
    expect(tagsMessage([tag(1, 'a'), tag(2, 'b')], [], 4)).toBe('The tags &quot;a&quot; and &quot;b&quot; will be added to 4 selected document(s).')
    expect(tagsMessage([], [tag(1, 'a')], 2)).toBe('The tag &quot;a&quot; will be removed from 2 selected document(s).')
    expect(tagsMessage([], [tag(1, 'a'), tag(2, 'b')], 2)).toContain('will be removed')
    expect(tagsMessage([tag(1, 'a')], [tag(2, 'b')], 2)).toBe(
      'On 2 selected document(s), the tags &quot;a&quot; will be added and the tags &quot;b&quot; removed.',
    )
    expect(tagsMessage([tag(1, '<img src=x onerror=alert(1)>')], [], 1)).not.toContain('<img')
  })

  it('setCustomFields und Einzelzuweisungen', () => {
    expect(customFieldsMessage([tag(1, 'f')], [], 3)).toBe('The custom field &quot;f&quot; will be added to 3 selected document(s).')
    expect(customFieldsMessage([], [tag(1, 'f'), tag(2, 'g')], 3)).toBe(
      'The custom fields &quot;f&quot; and &quot;g&quot; will be removed from 3 selected document(s).',
    )
    expect(singleMessage('correspondent', tag(1, 'ACME'), 2)).toEqual({
      title: 'Confirm correspondent change',
      message: 'The correspondent &quot;ACME&quot; will be set on 2 selected document(s).',
    })
    expect(singleMessage('document_type', null, 2).message).toBe('The document type will be removed from 2 selected document(s).')
    expect(singleMessage('storage_path', null, 1).title).toBe('Confirm storage path change')
  })

  it('escapeHtml und Drehwinkel', () => {
    expect(escapeHtml(`<a href="x">'&</a>`)).toBe('&lt;a href=&quot;x&quot;&gt;&#39;&amp;&lt;/a&gt;')
    expect(rotationDegrees(0)).toBe(0)
    expect(rotationDegrees(-90)).toBe(270)
    expect(rotationDegrees(450)).toBe(90)
    expect(rotationDegrees(-360) === 0).toBe(true) // -0 as upstream, JSON yields 0
  })

  it('Feldwerte-Dialog: Steuerelemente je Feld, Werte bleiben beim Ergänzen', () => {
    let v = keepValues({}, [3, 5])
    expect(v).toEqual({ 3: null, 5: null })
    v = { ...v, 3: 'x' }
    v = keepValues(v, [3, 7])
    expect(v).toEqual({ 3: 'x', 7: null })
    expect(formValue({ ...v, 9: 'alt', 7: undefined }, [3, 7])).toEqual({ 3: 'x', 7: null })
  })
})
