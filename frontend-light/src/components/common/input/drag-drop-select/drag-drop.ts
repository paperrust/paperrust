// Logic of common/input/drag-drop-select/drag-drop-select.component.ts (drop) – framework-free for Vitest.
export interface DdItem {
  id: string
  name: string
}

/** writeValue: IDs → entries (unknown IDs are dropped) */
export function selectedFromValue(items: DdItem[], value: string[] | null | undefined): DdItem[] {
  return (value ?? []).map((id) => items.find((i) => i.id === id)).filter((i): i is DdItem => !!i)
}

export const unselected = (items: DdItem[], selected: DdItem[]) => items.filter((i) => !selected.includes(i))

/** CdkDragDrop between/within the lists 'selected' and 'unselected' → new selection */
export function dropItem(
  items: DdItem[],
  selected: DdItem[],
  from: 'selected' | 'unselected',
  to: 'selected' | 'unselected',
  previousIndex: number,
  currentIndex: number,
): DdItem[] {
  const s = [...selected]
  if (from === to && to === 'selected') {
    const [m] = s.splice(previousIndex, 1)
    s.splice(Math.min(currentIndex, s.length), 0, m)
  } else if (to === 'selected') {
    s.splice(currentIndex, 0, unselected(items, selected)[previousIndex])
  } else if (to === 'unselected' && from === 'selected') {
    s.splice(previousIndex, 1)
  }
  return s.filter(Boolean)
}
