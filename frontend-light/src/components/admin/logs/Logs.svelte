<!-- Port of admin/logs/logs.component.html (route logs). Host <pngx-logs> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/logs.css'
  import { onDestroy, tick } from 'svelte'
  import { t } from '../../../lib/i18n'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import { getLog, hasChanges, listLogs, parseLogsWithLevel } from './logs'

  let logs = $state<{ message: string; level: number }[]>([])
  let logFiles = $state<string[]>([])
  let activeLog = $state<string | undefined>(undefined)
  let autoRefreshEnabled = $state(true)
  let limit = $state(5000)
  let showJumpToBottom = $state(false)
  let loading = $state(true)
  let logContainer = $state<HTMLElement>()
  let timer: ReturnType<typeof setInterval> | undefined
  let debounce: ReturnType<typeof setTimeout> | undefined
  let destroyed = false

  listLogs().then((result) => {
    if (destroyed) return
    logFiles = result
    loading = false
    if (logFiles.length > 0) {
      activeLog = logFiles[0]
      reloadLogs()
    }
    // timer(5000, 5000) with filter autoRefreshEnabled
    timer = setInterval(() => autoRefreshEnabled && reloadLogs(), 5000)
  })
  onDestroy(() => {
    destroyed = true
    clearInterval(timer)
    clearTimeout(debounce)
  })

  function onLimitChange() {
    clearTimeout(debounce)
    debounce = setTimeout(reloadLogs, 300)
  }

  function isNearBottom() {
    const el = logContainer
    if (!el) return true
    return el.scrollHeight - el.scrollTop - el.clientHeight <= 40
  }

  async function reloadLogs() {
    if (!activeLog) return
    loading = true
    const stick = isNearBottom()
    try {
      const parsed = parseLogsWithLevel(await getLog(activeLog, limit))
      if (destroyed) return
      loading = false
      if (hasChanges(parsed, logs)) {
        logs = parsed
        if (stick) scrollToBottom()
        showJumpToBottom = !stick
      }
    } catch {
      logs = []
      loading = false
    }
  }

  async function scrollToBottom() {
    if (!logContainer) return
    await tick()
    logContainer.scrollTop = logContainer.scrollHeight
    showJumpToBottom = false
  }

  function select(e: Event, f: string) {
    e.preventDefault()
    if (f === activeLog) return
    activeLog = f
    reloadLogs()
  }
</script>

<pngx-page-header>
<PageHeader
  title={t('common.logs')}
  info={t('admin.logs.review_log_files_application')}>
  <div class="input-group input-group-sm align-items-center">
    <div class="input-group input-group-sm me-3">
      <label class="input-group-text text-muted" for="logLineLimit">{t('common.show')}</label>
      <input
        id="logLineLimit"
        class="form-control"
        type="number"
        min="100"
        step="100"
        bind:value={limit}
        oninput={onLimitChange}
        style={"width: 100px;"}>
      <span class="input-group-text text-muted">{t('admin.logs.lines')}</span>
    </div>
    <div class="form-check form-switch mt-1">
      <input id="autoRefreshSwitch" class="form-check-input" type="checkbox" role="switch" bind:checked={autoRefreshEnabled}>
      <label class="form-check-label" for="autoRefreshSwitch">{t('common.auto_refresh')}</label>
    </div>
  </div>
</PageHeader>
</pngx-page-header>

<ul class="nav-tabs nav" role="tablist">
  {#each logFiles as logFile, i (logFile)}
    <li class="nav-item" role="presentation">
      <a href="" class="nav-link" class:active={activeLog === logFile} role="tab" id="ngb-nav-{i}" aria-selected={activeLog === logFile} aria-disabled="false" onclick={(e) => select(e, logFile)}>
        {logFile}.log
      </a>
    </li>
  {/each}
  {#if loading || !logFiles.length}
    <div class="ps-2 d-flex align-items-center">
      <div class="spinner-border spinner-border-sm me-2" role="status"></div>
      {#if !logFiles.length}
        {t('common.loading')}
      {/if}
    </div>
  {/if}
</ul>

<div bind:this={logContainer} class="bg-dark text-light font-monospace log-container p-3" onscroll={() => (showJumpToBottom = !isNearBottom())}>
  {#if loading && !logFiles.length}
    <div>
      <div class="spinner-border spinner-border-sm me-2" role="status"></div>
      {t('common.loading')}
    </div>
  {:else}
    {#each logs as log, i (i)}
      <p class="m-0 p-0 {'log-entry-' + log.level}">{log.message}</p>
    {/each}
  {/if}
</div>
<button
  type="button"
  class="btn btn-sm btn-secondary jump-to-bottom position-fixed bottom-0 end-0 m-5"
  class:visible={showJumpToBottom}
  onclick={scrollToBottom}
>
  ↓ <span>{t('admin.logs.jump_bottom')}</span>
</button>
