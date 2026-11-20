// API v2 /rs/api/v2/settings (docs/SETTINGS.md §3) – only for the "System" tab.
import { api } from '../../../../api/client'
import { getCookie } from '../../../../api/v1'

export const BASE = '/rs/api/v2/settings'

export type SettingType =
  | { kind: 'bool' | 'on_off' | 'str' | 'list' | 'path' | 'url' | 'cron' | 'time_zone' }
  | { kind: 'int'; min: number | null; max: number | null }
  | { kind: 'float'; min: number | null }
  | { kind: 'json'; shape: 'any' | 'object' | 'array' }
  | { kind: 'choice'; choices: string[] }

export interface SettingItem {
  key: string
  /** displayed name (PAPERRUST_X); missing on older servers */
  name?: string
  aliases: string[]
  alias_of: string | null
  category: string
  class: 'A' | 'B' | 'C' | 'D' | 'X'
  effect: 'live' | 'worker' | 'server' | 'once' | 'none'
  reindex: boolean
  secret: boolean
  type: SettingType
  default: unknown
  value: unknown
  is_set: boolean
  unreadable: boolean
  source: 'env' | 'file' | 'db' | 'appconfig' | 'desktop' | 'default'
  source_detail: string | null
  shadowed: { source: string; value: unknown } | null
  locked: boolean
  lock_reason: string | null
  restart_pending: boolean
  django_consumer: boolean
  appconfig_field: string | null
  in_db: boolean
  updated_at: string | null
  updated_by: string | null
  /** Short description (German, as in the registry) */
  help: string
  /** English short description (from API with `help_en`; older servers do not deliver it) */
  help_en?: string
  doc: string
}

export interface SettingsList {
  rev: number
  restart_supported: boolean
  restart_pending_count: number
  mixed_mode: boolean
  items: SettingItem[]
}

export interface OriginItem {
  name: string
  key: string | null
  value: string | null
  secret: boolean
  source: string
  class: string | null
  verdict: 'adoptable' | 'adopted' | 'must_stay' | 'no_effect' | 'obsolete' | 'unknown'
  reason: string | null
}

export interface HistoryRow {
  id: number
  key: string
  old_value: string | null
  new_value: string | null
  action: string
  at: string
  user_name: string | null
}

/** Read v2 error `{"error":{"code","message","fields"}}` from an ApiError. */
export function v2Error(e: any): { code: string; message: string; fields: Record<string, string[]> } {
  const err = e?.body?.error ?? e?.detail?.error ?? {}
  return { code: err.code ?? 'error', message: err.message ?? String(e?.message ?? e), fields: err.fields ?? {} }
}

export const listSettings = () => api.get<SettingsList>(BASE)
export const patchSettings = (values: Record<string, unknown>, expected_rev: number) =>
  api.request<any>('PATCH', BASE, { values, expected_rev })
export const resetSetting = (key: string) => api.request<any>('DELETE', `${BASE}/${encodeURIComponent(key)}`)
export const history = (cursor?: string | null) =>
  api.get<{ items: HistoryRow[]; next_cursor: string | null }>(`${BASE}/history${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`)
export const restart = (password: string) => api.request<any>('POST', `${BASE}/restart`, { password })
export const origin = () => api.get<{ items: OriginItem[]; summary: Record<string, number>; rev: number }>(`${BASE}/origin`)
export const adopt = (body: { keys?: string[]; values?: Record<string, string> }) =>
  api.request<{ adopted: string[]; skipped: { name: string; reason: string | null }[]; rev: number }>('POST', `${BASE}/adopt`, body)
export const exportSettings = () => api.request<any>('POST', `${BASE}/export`, {})
export const importSettings = (data: unknown, passphrase: string | null, dry_run: boolean) =>
  api.request<{ dry_run: boolean; items: { key: string; status: string; current: unknown; new: unknown }[]; applied: number }>(
    'POST',
    `${BASE}/import`,
    { data, passphrase: passphrase || null, dry_run },
  )

/** `POST /settings/check-file` with the file content as text. */
export async function checkFile(text: string): Promise<{ kind: string; service: string | null; env_files: string[]; items: OriginItem[]; slim_env: string }> {
  const csrf = getCookie('csrftoken', typeof document !== 'undefined' ? document.cookie : '')
  const res = await fetch(`${BASE}/check-file`, {
    method: 'POST',
    headers: { 'Content-Type': 'text/plain', Accept: 'application/json', ...(csrf ? { 'X-CSRFToken': csrf } : {}) },
    credentials: 'same-origin',
    body: text,
  })
  if (!res.ok) throw { body: await res.json().catch(() => null), message: res.statusText }
  return res.json()
}

/** Save text as file (download in the browser). */
export function saveText(name: string, text: string, type = 'text/plain') {
  const url = URL.createObjectURL(new Blob([text], { type }))
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
