// EntriesComponent.writeValue / inputChange (framework-free)
export const entriesFromValue = (v: Record<string, string> | null | undefined): [string, string][] => Object.entries(v ?? {})
/** remove empty keys */
export const valueFromEntries = (entries: [string, string][]): Record<string, string> => Object.fromEntries(entries.filter(([k]) => k?.length))
