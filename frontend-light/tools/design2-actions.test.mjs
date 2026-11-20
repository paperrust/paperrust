// Guard (feedback 2026-11-18): actions of the classic UI must not silently go missing in the PaperRust design.
// Every action here has an entry with a fixed id in DocDetail.svelte or BulkBar.svelte.
import { expect, it as test } from 'vitest'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ws = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'design2', 'workspace')
const read = (f) => fs.readFileSync(path.join(ws, f), 'utf8')

const DETAIL = ['d2-act-pdf-editor', 'd2-act-redact', 'd2-act-shrink', 'd2-act-convert', 'd2-act-reprocess', 'd2-act-print', 'd2-act-email', 'd2-act-share', 'd2-act-download-original', 'd2-doc-run-workflow', 'd2-doc-delete']
const BULK = ['d2-bulk-rotate', 'd2-bulk-merge', 'd2-bulk-merge-versions', 'd2-bulk-convert', 'd2-bulk-reprocess', 'd2-bulk-path', 'd2-bulk-field', 'd2-bulk-bundle', 'd2-bulk-email', 'd2-bulk-dl-originals', 'd2-bulk-visibility', 'd2-bulk-run-workflow', 'd2-bulk-delete']

test('Dokumentansicht hat alle Aktionen', () => {
  const s = read('DocDetail.svelte')
  expect(DETAIL.filter((id) => !s.includes(`id="${id}"`))).toEqual([])
})
test('Mehrfachauswahl hat alle Aktionen', () => {
  const s = read('BulkBar.svelte')
  expect(BULK.filter((id) => !s.includes(`id="${id}"`))).toEqual([])
})
test('klassische Dialoge haben einen Ausgabeort im Arbeitsbereich', () => {
  expect(read('Workspace.svelte')).toMatch(/classicDialogs\.host/)
})
