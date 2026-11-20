// Vitest: AI review list (design2/ai/review.ts)
import { describe, expect, it } from 'vitest'
import { alreadyThere, applyBody, canUndo, certainty, newValue, oldValue, openIndexes, summary, type DocNow, type Names } from './review'
import type { Review, ReviewChange } from './api'

const ch = (o: Partial<ReviewChange>): ReviewChange => ({ field: 'title', existing_id: null, new_name: null, value: null, confidence: 0.95, apply: false, why: '', ...o })
const names: Names = {
  tag: (id) => ({ 5: 'Rechnung' })[id] ?? null,
  correspondent: (id) => ({ 1: 'Swisscom' })[id] ?? null,
  documentType: () => null,
  storagePath: () => null,
  field: (id) => ({ 9: 'Betrag' })[id] ?? null,
}
const doc: DocNow = { title: 'Scan 001', created: '2024-05-01', correspondent: null, document_type: 3, storage_path: null, tags: [5], custom_fields: [{ field: 9, value: 'EUR1.00' }] }
const review = (changes: ReviewChange[], o: Partial<Review> = {}): Review => ({ id: 1, document: 2, created: '', run_id: '', trigger: 'consumed', mode: 'suggest', status: 'pending', changes, audit: null, decided_by: null, decided_at: null, ...o })

describe('KI-Prüfliste', () => {
  it('Sicherheit verständlich: sicher ab 0.9, vermutlich ab 0.6, sonst unsicher', () => {
    expect(certainty(0.95)).toBe('sure')
    expect(certainty(0.9)).toBe('sure')
    expect(certainty(0.6)).toBe('likely')
    expect(certainty(0.5)).toBe('unsure')
  })
  it('alt → neu je Feld', () => {
    expect(newValue(ch({ field: 'correspondent', existing_id: 1 }), names)).toEqual({ text: 'Swisscom', isNew: false })
    expect(newValue(ch({ field: 'correspondent', new_name: 'Sunrise' }), names)).toEqual({ text: 'Sunrise', isNew: true })
    expect(newValue(ch({ field: 'document_type', existing_id: 7 }), names).text).toBe('#7')
    expect(oldValue(ch({ field: 'title', value: 'x' }), doc, names)).toBe('Scan 001')
    expect(oldValue(ch({ field: 'correspondent' }), doc, names)).toBe('')
    expect(oldValue(ch({ field: 'tags' }), doc, names)).toBeNull()
    expect(oldValue(ch({ field: 'custom_fields', existing_id: 9 }), doc, names)).toBe('EUR1.00')
    expect(alreadyThere(ch({ field: 'tags', existing_id: 5 }), doc)).toBe(true)
    expect(alreadyThere(ch({ field: 'document_type', existing_id: 3 }), doc)).toBe(true)
    expect(alreadyThere(ch({ field: 'created', value: '2024-05-02' }), doc)).toBe(false)
  })
  it('offene Vorschläge, Zusammenfassung, Rückgängig', () => {
    const r = review([ch({}), ch({ applied: true }), ch({ dismissed: true }), ch({ confidence: 0.5 })])
    expect(openIndexes(r)).toEqual([0, 3])
    expect(summary(r)).toEqual({ open: 2, sure: 1 })
    expect(canUndo(r)).toBe(false)
    expect(canUndo({ ...r, audit: 4, status: 'partial' })).toBe(true)
    expect(canUndo({ ...r, audit: 4, status: 'undone' })).toBe(false)
  })
  it('Anfragekörper: Auswahl sortiert, nur Änderungen der Auswahl, Verwerfen ohne Doppel', () => {
    expect(applyBody([3, 1, 3], { 1: { value: 'Neu' }, 2: { value: 'x' } }, [2, 3])).toEqual({ changes: [1, 3], overrides: { '1': { value: 'Neu' } }, dismiss: [2] })
    expect(applyBody([], {}, [0])).toEqual({ changes: [], dismiss: [0] })
    expect(applyBody([0], {})).toEqual({ changes: [0] })
  })
})
