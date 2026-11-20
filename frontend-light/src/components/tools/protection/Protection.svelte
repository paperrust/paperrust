<!-- Ransomware protection (PaperRust extension, superuser only). Route protection, host <pngx-protection> in App.svelte.
     Top: state (normal / read-only), guard events with affected files, lift read-only mode.
     Bottom: encrypted backups (target, last success, size, errors, runs, back up now).
     API: /api/x-guard/… (crates/pl-guard/src/api.rs). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { session } from '../../../stores/settings.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import {
    backupOverview, formatBytes, guardEvent, guardOverview, liftReadOnly, REASON_KEYS, runBackup,
    type BackupOverview, type Finding, type GuardEvent, type GuardOverview,
  } from './guard-api'

  const DOCS = '/rs/docs/ransomware.md'
  const isSuperuser = $derived(!!session.data?.user?.is_superuser)

  let overview = $state<GuardOverview | null>(null)
  let backup = $state<BackupOverview | null>(null)
  let openEvent = $state<number | null>(null)
  let findings = $state<Finding[]>([])
  let loadingFindings = $state(false)
  let note = $state('')
  let lifting = $state(false)
  let starting = $state(false)

  onMount(() => {
    if (!isSuperuser) return
    load()
  })

  async function load() {
    try {
      overview = await guardOverview()
      const ev = overview.state.event_id
      if (overview.state.read_only && ev && openEvent === null) showEvent(ev)
    } catch {
      toastService.showError(t('protection.could_not_load'))
    }
    try {
      backup = await backupOverview()
    } catch {
      toastService.showError(t('protection.could_not_load'))
    }
  }

  async function showEvent(id: number) {
    if (openEvent === id) {
      openEvent = null
      return
    }
    openEvent = id
    loadingFindings = true
    try {
      findings = (await guardEvent(id)).findings
    } catch {
      findings = []
      toastService.showError(t('protection.could_not_load'))
    } finally {
      loadingFindings = false
    }
  }

  async function lift() {
    lifting = true
    try {
      await liftReadOnly(note.trim())
      note = ''
      toastService.show({ content: t('protection.lifted'), delay: 5000, level: 'success' })
      window.dispatchEvent(new Event('paperrust:guard-changed'))
      await load()
    } catch {
      toastService.showError(t('protection.could_not_lift'))
    } finally {
      lifting = false
    }
  }

  async function startBackup() {
    starting = true
    try {
      await runBackup()
      toastService.show({ content: t('protection.backup_started'), delay: 5000, level: 'success' })
      setTimeout(load, 3000)
    } catch (e: any) {
      toastService.showError(e?.body?.non_field_errors?.[0] ?? t('protection.could_not_start_backup'))
    } finally {
      starting = false
    }
  }

  const fmt = (s: string | null | undefined) => (s ? new Date(s).toLocaleString() : '–')
  const reason = (r: string) => (REASON_KEYS[r] ? t(REASON_KEYS[r]) : r)
  const statusClass = (s: string) => (s === 'ok' ? 'text-bg-success' : s === 'error' ? 'text-bg-danger' : 'text-bg-secondary')
  const statusLabel = (s: string) => (s === 'ok' ? t('protection.run_ok') : s === 'error' ? t('protection.run_error') : t('protection.run_running'))
  const eventLabel = (e: GuardEvent) => (e.status === 'open' ? t('protection.event_open') : t('protection.event_resolved'))
</script>

<pngx-page-header>
  <PageHeader title={t('protection.title')} subTitle={t('protection.subtitle')}>
    <button class="btn btn-sm btn-outline-secondary me-2" type="button" onclick={load}>
      <pngx-bs-icon name="arrow-clockwise" class="me-1">{@html icon('arrow-clockwise')}</pngx-bs-icon>{t('protection.reload')}
    </button>
    <a class="btn btn-sm btn-outline-secondary" href={DOCS} target="_blank" rel="noopener noreferrer">
      <pngx-bs-icon name="question-circle" class="me-1">{@html icon('question-circle')}</pngx-bs-icon>{t('common.documentation')}
    </a>
  </PageHeader>
</pngx-page-header>

