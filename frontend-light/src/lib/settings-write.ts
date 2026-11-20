// Write side of SettingsService (set/storeSettings) as pure functions (Vitest).
const UNSAFE = new Set(['__proto__', 'constructor', 'prototype'])

/** SettingsService.set: "general-settings:a:b-c" → settings.a.b_c = value (verschachtelt, in place) */
export function setSettingValue(settings: Record<string, any>, key: string, value: unknown): void {
  let obj: any = settings
  const parts = key.replace('general-settings:', '').split(':')
  parts.forEach((p, i) => {
    p = p.replace(/-/g, '_')
    if (UNSAFE.has(p)) return
    if (!Object.prototype.hasOwnProperty.call(obj, p)) obj[p] = {}
    if (i == parts.length - 1) obj[p] = value
    else obj = obj[p]
  })
}

/** Deep copy (UI settings are JSON) */
export const cloneSettings = <T>(s: T): T => JSON.parse(JSON.stringify(s ?? {}))
