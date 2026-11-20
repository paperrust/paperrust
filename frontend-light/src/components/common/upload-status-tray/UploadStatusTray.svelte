<!-- Port of common/upload-status-tray/upload-status-tray.component.html (+ ngb-alert/ngb-progressbar-Markup).
     Host <pngx-upload-status-tray> in the parent markup (AppFrame). -->
<script lang="ts">
  import '../../../styles/components/upload-status-tray.css'
  import '../../../styles/parts/alert.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { router } from '../../../router.svelte'
  import { consumerStatus as svc, FileStatusPhase, type FileStatus } from '../../../stores/uploads.svelte'

  const canAddDocuments = $derived(session.can('add', 'document'))
  const statuses = $derived(svc.getConsumerStatus())
  const collapsed = $derived(svc.trayCollapsed)
  const slimSidebarEnabled = $derived(session.get(SETTINGS_KEYS.SLIM_SIDEBAR))
  /** ngb-alert closes with a fade-out transition, then (closed) */
  let closing = $state(new Set<FileStatus>())

  function getStatusSummary() {
    const strings: string[] = []
    const countUploadingAndProcessing = svc.getConsumerStatusNotCompleted().length
    const countFailed = svc.getConsumerStatus(FileStatusPhase.FAILED).length
    const countSuccess = svc.getConsumerStatus(FileStatusPhase.SUCCESS).length
    if (countUploadingAndProcessing > 0) strings.push(t('ui.processing', { PH: countUploadingAndProcessing }))
    if (countFailed > 0) strings.push(t('ui.failed', { PH: countFailed }))
    if (countSuccess > 0) strings.push(t('ui.added', { PH: countSuccess }))
    // Source text ", " (key after whitespace collapsing ",")
    const sep = t('ui.text')
    return strings.join(sep === ',' ? ', ' : sep)
  }
  const summary = $derived(getStatusSummary())
  const completed = $derived(svc.getConsumerStatusCompleted())

  const isFinished = (s: FileStatus) => s.phase == FileStatusPhase.FAILED || s.phase == FileStatusPhase.SUCCESS
  function getStatusColor(s: FileStatus) {
    switch (s.phase) {
      case FileStatusPhase.FAILED:
        return 'danger'
      case FileStatusPhase.SUCCESS:
        return 'success'
      default:
        return 'primary'
    }
  }
  const pct = (s: FileStatus) => 100 * Math.min(Math.max(s.getProgress() || 0, 0), 1)
  const here = $derived(router.href(router.path) + router.query)

  function dismissCompleted(e: Event) {
    e.preventDefault()
    for (const s of svc.getConsumerStatusCompleted()) svc.dismiss(s)
  }
  function closeAlert(s: FileStatus) {
    closing = new Set([...closing, s])
    setTimeout(() => {
      svc.dismiss(s)
      closing = new Set([...closing].filter((x) => x !== s))
    }, 150)
  }
</script>

{#if canAddDocuments && statuses.length > 0}
  <div class="fixed-bottom p-2 p-md-4 d-flex justify-content-end pe-none consumer-status-list {slimSidebarEnabled ? 'col-slim' : 'offset-sidebar-expanded'}">
    <div class="col col-lg-4 col-xl-3 ps-0 pe-0 ps-lg-3 pe-lg-0 pe-auto overflow-y-auto">
      <div class="card shadow-sm consumer-status-card">
        <div class="card-body">
          <div class="d-flex flex-wrap align-items-center column-gap-2">
            <p class="m-0 small text-muted">{summary}</p>
            <div class="d-flex align-items-center gap-2 ms-auto">
              {#if completed.length > 0}
                <a class="btn-link small text-nowrap" onclick={dismissCompleted} href={here}>
                  <span class="me-1">{t('ui.dismiss_completed')}</span>
                  <pngx-bs-icon name="check2-all">{@html icon('check2-all')}</pngx-bs-icon>
                </a>
              {/if}
              <button type="button" class="btn btn-sm btn-link p-0 text-muted" onclick={() => (svc.trayCollapsed = !svc.trayCollapsed)}
                aria-expanded={!collapsed} aria-label={t('ui.toggle_document_processing_status')}>
                <pngx-bs-icon>{@html icon(collapsed ? 'chevron-up' : 'chevron-down')}</pngx-bs-icon>
              </button>
            </div>
          </div>
          {#if !collapsed}
            {#each statuses as status (status)}
              {@render consumerAlert(status)}
            {/each}
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

{#snippet consumerAlert(status: FileStatus)}
  <ngb-alert class="mt-2 mb-0 alert fade alert-secondary" class:show={!closing.has(status)} class:alert-dismissible={isFinished(status)} role="alert">
    <h6 class="alert-heading">{status.filename}</h6>
    {#if !isFinished(status) || (isFinished(status) && !status.documentId)}
      <p class="mb-0 pb-1">{status.message}</p>
    {/if}
    <ngb-progressbar class="progress" role="progressbar" aria-valuenow={Math.min(Math.max(status.getProgress() || 0, 0), 1)} aria-valuemin="0" aria-valuemax="1" aria-label="progress bar"><div class="progress-bar text-bg-{getStatusColor(status)}" style:width="{pct(status)}%"></div></ngb-progressbar>
    {#if session.can('view', 'document')}
      <div>
        {#if isFinished(status)}
          <div>
            {#if status.documentId}
              <a class="btn btn-sm btn-outline-primary btn-open" href={router.href(`documents/${status.documentId}`)} onclick={() => svc.dismiss(status)}>
                <small>{t('common.open_document')}</small>
                <pngx-bs-icon name="arrow-right-short">{@html icon('arrow-right-short')}</pngx-bs-icon>
              </a>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
    {#if isFinished(status)}
      <button type="button" class="btn-close" aria-label="Close" onclick={() => closeAlert(status)}></button>
    {/if}
  </ngb-alert>
{/snippet}
