// CustomFieldsValuesComponent.selectedFields setter: { fieldId: value } only for selected fields (missing = null)
export function valuesForSelected(selected: number[], value: Record<string, any> | null | undefined): Record<string, any> {
  return selected.reduce((acc: Record<string, any>, id) => {
    acc[id] = value?.[id] ?? null
    return acc
  }, {})
}
