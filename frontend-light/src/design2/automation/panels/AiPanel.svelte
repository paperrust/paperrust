<!-- AI condition (topic 9 B): "Only if the AI thinks: …, confidence ≥ …" (paperrust_workflow_ext.ai_condition).
     Visible only with AI; if AI is off, a hint is shown here (the condition is then not checked). -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import ObjectPicker from '../../components/ObjectPicker.svelte'
  import type { AiCondition, AiField } from '../api'
  import { aiNameKind } from '../model'
  import { auto } from '../store.svelte'

  let { cond, reason = null, errors = [] }: { cond: AiCondition; reason?: string | null; errors?: string[] } = $props()
  const FIELDS: { k: AiField; label: string }[] = [
    { k: 'document_type', label: 'common.document_type' },
    { k: 'correspondent', label: 'common.correspondent' },
    { k: 'storage_path', label: 'common.storage_path' },
    { k: 'tags', label: 'm.auto.field_tag' },
  ]
  const REASON: Record<string, string> = {
    ai_unavailable: 'm.auto.ai.reason_unavailable',
    ai_disabled: 'm.auto.ai.reason_disabled',
    consumption_only: 'm.auto.ai.reason_consumption',
  }
</script>

<div class="au-form" id="au-ai-panel">
  {#if errors.length}<div class="au-banner danger" role="alert"><ul>{#each errors as e, i (i)}<li>{e}</li>{/each}</ul></div>{/if}
  <p class="au-hint">{t('m.auto.ai.intro')}</p>
  {#if reason && REASON[reason]}<div class="au-banner warn" role="status">{t(REASON[reason])}</div>{/if}
  <div class="au-field">
    <label for="au-ai-field">{t('m.auto.ai.what')}</label>
    <select id="au-ai-field" class="d2-select" value={cond.field} onchange={(e) => {
      cond.field = e.currentTarget.value as AiField
      cond.object_id = 0
    }}>
      {#each FIELDS as f (f.k)}<option value={f.k}>{t(f.label)}</option>{/each}
    </select>
  </div>
  <div class="au-field">
    <span class="lab">{t('m.auto.ai.value')}</span>
    <ObjectPicker id="au-ai-object" label={t('m.auto.ai.value')} options={auto.options(aiNameKind(cond.field))} selected={cond.object_id > 0 ? [cond.object_id] : []} onchange={(n) => (cond.object_id = n[0] ?? 0)} placeholder={t('m.auto.choose')} />
  </div>
  <div class="au-field">
    <label for="au-ai-conf">{t('m.auto.ai.confidence', { PCT: Math.round(cond.min_confidence * 100) })}</label>
    <input id="au-ai-conf" type="range" min="50" max="100" step="5" value={Math.round(cond.min_confidence * 100)} oninput={(e) => (cond.min_confidence = Number(e.currentTarget.value) / 100)} class="au-range" />
    <p class="au-hint">{t('m.auto.ai.confidence_hint')}</p>
  </div>
</div>
