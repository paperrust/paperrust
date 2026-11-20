// Logic of CustomFieldsBulkEditDialogComponent (initForm, form.value) without Angular forms.

/** initForm: remove controls of fields no longer selected, create new ones with null (existing values stay) */
export function keepValues(values: Record<string, unknown>, ids: number[]): Record<string, unknown> {
  const next: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(values)) if (ids.includes(parseInt(k))) next[k] = v
  for (const id of ids) if (!(String(id) in next)) next[String(id)] = null
  return next
}

/** form.value: {"<field ID>": value} for the selected fields */
export function formValue(values: Record<string, unknown>, ids: number[]): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(values)) if (ids.includes(parseInt(k))) out[k] = v === undefined ? null : v
  return out
}
