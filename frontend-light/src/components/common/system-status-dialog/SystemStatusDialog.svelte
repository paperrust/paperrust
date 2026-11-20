<!-- Port of common/system-status-dialog (SystemStatusDialogComponent). Data from /api/status/ (passed in by Settings),
     "Run Task" → POST /api/tasks/run/ (superuser only), WebSocket status from stores/uploads. -->
<script lang="ts">
  import type { Snippet } from 'svelte'
  import { onMount } from 'svelte'
  import { t, tRich, locale } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { formatDate } from '../../../lib/dates'
  import { Popover } from '../../../lib/popover.svelte'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { consumerStatus } from '../../../stores/uploads.svelte'
  import { systemStatus as fetchSystemStatus } from '../../../api/objects'
  import { runTask as runTaskApi } from '../../../api/admin'
  import { SystemStatusItemStatus as S, RunnableTask, isStale, fileSize, statusIconClass } from './system-status'

  let { modal, status: initialStatus }: { modal: ModalRef<any>; status?: any } = $props()

  // svelte-ignore state_referenced_locally
  let status = $state<any>(initialStatus)
  let copied = $state(false)
  let runningTasks = $state<string[]>([])
  let wsConnected = $state(consumerStatus.isConnected())

  const currentUserIsSuperUser = $derived(!!(session.data?.user as any)?.is_superuser)
  const aiEnabled = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const fmt = (d: string | null | undefined) => formatDate(d, 'medium', locale())

  onMount(() => {
    if (!status) fetchSystemStatus().then((s) => (status = s)).catch(() => {})
    // WebsocketStatusService.onConnectionStatus: poll the connection state (not reactive in the store) regularly
    const timer = setInterval(() => (wsConnected = consumerStatus.isConnected()), 1000)
    return () => clearInterval(timer)
  })

  // Popover per status button (triggers="click mouseenter:mouseleave")
  const pop = () => new Popover({ triggers: 'click mouseenter:mouseleave' })
  const popDatabase = pop()
  const popMigration = pop()
  const popRedis = pop()
  const popCelery = pop()
  const popIndex = pop()
  const popClassifier = pop()
  const popSanity = pop()
  const popLlm = pop()

  function copy() {
    const out = { ...status, websocket_connected: wsConnected ? S.OK : S.ERROR }
    navigator.clipboard?.writeText(JSON.stringify(out, null, 4)).catch(() => {})
    copied = true
    setTimeout(() => (copied = false), 3000)
  }

  const isRunning = (task: string) => runningTasks.includes(task)
  function setTaskRunning(task: string, running: boolean) {
    runningTasks = running ? [...runningTasks.filter((x) => x !== task), task] : runningTasks.filter((x) => x !== task)
  }

  async function runTask(task: string) {
    setTaskRunning(task, true)
    toastService.showInfo(`Task ${task} started`)
    try {
      await runTaskApi(task)
      setTaskRunning(task, false)
      fetchSystemStatus()
        .then((update) => (status = { ...status, ...update }))
        .catch(() => {})
    } catch (err) {
      setTaskRunning(task, false)
      toastService.showError(`Failed to start task ${task}, see the logs for more details`, err)
    }
  }
</script>

