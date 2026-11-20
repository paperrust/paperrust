// Vitest: "Automation" area (docs/WORKFLOWS-NEU.md) – draft ↔ stored form, condition chips, short form,
// sentence structure in all 10 languages, templates, order, error mapping and the guard test "every field of the classic
// workflow and mail rule dialogs is reachable in the new editor".
import { describe, expect, it } from 'vitest'
import { format } from '../../lib/i18n'
import classicWorkflow from '../../components/common/edit-dialog/workflow-edit-dialog/WorkflowEditDialog.svelte?raw'
import classicMailRule from '../../components/common/edit-dialog/mail-rule-edit-dialog/MailRuleEditDialog.svelte?raw'
import { TRIGGER_FILTER_DEFINITIONS, WORKFLOW_ACTION_OPTIONS, WORKFLOW_TYPE_OPTIONS, newAction, newTrigger } from '../../components/common/edit-dialog/workflow-edit-dialog/workflow-edit'
import {
  ago,
  clearCondition,
  codeKey,
  conditionActive,
  conditionAllowed,
  draftFromCombined,
  draftFromNgx,
  duplicateDraft,
  emptyDraft,
  errorTargets,
  fieldInSentence,
  findByWords,
  fingerprint,
  fullAction,
  fullTrigger,
  isMailTrigger,
  objCondition,
  parseCfQuery,
  problems,
  quickAction,
  reorderPlan,
  serializeCfQuery,
  setObjCondition,
  summarize,
  templateDraft,
  toSaveBody,
  visibleTemplates,
  type Names,
} from './model'
import { fill, hostOf, list, sentence, sentenceText, type Tr } from './sentence'
import type { CombinedWorkflow, Summary } from './api'

const editorSources = import.meta.glob(['./**/*.svelte', './model.ts'], { query: '?raw', import: 'default', eager: true }) as Record<string, string>
const EDITOR = Object.values(editorSources).join('\n')
const catalogs = import.meta.glob('../../../i18n/*.json', { import: 'default', eager: true }) as Record<string, Record<string, string>>
const LOCALES = ['en-US', 'de-DE', 'fr-FR', 'it-IT', 'es-ES', 'nl-NL', 'pt-BR', 'pl-PL', 'cs-CZ', 'sv-SE']
const cat = (loc: string) => catalogs[`../../../i18n/${loc}.json`]
const trFor = (loc: string): Tr => (k, p) => format(cat(loc)[k] ?? k, p ?? {}, loc)

const NAMES: Names = (kind, id) => ({ tag: { 1: 'Rechnung', 2: 'Steuer' }, correspondent: { 5: 'Swisscom' }, document_type: { 7: 'Rechnung' } } as Record<string, Record<number, string>>)[kind]?.[id] ?? null

function combined(over: Partial<CombinedWorkflow['workflow']> = {}): CombinedWorkflow {
  return {
    workflow: { id: 3, name: 'Rechnungen', order: 10, enabled: true, triggers: [{ id: 11, type: 2, filter_has_correspondent: 5 }], actions: [{ id: 21, type: 1, assign_tags: [1], email: null, webhook: null }], ...over },
    else_actions: [{ id: 31, type: 6, email: null, webhook: null }],
    ai_condition: null,
    ai: { available: false, enabled: false },
    summary: {} as Summary,
  }
}

