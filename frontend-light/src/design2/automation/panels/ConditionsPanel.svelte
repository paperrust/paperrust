<!-- Side sheet "Only if" (topic 4 A): common conditions as chips (text contains, tag, correspondent, type,
     storage path, file name), the rest under "Advanced" (kind of text comparison, case, custom field query,
     path pattern, mail rule, sources). Every condition of the ngx trigger is reachable (guard test). -->
<script lang="ts">
  import { tick } from 'svelte'
  import { t } from '../../../lib/i18n'
  import Icon from '../../components/Icon.svelte'
  import ObjectPicker from '../../components/ObjectPicker.svelte'
  import CfQuery from './CfQuery.svelte'
  import {
    ADVANCED_CONDITIONS,
    COMMON_CONDITIONS,
    MATCH_NONE,
    SOURCE_MAIL,
    WorkflowTriggerType,
    clearCondition,
    conditionActive,
    conditionAllowed,
    isMailTrigger,
    type CondKey,
    type NameKind,
  } from '../model'
  import { auto } from '../store.svelte'

  let { trigger, index, focus = null, errors = [] }: { trigger: Record<string, any>; index: number; focus?: CondKey | null; errors?: string[] } = $props()

  const pid = $derived(`au-cond-${index}`)
  const COND_LABEL: Record<CondKey, string> = {
    text: 'm.auto.cond.text',
    tags: 'm.auto.cond.tags',
    correspondent: 'm.auto.cond.correspondent',
    document_type: 'm.auto.cond.document_type',
    storage_path: 'm.auto.cond.storage_path',
    filename: 'm.auto.cond.filename',
    path: 'm.auto.cond.path',
    mail_rule: 'm.auto.cond.mail_rule',
    sources: 'm.auto.cond.sources',
    custom_field_query: 'm.auto.cond.custom_field_query',
    ai: 'm.auto.cond.ai',
  }
  const mail = $derived(isMailTrigger(trigger))
  const allowed = (k: CondKey) => conditionAllowed(k, trigger.type) && !(mail && (k === 'mail_rule' || k === 'sources'))
  // visible: set or just added
  // svelte-ignore state_referenced_locally
  let added = $state<CondKey[]>(focus ? [focus] : [])
  const shown = (k: CondKey) => allowed(k) && (conditionActive(trigger, k) || added.includes(k))
  const commonFree = $derived(COMMON_CONDITIONS.filter((k) => allowed(k) && !shown(k)))
  const advancedAllowed = $derived(ADVANCED_CONDITIONS.filter(allowed))
  const advancedOpen = $derived(advancedAllowed.some((k) => conditionActive(trigger, k)) || (focus !== null && ADVANCED_CONDITIONS.includes(focus)) || (trigger.matching_algorithm ?? 0) > 3 || trigger.is_insensitive === false)

  async function add(k: CondKey) {
    if (!added.includes(k)) added = [...added, k]
    if (k === 'text' && (trigger.matching_algorithm ?? MATCH_NONE) === MATCH_NONE) trigger.matching_algorithm = 3
    await tick()
    document.getElementById(`${pid}-${k}`)?.focus()
  }
  function remove(k: CondKey) {
    clearCondition(trigger, k)
    added = added.filter((x) => x !== k)
  }
  $effect(() => {
    if (focus) void tick().then(() => document.getElementById(`${pid}-${focus}`)?.focus())
  })

  // Object conditions: "is one of" (any; one selection = exactly this, …_is) and "is none of" (none)
  const OBJ: Record<'correspondent' | 'document_type' | 'storage_path', { is: string; any: string; none: string; kind: NameKind }> = {
    correspondent: { is: 'filter_has_correspondent', any: 'filter_has_any_correspondents', none: 'filter_has_not_correspondents', kind: 'correspondent' },
    document_type: { is: 'filter_has_document_type', any: 'filter_has_any_document_types', none: 'filter_has_not_document_types', kind: 'document_type' },
    storage_path: { is: 'filter_has_storage_path', any: 'filter_has_any_storage_paths', none: 'filter_has_not_storage_paths', kind: 'storage_path' },
  }
  /** "exactly" and "one of" set at the same time (AND combination) → show separately, merge nothing */
  const split = (k: keyof typeof OBJ) => trigger[OBJ[k].is] != null && (trigger[OBJ[k].any] ?? []).length > 0
  const anyOf = (k: keyof typeof OBJ): number[] => (split(k) ? [...(trigger[OBJ[k].any] ?? [])] : trigger[OBJ[k].is] != null ? [trigger[OBJ[k].is]] : [...(trigger[OBJ[k].any] ?? [])])
  function setAnyOf(k: keyof typeof OBJ, ids: number[]) {
    const f = OBJ[k]
    if (split(k)) {
      trigger[f.any] = ids
      return
    }
    if (ids.length === 1) {
      trigger[f.is] = ids[0]
      trigger[f.any] = []
    } else {
      trigger[f.is] = null
      trigger[f.any] = ids
    }
  }
  const clean = (n: (number | null)[]) => n.filter((x): x is number => x !== null)
  const TEXT_ALGOS = [
    { id: 3, label: 'm.auto.algo.literal', hint: 'm.auto.algo.literal_hint' },
    { id: 1, label: 'm.auto.algo.any', hint: 'm.auto.algo.any_hint' },
    { id: 2, label: 'm.auto.algo.all', hint: 'm.auto.algo.all_hint' },
    { id: 4, label: 'm.auto.algo.regex', hint: 'm.auto.algo.regex_hint' },
    { id: 5, label: 'm.auto.algo.fuzzy', hint: 'm.auto.algo.fuzzy_hint' },
    { id: 6, label: 'm.auto.algo.auto', hint: 'm.auto.algo.auto_hint' },
  ]
  const SOURCES = [
    { id: 1, label: 'dialog.workflow.consume_folder' },
    { id: 2, label: 'common.api_upload' },
    { id: SOURCE_MAIL, label: 'common.mail_fetch' },
    { id: 4, label: 'common.web_ui' },
  ]
  function toggleSource(id: number, on: boolean) {
    const s = new Set<number>(trigger.sources ?? [])
    if (on) s.add(id)
    else s.delete(id)
    trigger.sources = [...s].sort()
  }
