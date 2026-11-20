<!-- History per workflow (topic 8 A): runs with result (green = ok, red = error, gray = skipped), errors prominent,
     link to the document, codes translated. Data: GET /rs/api/v2/automation/workflows/{id}/runs (newest first). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import Icon from '../components/Icon.svelte'
  import { listRuns, type RunItem, type RunResult, type RunsPage } from './api'
  import { ago, codeKey, dateTime } from './model'

  let { workflowId, onclose }: { workflowId: number; onclose: () => void } = $props()
  const loc = locale()
  let filter = $state<RunResult | null>(null)
  let items = $state<RunItem[]>([])
  let page = $state<RunsPage | null>(null)
  let loading = $state(false)
  let failed = $state(false)

  async function load(more = false) {
    loading = true
    failed = false
    try {
      const r = await listRuns(workflowId, { cursor: more ? page?.next_cursor : null, result: filter })
      items = more ? [...items, ...r.items] : r.items
      page = r
    } catch {
      failed = true
    } finally {
      loading = false
    }
  }
  onMount(() => load())
  function setFilter(f: RunResult | null) {
    filter = f
    void load()
  }

  const TRIG: Record<string, string> = { consumption: 'm.auto.trig.consumption', document_added: 'm.auto.trig.added', document_updated: 'm.auto.trig.updated', scheduled: 'm.auto.trig.scheduled', manual: 'm.auto.trig.manual' }
  const RES: Record<RunResult, string> = { ok: 'm.auto.runs.ok', error: 'm.auto.runs.error', skipped: 'm.auto.runs.skipped' }
  const FIELD: Record<string, string> = {
    title: 'common.title',
    correspondent: 'common.correspondent',
    document_type: 'common.document_type',
    storage_path: 'common.storage_path',
    owner: 'common.owner',
    tags: 'common.tags',
    custom_fields: 'common.custom_fields',
    permissions: 'm.auto.field_permissions',
    email: 'common.email',
    webhook: 'dialog.workflow.webhook',
    password_removal: 'dialog.workflow.password_removal',
    trash: 'common.move_trash',
    remote_ocr: 'common.remote_ocr',
    ai_suggestions: 'm.auto.act.ai',
  }
  const fields = (r: RunItem) => r.changed_fields.map((f) => t(FIELD[f] ?? f)).join(', ')
</script>

<SideSheet title={t('m.auto.history')} {onclose} width="600px">
  <div class="au-form" id="au-runs">
    {#if page}
      <p class="au-hint" id="au-runs-counts">
        {t('m.auto.runs.counts', { N: page.counts.this_week, E: page.counts.errors_this_week, TOTAL: page.counts.total })}
        {#if page.retention_days > 0}· {t('m.auto.runs.retention', { N: page.retention_days })}{/if}
      </p>
    {/if}
    <div class="au-seg" role="group" aria-label={t('m.auto.runs.filter')}>
      <button type="button" aria-pressed={filter === null} onclick={() => setFilter(null)}>{t('m.auto.runs.all')}</button>
      <button type="button" aria-pressed={filter === 'error'} id="au-runs-errors" onclick={() => setFilter('error')}>{t('m.auto.runs.only_errors')}</button>
      <button type="button" aria-pressed={filter === 'ok'} onclick={() => setFilter('ok')}>{t('m.auto.runs.only_ok')}</button>
      <button type="button" aria-pressed={filter === 'skipped'} onclick={() => setFilter('skipped')}>{t('m.auto.runs.only_skipped')}</button>
    </div>
    {#if failed}
      <div class="au-banner danger" role="alert">{t('m.auto.load_failed')} <button type="button" class="d2-btn sm" onclick={() => load()}>{t('m.auto.retry')}</button></div>
    {:else if !items.length && !loading}
      <p class="d2-muted">{t('m.auto.runs.empty')}</p>
    {:else}
      <ul class="au-runs" id="au-runs-list">
        {#each items as r (r.id)}
          <li class="au-run" data-result={r.result}>
            <span class="au-st {r.result}">{#if r.result === 'error'}<Icon name="exclamation-triangle" />{/if}{t(RES[r.result])}</span>
            <div class="what">
              <div>
                {#if r.document && r.document.id != null}
                  <a href={router.href(`documents/${r.document.id}`)}>{('title' in r.document && r.document.title) || t('m.auto.runs.document', { N: r.document.id })}</a>
                  {#if 'in_trash' in r.document && r.document.in_trash}<span class="au-hint"> ({t('m.auto.runs.in_trash')})</span>{/if}
                  {#if 'deleted' in r.document && r.document.deleted}<span class="au-hint"> ({t('m.auto.runs.deleted')})</span>{/if}
                {:else if r.document && 'hidden' in r.document}
                  <span>{t('m.auto.runs.hidden_document')}</span>
                {:else}
                  <span>{r.source === 3 ? t('m.auto.runs.while_mail') : t('m.auto.runs.while_consuming')}</span>
                {/if}
                {#if r.branch === 'else'}<span class="au-st else">{t('m.auto.with_else')}</span>{/if}
              </div>
              <div class="au-hint">{t(codeKey(r.code))} · {t(TRIG[r.trigger_key] ?? 'm.auto.trig.added')}</div>
              {#if r.changed_fields.length && r.result === 'ok'}<p class="fields">{t('m.auto.runs.changed', { PH: fields(r) })}</p>{/if}
              {#if r.message && r.result !== 'ok'}<p class="msg" style:color={r.result === 'error' ? undefined : 'var(--d2-text-2)'}>{r.message}</p>{/if}
            </div>
            <span class="when" title={dateTime(r.run_at, loc)}>{ago(r.run_at, loc)}</span>
          </li>
        {/each}
      </ul>
      {#if loading}<p class="d2-muted" aria-busy="true">{t('common.loading')}</p>{/if}
      {#if page?.next_cursor && !loading}<div><button type="button" class="d2-btn sm" onclick={() => load(true)}>{t('m.auto.load_more')}</button></div>{/if}
    {/if}
  </div>
</SideSheet>
