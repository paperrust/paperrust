<!-- "Reprocess existing archive" (Settings → AI): selection via filter, preview "how many", start in mode
     suggest/apply, progress, abort, resume after restart. API: /api/x-ai/jobs/ (superuser).
     Apply only if the server allows automatic applying (PAPERRUST_AI_AUTO_MODE=apply). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { router } from '../../../router.svelte'
  import { data } from '../../../stores/data.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import Icon from '../../components/Icon.svelte'
  import { cancelJob, createJob, listJobs, previewJob, resumeJob, type Job, type JobFilter } from '../../ai/api'

  let { autoMode = 'off' }: { autoMode?: 'off' | 'suggest' | 'apply' } = $props()

  let filter = $state<JobFilter>({ only_unprocessed: true, without_correspondent: false, without_document_type: false, without_tags: false, tags: [], created_from: null, created_to: null, max_documents: 0 })
  let mode = $state<'suggest' | 'apply'>('suggest')
  let count = $state<{ count: number; limit: number; per_minute: number } | null>(null)
  let jobs = $state<Job[]>([])
  let busy = $state(false)
  let timer: ReturnType<typeof setInterval> | null = null

  const tags = $derived([...data.tags.values()].sort((a, b) => a.name.localeCompare(b.name)))
  const running = $derived(jobs.some((j) => j.status === 'queued' || j.status === 'running'))
  const STATUS_KEY: Record<Job['status'], string> = {
    queued: 'm.ai_settings.job_queued',
    running: 'm.ai_settings.job_running',
    done: 'm.ai_settings.job_done',
    cancelled: 'm.ai_settings.job_cancelled',
    failed: 'm.ai_settings.job_failed',
    interrupted: 'm.ai_settings.job_interrupted',
  }

  async function refresh() {
    try {
      jobs = (await listJobs()).results
    } catch {
      // List stays
    }
  }
  onMount(() => {
    refresh()
    timer = setInterval(() => running && refresh(), 5000)
    return () => timer && clearInterval(timer)
  })

  const clean = (): JobFilter => ({ ...filter, created_from: filter.created_from || null, created_to: filter.created_to || null, max_documents: Number(filter.max_documents) || 0 })
  async function estimate() {
    busy = true
    try {
      count = await previewJob(clean())
    } catch (e) {
      toastService.showError(t('m.ai_settings.test_failed'), e)
    } finally {
      busy = false
    }
  }
  async function start() {
    busy = true
    try {
      await createJob(mode, clean())
      count = null
      await refresh()
    } catch (e: any) {
      toastService.showError(e?.status === 409 ? t('m.ai_settings.job_busy') : t('m.ai_settings.could_not_start'), e)
    } finally {
      busy = false
    }
  }
  async function act(fn: () => Promise<Job>) {
    try {
      await fn()
      await refresh()
    } catch (e) {
      toastService.showError(t('m.ai_settings.could_not_save'), e)
    }
  }
  const minutes = (n: number, perMin: number) => Math.max(1, Math.ceil(n / Math.max(1, perMin)))
  const pct = (j: Job) => (j.total ? Math.round((100 * j.processed) / j.total) : 0)
</script>

