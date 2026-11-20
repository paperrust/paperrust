// Settings → AI (PaperRust design): mapping form ↔ settings registry (/rs/api/v2/settings, docs/SETTINGS.md
// §1.15). Pure functions (Vitest: model.test.ts). The API key is never displayed; empty = unchanged.
import type { SettingItem } from '../../../components/admin/settings/system/api'

/** Registry keys (with aliases PAPERRUST_AI_* ↔ PAPERLESS_AI_*) */
export const KEYS = {
  enabled: 'PAPERLESS_AI_ENABLED',
  backend: 'PAPERLESS_AI_LLM_BACKEND',
  endpoint: 'PAPERLESS_AI_LLM_ENDPOINT',
  model: 'PAPERLESS_AI_LLM_MODEL',
  apiKey: 'PAPERLESS_AI_LLM_API_KEY',
  language: 'PAPERLESS_AI_LLM_OUTPUT_LANGUAGE',
  autoMode: 'PAPERRUST_AI_AUTO_MODE',
  minConfidence: 'PAPERRUST_AI_AUTO_MIN_CONFIDENCE',
  triggerTag: 'PAPERRUST_AI_AUTO_TRIGGER_TAG',
  maxNew: 'PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS',
  maxDocs: 'PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN',
  createMissing: 'PAPERRUST_AI_AUTO_CREATE_MISSING',
} as const
export type FormKey = keyof typeof KEYS

export interface AiForm {
  enabled: boolean
  backend: 'ollama' | 'openai-like'
  endpoint: string
  model: string
  apiKey: string
  language: string
  autoMode: 'off' | 'suggest' | 'apply'
  minConfidence: number
  triggerTag: string
  maxNew: number
  maxDocs: number
  createMissing: boolean
}

export const DEFAULT_ENDPOINT: Record<AiForm['backend'], string> = {
  ollama: 'http://localhost:11434',
  'openai-like': 'https://api.openai.com/v1',
}

/** Entry for a key (also via display name/alias). */
export function findItem(items: SettingItem[], key: string): SettingItem | undefined {
  const alt = key.startsWith('PAPERLESS_') ? key.replace('PAPERLESS_', 'PAPERRUST_') : key.replace('PAPERRUST_', 'PAPERLESS_')
  return items.find((i) => i.key === key || i.name === key || i.aliases?.includes(key) || i.key === alt || i.aliases?.includes(alt))
}

const truthy = (v: unknown) => v === true || (typeof v === 'string' && ['1', 'true', 'yes', 'y', 't', 'on'].includes(v.trim().toLowerCase()))
const num = (v: unknown, d: number) => {
  const n = typeof v === 'number' ? v : typeof v === 'string' && v.trim() !== '' ? Number(v) : NaN
  return Number.isFinite(n) ? n : d
}
const str = (v: unknown) => (v === null || v === undefined ? '' : String(v))

/** Form from the registry. */
export function formFrom(items: SettingItem[]): AiForm {
  const v = (k: FormKey) => findItem(items, KEYS[k])?.value
  const backend = str(v('backend')) === 'openai-like' ? 'openai-like' : 'ollama'
  const mode = str(v('autoMode')).toLowerCase()
  return {
    enabled: truthy(v('enabled')),
    backend,
    endpoint: str(v('endpoint')),
    model: str(v('model')),
    apiKey: '',
    language: str(v('language')),
    autoMode: mode === 'suggest' || mode === 'apply' ? mode : 'off',
    minConfidence: num(v('minConfidence'), 0.9),
    triggerTag: str(v('triggerTag')),
    maxNew: num(v('maxNew'), 3),
    maxDocs: num(v('maxDocs'), 50),
    createMissing: truthy(v('createMissing')),
  }
}

/** Is a value locked (e.g. set via environment variable)? */
export function locked(items: SettingItem[], k: FormKey): string | null {
  const it = findItem(items, KEYS[k])
  return it?.locked ? (it.lock_reason ?? 'locked') : null
}

/** Is a secret stored? */
export function secretStored(items: SettingItem[], k: FormKey): boolean {
  return !!findItem(items, KEYS[k])?.is_set
}

/**
 * Changes as PATCH values (registry key → JSON). Only changed ones, nothing locked; empty texts → null
 * (reset); API key only if entered.
 */
export function patchValues(before: AiForm, after: AiForm, items: SettingItem[]): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const k of Object.keys(KEYS) as FormKey[]) {
    if (k === 'apiKey') continue
    if (before[k] === after[k] || locked(items, k)) continue
    const key = findItem(items, KEYS[k])?.key ?? KEYS[k]
    const val = after[k]
    out[key] = typeof val === 'string' ? (val.trim() === '' ? null : val.trim()) : val
  }
  if (after.apiKey.trim() && !locked(items, 'apiKey')) out[findItem(items, KEYS.apiKey)?.key ?? KEYS.apiKey] = after.apiKey.trim()
  return out
}

/** Confidence threshold as an understandable level */
export const CONFIDENCE_STEPS = [
  { v: 0.95, key: 'm.ai_settings.conf_very_sure' },
  { v: 0.9, key: 'm.ai_settings.conf_sure' },
  { v: 0.75, key: 'm.ai_settings.conf_likely' },
] as const

/** Connection error code (server) → translation key */
export function checkErrorKey(code: string | null): string {
  switch (code) {
    case 'timeout':
      return 'm.ai_settings.err_timeout'
    case 'unauthorized':
      return 'm.ai_settings.err_unauthorized'
    case 'not_found':
      return 'm.ai_settings.err_not_found'
    case 'blocked_address':
      return 'm.ai_settings.err_blocked'
    case 'unresolvable':
    case 'unreachable':
      return 'm.ai_settings.err_unreachable'
    default:
      return 'm.ai_settings.err_other'
  }
}

/** Known image models (for AI text recognition) – suggestion when the provider reports them. */
export function visionCandidates(models: string[]): string[] {
  return models.filter((m) => /(llava|minicpm-v|qwen2(\.5)?-?vl|qwen3-vl|llama3\.2-vision|gemma3|bakllava|moondream|gpt-4o|gpt-4\.1|gpt-5|pixtral|mistral-small3)/i.test(m))
}