{#if !isSuperuser}
  <div class="alert alert-warning">{t('protection.superuser_only')}</div>
{:else if overview}
  <!-- Guard -->
  <h5 class="mt-2">{t('protection.guard_heading')}</h5>
  {#if overview.state.read_only}
    <div class="card border-danger mb-3">
      <div class="card-body">
        <h6 class="text-danger">
          <pngx-bs-icon name="shield-exclamation" class="me-1">{@html icon('shield-exclamation')}</pngx-bs-icon>{t('protection.read_only_active')}
        </h6>
        <p class="mb-1">{overview.state.reason}</p>
        <p class="small text-muted mb-2">{t('protection.since', { PH: fmt(overview.state.since) })}</p>
        <ol class="small mb-3">
          <li>{t('protection.step_isolate')}</li>
          <li>{t('protection.step_review')}</li>
          <li>{t('protection.step_restore')}</li>
          <li>{t('protection.step_lift')}</li>
        </ol>
        <div class="d-flex flex-wrap gap-2 align-items-center">
          <input class="form-control form-control-sm w-auto flex-grow-1" maxlength="2000" bind:value={note} placeholder={t('protection.note_placeholder')} aria-label={t('protection.note_placeholder')} />
          <pngx-confirm-button>
            <ConfirmButton label={t('protection.lift')} confirmMessage={t('protection.lift_confirm')} buttonClasses="btn-sm btn-danger" disabled={lifting} onconfirm={lift} />
          </pngx-confirm-button>
        </div>
      </div>
    </div>
  {:else}
    <div class="alert alert-success d-flex align-items-center gap-2">
      <pngx-bs-icon name="shield-check">{@html icon('shield-check')}</pngx-bs-icon>
      <span>{overview.guard.enabled ? t('protection.normal') : t('protection.guard_off')}</span>
    </div>
  {/if}
  <p class="small text-muted">
    {t('protection.guard_settings', { PH: String(overview.guard.threshold), PH_1: String(overview.guard.window_minutes) })}
    {overview.guard.watch ? t('protection.watch_on') : t('protection.watch_off')}
  </p>

  <h6>{t('protection.events_heading')}</h6>
  {#if overview.results.length}
    <ul class="list-group mb-4">
      {#each overview.results as e (e.id)}
        <li class="list-group-item">
          <div class="d-flex flex-wrap align-items-center gap-2">
            <span class="badge {e.status === 'open' ? 'text-bg-danger' : 'text-bg-secondary'}">{eventLabel(e)}</span>
            <span class="me-auto">{fmt(e.created)} – {e.summary}</span>
            <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => showEvent(e.id)}>
              {t('protection.affected_files', { PH: String(e.files) })}
            </button>
          </div>
          {#if e.note}<div class="small text-muted mt-1">{e.note}</div>{/if}
          {#if openEvent === e.id}
            {#if loadingFindings}
              <div class="spinner-border spinner-border-sm mt-2" role="status"></div>
            {:else}
              <div class="table-responsive mt-2">
                <table class="table table-sm small mb-0">
                  <thead><tr><th>{t('protection.file')}</th><th>{t('protection.reason')}</th><th>{t('protection.detail')}</th></tr></thead>
                  <tbody>
                    {#each findings as f (f.id)}
                      <tr><td><code>{f.path}</code></td><td>{reason(f.reason)}</td><td class="text-muted">{f.detail}</td></tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <div class="alert alert-light border small mb-4">{t('protection.no_events')}</div>
  {/if}

  <!-- Backups -->
  <h5>{t('protection.backup_heading')}</h5>
  {#if backup}
    {#if backup.config_error}
      <div class="alert alert-warning small">{backup.config_error}</div>
    {:else if !backup.target}
      <div class="alert alert-warning small">{t('protection.backup_not_configured')}</div>
    {:else}
      <div class="card mb-3">
        <div class="card-body small">
          <div class="row g-2">
            <div class="col-sm-6"><span class="text-muted">{t('protection.backup_target')}:</span> <code>{backup.target}</code></div>
            <div class="col-sm-6"><span class="text-muted">{t('protection.backup_schedule')}:</span> <code>{backup.schedule}</code></div>
            <div class="col-sm-6">
              <span class="text-muted">{t('protection.last_success')}:</span>
              {#if backup.last_success}{fmt(backup.last_success.finished)}{:else}<span class="text-danger">{t('protection.never')}</span>{/if}
            </div>
            {#if backup.last_success}
              <div class="col-sm-6">
                <span class="text-muted">{t('protection.backup_size')}:</span>
                {t('protection.backup_size_value', { PH: String(backup.last_success.files_total), PH_1: formatBytes(backup.last_success.bytes_total + backup.last_success.db_bytes) })}
              </div>
            {/if}
          </div>
          <button class="btn btn-sm btn-primary mt-3" type="button" onclick={startBackup} disabled={starting}>
            {#if starting}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{:else}<pngx-bs-icon name="cloud-arrow-up" class="me-1">{@html icon('cloud-arrow-up')}</pngx-bs-icon>{/if}{t('protection.backup_now')}
          </button>
        </div>
      </div>
    {/if}
    {#if backup.runs.length}
      <div class="table-responsive">
        <table class="table table-sm small">
          <thead>
            <tr>
              <th>{t('protection.run_started')}</th><th>{t('protection.run_status')}</th><th>{t('protection.run_snapshot')}</th>
              <th class="text-end">{t('protection.run_files')}</th><th class="text-end">{t('protection.run_uploaded')}</th><th>{t('protection.run_message')}</th>
            </tr>
          </thead>
          <tbody>
            {#each backup.runs as r (r.id)}
              <tr>
                <td>{fmt(r.started)}</td>
                <td><span class="badge {statusClass(r.status)}">{statusLabel(r.status)}</span></td>
                <td><code>{r.snapshot ?? '–'}</code></td>
                <td class="text-end">{r.files_total} (+{r.files_new})</td>
                <td class="text-end">{formatBytes(r.bytes_new)}</td>
                <td class="text-muted text-break">{r.error || r.warnings}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
{:else}
  <div class="spinner-border spinner-border-sm" role="status"></div>
{/if}
