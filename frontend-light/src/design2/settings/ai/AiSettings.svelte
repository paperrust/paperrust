<!-- Settings → AI (PaperRust design): one main switch, provider with connection test and model list, embeddings
     and the state of the AI index (system status, update via task "llm_index"), what may happen automatically, custom fields via AI, AI text recognition, "Advanced", custom prompts and "Reprocess
     archive". Everything is optional and off by default. Saving: settings registry (/rs/api/v2/settings)
     + AI options (/api/x-ai/options/). Administrators (superusers) only. -->
<script lang="ts">
  import { onMount, type Component } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { router } from '../../../router.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import Icon from '../../components/Icon.svelte'
  import { listSettings, patchSettings, v2Error, type SettingItem } from '../../../components/admin/settings/system/api'
  import { checkProvider, getOptions, patchOptions, type AiOptions, type ConnectionCheck, type FieldInfo } from '../../ai/api'
  import { aiStatus } from '../../ai/status.svelte'
  import { systemStatus } from '../../../api/objects'
  import { runTask } from '../../../api/admin'
  import { checkErrorKey, CONFIDENCE_STEPS, DEFAULT_EMBEDDING_MODEL, DEFAULT_ENDPOINT, EMBEDDING_BACKENDS, formFrom, indexInfo, locked, patchValues, secretStored, visionCandidates, type AiForm, type IndexInfo } from './model'

  let items = $state<SettingItem[]>([])
  let rev = 0
  let form = $state<AiForm | null>(null)
  let saved = $state<AiForm | null>(null)
  let opts = $state<AiOptions | null>(null)
  let savedOpts = $state('')
  let fields = $state<FieldInfo[]>([])
  let loadError = $state(false)
  let saving = $state(false)
  let checking = $state(false)
  let check = $state<ConnectionCheck | null>(null)
  let replaceTag = $state('')
  let Prompts = $state<Component<any> | null>(null)
  let Backfill = $state<Component<any> | null>(null)
  let showPrompts = $state(false)
  let showBackfill = $state(false)
  // AI index (semantic search): state from the system status, update via the task "llm_index" (superusers)
  let index = $state<IndexInfo | null>(null)
  let indexing = $state(false)
  const EMB_ENDPOINT: Record<string, string> = { ollama: 'http://localhost:11434', 'openai-like': 'https://api.openai.com/v1', huggingface: 'http://localhost:8080' }
  const EMB_LABEL: Record<string, string> = { ollama: 'm.ai_settings.ollama', 'openai-like': 'm.ai_settings.openai', huggingface: 'm.ai_settings.emb_huggingface' }
  const INDEX_LABEL: Record<IndexInfo['state'], string> = {
    ok: 'm.ai_settings.index_ok',
    warning: 'm.ai_settings.index_never',
    error: 'm.ai_settings.index_error',
    disabled: 'm.ai_settings.index_disabled',
    unknown: 'm.ai_settings.index_unknown',
  }

  const isAdmin = $derived(!!aiStatus.data?.can_configure)

  async function load() {
    loadError = false
    try {
      const [s, o] = await Promise.all([listSettings(), getOptions()])
      items = s.items
      rev = s.rev
      form = formFrom(s.items)
      saved = { ...form }
      opts = o.options
      savedOpts = JSON.stringify(o.options)
      fields = o.custom_fields
      replaceTag = o.options.tag_after.action === 'replace' ? o.options.tag_after.tag : ''
    } catch {
      loadError = true
    }
  }
  function loadIndex() {
    systemStatus()
      .then((s) => (index = indexInfo(s)))
      .catch(() => (index = null))
  }
  async function updateIndex() {
    indexing = true
    try {
      await runTask('llm_index')
      toastService.showInfo(t('m.ai_settings.index_started'))
      loadIndex()
    } catch (e) {
      toastService.showError(t('m.ai_settings.index_start_failed'), e)
    } finally {
      indexing = false
    }
  }
  onMount(() => {
    if (!aiStatus.data) aiStatus.refresh()
    load()
    loadIndex()
  })
  $effect(() => {
    if (showPrompts && !Prompts) import('./PromptEditor.svelte').then((m) => (Prompts = m.default))
    if (showBackfill && !Backfill) import('./Backfill.svelte').then((m) => (Backfill = m.default))
  })

  const lock = (k: Parameters<typeof locked>[1]) => locked(items, k)
  const dirty = $derived(!!form && !!saved && !!opts && (Object.keys(patchValues(saved, form, items)).length > 0 || JSON.stringify(opts) !== savedOpts))

  async function testConnection() {
    if (!form) return
    checking = true
    check = null
    try {
      check = await checkProvider({ backend: form.backend, endpoint: form.endpoint || undefined, model: form.model || undefined, api_key: form.apiKey || undefined })
    } catch {
      check = { ok: false, models: [], model_available: null, error: 'other' }
    } finally {
      checking = false
    }
  }

  async function save() {
    if (!form || !saved || !opts) return
    saving = true
    try {
      if (opts.tag_after.action === 'replace') opts.tag_after = { action: 'replace', tag: replaceTag }
      const values = patchValues(saved, form, items)
      if (Object.keys(values).length) await patchSettings(values, rev)
      if (JSON.stringify(opts) !== savedOpts) await patchOptions(opts)
      toastService.show({ content: t('m.ai_settings.saved'), delay: 4000, level: 'success' })
      await load()
      aiStatus.refresh()
    } catch (e) {
      const err = v2Error(e)
      toastService.showError(t('m.ai_settings.could_not_save'), { message: err.message })
    } finally {
      saving = false
    }
  }

  function toggleField(id: number, on: boolean) {
    if (!opts) return
    opts.custom_fields = on ? [...new Set([...opts.custom_fields, id])] : opts.custom_fields.filter((x) => x !== id)
  }
  const visionModels = $derived(check ? visionCandidates(check.models) : [])
