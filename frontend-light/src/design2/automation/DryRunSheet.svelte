<!-- Test run (topic 7 C): hit list over the newest documents (POST /dry-run, nothing is changed) +
     before/after on one document + "apply to these" (POST /workflows/{id}/apply) with confirmation.
     The current state in the editor is tested (also unsaved); applying works only with a saved state. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import Icon from '../components/Icon.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import { applyWorkflow, dryRun, errorOf, type DryChange, type DryDoc, type DryRun, type SaveBody } from './api'
  import { codeKey, dateTime, TRIGGER_KEY } from './model'

  let {
    body,
    workflowId,
    dirty,
    triggers,
    hasAi,
    aiOn,
    onclose,
    onsave,
  }: { body: SaveBody; workflowId: number | null; dirty: boolean; triggers: number[]; hasAi: boolean; aiOn: boolean; onclose: () => void; onsave: () => Promise<boolean> } = $props()

  const loc = locale()
  const uniqueTriggers = $derived([...new Set(triggers)])
  // svelte-ignore state_referenced_locally
  let triggerType = $state<number | null>(triggers.find((x) => x !== 1) ?? triggers[0] ?? null)
  let limit = $state(50)
  let withAi = $state(false)
  let res = $state<DryRun | null>(null)
  let running = $state(false)
  let error = $state<string | null>(null)
  let openDoc = $state<number | null>(null)
  let picked = $state<number[]>([])
  let confirmApply = $state(false)
  let applying = $state(false)
  const canApply = session.can('change', 'document')

  const TRIG: Record<string, string> = { consumption: 'm.auto.trig.consumption', document_added: 'm.auto.trig.added', document_updated: 'm.auto.trig.updated', scheduled: 'm.auto.trig.scheduled', manual: 'm.auto.trig.manual' }
  const NOTE: Record<string, string> = {
    consumption_approximate: 'm.auto.dry.note_consumption',
    schedule_window_not_checked: 'm.auto.dry.note_schedule',
    ai_condition_ignored: 'm.auto.dry.note_ai_ignored',
    ai_condition_not_checked: 'm.auto.dry.note_ai_not_checked',
  }
  const RESULT: Record<string, string> = { main: 'm.auto.dry.result_main', else: 'm.auto.dry.result_else', skipped: 'm.auto.dry.result_skipped', error: 'm.auto.dry.result_error' }

  async function run() {
    running = true
    error = null
    openDoc = null
    try {
      const b = { ...body, name: body.name || t('m.auto.untitled') }
      res = await dryRun({ workflow: b, limit, trigger_type: triggerType ?? undefined, with_ai: hasAi && withAi ? true : undefined })
      picked = res.documents.filter((d) => d.result === 'main' || d.result === 'else').map((d) => d.id)
    } catch (e) {
      const err = errorOf(e)
      error = err.code === 'invalid' ? t('m.auto.dry.invalid') + (err.fields ? ' ' + flat(err.fields) : '') : err.status === 403 ? t('m.auto.not_allowed') : err.message || t('m.auto.dry.failed')
    } finally {
      running = false
    }
  }
  const flat = (v: unknown): string => (v == null ? '' : typeof v === 'string' ? v : Array.isArray(v) ? v.map(flat).filter(Boolean).join(' ') : typeof v === 'object' ? Object.values(v as object).map(flat).filter(Boolean).join(' ') : String(v))
  onMount(run)

  // ---- Display of the changes -------------------------------------------------------------------------
  const FIELD: Record<string, string> = {
    title: 'common.title',
    correspondent: 'common.correspondent',
    document_type: 'common.document_type',
    storage_path: 'common.storage_path',
    owner: 'common.owner',
    tags: 'common.tags',
    custom_field: 'm.auto.field_custom',
    view_users: 'm.auto.dry.view_users',
    view_groups: 'm.auto.dry.view_groups',
    change_users: 'm.auto.dry.change_users',
    change_groups: 'm.auto.dry.change_groups',
    in_trash: 'm.auto.dry.in_trash',
    permissions: 'm.auto.field_permissions',
  }
  const EFFECT: Record<string, string> = { email: 'm.auto.dry.effect_email', webhook: 'm.auto.dry.effect_webhook', password_removal: 'm.auto.dry.effect_password', trash: 'm.auto.dry.effect_trash', ai_suggestions: 'm.auto.dry.effect_ai' }
  const show = (v: unknown): string => {
    if (v == null || v === '') return '–'
    if (Array.isArray(v)) return v.map(show).join(', ')
    if (typeof v === 'object') {
      const o = v as Record<string, unknown>
      return String(o.name ?? o.title ?? o.username ?? (o.id != null ? `#${o.id}` : JSON.stringify(o)))
    }
    if (typeof v === 'boolean') return v ? t('common.yes') : t('common.no')
    return String(v)
  }
  const fieldName = (c: DryChange) => (c.field === 'custom_field' && c.custom_field ? show(c.custom_field) : t(FIELD[c.field] ?? c.field))
  function togglePick(id: number, on: boolean) {
    picked = on ? [...picked, id] : picked.filter((x) => x !== id)
  }
  const pickable = (d: DryDoc) => d.result === 'main' || d.result === 'else'

  async function apply() {
    if (!workflowId || !picked.length) return
    applying = true
    try {
      const r = await applyWorkflow(workflowId, { document_ids: picked, trigger_type: res?.trigger_type })
      toastService.show({ content: t('m.auto.dry.applied', { N: r.documents }), delay: 6000, level: 'success', actionName: t('m.auto.history'), action: () => router.navigate(`automation/workflows/${workflowId}?tab=runs`) })
      confirmApply = false
      onclose()
    } catch (e) {
      const err = errorOf(e)
      toastService.showError(err.status === 403 ? t('m.auto.dry.apply_forbidden') : err.status === 503 ? t('m.auto.dry.apply_no_queue') : t('m.auto.dry.apply_failed'), e)
    } finally {
      applying = false
    }
  }
  async function saveThenApply() {
    if (await onsave()) confirmApply = true
  }
