<!-- Custom prompts (Settings → AI): edit template per task, insert placeholders by click, save,
     "Reset" to the built-in template and "Test on document" (only view the prompt or try it with AI).
     API: /api/x-ai/prompts/ (superuser). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, msg } from '../../../lib/i18n'
  import { toastService } from '../../../stores/toasts.svelte'
  import Icon from '../../components/Icon.svelte'
  import { listPrompts, previewPrompt, resetPrompt, savePrompt, type PromptInfo } from '../../ai/api'

  const KIND_LABEL: Record<PromptInfo['kind'], [string, string]> = {
    classification: [msg('m.ai_settings.prompt_classification'), msg('m.ai_settings.prompt_classification_hint')],
    title: [msg('m.ai_settings.prompt_title'), msg('m.ai_settings.prompt_title_hint')],
    custom_fields: [msg('m.ai_settings.prompt_custom_fields'), msg('m.ai_settings.prompt_custom_fields_hint')],
    vision_ocr: [msg('m.ai_settings.prompt_vision'), msg('m.ai_settings.prompt_vision_hint')],
    chat_system: [msg('m.ai_settings.prompt_chat'), msg('m.ai_settings.prompt_chat_hint')],
  }
  const PH_LABEL: Record<string, string> = {
    content: msg('m.ai_settings.ph_content'),
    filename: msg('m.ai_settings.ph_filename'),
    candidates: msg('m.ai_settings.ph_candidates'),
    candidate_rules: msg('m.ai_settings.ph_candidate_rules'),
    language: msg('m.ai_settings.ph_language'),
    fields: msg('m.ai_settings.ph_fields'),
    today: msg('m.ai_settings.ph_today'),
  }

  let prompts = $state<PromptInfo[]>([])
  let kind = $state<PromptInfo['kind']>('classification')
  let text = $state('')
  let errors = $state<string[]>([])
  let busy = $state(false)
  let docId = $state<number | null>(null)
  let preview = $state<{ prompt: string; result: unknown; error: string | null } | null>(null)
  let area = $state<HTMLTextAreaElement | null>(null)

  const cur = $derived(prompts.find((p) => p.kind === kind) ?? null)
  const changed = $derived(!!cur && text !== cur.template)
  const needsDoc = $derived(kind === 'classification' || kind === 'title' || kind === 'custom_fields')

  onMount(async () => {
    try {
      prompts = (await listPrompts()).results
      pick('classification')
    } catch {
      toastService.showError(t('m.ai_settings.load_failed'))
    }
  })

  function pick(k: PromptInfo['kind']) {
    kind = k
    text = prompts.find((p) => p.kind === k)?.template ?? ''
    errors = []
    preview = null
  }
  function insert(ph: string) {
    const s = area?.selectionStart ?? text.length
    const e = area?.selectionEnd ?? text.length
    text = text.slice(0, s) + `{{${ph}}}` + text.slice(e)
  }
  function errorText(code: string): string {
    const [c, name] = code.split(':')
    if (c === 'missing') return t('m.ai_settings.err_missing', { PH: `{{${name}}}` })
    if (c === 'unknown') return t('m.ai_settings.err_unknown', { PH: `{{${name}}}` })
    if (c === 'too_long') return t('m.ai_settings.err_too_long')
    return t('m.ai_settings.err_empty')
  }
  function update(p: PromptInfo) {
    prompts = prompts.map((x) => (x.kind === p.kind ? p : x))
    text = p.template
  }
  async function save() {
    busy = true
    errors = []
    try {
      update(await savePrompt(kind, text))
      toastService.show({ content: t('m.ai_settings.prompt_saved'), delay: 3000, level: 'success' })
    } catch (e: any) {
      errors = e?.body?.template ?? []
      if (!errors.length) toastService.showError(t('m.ai_settings.could_not_save'), e)
    } finally {
      busy = false
    }
  }
  async function reset() {
    busy = true
    try {
      update(await resetPrompt(kind))
      errors = []
    } catch (e) {
      toastService.showError(t('m.ai_settings.could_not_save'), e)
    } finally {
      busy = false
    }
  }
  async function test(run: boolean) {
    busy = true
    preview = null
    errors = []
    try {
      preview = await previewPrompt(kind, { document: docId ?? undefined, template: text, run })
    } catch (e: any) {
      errors = e?.body?.template ?? []
      if (!errors.length) toastService.showError(e?.body?.document ? t('m.ai_settings.need_document') : t('m.ai_settings.test_failed'), e)
    } finally {
      busy = false
    }
  }
</script>

