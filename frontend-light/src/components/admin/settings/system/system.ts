// "System" tab: framework-free logic (grouping, search, form values ↔ API values).
// docs/SETTINGS.md §4.1.
import type { SettingItem } from './api'
import { locale, msg, t } from '../../../../lib/i18n'

/** Categories in user language (§4.1) → schema categories. Order = display. */
export const GROUPS: { id: string; label: string; categories: string[] }[] = [
  { id: 'general', label: msg('admin.settings.general_appearance'), categories: ['branding'] },
  { id: 'documents', label: msg('admin.settings.documents_file_names'), categories: ['documents'] },
  { id: 'consumer', label: msg('admin.settings.consumption'), categories: ['consumer'] },
  { id: 'barcodes', label: msg('common.barcodes'), categories: ['barcodes'] },
  { id: 'ocr', label: msg('admin.settings.ocr'), categories: ['ocr'] },
  { id: 'mail', label: msg('admin.settings.mail_fetching'), categories: ['mail_fetch'] },
  { id: 'email', label: msg('admin.settings.sending_email'), categories: ['email'] },
  { id: 'auth', label: msg('admin.settings.login_sso'), categories: ['auth'] },
  { id: 'tasks', label: msg('admin.settings.tasks_schedules'), categories: ['tasks'] },
  { id: 'ai', label: msg('common.ai'), categories: ['ai'] },
  { id: 'integrity', label: msg('admin.settings.integrity_sync'), categories: ['integrity'] },
  { id: 'logging', label: msg('common.logs'), categories: ['logging'] },
  { id: 'network', label: msg('admin.settings.network_security'), categories: ['network', 'webhooks'] },
  { id: 'storage', label: msg('admin.settings.storage_database'), categories: ['database', 'paths'] },
  { id: 'desktop', label: msg('admin.settings.desktop'), categories: ['desktop'] },
]

export function groupOf(item: SettingItem): string {
  return GROUPS.find((g) => g.categories.includes(item.category))?.id ?? 'general'
}

/**
 * Label of a setting in the active language: German → `help` (registry), otherwise
 * `help_en` (translatable via the dictionary, key = English text); without `help_en` (older
 * server) it stays with `help`.
 */
export function helpText(item: SettingItem, loc: string = locale()): string {
  if (loc.toLowerCase().split(/[-_]/)[0] === 'de' || !item.help_en) return item.help
  return t(item.help_en)
}

/**
 * Displayed variable name: `name` from the server (PAPERRUST_X), otherwise derived from the key
 * (`PAPERLESS_X` → `PAPERRUST_X`; older servers, history/import with canonical key).
 */
export function displayKey(key: string): string {
  if (key.startsWith('PAPERLESS_RS_')) return 'PAPERRUST_' + key.slice(13)
  if (key.startsWith('PAPERLESS_')) return 'PAPERRUST_' + key.slice(10)
  return key
}
export const displayName = (item: SettingItem): string => item.name ?? displayKey(item.key)

/** Former names that remain valid (without the displayed name). */
export const formerNames = (item: SettingItem): string[] => item.aliases.filter((a) => a !== displayName(item) && a.startsWith('PAPERLESS_'))

/** File name from `source_detail` (path of the configuration file read). */
export function fileName(detail: string | null): string {
  const base = (detail ?? '').split(/[\\/]/).pop()
  return base || 'paperrust.conf'
}

/** Changeable in the UI (A/B, not locked)? */
export function editable(item: SettingItem): boolean {
  return !item.locked && (item.class === 'A' || item.class === 'B' || item.key.startsWith('desktop.'))
}

export interface Filter {
  q: string
  changed: boolean
  restart: boolean
  /** Show bootstrap/environment-only/ineffective */
  readOnly: boolean
}

export function matches(item: SettingItem, f: Filter): boolean {
  if (!f.readOnly && !editable(item) && !item.restart_pending) {
    // locked A/B values (from environment) remain visible
    if (!(item.class === 'A' || item.class === 'B')) return false
  }
  if (f.changed && !(item.source === 'db' || item.source === 'appconfig')) return false
  if (f.restart && !item.restart_pending) return false
  const q = f.q.trim().toLowerCase()
  if (!q) return true
  return (
    item.key.toLowerCase().includes(q) ||
    displayName(item).toLowerCase().includes(q) ||
    item.help.toLowerCase().includes(q) ||
    (item.help_en ?? '').toLowerCase().includes(q) ||
    item.aliases.some((a) => a.toLowerCase().includes(q))
  )
}