</script>

<SideSheet title={t('m.auto.dryrun')} {onclose} width="720px">
  <div class="au-form" id="au-dryrun">
    <p class="au-hint">{t('m.auto.dry.intro')}</p>
    <div class="au-row2">
      {#if uniqueTriggers.length > 1}
        <div class="au-field">
          <label for="au-dry-trigger">{t('m.auto.dry.trigger')}</label>
          <select id="au-dry-trigger" class="d2-select" bind:value={triggerType}>
            {#each uniqueTriggers as ty (ty)}<option value={ty}>{t(TRIG[TRIGGER_KEY[ty]] ?? 'm.auto.trig.added')}</option>{/each}
          </select>
        </div>
      {/if}
      <div class="au-field">
        <label for="au-dry-limit">{t('m.auto.dry.limit')}</label>
        <select id="au-dry-limit" class="d2-select" bind:value={limit}>
          {#each [20, 50, 100, 200] as n (n)}<option value={n}>{t('m.auto.dry.limit_n', { N: n })}</option>{/each}
        </select>
      </div>
    </div>
    {#if hasAi && aiOn}
      <label class="au-check"><input type="checkbox" id="au-dry-ai" bind:checked={withAi} /><span>{t('m.auto.dry.with_ai')}<br /><span class="au-hint">{t('m.auto.dry.with_ai_hint')}</span></span></label>
    {/if}
    <div><button type="button" class="d2-btn" id="au-dry-run" disabled={running} onclick={run}><Icon name="play" />{running ? t('m.auto.dry.running') : t('m.auto.dry.run_again')}</button></div>
    {#if error}<div class="au-banner danger" role="alert">{error}</div>{/if}

    {#if res}
      {#each res.notes as n (n)}<div class="au-banner warn" role="status">{t(NOTE[n] ?? 'm.auto.dry.note_other')}</div>{/each}
      <div class="au-counts" id="au-dry-counts" aria-live="polite">
        <span class="au-st main">{t('m.auto.dry.count_main', { N: res.counts.main })}</span>
        {#if res.counts.else}<span class="au-st else">{t('m.auto.dry.count_else', { N: res.counts.else })}</span>{/if}
        <span class="au-st skipped">{t('m.auto.dry.count_skipped', { N: res.counts.skipped })}</span>
        {#if res.counts.error}<span class="au-st error">{t('m.auto.dry.count_error', { N: res.counts.error })}</span>{/if}
        <span class="au-hint">{t('m.auto.dry.count_checked', { N: res.counts.checked })}</span>
      </div>
      {#if !res.documents.length}
        <p class="d2-muted">{t('m.auto.dry.no_documents')}</p>
      {:else}
        <ul class="au-docs" id="au-dry-docs">
          {#each res.documents as d (d.id)}
            <li data-doc={d.id}>
              <div class="au-doc">
                {#if pickable(d) && canApply}
                  <input type="checkbox" aria-label={t('m.auto.dry.pick', { PH: d.title })} checked={picked.includes(d.id)} onchange={(e) => togglePick(d.id, e.currentTarget.checked)} />
                {:else}<span></span>{/if}
                <span class="t">
                  <button type="button" class="open" aria-expanded={openDoc === d.id} onclick={() => (openDoc = openDoc === d.id ? null : d.id)}>{d.title}</button>
                  {#if d.result === 'skipped' && d.reason}<span class="au-hint"> – {d.reason}</span>{/if}
                  {#if d.result !== 'skipped' && d.code !== 'ok' && d.code !== 'else'}<span class="au-hint"> – {t(codeKey(d.code))}</span>{/if}
                </span>
                <span class="au-st {d.result}">{t(RESULT[d.result])}{#if pickable(d) && d.changes.length}{' · '}{t('m.auto.dry.n_changes', { N: d.changes.length })}{/if}</span>
              </div>
              {#if openDoc === d.id}
                {#if d.changes.length || d.effects.length}
                  <dl class="au-diff">
                    {#each d.changes as c, ci (ci)}
                      <dt>{fieldName(c)}</dt>
                      {#if c.added || (Array.isArray(c.removed) && c.field !== 'custom_field')}
                        <dd class="old">{Array.isArray(c.removed) && c.removed.length ? '− ' + show(c.removed) : ''}</dd>
                        <dd class="new">{c.added?.length ? '+ ' + show(c.added) : ''}</dd>
                      {:else}
                        <dd class="old">{show(c.before)}</dd>
                        <dd class="new">{c.removed === true ? t('m.auto.dry.removed') : show(c.after)}</dd>
                      {/if}
                    {/each}
                    {#each d.effects as ef, ei (ei)}
                      <dt>{t(EFFECT[ef.kind] ?? 'm.auto.dry.effect_other')}</dt>
                      <dd></dd>
                      <dd class="new">{ef.target ?? ''}</dd>
                    {/each}
                  </dl>
                {:else}
                  <p class="au-hint au-diff-note">{d.result === 'skipped' ? t('m.auto.dry.nothing_skipped') : t('m.auto.dry.nothing_changes')}</p>
                {/if}
                <p class="au-diff-note"><a href={router.href(`documents/${d.id}`)} target="_blank" rel="noopener">{t('common.open_document')} <Icon name="box-arrow-up-right" /></a>{#if d.added} · <span class="au-hint">{t('common.added')}: {dateTime(d.added, loc)}</span>{/if}</p>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
  {#snippet footer()}
    {#if res && canApply}
      <span class="au-hint au-push">{t('m.auto.dry.picked', { N: picked.length })}</span>
      {#if !workflowId}
        <span class="au-hint">{t('m.auto.dry.save_first')}</span>
        <button type="button" class="d2-btn" onclick={onclose}>{t('common.close')}</button>
      {:else if dirty}
        <button type="button" class="d2-btn primary" id="au-dry-save-apply" disabled={!picked.length} onclick={saveThenApply}>{t('m.auto.dry.save_and_apply')}</button>
      {:else}
        <button type="button" class="d2-btn primary" id="au-dry-apply" disabled={!picked.length} onclick={() => (confirmApply = true)}>{t('m.auto.dry.apply', { N: picked.length })}</button>
      {/if}
    {:else}
      <button type="button" class="d2-btn" onclick={onclose}>{t('common.close')}</button>
    {/if}
  {/snippet}
</SideSheet>

{#if confirmApply}
  <ConfirmDialog title={t('m.auto.dry.confirm_title', { N: picked.length })} confirmLabel={t('m.auto.dry.confirm_apply')} busy={applying} onconfirm={apply} oncancel={() => (confirmApply = false)}>
    <p>{t('m.auto.dry.confirm_text')}</p>
  </ConfirmDialog>
{/if}
