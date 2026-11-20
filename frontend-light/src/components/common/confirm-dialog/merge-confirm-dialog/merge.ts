// Logic of merge-confirm-dialog.component.ts and merge-as-versions-confirm-dialog.component.ts (onDrop).
import { moveItemInArray } from '../../../../lib/cdk-drag'

/** MergeConfirmDialogComponent.onDrop: new order of the document IDs */
export function reorder(ids: number[], from: number, to: number): number[] {
  const next = ids.concat()
  moveItemInArray(next, from, to)
  return next
}

/** MergeAsVersionsConfirmDialogComponent.versionDocumentIDs */
export const versionIds = (ids: number[], rootId: number) => ids.filter((id) => id !== rootId)

/** MergeAsVersionsConfirmDialogComponent.onDrop: the root document keeps its place, only the versions move */
export function reorderVersions(ids: number[], rootId: number, from: number, to: number): number[] {
  const versions = reorder(versionIds(ids, rootId), from, to)
  let i = 0
  return ids.map((id) => (id === rootId ? id : versions[i++]))
}
