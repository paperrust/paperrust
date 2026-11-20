// Manual workflow run (PaperRust; idea from paperless-ngx dev #14704, own implementation).
// Loads the active workflows with trigger "Manual" – only with the permission "View workflows"
// (the server requires it when starting as well). Error → empty list (menu item stays hidden).
import { api } from '../api/client'
import type { ManualWorkflow } from '../api/v1'

export async function loadManualWorkflows(canViewWorkflows: boolean): Promise<ManualWorkflow[]> {
  if (!canViewWorkflows) return []
  try {
    return (await api.listManualWorkflows()).results ?? []
  } catch {
    return []
  }
}

/**
 * May the user start a workflow by hand on the selection? Only with the permission "Change documents"
 * and if he owns every selected document (superuser/document without owner count as owned) –
 * edit permission alone is not enough, because workflow actions can change owner and permissions.
 * The server checks the same (bulk_edit run_workflow only for owners). Selected documents that are not
 * loaded count as "not certainly owned" (except for superusers).
 */
export function canRunManualWorkflow(opts: {
  canChange: boolean
  selectedIds: Iterable<number>
  docs: readonly { id: number; owner?: number | null }[]
  owns: (d: { owner?: number | null }) => boolean
  isSuperuser?: boolean
  workflowCount: number
}): boolean {
  if (!opts.canChange || opts.workflowCount === 0) return false
  if (opts.isSuperuser) return true
  const byId = new Map(opts.docs.map((d) => [d.id, d]))
  let any = false
  for (const id of opts.selectedIds) {
    any = true
    const d = byId.get(id)
    if (!d || !opts.owns(d)) return false
  }
  return any
}
