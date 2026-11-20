import { describe, expect, it } from 'vitest'
import { reorder, reorderVersions, versionIds } from './merge'

describe('merge dialogs (onDrop)', () => {
  it('MergeConfirmDialog: Reihenfolge verschieben ohne Original zu ändern', () => {
    const ids = [1, 2, 3, 4]
    expect(reorder(ids, 0, 2)).toEqual([2, 3, 1, 4])
    expect(reorder(ids, 3, 0)).toEqual([4, 1, 2, 3])
    expect(ids).toEqual([1, 2, 3, 4])
  })

  it('MergeAsVersions: Stammdokument bleibt an seinem Platz', () => {
    expect(versionIds([5, 6, 7], 6)).toEqual([5, 7])
    // Versions [5, 7, 8] → 8 moved to the front → [8, 5, 7]; root 6 stays at index 1
    expect(reorderVersions([5, 6, 7, 8], 6, 2, 0)).toEqual([8, 6, 5, 7])
    expect(reorderVersions([5, 6, 7], 5, 1, 0)).toEqual([5, 7, 6])
  })
})
