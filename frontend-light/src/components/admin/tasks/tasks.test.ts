import { describe, expect, it } from 'vitest'
import { paramsForSection, query, taskDisplayName, taskMatchesFilters, taskResultMessage, taskResultPreview, taskTypeLabel, triggerSourceLabel } from './tasks'

const task = (extra: any = {}) => ({ id: 1, task_type: 'consume_file', task_type_display: 'Consume File', trigger_source: 'web_ui', trigger_source_display: 'Web UI', status: 'failure', ...extra })

describe('TasksComponent', () => {
  it('Typ/Quelle aus dem eigenen Katalog, nicht aus der Serverübersetzung (Accept-Language)', () => {
    const de = task({ task_type: 'empty_trash', task_type_display: 'Papierkorb leeren', trigger_source: 'scheduled', trigger_source_display: 'Geplant', input_data: null })
    expect(taskTypeLabel(de)).toBe('Empty trash')
    expect(triggerSourceLabel(de)).toBe('Scheduled')
    expect(taskDisplayName(de)).toBe('Empty trash')
    expect(taskTypeLabel(task({ task_type: 'unbekannt', task_type_display: 'Neu' }))).toBe('Neu')
  })
  it('Parameter je Abschnitt wie HttpParams (status mehrfach, Filter angehängt)', () => {
    const f = { taskType: 'mail_fetch', triggerSource: null, filterText: 'abc', filterTargetID: 1 }
    expect(query(paramsForSection('needs_attention', f))).toBe('?acknowledged=false&status=failure&status=revoked&task_type=mail_fetch&result=abc')
    expect(query(paramsForSection('all', { ...f, taskType: null, filterText: '' }))).toBe('?acknowledged=false')
  })
  it('Anzeigename, Ergebnistext und Vorschau', () => {
    expect(taskDisplayName(task({ input_data: { filename: 'a.pdf' } }))).toBe('a.pdf')
    expect(taskDisplayName(task())).toBe('Import file') // own catalog instead of task_type_display
    expect(taskResultMessage(task({ result_data: { document_id: 5 } }))).toBe('Done. New document 5 created')
    expect(taskResultMessage(task({ result_data: { duplicate_of: 7 } }))).toBe('Same as document #7')
    expect(taskResultPreview(task({ result_data: { reason: 'x'.repeat(60) } }))).toBe('x'.repeat(50) + '...')
  })
  it('Filter nach Typ/Auslöser; Text filtert nur der Server (upstream fix fefce7be1)', () => {
    const tk = task({ result_data: { error_message: 'Timeout' } })
    expect(taskMatchesFilters(tk, { taskType: null, triggerSource: null, filterText: 'web', filterTargetID: 0 })).toBe(true)
    expect(taskMatchesFilters(tk, { taskType: null, triggerSource: null, filterText: 'time', filterTargetID: 1 })).toBe(true)
    expect(taskMatchesFilters(tk, { taskType: 'mail_fetch', triggerSource: null, filterText: '', filterTargetID: 0 })).toBe(false)
    // found by the server, display text translated ("E-Mail" instead of "email") → stays visible
    const translated = task({ trigger_source: 'email_consume', trigger_source_display: 'E-Mail' })
    expect(taskMatchesFilters(translated, { taskType: null, triggerSource: null, filterText: 'email', filterTargetID: 0 })).toBe(true)
    // Result hit of the server with translated result text → stays visible
    expect(taskMatchesFilters(task({ result_data: { duplicate_of: 3 } }), { taskType: null, triggerSource: null, filterText: 'duplicate', filterTargetID: 1 })).toBe(true)
    // the text is still passed on to the server as a parameter
    expect(query(paramsForSection('all', { taskType: null, triggerSource: null, filterText: 'email', filterTargetID: 0 }))).toBe('?acknowledged=false&name=email')
  })
})
