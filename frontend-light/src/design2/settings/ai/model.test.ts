// Vitest: Settings → AI (design2/settings/ai/model.ts)
import { describe, expect, it } from 'vitest'
import { checkErrorKey, findItem, formFrom, patchValues, visionCandidates } from './model'

const item = (key: string, value: unknown, extra: Record<string, unknown> = {}) => ({ key, name: key.replace('PAPERLESS_', 'PAPERRUST_'), aliases: [], value, locked: false, is_set: value !== null, ...extra }) as any

describe('Einstellungen KI', () => {
  const items = [
    item('PAPERLESS_AI_ENABLED', true),
    item('PAPERLESS_AI_LLM_BACKEND', 'openai-like'),
    item('PAPERLESS_AI_LLM_MODEL', 'gpt-4o-mini'),
    item('PAPERLESS_AI_LLM_API_KEY', '********', { is_set: true }),
    item('PAPERRUST_AI_AUTO_MODE', 'suggest'),
    item('PAPERRUST_AI_AUTO_MIN_CONFIDENCE', '0.95'),
    item('PAPERLESS_AI_LLM_ENDPOINT', null, { locked: true, lock_reason: 'env' }),
  ]
  it('Formular aus dem Register (Aliasse, Standardwerte, Schlüssel nie angezeigt)', () => {
    expect(findItem(items, 'PAPERRUST_AI_ENABLED')?.key).toBe('PAPERLESS_AI_ENABLED')
    const f = formFrom(items)
    expect(f.enabled).toBe(true)
    expect(f.backend).toBe('openai-like')
    expect(f.apiKey).toBe('')
    expect(f.autoMode).toBe('suggest')
    expect(f.minConfidence).toBe(0.95)
    expect(f.maxDocs).toBe(50)
    expect(formFrom([]).autoMode).toBe('off')
  })
  it('nur Geändertes, nichts Gesperrtes, leer = zurücksetzen, Schlüssel nur wenn eingegeben', () => {
    const a = formFrom(items)
    const b = { ...a, model: ' llama3.1 ', endpoint: 'http://x', triggerTag: '', autoMode: 'apply' as const }
    expect(patchValues(a, b, items)).toEqual({ PAPERLESS_AI_LLM_MODEL: 'llama3.1', PAPERRUST_AI_AUTO_MODE: 'apply' })
    expect(patchValues(a, { ...a, apiKey: 'sk-1' }, items)).toEqual({ PAPERLESS_AI_LLM_API_KEY: 'sk-1' })
    expect(patchValues({ ...a, triggerTag: 'KI' }, a, items)).toEqual({ PAPERRUST_AI_AUTO_TRIGGER_TAG: null })
  })
  it('Fehlertexte und Bildmodelle', () => {
    expect(checkErrorKey('unauthorized')).toBe('m.ai_settings.err_unauthorized')
    expect(checkErrorKey('whatever')).toBe('m.ai_settings.err_other')
    expect(visionCandidates(['llama3.1', 'llava:13b', 'qwen2.5vl:7b', 'minicpm-v', 'gpt-4o'])).toEqual(['llava:13b', 'qwen2.5vl:7b', 'minicpm-v', 'gpt-4o'])
  })
})