describe('Entwurf ↔ Speicherform', () => {
  it('ergänzt fehlende Felder und schickt E-Mail/Webhook nur bei der passenden Aktionsart', () => {
    const d = draftFromCombined(combined())
    expect(Object.keys(d.workflow.triggers[0])).toEqual(expect.arrayContaining(Object.keys(newTrigger())))
    expect(Object.keys(d.workflow.actions[0])).toEqual(expect.arrayContaining(Object.keys(newAction())))
    const b = toSaveBody(d)
    expect(b.id).toBe(3)
    expect(b.actions[0].email).toBeNull()
    expect(b.actions[0].webhook).toBeNull()
    expect(b.actions[0].id).toBe(21)
    expect(b.else_actions).toHaveLength(1)
    expect(b.else_actions[0].type).toBe(6)
    expect(b.ai_condition).toBeNull()
  })
  it('E-Mail-Aktion behält ihr Objekt, Passwörter werden zur Liste, „alle entfernen“ lässt Einzelauswahl weg', () => {
    const d = emptyDraft('x')
    d.workflow.actions.push(fullAction({ type: 3, email: { to: 'a@b.ch', subject: 's', body: 'b', include_document: true } }))
    d.workflow.actions.push(fullAction({ type: 5, passwords: 'eins\n\n zwei ' }))
    d.workflow.actions.push(fullAction({ type: 2, remove_all_tags: true, remove_tags: [1, 2] }))
    const b = toSaveBody(d)
    expect(b.actions[0].email).toMatchObject({ to: 'a@b.ch', include_document: true })
    expect(b.actions[0].webhook).toBeNull()
    expect(b.actions[1].passwords).toEqual(['eins', 'zwei'])
    expect('remove_tags' in b.actions[2]).toBe(false)
    expect(b.actions[2].remove_all_tags).toBe(true)
  })
  it('Kopie: ohne ids, ausgeschaltet, neuer Name; Fingerabdruck erkennt Änderungen', () => {
    const d = draftFromCombined(combined())
    const c = duplicateDraft(d, 'Kopie')
    const b = toSaveBody(c)
    expect(b.id).toBeUndefined()
    expect(b.enabled).toBe(false)
    expect(b.name).toBe('Kopie')
    expect(b.triggers[0].id).toBeUndefined()
    expect(b.actions[0].id).toBeUndefined()
    expect(b.else_actions[0].id).toBeUndefined()
    const f = fingerprint(d)
    d.workflow.actions[0].assign_tags = [1, 2]
    expect(fingerprint(d)).not.toBe(f)
  })
  it('ngx-Entwurf (Vorschlag/KI) wird ohne ids übernommen', () => {
    const d = draftFromNgx({ name: 'V', triggers: [{ id: 9, type: 3 }], actions: [{ id: 8, type: 1, email: { id: 4 } }] })
    expect(d.workflow.triggers[0].id).toBeUndefined()
    expect(d.workflow.actions[0].id).toBeUndefined()
    expect(d.workflow.actions[0].email.id ?? null).toBeNull()
    expect('id' in (toSaveBody(d).actions[0].email ?? {})).toBe(false)
  })
  it('Prüfung vor dem Speichern', () => {
    const d = emptyDraft('')
    expect(problems(d)).toEqual(['no_name', 'no_action'])
    d.workflow.name = 'x'
    d.else_actions.push(fullAction({ type: 6 }))
    d.ai_condition = { field: 'tags', object_id: 0, min_confidence: 0.9 }
    expect(problems(d)).toEqual(['ai_condition_incomplete'])
  })
})

describe('Bedingungen (Chips)', () => {
  it('Objekt-Bedingung: Modus und Auswahl hin und zurück', () => {
    const t = fullTrigger({ type: 2 })
    setObjCondition(t, 'correspondent', 'any', [5, 6])
    expect(objCondition(t, 'correspondent')).toEqual({ mode: 'any', ids: [5, 6] })
    setObjCondition(t, 'correspondent', 'is', [5])
    expect(t.filter_has_any_correspondents).toEqual([])
    expect(t.filter_has_correspondent).toBe(5)
    expect(conditionActive(t, 'correspondent')).toBe(true)
    clearCondition(t, 'correspondent')
    expect(conditionActive(t, 'correspondent')).toBe(false)
  })
  it('Einlesen: nur Dateiname, Pfad, Quelle, Mailregel; sonst Inhalt/Objekte', () => {
    expect(conditionAllowed('text', 1)).toBe(false)
    expect(conditionAllowed('path', 1)).toBe(true)
    expect(conditionAllowed('filename', 3)).toBe(true)
    expect(conditionAllowed('mail_rule', 2)).toBe(false)
    expect(conditionAllowed('tags', 4)).toBe(true)
    expect(isMailTrigger({ type: 1, filter_mailrule: 4 })).toBe(true)
    expect(isMailTrigger({ type: 2, filter_mailrule: 4 })).toBe(false)
  })
  it('Benutzerfeld-Abfrage: einfache Form hin und zurück, verschachtelte bleibt Text', () => {
    expect(parseCfQuery(null)).toEqual({ op: 'AND', atoms: [] })
    expect(parseCfQuery('[3, "exact", "x"]')).toEqual({ op: 'AND', atoms: [[3, 'exact', 'x']] })
    const s = parseCfQuery('["OR", [[3, "exists", true], [4, "gt", 5]]]')!
    expect(s.op).toBe('OR')
    expect(JSON.parse(serializeCfQuery(s)!)).toEqual(['OR', [[3, 'exists', true], [4, 'gt', 5]]])
    expect(parseCfQuery('["AND", [["OR", [[1, "exists", true]]]]]')).toBeNull()
    expect(serializeCfQuery({ op: 'AND', atoms: [] })).toBeNull()
  })
})

