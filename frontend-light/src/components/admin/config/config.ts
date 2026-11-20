// Logic of admin/config/config.component.ts + ConfigService (framework-free for Vitest).
import { PaperlessConfigOptions, type ConfigOption } from './config-options'

/** getCategorySections: sections in order of first occurrence (null = without section) */
export function categorySections(category: string): (string | null)[] {
  return [...new Set(PaperlessConfigOptions.filter((o) => o.category === category).map((o) => o.section ?? null))]
}

export function categoryOptions(category: string, section: string | null = null): ConfigOption[] {
  return PaperlessConfigOptions.filter((o) => o.category === category && (o.section ?? null) === section)
}

/** Form = id + all option keys (FormGroup controls, values unchanged like patchValue) */
export function configToForm(config: Record<string, any>): Record<string, any> {
  const f: Record<string, any> = { id: config.id }
  for (const o of PaperlessConfigOptions) f[o.key] = config[o.key] === undefined ? null : config[o.key]
  return f
}

/** dirtyCheck (without externally_configured_variables) */
export const isConfigDirty = (form: Record<string, any>, store: Record<string, any>) =>
  Object.keys(store).some((k) => JSON.stringify(form[k] ?? null) !== JSON.stringify(store[k] ?? null))

/** ConfigService.saveConfig: do not send app_logo as text */
export function formToSave(form: Record<string, any>): Record<string, any> {
  const c = { ...form }
  if (typeof c.app_logo === 'string') delete c.app_logo
  return c
}
