<!-- New workflow (topic 6 B): gallery of fixed templates, suggestions from the archive (GET /suggestions, without AI:
     "41 of 43 documents") and – only with AI – "Describe what should happen" (POST /draft-from-description).
     Each choice yields a draft (nothing saved) that the editor opens for review. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { locale, t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import Icon from '../components/Icon.svelte'
  import { draftFromDescription, draftFromSuggestion, errorOf, listSuggestions, type Suggestion } from './api'
  import { draftFromNgx, fieldInSentence, findByWords, templateDraft, visibleTemplates, type Draft, type NameKind, type TemplateKey } from './model'
  import { auto } from './store.svelte'

  let { onclose }: { onclose: () => void } = $props()

  const TPL_TEXT: Record<TemplateKey, [string, string]> = {
    invoices: ['m.auto.tpl.invoices', 'm.auto.tpl.invoices_text'],
    ai_presort: ['m.auto.tpl.ai_presort', 'm.auto.tpl.ai_presort_text'],
    passwords: ['m.auto.tpl.passwords', 'm.auto.tpl.passwords_text'],
    tax: ['m.auto.tpl.tax', 'm.auto.tpl.tax_text'],
    notify: ['m.auto.tpl.notify', 'm.auto.tpl.notify_text'],
    blank: ['m.auto.tpl.blank', 'm.auto.tpl.blank_text'],
  }
  const templates = $derived(visibleTemplates({ aiOn: auto.aiOn, emailOn: auto.emailOn }))

  function open(draft: Draft, note: string | null = null) {
    auto.pending = { draft, note }
    onclose()
    router.navigate('automation/workflows/new')
  }

  function fromTemplate(key: TemplateKey) {
    const list = (k: NameKind) => auto.options(k)
    const me: any = session.data?.user
    open(
      templateDraft(key, {
        name: key === 'blank' ? '' : t(TPL_TEXT[key][0]),
        find: (k, words) => findByWords(list(k), words),
        userEmail: me?.email ?? null,
      }),
    )
  }

  // ---- Suggestions from the archive ------------------------------------------------------------------------
  let sugg = $state<Suggestion[] | null>(null)
  let suggFailed = $state(false)
  let showCovered = $state(false)
  let busyKey = $state<string | null>(null)
  const canSuggest = session.can('view', 'document')
  async function loadSuggestions(refresh = false) {
    suggFailed = false
    try {
      sugg = (await listSuggestions(refresh)).items
    } catch {
      suggFailed = true
      sugg = []
    }
  }
  onMount(() => {
    if (canSuggest) void loadSuggestions()
  })
  const shown = $derived((sugg ?? []).filter((s) => showCovered || !s.covered_by_workflow))
  const fieldLabel: Record<string, string> = { correspondent: 'common.correspondent', document_type: 'common.document_type', tags: 'm.auto.field_tag', tag: 'm.auto.field_tag', storage_path: 'common.storage_path' }
  function suggText(s: Suggestion): string {
    const to = t('m.auto.s.quote', { PH: s.to.name ?? `#${s.to.id}` })
    const field = (f: string) => fieldInSentence(t(fieldLabel[f] ?? f), locale())
    const toField = field(s.to.field)
    if (s.from.field === 'filename_prefix') return t('m.auto.sugg_filename', { PH: s.from.value ?? '', FIELD: toField, TO: to })
    return t('m.auto.sugg_object', { FROM_FIELD: field(s.from.field), FROM: t('m.auto.s.quote', { PH: s.from.name ?? `#${s.from.id}` }), FIELD: toField, TO: to })
  }
  async function takeSuggestion(s: Suggestion) {
    busyKey = s.key
    try {
      const r = await draftFromSuggestion(s.key)
      const d = draftFromNgx(r.workflow)
      if (!d.workflow.name) d.workflow.name = suggText(s)
      open(d)
    } catch (e) {
      suggError = errorOf(e).message || t('m.auto.could_not_load')
    } finally {
      busyKey = null
    }
  }
  let suggError = $state<string | null>(null)

  // ---- Describe with AI --------------------------------------------------------------------------------
  let desc = $state('')
  let aiBusy = $state(false)
  let aiError = $state<string | null>(null)
  async function fromDescription() {
    if (!desc.trim()) return
    aiBusy = true
    aiError = null
    try {
      const r = await draftFromDescription(desc.trim())
      if (!r.workflow) {
        aiError = r.explanation || t('m.auto.ai_no_draft')
        return
      }
      const d = draftFromNgx(r.workflow)
      d.workflow.enabled = false
      const notes = [r.explanation, ...(r.missing ?? []).map((m) => (typeof m === 'string' ? m : JSON.stringify(m)))].filter(Boolean)
      open(d, notes.length ? notes.join(' · ') : null)
    } catch (e) {
      const err = errorOf(e)
      aiError = err.status === 429 ? t('m.auto.ai_throttled') : err.status === 503 || err.status === 501 ? t('m.auto.ai_unavailable') : err.message || t('m.auto.could_not_load')
    } finally {
      aiBusy = false
    }
  }
</script>

<SideSheet title={t('m.auto.new_workflow')} {onclose} width="640px">
  <div class="au-form" id="au-gallery">
    {#if auto.aiOn}
      <section class="au-group" aria-labelledby="au-ai-desc-h">
        <h3 id="au-ai-desc-h"><Icon name="stars" /> {t('m.auto.ai_build_title')}</h3>
        <p class="au-hint">{t('m.auto.ai_build_hint')}</p>
        <label class="d2-sr" for="au-ai-desc">{t('m.auto.ai_build_title')}</label>
        <textarea id="au-ai-desc" class="d2-textarea" rows="3" maxlength="2000" placeholder={t('m.auto.ai_build_placeholder')} bind:value={desc}></textarea>
        {#if aiError}<p class="au-err" role="alert">{aiError}</p>{/if}
        <div><button type="button" class="d2-btn" disabled={aiBusy || !desc.trim()} onclick={fromDescription}>{aiBusy ? t('m.auto.ai_building') : t('m.auto.ai_build_button')}</button></div>
      </section>
    {/if}

    <section aria-labelledby="au-tpl-h">
      <h3 class="au-section-title" id="au-tpl-h">{t('m.auto.templates')}</h3>
      <div class="au-tiles">
        {#each templates as tpl (tpl.key)}
          <button type="button" class="au-tile" id="au-tpl-{tpl.key}" onclick={() => fromTemplate(tpl.key)}>
            <b><Icon name={tpl.icon} />{t(TPL_TEXT[tpl.key][0])}</b>
            <span>{t(TPL_TEXT[tpl.key][1])}</span>
          </button>
        {/each}
      </div>
    </section>

    {#if canSuggest}
      <section aria-labelledby="au-sugg-h">
        <h3 class="au-section-title" id="au-sugg-h">{t('m.auto.suggestions')}</h3>
        <p class="au-hint">{t('m.auto.suggestions_hint')}</p>
        {#if suggError}<p class="au-err" role="alert">{suggError}</p>{/if}
        {#if sugg === null}
          <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
        {:else if suggFailed}
          <p class="au-err">{t('m.auto.could_not_load')}</p>
        {:else if !shown.length}
          <p class="d2-muted">{t('m.auto.no_suggestions')}</p>
        {:else}
          <ul class="au-sugg" id="au-suggestions">
            {#each shown as s (s.key)}
              <li>
                <div class="txt">
                  <div>{suggText(s)}</div>
                  <div class="share">{t('m.auto.sugg_share', { N: s.matches, TOTAL: s.total })}{#if s.covered_by_workflow} · {t('m.auto.sugg_covered')}{/if}</div>
                  <div class="au-bar" aria-hidden="true"><i style:width="{Math.round(s.share * 100)}%"></i></div>
                </div>
                <button type="button" class="d2-btn sm" disabled={busyKey !== null} onclick={() => takeSuggestion(s)}>{busyKey === s.key ? t('common.loading') : t('m.auto.use')}</button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if sugg?.some((s) => s.covered_by_workflow)}
          <label class="au-check au-mt2"><input type="checkbox" bind:checked={showCovered} />{t('m.auto.show_covered')}</label>
        {/if}
        <div class="au-mt2"><button type="button" class="d2-btn ghost sm" onclick={() => loadSuggestions(true)}><Icon name="arrow-clockwise" />{t('m.auto.refresh')}</button></div>
      </section>
    {/if}
  </div>
</SideSheet>