</script>

{#snippet head(k: CondKey)}
  <div class="au-headline">
    <h3>{t(COND_LABEL[k])}</h3>
    <button type="button" class="d2-btn ghost sm" id="{pid}-{k}-remove" onclick={() => remove(k)}><Icon name="x-lg" />{t('m.auto.remove')}</button>
  </div>
{/snippet}
{#snippet objBlock(k: 'correspondent' | 'document_type' | 'storage_path')}
  {@const f = OBJ[k]}
  <section class="au-group" aria-label={t(COND_LABEL[k])}>
    {@render head(k)}
    {#if split(k)}
      <div class="au-field">
        <span class="lab">{t('m.auto.cond.exactly')}</span>
        <ObjectPicker id="{pid}-{k}-exact" label={t('m.auto.cond.exactly')} options={auto.options(f.kind)} selected={[trigger[f.is]]} onchange={(n) => (trigger[f.is] = clean(n)[0] ?? null)} placeholder={t('m.auto.none_selected')} />
      </div>
    {/if}
    <div class="au-field">
      <span class="lab">{t('m.auto.cond.is_one_of')}</span>
      <ObjectPicker id="{pid}-{k}" multi label={t(COND_LABEL[k])} options={auto.options(f.kind)} selected={anyOf(k)} onchange={(n) => setAnyOf(k, clean(n))} placeholder={t('m.auto.none_selected')} />
    </div>
    <div class="au-field">
      <span class="lab">{t('m.auto.cond.is_none_of')}</span>
      <ObjectPicker id="{pid}-{k}-none" multi label={t('m.auto.cond.is_none_of')} options={auto.options(f.kind)} selected={trigger[f.none] ?? []} onchange={(n) => (trigger[f.none] = clean(n))} placeholder={t('m.auto.none_selected')} />
    </div>
  </section>
{/snippet}

<div class="au-form" id="{pid}-panel">
  {#if errors.length}<div class="au-banner danger" role="alert"><ul>{#each errors as e, i (i)}<li>{e}</li>{/each}</ul></div>{/if}
  <p class="au-hint">{t('m.auto.cond.intro')}</p>
  {#if trigger.type === WorkflowTriggerType.Consumption}
    <p class="au-hint">{t('m.auto.cond.consumption_hint')}</p>
  {/if}

  {#if shown('text')}
    <section class="au-group" aria-label={t(COND_LABEL.text)}>
      {@render head('text')}
      <div class="au-field">
        <label for="{pid}-text">{t('m.auto.cond.text_label')}</label>
        <input id="{pid}-text" class="d2-input" value={trigger.match ?? ''} oninput={(e) => {
          trigger.match = e.currentTarget.value
          if ((trigger.matching_algorithm ?? 0) === MATCH_NONE) trigger.matching_algorithm = 3
        }} />
        <p class="au-hint">{t(TEXT_ALGOS.find((a) => a.id === trigger.matching_algorithm)?.hint ?? 'm.auto.algo.literal_hint')}</p>
      </div>
    </section>
  {/if}
  {#if shown('tags')}
    <section class="au-group" aria-label={t(COND_LABEL.tags)}>
      {@render head('tags')}
      <div class="au-field">
        <span class="lab">{t('dialog.workflow.any_tags')}</span>
        <ObjectPicker id="{pid}-tags" multi label={t('dialog.workflow.any_tags')} options={auto.options('tag')} selected={trigger.filter_has_tags ?? []} onchange={(n) => (trigger.filter_has_tags = clean(n))} placeholder={t('m.auto.none_selected')} />
      </div>
      <div class="au-field">
        <span class="lab">{t('dialog.workflow.all_tags')}</span>
        <ObjectPicker id="{pid}-tags-all" multi label={t('dialog.workflow.all_tags')} options={auto.options('tag')} selected={trigger.filter_has_all_tags ?? []} onchange={(n) => (trigger.filter_has_all_tags = clean(n))} placeholder={t('m.auto.none_selected')} />
      </div>
      <div class="au-field">
        <span class="lab">{t('dialog.workflow.not_tags')}</span>
        <ObjectPicker id="{pid}-tags-none" multi label={t('dialog.workflow.not_tags')} options={auto.options('tag')} selected={trigger.filter_has_not_tags ?? []} onchange={(n) => (trigger.filter_has_not_tags = clean(n))} placeholder={t('m.auto.none_selected')} />
      </div>
    </section>
  {/if}
  {#if shown('correspondent')}{@render objBlock('correspondent')}{/if}
  {#if shown('document_type')}{@render objBlock('document_type')}{/if}
  {#if shown('storage_path')}{@render objBlock('storage_path')}{/if}
  {#if shown('filename')}
    <section class="au-group" aria-label={t(COND_LABEL.filename)}>
      {@render head('filename')}
      <div class="au-field">
        <label for="{pid}-filename">{t('dialog.workflow.filter_filename')}</label>
        <input id="{pid}-filename" class="d2-input" placeholder="*.pdf" value={trigger.filter_filename ?? ''} oninput={(e) => (trigger.filter_filename = e.currentTarget.value || null)} />
        <p class="au-hint">{t('dialog.workflow.apply_documents_match_filename')}</p>
      </div>
    </section>
  {/if}

  {#if commonFree.length}
    <div class="au-field">
      <span class="lab">{t('m.auto.add_condition')}</span>
      <div class="au-chips" id="{pid}-chips">
        {#each commonFree as k (k)}
          <button type="button" class="d2-chip" id="{pid}-add-{k}" onclick={() => add(k)}><Icon name="plus-lg" />{t(COND_LABEL[k])}</button>
        {/each}
      </div>
    </div>
  {/if}

  {#if advancedAllowed.length || shown('text')}
    <details class="au-details" open={advancedOpen}>
      <summary id="{pid}-advanced">{t('m.auto.advanced')}</summary>
      <div class="au-form">
        {#if shown('text')}
          <div class="au-field">
            <label for="{pid}-algo">{t('m.auto.cond.algorithm')}</label>
            <select id="{pid}-algo" class="d2-select" value={trigger.matching_algorithm} onchange={(e) => (trigger.matching_algorithm = Number(e.currentTarget.value))}>
              {#each TEXT_ALGOS as a (a.id)}<option value={a.id}>{t(a.label)}</option>{/each}
            </select>
          </div>
          <label class="au-check"><input type="checkbox" id="{pid}-insensitive" checked={trigger.is_insensitive !== false} onchange={(e) => (trigger.is_insensitive = e.currentTarget.checked)} />{t('common.case_insensitive')}</label>
        {/if}
        {#if allowed('custom_field_query')}
          <CfQuery id="{pid}-custom_field_query" value={trigger.filter_custom_field_query ?? null} onchange={(v) => (trigger.filter_custom_field_query = v)} />
        {/if}
        {#if allowed('path')}
          <div class="au-field">
            <label for="{pid}-path">{t('dialog.workflow.filter_path')}</label>
            <input id="{pid}-path" class="d2-input" placeholder="*/scanner/*" value={trigger.filter_path ?? ''} oninput={(e) => (trigger.filter_path = e.currentTarget.value || null)} />
            <p class="au-hint">{t('dialog.workflow.apply_documents_match_path')}</p>
          </div>
        {/if}
        {#if allowed('mail_rule')}
          <div class="au-field">
            <label for="{pid}-mail_rule">{t('dialog.workflow.filter_mail_rule')}</label>
            <select id="{pid}-mail_rule" class="d2-select" value={trigger.filter_mailrule ?? ''} onchange={(e) => (trigger.filter_mailrule = e.currentTarget.value === '' ? null : Number(e.currentTarget.value))}>
              <option value="">{t('m.auto.any_rule')}</option>
              {#each auto.mailRules as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
            </select>
            <p class="au-hint">{t('dialog.workflow.apply_documents_consumed_via')}</p>
          </div>
        {/if}
        {#if allowed('sources')}
          <fieldset class="au-field" id="{pid}-sources" data-plain="">
            <legend class="lab">{t('dialog.workflow.filter_sources')}</legend>
            {#each SOURCES as s (s.id)}
              <label class="au-check"><input type="checkbox" checked={(trigger.sources ?? []).includes(s.id)} onchange={(e) => toggleSource(s.id, e.currentTarget.checked)} />{t(s.label)}</label>
            {/each}
            <p class="au-hint">{t('m.auto.cond.sources_hint')}</p>
          </fieldset>
        {/if}
      </div>
    </details>
  {/if}
</div>