{#snippet popWindow(p: Popover, body: Snippet)}
  {#if p.open}
    <ngb-popover-window role="tooltip" class="popover" id={p.id} use:p.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">{@render body()}</div></ngb-popover-window>
  {/if}
{/snippet}

{#snippet okIcon()}
  <pngx-bs-icon name="check-circle-fill" class="text-success ms-2 lh-1">{@html icon('check-circle-fill')}</pngx-bs-icon>
{/snippet}
{#snippet warnIcon(cls: string)}
  <pngx-bs-icon name="exclamation-triangle-fill" class="ms-2 lh-1 {cls}">{@html icon('exclamation-triangle-fill')}</pngx-bs-icon>
{/snippet}

{#snippet runButton(task: string)}
  {#if currentUserIsSuperUser}
    {#if isRunning(task)}
      <div class="spinner-border spinner-border-sm ms-2" role="status"></div>
    {:else}
      <button class="btn btn-sm d-flex align-items-center btn-dark small ms-2" style={"font-size: 0.75rem;"} onclick={() => runTask(task)}>
        <pngx-bs-icon name="play-fill" class="me-1">{@html icon('play-fill')}</pngx-bs-icon>
        {t('admin.status.run_task')}
      </button>
    {/if}
  {/if}
{/snippet}

<pngx-system-status-dialog><div class="modal-header">
  <h6 class="modal-title" id="modal-basic-title">{t('admin.settings.system_status')}</h6>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
  {#if !status}
    <div class="w-100 h-100 d-flex align-items-center justify-content-center">
      <div>
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>
        {t('common.loading')}
      </div>
    </div>
  {:else}
    <div class="row row-cols-1 row-cols-md-4 g-3">
      <div class="col">
        <div class="card bg-light h-100">
          <div class="card-header">
            <h6 class="card-title mb-0">{t('admin.status.environment')}</h6>
          </div>
          <div class="card-body">
            <dl class="card-text">
              <dt>{t('admin.status.version')}</dt>
              <dd>
                {status.pngx_version}
              </dd>
              <dt>{t('admin.status.install_type')}</dt>
              <dd>{status.install_type}</dd>
              <dt>{t('admin.status.server_os')}</dt>
              <dd>{status.server_os}</dd>
              <dt>{t('admin.status.media_storage')}</dt>
              <dd>
                {#if status.storage}
                  {@const used = status.storage.total - status.storage.available}
                  <ngb-progressbar style={"height: 4px;"} class="mt-2 mb-1 progress" role="progressbar" aria-valuenow={used} aria-valuemin="0" aria-valuemax={status.storage.total}><div class="progress-bar bg-primary" style="width: {status.storage.total ? Math.min(100, Math.max(0, (used / status.storage.total) * 100)) : 0}%;"></div></ngb-progressbar>
                  <span class="small">{fileSize(status.storage.available)} {t('admin.status.available')} ({fileSize(status.storage.total)} {t('admin.status.total_lower')})</span>
                {/if}
              </dd>
            </dl>
          </div>
        </div>
      </div>

      <div class="col">
        <div class="card bg-light h-100">
          <div class="card-header">
            <h6 class="card-title mb-0">{t('admin.status.database')}</h6>
          </div>
          <div class="card-body">
            <dl class="card-text">
              <dt>{t('ui.type')}</dt>
              <dd>{status.database?.type}</dd>
              <dt>{t('common.status')}</dt>
              <dd>
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popDatabase.triggerEl>
                  {status.database?.status}
                  {#if status.database?.status === S.OK}{@render okIcon()}{:else}{@render warnIcon('text-danger')}{/if}
                </button>
                {#snippet dbBody()}
                  {#if status.database?.status === S.OK}{status.database?.url}{:else}{status.database?.url}: {status.database?.error}{/if}
                {/snippet}
                {@render popWindow(popDatabase, dbBody)}
              </dd>
              <dt>{t('admin.status.migration_status')}</dt>
              <dd>
                {#if status.database?.migration_status}
                  {@const ms = status.database.migration_status}
                  <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popMigration.triggerEl>
                    {#if (ms.unapplied_migrations ?? []).length === 0}
                      {t('admin.status.up_to_date')}{@render okIcon()}
                    {:else}
                      {ms.unapplied_migrations.length} Pending{@render warnIcon('text-warning')}
                    {/if}
                  </button>
                  {#snippet migBody()}
                    <h6>{t('admin.status.latest_migration')}:</h6> <span class="font-monospace small">{ms.latest_migration}</span>
                    {#if (ms.unapplied_migrations ?? []).length > 0}
                      <h6 class="mt-3">{t('admin.status.pending_migrations')}:</h6>
                      <ul>
                        {#each ms.unapplied_migrations as migration (migration)}
                          <li class="font-monospace small">{migration}</li>
                        {/each}
                      </ul>
                    {/if}
                  {/snippet}
                  {@render popWindow(popMigration, migBody)}
                {/if}
              </dd>
            </dl>
          </div>
        </div>
      </div>

      <div class="col">
        <div class="card bg-light h-100">
          <div class="card-header">
            <h6 class="card-title mb-0">{t('admin.status.task_queue')}</h6>
          </div>
          <div class="card-body">
            <dl class="card-text">
              <dt>{t('admin.status.redis')}</dt>
              <dd>
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popRedis.triggerEl>
                  {status.tasks?.redis_status}
                  {#if status.tasks?.redis_status === S.OK}{@render okIcon()}{:else}{@render warnIcon('text-danger')}{/if}
                </button>
                {#snippet redisBody()}
                  {#if status.tasks?.redis_status === S.OK}{status.tasks?.redis_url}{:else}{status.tasks?.redis_url}: {status.tasks?.redis_error}{/if}
                {/snippet}
                {@render popWindow(popRedis, redisBody)}
              </dd>
              <dt>{t('admin.status.celery')}</dt>
              <dd>
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popCelery.triggerEl>
                  {status.tasks?.celery_status}
                  {#if status.tasks?.celery_status === S.OK}{@render okIcon()}{:else}{@render warnIcon(statusIconClass(status.tasks?.celery_status))}{/if}
                </button>
                {#snippet celeryBody()}
                  {#if status.tasks?.celery_status === S.OK}{status.tasks?.celery_url}{:else}{status.tasks?.celery_error}{/if}
                {/snippet}
                {@render popWindow(popCelery, celeryBody)}
              </dd>
              {#if status.tasks?.summary}
                {@const summary = status.tasks.summary}
                <dt>{#each tRich('admin.status.recent_activity', { INTERPOLATION: summary.days }) as seg}{#if seg.tag === 'START_TAG_SPAN'}<span class="small text-muted fw-light">{seg.text}</span>{:else}{seg.text}{/if}{/each}</dt>
                <dd class="mb-0">
                  {#if summary.total_count > 0}
                    <ul class="list-group border-light mt-2">
                      <li class="list-group-item list-group-item-action d-flex justify-content-between align-items-center">
                        <span class="small">{t('admin.status.total')}:</span>
                        <span class="badge bg-light rounded-pill">{summary.total_count}</span>
                      </li>
                      <li class="list-group-item list-group-item-action d-flex justify-content-between align-items-center">
                        <span class="small">{t('admin.status.successful')}:</span>
                        <span class="badge bg-primary rounded-pill">{summary.success_count}</span>
                      </li>
                      <li class="list-group-item list-group-item-action d-flex justify-content-between align-items-center">
                        <span class="small">{t('dialog.share_bundle.status_failed')}:</span>
                        <span class="badge bg-danger rounded-pill">{summary.failure_count}</span>
                      </li>
                      <li class="list-group-item list-group-item-action d-flex justify-content-between align-items-center">
                        <span class="small">{t('dialog.share_bundle.status_pending')}:</span>
                        <span class="badge bg-warning rounded-pill">{summary.pending_count}</span>
                      </li>
                    </ul>
                  {:else}
                    <span class="small text-muted">{t('admin.status.no_recent_tasks')}</span>
                  {/if}
                </dd>
              {/if}
            </dl>
          </div>
        </div>
      </div>

      <div class="col">
        <div class="card bg-light h-100">
          <div class="card-header">
            <h6 class="card-title mb-0">{t('admin.status.health')}</h6>
          </div>
          <div class="card-body">
            <dl class="card-text">
              <dt>{t('admin.status.search_index')}</dt>
              <dd class="d-flex align-items-center">
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popIndex.triggerEl>
                  {status.tasks?.index_status}
                  {#if status.tasks?.index_status === S.OK}{@render okIcon()}{:else}{@render warnIcon('text-danger')}{/if}
                </button>
                {#snippet indexBody()}
                  {#if status.tasks?.index_status === S.OK}
                    <h6>{t('admin.status.last_updated')}:</h6> <span class="font-monospace small">{fmt(status.tasks?.index_last_modified)}</span>
                  {:else}
                    <h6>{t('common.error')}:</h6> <span class="font-monospace small">{status.tasks?.index_error}</span>
                  {/if}
                {/snippet}
                {@render popWindow(popIndex, indexBody)}
              </dd>
              <dt>{t('admin.status.classifier')}</dt>
              <dd class="d-flex align-items-center">
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popClassifier.triggerEl>
                  {status.tasks?.classifier_status}
                  {#if status.tasks?.classifier_status === S.OK}
                    {#if isStale(status.tasks?.classifier_last_trained)}{@render warnIcon('text-warning')}{:else}{@render okIcon()}{/if}
                  {:else}
                    {@render warnIcon(statusIconClass(status.tasks?.classifier_status))}
                  {/if}
                </button>
                {#snippet classifierBody()}
                  {#if status.tasks?.classifier_status === S.OK}
                    <h6>{t('admin.status.last_trained')}:</h6> <span class="font-monospace small">{fmt(status.tasks?.classifier_last_trained)}</span>
                  {:else}
                    <h6>{t('common.error')}:</h6> <span class="font-monospace small">{status.tasks?.classifier_error}</span>
                  {/if}
                {/snippet}
                {@render popWindow(popClassifier, classifierBody)}
                {@render runButton(RunnableTask.TrainClassifier)}
              </dd>
              <dt>{t('admin.status.sanity_checker')}</dt>
              <dd class="d-flex align-items-center">
                <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popSanity.triggerEl>
                  {status.tasks?.sanity_check_status}
                  {#if status.tasks?.sanity_check_status === S.OK}
                    {#if isStale(status.tasks?.sanity_check_last_run)}{@render warnIcon('text-warning')}{:else}{@render okIcon()}{/if}
                  {:else}
                    {@render warnIcon(statusIconClass(status.tasks?.sanity_check_status))}
                  {/if}
                </button>
                {#snippet sanityBody()}
                  {#if status.tasks?.sanity_check_status === S.OK}
                    <h6>{t('admin.status.last_run')}:</h6> <span class="font-monospace small">{fmt(status.tasks?.sanity_check_last_run)}</span>
                  {:else}
                    <h6>{t('common.error')}:</h6> <span class="font-monospace small">{status.tasks?.sanity_check_error}</span>
                  {/if}
                {/snippet}
                {@render popWindow(popSanity, sanityBody)}
                {@render runButton(RunnableTask.SanityCheck)}
              </dd>
              <dt>{t('admin.status.websocket')}</dt>
              <dd>
                <span class="btn btn-sm pe-none align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"}>
                  {#if wsConnected}
                    {t('admin.status.ok')}
                    {@render okIcon()}
                  {:else}
                    {t('common.error')}
                    {@render warnIcon('text-danger')}
                  {/if}
                </span>
              </dd>
              {#if aiEnabled}
                <dt>{t('admin.status.ai_index')}</dt>
                <dd class="d-flex align-items-center">
                  <button class="btn btn-sm d-flex align-items-center btn-dark text-uppercase small" style={"font-size: 0.75rem;"} use:popLlm.triggerEl>
                    {status.tasks?.llmindex_status}
                    {#if status.tasks?.llmindex_status === S.OK}
                      {#if isStale(status.tasks?.llmindex_last_modified)}{@render warnIcon('text-warning')}{:else}{@render okIcon()}{/if}
                    {:else}
                      {@render warnIcon(statusIconClass(status.tasks?.llmindex_status, true))}
                    {/if}
                  </button>
                  {#snippet llmBody()}
                    {#if status.tasks?.llmindex_status === S.OK}
                      <h6>{t('admin.status.last_run')}:</h6> <span class="font-monospace small">{fmt(status.tasks?.llmindex_last_modified)}</span>
                    {:else}
                      <h6>{t('common.error')}:</h6> <span class="font-monospace small">{status.tasks?.llmindex_error}</span>
                    {/if}
                  {/snippet}
                  {@render popWindow(popLlm, llmBody)}
                  {@render runButton(RunnableTask.LlmIndex)}
                </dd>
              {/if}
            </dl>
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>
<div class="modal-footer">
  <button class="btn btn-sm d-flex align-items-center btn-dark btn-outline-secondary" onclick={copy}>
    {#if !copied}
      <pngx-bs-icon name="clipboard-fill" class="me-1">{@html icon('clipboard-fill')}</pngx-bs-icon>
    {/if}
    {#if copied}
      <pngx-bs-icon name="clipboard-check-fill" class="me-1">{@html icon('clipboard-check-fill')}</pngx-bs-icon>
    {/if}
    {t('common.copy')}
  </button>
</div></pngx-system-status-dialog>