describe('Kurzform aus dem Entwurf (gleiche Form wie der Server)', () => {
  it('Auslöser, Bedingungen, Aktionen und KI-Bedingung', () => {
    const d = draftFromCombined(combined({ triggers: [{ type: 2, filter_has_correspondent: 5, matching_algorithm: 3, match: 'Rechnung', filter_has_tags: [1, 2] }] }))
    d.ai_condition = { field: 'document_type', object_id: 7, min_confidence: 0.8 }
    const s = summarize(d, NAMES, { available: true, enabled: true })
    expect(s.trigger_types).toEqual([2])
    expect(s.triggers[0].conditions.map((c) => c.kind)).toEqual(['content_match', 'tags_any', 'correspondent_is'])
    expect(s.triggers[0].conditions[2].object).toEqual({ id: 5, name: 'Swisscom' })
    expect(s.actions[0].items).toEqual([{ kind: 'assign_tags', objects: [{ id: 1, name: 'Rechnung' }] }])
    expect(s.else_actions[0].type_key).toBe('move_to_trash')
    expect(s.ai_condition).toMatchObject({ field: 'document_type', object: { id: 7, name: 'Rechnung' }, active: true, inactive_reason: null })
    const off = summarize(d, NAMES)
    expect(off.ai_condition?.inactive_reason).toBe('ai_unavailable')
  })
})