<div class="bf">
  <fieldset class="flt">
    <legend class="d2-label">{t('m.ai_settings.which_documents')}</legend>
    <label class="d2-check"><input type="checkbox" bind:checked={filter.only_unprocessed} />{t('m.ai_settings.only_unprocessed')}</label>
    <label class="d2-check"><input type="checkbox" bind:checked={filter.without_correspondent} />{t('m.ai_settings.without_correspondent')}</label>
    <label class="d2-check"><input type="checkbox" bind:checked={filter.without_document_type} />{t('m.ai_settings.without_document_type')}</label>
    <label class="d2-check"><input type="checkbox" bind:checked={filter.without_tags} />{t('m.ai_settings.without_tags')}</label>
    <div class="grid2">
      <label class="d2-label" for="d2-ai-bf-tags">{t('m.ai_settings.with_tags')}</label>
      <select class="d2-select" id="d2-ai-bf-tags" multiple size="4" bind:value={filter.tags}>
        {#each tags as tg (tg.id)}<option value={tg.id}>{tg.name}</option>{/each}
      </select>
      <label class="d2-label" for="d2-ai-bf-from">{t('m.ai_settings.created_from')}</label>
      <input class="d2-input" id="d2-ai-bf-from" type="date" bind:value={filter.created_from} />
      <label class="d2-label" for="d2-ai-bf-to">{t('m.ai_settings.created_to')}</label>
      <input class="d2-input" id="d2-ai-bf-to" type="date" bind:value={filter.created_to} />
      <label class="d2-label" for="d2-ai-bf-max">{t('m.ai_settings.at_most')}</label>
      <input class="d2-input num" id="d2-ai-bf-max" type="number" min="0" bind:value={filter.max_documents} placeholder="0" />
      <label class="d2-label" for="d2-ai-bf-mode">{t('m.ai_settings.what_happens')}</label>
      <select class="d2-select" id="d2-ai-bf-mode" bind:value={mode}>
        <option value="suggest">{t('m.ai_settings.mode_suggest')}</option>
        <option value="apply" disabled={autoMode !== 'apply'}>{t('m.ai_settings.mode_apply')}</option>
      </select>
    </div>
    {#if autoMode !== 'apply'}<p class="d2-muted small">{t('m.ai_settings.apply_needs_mode')}</p>{/if}
  </fieldset>
  <div class="row">
    <button class="d2-btn sm" disabled={busy} onclick={estimate}><Icon name="search" />{t('m.ai_settings.how_many')}</button>
    {#if count}
      <span>{t('m.ai_settings.count_result', { PH: Math.min(count.count, count.limit), PH_1: minutes(Math.min(count.count, count.limit), count.per_minute) })}</span>
      <button class="d2-btn primary sm" disabled={busy || running || count.count === 0} onclick={start} id="d2-ai-bf-start"><Icon name="stars" />{t('m.ai_settings.start')}</button>
    {/if}
  </div>

  {#if jobs.length}
    <h3>{t('m.ai_settings.jobs')}</h3>
    <ul class="jobs">
      {#each jobs as j (j.id)}
        <li>
          <div class="jh">
            <strong>{t(STATUS_KEY[j.status])}</strong>
            <span class="d2-muted small">{j.created ? new Date(j.created).toLocaleString() : ''} · {j.mode === 'apply' ? t('m.ai_settings.mode_apply') : t('m.ai_settings.mode_suggest')}</span>
            <span class="grow"></span>
            {#if j.status === 'queued' || j.status === 'running'}
              <button class="d2-btn ghost sm" disabled={j.cancel_requested} onclick={() => act(() => cancelJob(j.id))}><Icon name="x-lg" />{t('m.ai_settings.stop')}</button>
            {:else if j.status === 'interrupted'}
              <button class="d2-btn ghost sm" disabled={running} onclick={() => act(() => resumeJob(j.id))}><Icon name="arrow-clockwise" />{t('m.ai_settings.resume')}</button>
              <button class="d2-btn ghost sm" onclick={() => act(() => cancelJob(j.id))}><Icon name="x-lg" />{t('m.ai_settings.stop')}</button>
            {/if}
          </div>
          <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax={j.total} aria-valuenow={j.processed} aria-label={t('m.ai_settings.progress')}><span style:width="{pct(j)}%"></span></div>
          <span class="d2-muted small">{t('m.ai_settings.job_counts', { PH: j.processed, PH_1: j.total, PH_2: j.suggested, PH_3: j.applied, PH_4: j.errors })}</span>
          {#if j.suggested > 0}<a class="small" href={router.href('ai/review')}>{t('m.ai.review_title')}</a>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .bf {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
  }
  .flt {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    margin: 0;
    padding: 0;
    border: 0;
  }
  .grid2 {
    display: grid;
    grid-template-columns: minmax(140px, 220px) minmax(0, 1fr);
    gap: var(--d2-s2) var(--d2-s3);
    align-items: center;
  }
  .num {
    max-width: 140px;
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
  h3 {
    font-size: var(--d2-fs-md);
  }
  .jobs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
  }
  .jobs li {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .jh {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--d2-s2);
  }
  .grow {
    flex: 1;
  }
  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--d2-surface-3);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--d2-accent);
  }
  @media (max-width: 700px) {
    .grid2 {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
