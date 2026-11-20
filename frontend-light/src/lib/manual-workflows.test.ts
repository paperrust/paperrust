import { describe, expect, it } from 'vitest'
import { canRunManualWorkflow } from './manual-workflows'
import confirmDialogSrc from '../components/common/confirm-dialog/run-workflow-confirm-dialog/RunWorkflowConfirmDialog.svelte?raw'
import documentDetailSrc from '../components/document-detail/DocumentDetail.svelte?raw'
import bulkEditorSrc from '../components/document-list/bulk-editor/BulkEditor.svelte?raw'
import bulkBarSrc from '../design2/workspace/BulkBar.svelte?raw'
import docDetailSrc from '../design2/workspace/DocDetail.svelte?raw'
import runSheetSrc from '../design2/workspace/RunWorkflowSheet.svelte?raw'

const ME = 5
const owns = (d: { owner?: number | null }) => !d.owner || d.owner === ME
const docs = [
  { id: 1, owner: ME },
  { id: 2, owner: null },
  { id: 3, owner: 9 },
]
const base = { canChange: true, docs, owns, workflowCount: 1 }

describe('canRunManualWorkflow (Workflow von Hand nur für Besitzer)', () => {
  it('erlaubt, wenn alle gewählten Dokumente dem Nutzer gehören (ohne Besitzer zählt mit)', () => {
    expect(canRunManualWorkflow({ ...base, selectedIds: [1, 2] })).toBe(true)
  })
  it('verbietet, sobald ein gewähltes Dokument jemand anderem gehört – auch mit Bearbeitungsrecht', () => {
    expect(canRunManualWorkflow({ ...base, selectedIds: [1, 3] })).toBe(false)
  })
  it('Superuser darf immer, auch für nicht geladene Dokumente', () => {
    expect(canRunManualWorkflow({ ...base, selectedIds: [3, 99], isSuperuser: true })).toBe(true)
  })
  it('nicht geladene Dokumente gelten als nicht sicher im Besitz', () => {
    expect(canRunManualWorkflow({ ...base, selectedIds: [1, 99] })).toBe(false)
  })
  it('ohne Änderungsrecht, ohne Workflows oder ohne Auswahl: nein', () => {
    expect(canRunManualWorkflow({ ...base, selectedIds: [1], canChange: false })).toBe(false)
    expect(canRunManualWorkflow({ ...base, selectedIds: [1], workflowCount: 0 })).toBe(false)
    expect(canRunManualWorkflow({ ...base, selectedIds: [] })).toBe(false)
  })
})

describe('„Filter ignorieren“ nur für ein einzelnes Dokument vorbelegt', () => {
  it('Detailansichten setzen forceDefault, Massenbearbeitung nicht', () => {
    expect(documentDetailSrc).toMatch(/forceDefault: true/)
    expect(docDetailSrc).toMatch(/<RunWorkflowSheet[^>]*\bforceDefault\b/)
    expect(bulkEditorSrc).not.toMatch(/forceDefault/)
    expect(bulkBarSrc).not.toMatch(/forceDefault/)
  })
  it('Dialoge übernehmen den Startwert statt fest false', () => {
    for (const s of [confirmDialogSrc, runSheetSrc]) {
      expect(s).toMatch(/forceDefault = false/)
      expect(s).toMatch(/let force = \$state\(untrack\(\(\) => forceDefault\)\)/)
    }
  })
})