describe('Satz mit klickbaren Teilen', () => {
  const s = (): Summary => {
    const d = draftFromCombined(combined({ triggers: [{ type: 2, filter_has_correspondent: 5, filter_has_tags: [1, 2] }] }))
    return summarize(d, NAMES)
  }
  it('Teile zeigen auf Karten; Schablone und Aufzählung aus der Sprache', () => {
    const r = sentence(s(), trFor('de-DE'), 'de-DE')
    const text = r.main.map((x) => x.text).join('')
    expect(text).toBe('Wenn ein Dokument hinzugefügt wurde und das Dokument einen der Tags „Rechnung“ oder „Steuer“ hat und der Korrespondent „Swisscom“ ist, dann den Tag „Rechnung“ hinzufügen.')
    expect(r.main.find((x) => x.text === 'ein Dokument hinzugefügt wurde')?.target).toEqual({ card: 'trigger', index: 0 })
    expect(r.main.find((x) => x.text.startsWith('der Korrespondent'))?.target).toEqual({ card: 'conditions', index: 0 })
    expect(r.main.find((x) => x.text.startsWith('den Tag'))?.target).toEqual({ card: 'action', index: 0 })
    expect(r.else?.map((x) => x.text).join('')).toBe('Sonst das Dokument in den Papierkorb legen.')
  })
  it('englisch und ohne Auslöser/Aktion: Platzhalter zum Anklicken', () => {
    const empty = summarize(emptyDraft('x'), NAMES)
    empty.triggers = []
    const r = sentence(empty, trFor('en-US'), 'en-US')
    expect(r.main.filter((x) => x.missing).map((x) => x.target)).toEqual([{ card: 'trigger', index: 0 }, { card: 'action', index: 0 }])
    expect(sentenceText(s(), trFor('en-US'), 'en-US')).toBe(
      'When a document has been added and the document has one of the tags “Rechnung” or “Steuer” and the correspondent is “Swisscom”, then add the tag “Rechnung”. Otherwise move the document to the trash.',
    )
  })
  it('gleiche Bedingungen an mehreren Auslösern werden zusammengefasst', () => {
    const d = draftFromNgx({ name: 'x', triggers: [{ type: 2, filter_has_correspondent: 5 }, { type: 3, filter_has_correspondent: 5 }], actions: [{ type: 1, assign_tags: [1] }] })
    expect(sentenceText(summarize(d, NAMES), trFor('de-DE'), 'de-DE')).toBe('Wenn ein Dokument hinzugefügt wurde oder ein Dokument geändert wird und der Korrespondent „Swisscom“ ist, dann den Tag „Rechnung“ hinzufügen.')
  })
  it('in allen 10 Sprachen vollständig, ohne Platzhalter-Reste oder doppelte Leerzeichen', () => {
    const d = draftFromNgx({
      name: 'x',
      triggers: [{ type: 4, schedule_offset_days: 30, schedule_date_field: 'created', schedule_is_recurring: true, schedule_recurring_interval_days: 7, filter_has_not_tags: [1], matching_algorithm: 1, match: 'a b' }],
      actions: [
        { type: 1, assign_title: 'T', assign_tags: [1, 2], assign_correspondent: 5, assign_view_users: [1] },
        { type: 2, remove_all_permissions: true, remove_document_types: [7] },
        { type: 3, email: { to: 'x@y.ch' } },
        { type: 4, webhook: { url: 'https://hook.example/p?k=geheim' } },
        { type: 5 },
        { type: 7 },
        { type: 8 },
      ],
    })
    d.ai_condition = { field: 'tags', object_id: 1, min_confidence: 0.85 }
    const sum = summarize(d, NAMES, { available: true, enabled: true })
    for (const loc of LOCALES) {
      const txt = sentenceText(sum, trFor(loc), loc)
      expect(txt, loc).not.toMatch(/m\.auto\.|\{\$|\u0001|\s{2,}|\bundefined\b|\bnull\b/)
      expect(txt, loc).toContain('85')
      expect(txt, loc).toContain('hook.example')
      expect(txt, loc).not.toContain('geheim')
    }
  })
  it('fill/list/hostOf', () => {
    const tr: Tr = (k, p) => (k === 'k' ? `A ${p?.X} B` : k)
    expect(fill(tr, 'k', { X: [{ text: 'x', target: { card: 'ai' } }] })).toEqual([{ text: 'A ' }, { text: 'x', target: { card: 'ai' } }, { text: ' B' }])
    expect(list([[{ text: 'a' }], [{ text: 'b' }], [{ text: 'c' }]], 'de-DE', 'disjunction').map((x) => x.text).join('')).toBe('a, b oder c')
    expect(hostOf('https://n8n.local:5678/webhook/abc')).toBe('n8n.local:5678')
  })
})

describe('Vorlagen, Reihenfolge, Fehler, Codes', () => {
  it('Vorlagen: KI-Vorlage nur mit KI, Benachrichtigen nur mit E-Mail; Objekte per Name gefunden', () => {
    expect(visibleTemplates({ aiOn: false, emailOn: false }).map((t) => t.key)).toEqual(['invoices', 'passwords', 'tax', 'blank'])
    expect(visibleTemplates({ aiOn: true, emailOn: true })).toHaveLength(6)
    const find = (_k: string, words: string[]) => findByWords([{ id: 1, name: 'Rechnung' }, { id: 2, name: 'Steuer 2024' }], words)
    const inv = templateDraft('invoices', { name: 'R', find })
    expect(inv.workflow.actions[0].assign_tags).toEqual([1])
    expect(templateDraft('tax', { name: 'S', find }).workflow.triggers[0].filter_has_tags).toEqual([2])
    expect(templateDraft('passwords', { name: 'P', find }).workflow.triggers[0].type).toBe(1)
    expect(templateDraft('blank', { name: '', find }).workflow.actions).toEqual([])
    expect(findByWords([{ id: 6, name: 'Invoices' }], ['invoice'])).toBe(6)
    expect(findByWords([{ id: 6, name: 'Invoice processing pipeline' }, { id: 7, name: 'Invoicing' }], ['invoice'])).toBe(6)
  })
  it('Schnell-Aktionen', () => {
    expect(quickAction('email')).toMatchObject({ action: { type: 3 }, focus: 'email_to' })
    expect(quickAction('ai').action.ai_suggestion_fields).toContain('tags')
  })
  it('Reihenfolge: nur geänderte Einträge werden gespeichert', () => {
    const items = [{ id: 1, order: 10 }, { id: 2, order: 20 }, { id: 3, order: 30 }]
    expect(reorderPlan(items, 2, 0)).toEqual([{ id: 3, order: 10 }, { id: 1, order: 20 }, { id: 2, order: 30 }])
    expect(reorderPlan(items, 0, 0)).toEqual([])
  })
  it('API-Feldfehler landen an der richtigen Karte', () => {
    const e = errorTargets({ name: ['This field may not be blank.'], triggers: [{}, { filter_path: ['bad'] }], actions: [{ email: { to: ['Enter a valid email.'] } }], else_actions: [{ webhook: { url: ['x'] } }], ai_condition: ['object missing'] })
    expect(e.general).toEqual(['name: This field may not be blank.'])
    expect(e.triggers).toEqual({ 1: ['filter_path: bad'] })
    expect(e.actions[0]).toEqual(['to: Enter a valid email.'])
    expect(e.else[0]).toEqual(['url: x'])
    expect(e.ai).toEqual(['object missing'])
  })
  it('Ergebnis-Codes sind übersetzt (jede Sprache), unbekannte fallen auf „Sonstiges“', () => {
    for (const code of ['ok', 'else', 'filters_bypassed', 'conditions_not_met', 'match_error', 'error', 'ai_met', 'ai_not_met', 'ai_not_met_else', 'ai_error', 'ai_not_checked', 'ai_deferred', 'ai_ignored_consumption', 'ai_ignored_unavailable', 'ai_ignored_disabled']) {
      expect(codeKey(code)).toBe(`m.auto.code.${code}`)
      for (const loc of LOCALES) expect(cat(loc)[codeKey(code)], `${loc} ${code}`).toBeTruthy()
    }
    expect(codeKey('neu')).toBe('m.auto.code.other')
  })
  it('relative Zeit', () => {
    expect(ago('2026-11-18T10:00:00Z', 'de-DE', Date.parse('2026-11-18T10:05:00Z'))).toBe('vor 5 Minuten')
  })
  it('jeder m.auto-Schlüssel der Quelltexte steht in allen Sprachen', () => {
    const keys = new Set([...EDITOR.matchAll(/'(m\.auto\.[a-z0-9_.]+)'/g)].map((m) => m[1]))
    expect(keys.size).toBeGreaterThan(200)
    for (const loc of LOCALES) expect([...keys].filter((k) => !cat(loc)[k]), loc).toEqual([])
  })
})

describe('Wächter: jedes Feld der klassischen Dialoge ist im neuen Editor erreichbar', () => {
  /** Fields of the classic WorkflowEditDialog: bound form fields, filter fields, permission fields */
  function classicWorkflowFields(): string[] {
    const out = new Set<string>()
    for (const m of classicWorkflow.matchAll(/bind:value=\{(?:f|a|dlg\.form)\.([a-z_]+(?:\.[a-z_]+)?)\}/g)) out.add(m[1])
    for (const m of classicWorkflow.matchAll(/removal\(a, [^,]+, '([a-z_]+)', '([a-z_]+)'/g)) out.add(m[1]).add(m[2])
    for (const m of classicWorkflow.matchAll(/`\$\{prefix\}_([a-z_]+)`/g)) for (const p of ['assign', 'remove']) out.add(`${p}_${m[1]}`)
    for (const m of classicWorkflow.matchAll(/bind:value=\{a\.(remove_all_[a-z_]+)\}/g)) out.add(m[1])
    // Filters of the "further filters" (definitions → trigger fields)
    const FIELD: Record<string, string> = {
      tags_any: 'filter_has_tags',
      tags_all: 'filter_has_all_tags',
      tags_none: 'filter_has_not_tags',
      correspondent_any: 'filter_has_any_correspondents',
      correspondent_is: 'filter_has_correspondent',
      correspondent_not: 'filter_has_not_correspondents',
      document_type_is: 'filter_has_document_type',
      document_type_any: 'filter_has_any_document_types',
      document_type_not: 'filter_has_not_document_types',
      storage_path_is: 'filter_has_storage_path',
      storage_path_any: 'filter_has_any_storage_paths',
      storage_path_not: 'filter_has_not_storage_paths',
      custom_field_query: 'filter_custom_field_query',
    }
    for (const d of TRIGGER_FILTER_DEFINITIONS) out.add(FIELD[d.id])
    return [...out].filter(Boolean).sort()
  }
  it('Liste der klassischen Workflow-Felder ist vollständig erfasst (Plausibilität)', () => {
    const f = classicWorkflowFields()
    for (const k of ['name', 'order', 'enabled', 'type', 'schedule_offset_days', 'schedule_date_field', 'schedule_date_custom_field', 'schedule_is_recurring', 'schedule_recurring_interval_days', 'filter_filename', 'sources', 'filter_path', 'filter_mailrule', 'matching_algorithm', 'match', 'is_insensitive', 'assign_title', 'assign_tags', 'assign_owner', 'assign_custom_fields_values', 'remove_tags', 'remove_all_permissions', 'email.subject', 'email.body', 'email.to', 'email.include_document', 'webhook.url', 'webhook.use_params', 'webhook.as_json', 'webhook.params', 'webhook.body', 'webhook.headers', 'webhook.include_document', 'passwords', 'ai_suggestion_fields', 'ai_create_missing', 'ai_overwrite_existing', 'assign_view_users', 'remove_change_groups', 'filter_has_not_storage_paths'])
      expect(f, k).toContain(k)
    expect(f.length).toBeGreaterThanOrEqual(70)
  })
  it('jedes klassische Workflow-Feld kommt in den Editor-Bausteinen vor', () => {
    // Name/order/active: header of the editor or list (order by dragging); kind: trigger/action sheet
    const ALIASES: Record<string, RegExp> = {
      name: /workflow\.name/,
      order: /\border\b/,
      enabled: /workflow\.enabled/,
      type: /trigger\.type|action\.type/,
    }
    const missing = classicWorkflowFields().filter((f) => {
      if (ALIASES[f]) return !ALIASES[f].test(EDITOR)
      const [a, b] = f.split('.')
      return b ? !new RegExp(`\\.${a}\\.${b}\\b`).test(EDITOR) : !new RegExp(`(['.])${a}\\b`).test(EDITOR)
    })
    expect(missing).toEqual([])
  })
  it('jede Auslöser- und Aktionsart ist wählbar', () => {
    const trig = editorSources['./panels/TriggerPanel.svelte']
    for (const o of WORKFLOW_TYPE_OPTIONS) expect(trig, `Auslöser ${o.id}`).toMatch(new RegExp(`: ${o.id}(,| \\})`))
    const act = editorSources['./panels/ActionPanel.svelte']
    for (const k of ['Assignment', 'Removal', 'Email', 'Webhook', 'PasswordRemoval', 'MoveToTrash', 'RemoteOcr', 'ApplyAiSuggestions']) expect(act, k).toContain(`WorkflowActionType.${k}`)
    expect(WORKFLOW_ACTION_OPTIONS).toHaveLength(8)
  })
  it('jedes Feld der klassischen Mailregel ist im Mail-Auslöser erreichbar', () => {
    const fields = [...new Set([...classicMailRule.matchAll(/bind:value=\{dlg\.form\.([a-z_]+)\}/g)].map((m) => m[1]))]
    expect(fields.length).toBeGreaterThanOrEqual(23)
    const form = editorSources['./panels/MailRuleForm.svelte']
    const missing = fields.filter((f) => !new RegExp(`(rule\\.${f}\\b|'${f}')`).test(form))
    expect(missing).toEqual([])
  })
})

describe('Vorschlags-Sätze: Feldnamen mitten im Satz', () => {
  it('schreibt Feldnamen klein, außer im Deutschen', () => {
    expect(fieldInSentence('Correspondent', 'en-US')).toBe('correspondent')
    expect(fieldInSentence('Document type', 'en-GB')).toBe('document type')
    expect(fieldInSentence('Étiquette', 'fr-FR')).toBe('étiquette')
    expect(fieldInSentence('Korrespondent', 'de-DE')).toBe('Korrespondent')
    expect(fieldInSentence('', 'en-US')).toBe('')
  })
})
