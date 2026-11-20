/** SelectComponent.checkForPrivateItems: add unknown selected IDs as "Private" entries. */
export function withPrivateItems<T extends { id: any }>(items: T[], value: any, privateLabel: string): (T | { id: any; name: string; private: true })[] {
  const ids = value == null ? [] : Array.isArray(value) ? value : [value]
  const missing = ids.filter((id) => id != null && !items.some((i) => i.id === id))
  return missing.length ? [...items, ...missing.map((id) => ({ id, name: privateLabel, private: true as const }))] : items
}