/** API value → form value (text for input fields, boolean for switches). */
export function toForm(item: SettingItem): string | boolean {
  const v = item.value
  switch (item.type.kind) {
    case 'bool':
    case 'on_off':
      return v === true
    case 'list':
      return Array.isArray(v) ? v.join(', ') : v == null ? '' : String(v)
    case 'json':
      return v == null ? '' : JSON.stringify(v, null, 2)
    default:
      if (item.secret) return ''
      return v == null ? '' : String(v)
  }
}

export type Parsed = { ok: true; value: unknown } | { ok: false; error: string }

/** Form value → API value (for PATCH). Empty input for secrets = unchanged (`skip`). */
export function fromForm(item: SettingItem, v: string | boolean): Parsed | { ok: true; skip: true } {
  const t = item.type
  if (t.kind === 'bool' || t.kind === 'on_off') return { ok: true, value: v === true }
  const s = String(v ?? '').trim()
  if (item.secret && s === '') return { ok: true, skip: true }
  switch (t.kind) {
    case 'int': {
      if (!/^-?\d+$/.test(s)) return { ok: false, error: 'admin.settings.please_enter_whole_number' }
      const n = Number(s)
      if (t.min != null && n < t.min) return { ok: false, error: `>= ${t.min}` }
      if (t.max != null && n > t.max) return { ok: false, error: `<= ${t.max}` }
      return { ok: true, value: n }
    }
    case 'float': {
      const n = Number(s.replace(',', '.'))
      if (s === '' || !Number.isFinite(n)) return { ok: false, error: 'admin.settings.please_enter_number' }
      if (t.min != null && n < t.min) return { ok: false, error: `>= ${t.min}` }
      return { ok: true, value: n }
    }
    case 'list':
      return { ok: true, value: s.split(',').map((x) => x.trim()).filter(Boolean) }
    case 'json':
      try {
        const j = JSON.parse(s)
        if (t.shape === 'object' && (j === null || typeof j !== 'object' || Array.isArray(j))) return { ok: false, error: 'common.invalid_json' }
        if (t.shape === 'array' && !Array.isArray(j)) return { ok: false, error: 'common.invalid_json' }
        return { ok: true, value: j }
      } catch {
        return { ok: false, error: 'common.invalid_json' }
      }
    case 'choice':
      if (!t.choices.includes(s)) return { ok: false, error: 'admin.settings.invalid_value' }
      return { ok: true, value: s }
    default:
      return { ok: true, value: s }
  }
}

/** Celery crontab in plain text: translation key + placeholders (simple cases only; otherwise `null`). */
export function cronText(expr: string): { text: string; params?: Record<string, string> } | null {
  if (expr.trim() === 'disable') return { text: 'admin.settings.disabled' }
  const p = expr.trim().split(' ')
  if (p.length !== 5) return null
  const [m, h, dom, mon, dow] = p
  const two = (x: string) => x.padStart(2, '0')
  if (/^\d+$/.test(m) && /^\d+$/.test(h) && dom === '*' && mon === '*') {
    const at = `${two(h)}:${two(m)}`
    if (dow === '*') return { text: 'admin.settings.every_day', params: { PH: at } }
    return { text: 'admin.settings.on_at', params: { PH: dow, PH_1: at } }
  }
  if (/^\*\/\d+$/.test(m) && h === '*' && dom === '*' && mon === '*' && dow === '*') return { text: 'admin.settings.every_minutes', params: { PH: m.slice(2) } }
  if (/^\d+$/.test(m) && (h === '*' || h === '*/1') && dom === '*' && mon === '*' && dow === '*') return { text: 'admin.settings.every_hour_minute', params: { PH: m } }
  return null
}

/** Changed fields of a group as PATCH values; errors per key. */
export function collectChanges(
  items: SettingItem[],
  form: Record<string, string | boolean>,
  initial: Record<string, string | boolean>,
): { values: Record<string, unknown>; errors: Record<string, string> } {
  const values: Record<string, unknown> = {}
  const errors: Record<string, string> = {}
  for (const it of items) {
    if (!editable(it)) continue
    if (form[it.key] === initial[it.key]) continue
    const r = fromForm(it, form[it.key])
    if ('error' in r) errors[it.key] = r.error
    else if ('value' in r) values[it.key] = r.value
  }
  return { values, errors }
}