<div class="pe">
  <div class="kinds" role="tablist" aria-label={t('m.ai_settings.prompts')}>
    {#each prompts as p (p.kind)}
      <button role="tab" aria-selected={kind === p.kind} class:on={kind === p.kind} onclick={() => pick(p.kind)}>
        {t(KIND_LABEL[p.kind][0])}{#if p.custom}<span class="dot" title={t('m.ai_settings.prompt_custom')}>●</span>{/if}
      </button>
    {/each}
  </div>
  {#if cur}
    <p class="d2-muted">{t(KIND_LABEL[kind][1])}</p>
    {#if cur.placeholders.length}
      <div class="phs">
        <span class="d2-muted small">{t('m.ai_settings.placeholders')}</span>
        {#each cur.placeholders as ph (ph)}
          <button class="d2-chip" onclick={() => insert(ph)} title={PH_LABEL[ph] ? t(PH_LABEL[ph]) : ph}>
            {`{{${ph}}}`}{#if cur.required.includes(ph)}*{/if}
          </button>
        {/each}
      </div>
      <ul class="legend d2-muted small">
        {#each cur.placeholders as ph (ph)}<li><code>{`{{${ph}}}`}</code> – {PH_LABEL[ph] ? t(PH_LABEL[ph]) : ph}</li>{/each}
      </ul>
    {/if}
    <textarea class="d2-textarea" rows="12" bind:this={area} bind:value={text} aria-label={t(KIND_LABEL[kind][0])} spellcheck="false"></textarea>
    {#if errors.length}
      <ul class="errs" role="alert">{#each errors as e (e)}<li>{errorText(e)}</li>{/each}</ul>
    {/if}
    <div class="row">
      <button class="d2-btn primary sm" disabled={busy || !changed} onclick={save}><Icon name="check-lg" />{t('m.ai_settings.prompt_save')}</button>
      <button class="d2-btn ghost sm" disabled={busy || (!cur.custom && !changed)} onclick={reset}><Icon name="arrow-counterclockwise" />{t('m.ai_settings.prompt_reset')}</button>
      {#if cur.custom}<span class="d2-muted small">{t('m.ai_settings.prompt_custom')}</span>{:else}<span class="d2-muted small">{t('m.ai_settings.prompt_builtin')}</span>{/if}
    </div>
    <div class="test">
      <h3>{t('m.ai_settings.try_on_document')}</h3>
      <div class="row">
        {#if needsDoc}
          <label class="d2-label" for="d2-ai-pdoc">{t('m.ai_settings.document_id')}</label>
          <input class="d2-input num" id="d2-ai-pdoc" type="number" min="1" bind:value={docId} />
        {/if}
        <button class="d2-btn sm" disabled={busy || (needsDoc && !docId)} onclick={() => test(false)}><Icon name="card-text" />{t('m.ai_settings.show_prompt')}</button>
        {#if needsDoc}
          <button class="d2-btn sm" disabled={busy || !docId} onclick={() => test(true)}><Icon name="stars" />{t('m.ai_settings.run_prompt')}</button>
        {/if}
      </div>
      {#if preview}
        <pre class="out" aria-label={t('m.ai_settings.show_prompt')}>{preview.prompt}</pre>
        {#if preview.error}<p class="err">{t('m.ai_settings.test_failed')} ({preview.error})</p>{/if}
        {#if preview.result !== null && preview.result !== undefined}
          <h3>{t('m.ai_settings.ai_answer')}</h3>
          <pre class="out">{typeof preview.result === 'string' ? preview.result : JSON.stringify(preview.result, null, 2)}</pre>
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .pe {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
  }
  .kinds {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s1);
  }
  .kinds button {
    padding: 4px 10px;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r1);
    background: var(--d2-surface);
    color: var(--d2-text-2);
    font: inherit;
    cursor: pointer;
  }
  .kinds button.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  .dot {
    margin-left: 4px;
    color: var(--d2-accent-text);
    font-size: 0.6rem;
  }
  .phs {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s1);
    align-items: center;
  }
  .phs .d2-chip {
    cursor: pointer;
    font-family: ui-monospace, monospace;
  }
  .legend {
    margin: 0;
    padding-left: var(--d2-s4);
  }
  textarea {
    font-family: ui-monospace, monospace;
    font-size: var(--d2-fs-sm);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s2);
    align-items: center;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
  .errs,
  .err {
    color: var(--d2-danger);
    margin: 0;
  }
  .test {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    padding-top: var(--d2-s3);
    border-top: 1px solid var(--d2-border);
  }
  h3 {
    font-size: var(--d2-fs-md);
  }
  .num {
    max-width: 120px;
  }
  .out {
    max-height: 320px;
    overflow: auto;
    margin: 0;
    padding: var(--d2-s3);
    border-radius: var(--d2-r1);
    background: var(--d2-surface-2);
    white-space: pre-wrap;
    font-size: var(--d2-fs-xs);
  }
</style>
