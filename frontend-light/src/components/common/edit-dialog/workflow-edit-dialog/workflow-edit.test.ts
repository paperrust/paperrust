import { describe, expect, it } from 'vitest'
import {
  actionForm,
  allowedActionTypes,
  applyFilters,
  buildFilters,
  copyWorkflow,
  filterTypeOptions,
  moveItem,
  newAction,
  newTrigger,
  nextFilterType,
  parsePasswords,
  triggerForm,
  workflowFormValues,
  WorkflowActionType,
} from './workflow-edit'

describe('WorkflowEditDialogComponent', () => {
  it('Filter aus Auslöserfeldern und zurück (leere Werte entfallen, Einzelwert aus Liste)', () => {
    const trigger = { ...newTrigger(), filter_has_tags: [1, 2], filter_has_document_type: 3, filter_custom_field_query: '["AND",[[1,"exists","true"]]]' }
    const filters = buildFilters(trigger)
    expect(filters.map((f) => f.type)).toEqual(['tags_any', 'document_type_is', 'custom_field_query'])
    expect(nextFilterType(filters)).toBe('tags_all')
    expect(filterTypeOptions(filters, 0).find((o) => o.id === 'document_type_is')?.disabled).toBe(true)
    const out = applyFilters({ ...trigger }, [...filters, { type: 'correspondent_is', values: [5] }, { type: 'tags_none', values: [] }])
    expect(out).toMatchObject({ filter_has_tags: [1, 2], filter_has_document_type: 3, filter_has_correspondent: 5, filter_has_not_tags: [] })
    expect(out).not.toHaveProperty('filters')
  })
  it('gesendeter Wert: Webhook/E-Mail nur beim passenden Typ, Passwörter zeilenweise, „alle entfernen“ lässt Einzelauswahl weg', () => {
    const removal = actionForm({ ...newAction(), type: WorkflowActionType.Removal, remove_all_tags: true, remove_tags: [1] })
    const pw = { ...actionForm({ ...newAction(), type: WorkflowActionType.PasswordRemoval }), passwords: 'a\n b ,c\n\n' }
    const v = workflowFormValues({ name: 'W', order: 1, enabled: true, triggers: [triggerForm(newTrigger())], actions: [removal, pw] })
    expect(v.actions[0]).not.toHaveProperty('remove_tags')
    expect(v.actions[0].webhook).toBeNull()
    expect(v.actions[0].email).toBeNull()
    expect(v.actions[1].passwords).toEqual(['a', 'b', 'c'])
    expect(parsePasswords(['x '])).toEqual(['x'])
  })
  it('erlaubte Aktionen: E-Mail nur mit E-Mail-Versand, Remote-OCR nur bei Einlese-Auslöser, KI nicht bei reinem Einlesen', () => {
    const form = { triggers: [{ type: 1 }], actions: [] as any[] }
    const ids = (s: any) => allowedActionTypes(form, s).map((a) => a.id)
    expect(ids({ emailEnabled: false, remoteOcrConfigured: true, aiEnabled: true })).toEqual([1, 2, 4, 5, 6, 7])
    expect(ids({ emailEnabled: true, remoteOcrConfigured: false, aiEnabled: false })).toEqual([1, 2, 3, 4, 5, 6])
  })
  it('Kopie ohne IDs, verschieben wie moveItemInArray', () => {
    const c = copyWorkflow({ id: 1, name: 'A', triggers: [{ id: 2 }], actions: [{ id: 3, webhook: { id: 4 }, email: null }] })
    expect(c).toMatchObject({ id: null, name: 'A (copy)', triggers: [{ id: null }], actions: [{ id: null, webhook: { id: null } }] })
    expect(moveItem([1, 2, 3], 0, 2)).toEqual([2, 3, 1])
  })
})