</script>

<div class="ai-set" id="d2-ai-settings">
  {#if aiStatus.data && !isAdmin}
    <section class="card">
      <h2><Icon name="stars" />{t('m.ai_settings.title')}</h2>
      <p class="d2-muted">{aiStatus.data.enabled ? t('m.ai_settings.on_admin_only') : t('m.ai_settings.off_admin_only')}</p>
    </section>
  {:else if loadError}
    <section class="card">
      <p>{t('m.ai_settings.load_failed')}</p>
      <button class="d2-btn" onclick={load}>{t('m.ai.reload')}</button>
    </section>
  {:else if !form || !opts}
    <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
  {:else}
    <section class="card main-switch" aria-labelledby="d2-ai-h-main">
      <h2 id="d2-ai-h-main"><Icon name="stars" />{t('m.ai_settings.title')}</h2>
      <label class="switch">
        <input type="checkbox" bind:checked={form.enabled} disabled={!!lock('enabled')} id="d2-ai-enabled" />
        <span><strong>{t('m.ai_settings.use_ai')}</strong><br /><span class="d2-muted">{t('m.ai_settings.use_ai_hint')}</span></span>
      </label>
      <p class="d2-muted small"><Icon name="shield-check" /> {t('m.ai_settings.privacy')}</p>
      {#if lock('enabled')}<p class="d2-muted small">{t('m.ai_settings.locked')}</p>{/if}
    </section>

    {#if form.enabled}
      <section class="card" aria-labelledby="d2-ai-h-provider">
        <h2 id="d2-ai-h-provider">{t('m.ai_settings.provider')}</h2>
        <div class="choices" role="radiogroup" aria-labelledby="d2-ai-h-provider">
          <label class="choice" class:on={form.backend === 'ollama'}>
            <input type="radio" name="d2-ai-backend" value="ollama" bind:group={form.backend} disabled={!!lock('backend')} />
            <span class="ttl">{t('m.ai_settings.ollama')}</span>
            <span class="d2-muted">{t('m.ai_settings.ollama_hint')}</span>
          </label>
          <label class="choice" class:on={form.backend === 'openai-like'}>
            <input type="radio" name="d2-ai-backend" value="openai-like" bind:group={form.backend} disabled={!!lock('backend')} />
            <span class="ttl">{t('m.ai_settings.openai')}</span>
            <span class="d2-muted">{t('m.ai_settings.openai_hint')}</span>
          </label>
        </div>
        <div class="grid2">
          <label class="d2-label" for="d2-ai-endpoint">{t('m.ai_settings.address')}</label>
          <input class="d2-input" id="d2-ai-endpoint" bind:value={form.endpoint} placeholder={DEFAULT_ENDPOINT[form.backend]} disabled={!!lock('endpoint')} />
          {#if form.backend === 'openai-like'}
            <label class="d2-label" for="d2-ai-key">{t('m.ai_settings.api_key')}</label>
            <input class="d2-input" id="d2-ai-key" type="password" autocomplete="off" bind:value={form.apiKey} placeholder={secretStored(items, 'apiKey') ? t('m.ai_settings.key_stored') : ''} disabled={!!lock('apiKey')} />
          {/if}
          <label class="d2-label" for="d2-ai-model">{t('m.ai_settings.model')}</label>
          <div class="row">
            <input class="d2-input" id="d2-ai-model" list="d2-ai-models" bind:value={form.model} placeholder={form.backend === 'ollama' ? 'llama3.1' : 'gpt-4o-mini'} disabled={!!lock('model')} />
            <datalist id="d2-ai-models">{#each check?.models ?? [] as m (m)}<option value={m}></option>{/each}</datalist>
            <button class="d2-btn" onclick={testConnection} disabled={checking} id="d2-ai-test"><Icon name="arrow-repeat" />{t('m.ai_settings.test')}</button>
          </div>
        </div>
        {#if check}
          <p class="result" class:ok={check.ok} role="status">
            {#if check.ok}
              <Icon name="check-circle" />{t('m.ai_settings.test_ok', { PH: check.models.length })}
              {#if check.model_available === false}<br /><span class="warn">{t('m.ai_settings.model_missing')}</span>{/if}
            {:else}
              <Icon name="x-circle" />{t(checkErrorKey(check.error))}
            {/if}
          </p>
        {/if}
      </section>

      <section class="card" aria-labelledby="d2-ai-h-emb" id="d2-ai-embeddings">
        <h2 id="d2-ai-h-emb">{t('m.ai_settings.embeddings')}</h2>
        <p class="d2-muted">{t('m.ai_settings.embeddings_hint')}</p>
        <div class="grid2">
          <label class="d2-label" for="d2-ai-emb-backend">{t('m.ai_settings.emb_backend')}</label>
          <select class="d2-select" id="d2-ai-emb-backend" bind:value={form.embBackend} disabled={!!lock('embBackend')}>
            <option value="">{t('m.ai_settings.emb_off')}</option>
            {#each EMBEDDING_BACKENDS as b (b)}<option value={b}>{t(EMB_LABEL[b])}</option>{/each}
          </select>
          {#if form.embBackend}
            <label class="d2-label" for="d2-ai-emb-endpoint">{t('m.ai_settings.address')}</label>
            <input class="d2-input" id="d2-ai-emb-endpoint" bind:value={form.embEndpoint} placeholder={EMB_ENDPOINT[form.embBackend]} disabled={!!lock('embEndpoint')} />
            {#if form.embBackend !== 'ollama'}
              <label class="d2-label" for="d2-ai-emb-key">{t('m.ai_settings.api_key')}</label>
              <input class="d2-input" id="d2-ai-emb-key" type="password" autocomplete="off" bind:value={form.embApiKey} placeholder={secretStored(items, 'embApiKey') ? t('m.ai_settings.key_stored') : ''} disabled={!!lock('embApiKey')} />
            {/if}
            <label class="d2-label" for="d2-ai-emb-model">{t('m.ai_settings.model')}</label>
            <input class="d2-input" id="d2-ai-emb-model" bind:value={form.embModel} placeholder={DEFAULT_EMBEDDING_MODEL[form.embBackend]} disabled={!!lock('embModel')} />
          {/if}
        </div>
        {#if lock('embBackend') || lock('embEndpoint') || lock('embModel') || lock('embApiKey')}<p class="d2-muted small">{t('m.ai_settings.locked')}</p>{/if}
        {#if index}
          <div class="index" id="d2-ai-index">
            <span class="d2-label">{t('m.ai_settings.index')}</span>
            <span class="istate {index.state}" role="status">
              {t(INDEX_LABEL[index.state])}{#if index.lastRun && index.state !== 'disabled'} · {t('m.ai_settings.index_last_run', { PH: new Date(index.lastRun).toLocaleString() })}{/if}
            </span>
            {#if index.state === 'error' && index.error}<span class="d2-muted small">{index.error}</span>{/if}
            <div class="row">
              <button class="d2-btn" onclick={updateIndex} disabled={indexing || index.state === 'disabled'} id="d2-ai-index-run"><Icon name="arrow-repeat" />{t('m.ai_settings.index_update')}</button>
              <span class="d2-muted small">{t('m.ai_settings.index_update_hint')}</span>
            </div>
          </div>
        {/if}
      </section>

      <section class="card" aria-labelledby="d2-ai-h-auto">
        <h2 id="d2-ai-h-auto">{t('m.ai_settings.automatic')}</h2>
        <p class="d2-muted">{t('m.ai_settings.automatic_hint')}</p>
        <div class="choices three" role="radiogroup" aria-labelledby="d2-ai-h-auto">
          <label class="choice" class:on={form.autoMode === 'off'}>
            <input type="radio" name="d2-ai-mode" value="off" bind:group={form.autoMode} disabled={!!lock('autoMode')} />
            <span class="ttl">{t('m.ai_settings.mode_off')}</span><span class="d2-muted">{t('m.ai_settings.mode_off_hint')}</span>
          </label>
          <label class="choice" class:on={form.autoMode === 'suggest'}>
            <input type="radio" name="d2-ai-mode" value="suggest" bind:group={form.autoMode} disabled={!!lock('autoMode')} />
            <span class="ttl">{t('m.ai_settings.mode_suggest')}</span><span class="d2-muted">{t('m.ai_settings.mode_suggest_hint')}</span>
          </label>
          <label class="choice" class:on={form.autoMode === 'apply'}>
            <input type="radio" name="d2-ai-mode" value="apply" bind:group={form.autoMode} disabled={!!lock('autoMode')} />
            <span class="ttl">{t('m.ai_settings.mode_apply')}</span><span class="d2-muted">{t('m.ai_settings.mode_apply_hint')}</span>
          </label>
        </div>
        {#if form.autoMode === 'apply'}
          <div class="grid2">
            <label class="d2-label" for="d2-ai-conf">{t('m.ai_settings.apply_when')}</label>
            <select class="d2-select" id="d2-ai-conf" bind:value={form.minConfidence} disabled={!!lock('minConfidence')}>
              {#each CONFIDENCE_STEPS as s (s.v)}<option value={s.v}>{t(s.key)}</option>{/each}
            </select>
          </div>
        {/if}
        <label class="switch">
          <input type="checkbox" bind:checked={form.createMissing} disabled={!!lock('createMissing')} />
          <span>{t('m.ai_settings.create_missing')}<br /><span class="d2-muted">{t('m.ai_settings.create_missing_hint')}</span></span>
        </label>
        <div class="grid2">
          <label class="d2-label" for="d2-ai-tag">{t('m.ai_settings.trigger_tag')}</label>
          <div>
            <input class="d2-input" id="d2-ai-tag" bind:value={form.triggerTag} placeholder={t('m.ai_settings.trigger_tag_placeholder')} disabled={!!lock('triggerTag')} />
            <p class="d2-muted small">{t('m.ai_settings.trigger_tag_hint')}</p>
          </div>
          {#if form.triggerTag.trim()}
            <label class="d2-label" for="d2-ai-tagafter">{t('m.ai_settings.after_processing')}</label>
            <div class="row">
              <select class="d2-select" id="d2-ai-tagafter" value={opts.tag_after.action} onchange={(e) => (opts!.tag_after = { action: (e.currentTarget as HTMLSelectElement).value as any, tag: replaceTag } as any)}>
                <option value="remove">{t('m.ai_settings.tag_remove')}</option>
                <option value="replace">{t('m.ai_settings.tag_replace')}</option>
                <option value="keep">{t('m.ai_settings.tag_keep')}</option>
              </select>
              {#if opts.tag_after.action === 'replace'}
                <input class="d2-input" bind:value={replaceTag} placeholder={t('m.ai_settings.tag_replace_placeholder')} aria-label={t('m.ai_settings.tag_replace')} />
              {/if}
            </div>
          {/if}
        </div>
        <p class="d2-muted small">{t('m.ai_settings.workflow_hint')} <a href={router.href('automation/workflows')}>{t('m.ai_settings.open_workflows')}</a> · <a href={router.href('automation/review')}>{t('m.ai.review_title')}</a></p>
      </section>

      <section class="card" aria-labelledby="d2-ai-h-cf">
        <h2 id="d2-ai-h-cf">{t('m.ai_settings.custom_fields')}</h2>
        <label class="switch">
          <input type="checkbox" bind:checked={opts.custom_fields_enabled} id="d2-ai-cf" />
          <span>{t('m.ai_settings.custom_fields_on')}<br /><span class="d2-muted">{t('m.ai_settings.custom_fields_hint')}</span></span>
        </label>
        {#if opts.custom_fields_enabled}
          {#if fields.length === 0}
            <p class="d2-muted">{t('m.ai_settings.no_custom_fields')}</p>
          {:else}
            <ul class="fields">
              {#each fields as f (f.id)}
                <li>
                  <label class="d2-check">
                    <input type="checkbox" checked={opts.custom_fields.includes(f.id)} disabled={!f.supported} onchange={(e) => toggleField(f.id, (e.currentTarget as HTMLInputElement).checked)} />
                    {f.name}
                    {#if !f.supported}<span class="d2-muted small">({t('m.ai_settings.field_unsupported')})</span>{/if}
                  </label>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </section>

      <section class="card" aria-labelledby="d2-ai-h-vision">
        <h2 id="d2-ai-h-vision">{t('m.ai_settings.vision')}</h2>
        <label class="switch">
          <input type="checkbox" bind:checked={opts.vision_enabled} id="d2-ai-vision" />
          <span>{t('m.ai_settings.vision_on')}<br /><span class="d2-muted">{t('m.ai_settings.vision_hint')}</span></span>
        </label>
        {#if opts.vision_enabled}
          <div class="grid2">
            <label class="d2-label" for="d2-ai-vmodel">{t('m.ai_settings.vision_model')}</label>
            <div>
              <input class="d2-input" id="d2-ai-vmodel" list="d2-ai-vmodels" bind:value={opts.vision_model} placeholder={form.backend === 'ollama' ? 'qwen2.5vl:7b' : 'gpt-4o-mini'} />
              <datalist id="d2-ai-vmodels">{#each visionModels as m (m)}<option value={m}></option>{/each}</datalist>
              <p class="d2-muted small">{t('m.ai_settings.vision_model_hint')}</p>
            </div>
            <label class="d2-label" for="d2-ai-vwhen">{t('m.ai_settings.vision_when')}</label>
            <select class="d2-select" id="d2-ai-vwhen" bind:value={opts.vision_when}>
              <option value="off">{t('m.ai_settings.vision_when_off')}</option>
              <option value="difficult">{t('m.ai_settings.vision_when_difficult')}</option>
              <option value="always">{t('m.ai_settings.vision_when_always')}</option>
            </select>
            <label class="d2-label" for="d2-ai-vpages">{t('m.ai_settings.vision_max_pages')}</label>
            <input class="d2-input num" id="d2-ai-vpages" type="number" min="1" max="200" bind:value={opts.vision_limits.max_pages} />
            <label class="d2-label" for="d2-ai-vday">{t('m.ai_settings.vision_per_day')}</label>
            <input class="d2-input num" id="d2-ai-vday" type="number" min="0" bind:value={opts.vision_limits.max_pages_per_day} />
          </div>
        {/if}
      </section>

      <details class="card adv">
        <summary>{t('m.ai_settings.advanced')}</summary>
        <div class="grid2">
          <label class="d2-label" for="d2-ai-lang">{t('m.ai_settings.output_language')}</label>
          <div>
            <input class="d2-input" id="d2-ai-lang" bind:value={form.language} placeholder="de-de" disabled={!!lock('language')} />
            <p class="d2-muted small">{t('m.ai_settings.output_language_hint')}</p>
          </div>
          <label class="d2-label" for="d2-ai-maxdocs">{t('m.ai_settings.max_docs')}</label>
          <input class="d2-input num" id="d2-ai-maxdocs" type="number" min="1" bind:value={form.maxDocs} disabled={!!lock('maxDocs')} />
          <label class="d2-label" for="d2-ai-maxnew">{t('m.ai_settings.max_new')}</label>
          <input class="d2-input num" id="d2-ai-maxnew" type="number" min="1" bind:value={form.maxNew} disabled={!!lock('maxNew')} />
          <label class="d2-label" for="d2-ai-tokens">{t('m.ai_settings.vision_tokens')}</label>
          <input class="d2-input num" id="d2-ai-tokens" type="number" min="0" bind:value={opts.vision_limits.max_tokens_per_page} />
          <label class="d2-label" for="d2-ai-rate">{t('m.ai_settings.backfill_rate')}</label>
          <input class="d2-input num" id="d2-ai-rate" type="number" min="1" max="120" bind:value={opts.backfill_per_minute} />
        </div>
        <p class="d2-muted small">{t('m.ai_settings.more_in_system')} <a href={router.href('settings/system')}>{t('m.ai_settings.open_system')}</a></p>
      </details>

      <div class="savebar">
        <button class="d2-btn primary" disabled={!dirty || saving} onclick={save} id="d2-ai-save"><Icon name="check-lg" />{t('common.save')}</button>
        {#if dirty}<span class="d2-muted small">{t('m.ai_settings.unsaved')}</span>{/if}
      </div>

      <details class="card" bind:open={showPrompts}>
        <summary>{t('m.ai_settings.prompts')}</summary>
        <p class="d2-muted">{t('m.ai_settings.prompts_hint')}</p>
        {#if Prompts}{@const P = Prompts}<P />{/if}
      </details>

      <details class="card" bind:open={showBackfill}>
        <summary>{t('m.ai_settings.backfill')}</summary>
        <p class="d2-muted">{t('m.ai_settings.backfill_hint')}</p>
        {#if Backfill}{@const B = Backfill}<B autoMode={saved?.autoMode ?? 'off'} />{/if}
      </details>
    {:else}
      <div class="savebar">
        <button class="d2-btn primary" disabled={!dirty || saving} onclick={save} id="d2-ai-save"><Icon name="check-lg" />{t('common.save')}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .ai-set {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s4);
    max-width: 860px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
    padding: var(--d2-s4);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-lg);
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
  .switch {
    display: flex;
    gap: var(--d2-s3);
    align-items: flex-start;
    cursor: pointer;
  }
  .switch input {
    margin-top: 3px;
    width: 18px;
    height: 18px;
    accent-color: var(--d2-accent);
  }
  .choices {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--d2-s2);
  }
  .choices.three {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
  .choice {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r1);
    cursor: pointer;
  }
  .choice.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
  }
  .choice input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .choice:focus-within {
    outline: 2px solid var(--d2-accent);
    outline-offset: 1px;
  }
  .ttl {
    font-weight: 600;
  }
  .grid2 {
    display: grid;
    grid-template-columns: minmax(140px, 220px) minmax(0, 1fr);
    gap: var(--d2-s2) var(--d2-s3);
    align-items: center;
  }
  .row {
    display: flex;
    gap: var(--d2-s2);
    align-items: center;
  }
  .row .d2-input {
    flex: 1;
  }
  .num {
    max-width: 140px;
  }
  .result {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--d2-s2);
    color: var(--d2-danger);
  }
  .result.ok {
    color: var(--d2-ok);
  }
  .warn {
    color: var(--d2-warn);
  }
  .index {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    padding-top: var(--d2-s3);
    border-top: 1px solid var(--d2-border);
  }
  .istate.ok {
    color: var(--d2-ok);
  }
  .istate.warning {
    color: var(--d2-warn);
  }
  .istate.error {
    color: var(--d2-danger);
  }
  .fields {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--d2-s1) var(--d2-s3);
  }
  .savebar {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    position: sticky;
    bottom: 0;
    padding: var(--d2-s2) 0;
    background: var(--d2-bg);
  }
  @media (max-width: 700px) {
    .choices,
    .choices.three,
    .grid2 {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
