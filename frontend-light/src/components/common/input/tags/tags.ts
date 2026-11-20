// Logic from common/input/tags/tags.component.ts (hierarchy: add parents too, remove children too).
import type { Tag } from '../../../../api/v1'

type T = Tag & { children?: Tag[] }

/** getParentChain: ancestors from the root to the direct parent */
export function parentChain(id: number, tags: T[]): T[] {
  const chain: T[] = []
  let current = tags.find((t) => t.id == id)
  const guard = new Set<number>()
  while (current?.parent) {
    if (guard.has(current.parent)) break
    guard.add(current.parent)
    const parent = tags.find((t) => t.id == current!.parent)
    if (!parent) break
    chain.unshift(parent)
    current = parent
  }
  return chain
}

/** onAdd with preserveHierarchy: add all parents recursively */
export function addWithParents(value: number[], id: number, tags: T[]): number[] {
  let out = value.includes(id) ? [...value] : [...value, id]
  let tag = tags.find((t) => t.id == id)
  while (tag?.parent) {
    const parent = tags.find((t) => t.id == tag!.parent)
    if (!parent || out.includes(parent.id)) break
    out = [...out, parent.id]
    tag = parent
  }
  return out
}

/** removeTag with preserveHierarchy: also remove children (recursively) */
export function removeWithChildren(value: number[], id: number, tags: T[]): number[] {
  if (!value.includes(id)) return value
  let out = value.filter((x) => x !== id)
  const strip = (tag: T | undefined) => {
    for (const child of tag?.children ?? []) {
      out = out.filter((x) => x !== child.id)
      strip(child as T)
    }
  }
  strip(tags.find((t) => t.id == id))
  return out
}
