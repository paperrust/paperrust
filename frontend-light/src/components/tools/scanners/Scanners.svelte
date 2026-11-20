<!-- Manage scanners (PaperRust extension, superuser only). Route tools/scanners, host <pngx-scanners> in App.svelte.
     Top: eSCL scanners (search the network, add by address, rename, remove).
     Bottom: devices for SMTP reception (create token – shown only once –, revoke, delete).
     API: /api/x-scan/… (crates/pl-scan/src/api.rs). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { session } from '../../../stores/settings.svelte'
  import { api } from '../../../api/client'
  import { router } from '../../../router.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import {
    addScanner, createDevice, deleteDevice, deleteScanner, discover, fieldMessage, listDevices, listScanners, renameScanner, revokeDevice, smtpHost,
    type Device, type Found, type Scanner, type SmtpInfo,
  } from '../scan/scan-api'

  const DOCS = '/rs/docs/scanner.md'
  const isSuperuser = $derived(!!session.data?.user?.is_superuser)

  let scanners = $state<Scanner[]>([])
  let found = $state<Found[] | null>(null)
  let searching = $state(false)
  let searchError = $state('')
  let manualUrl = $state('')
  let manualName = $state('')
  let adding = $state<string | null>(null)
  let addError = $state('')
  let editing = $state<number | null>(null)
  let editName = $state('')

  let devices = $state<Device[]>([])
  let smtp = $state<SmtpInfo | null>(null)
  let users = $state<{ id: number; username: string }[]>([])
  let devName = $state('')
  let devOwner = $state<number | null>(null)
  let devIps = $state('')
  let devError = $state('')
  let creating = $state(false)
  let newToken = $state<{ name: string; token: string } | null>(null)
  let copied = $state(false)

  const host = typeof location !== 'undefined' ? location.hostname : ''

  onMount(() => {
    if (!isSuperuser) return
    loadScanners()
    loadDevices()
    api
      .get<{ results: { id: number; username: string; is_active?: boolean }[] }>('/api/users/?page_size=100000&ordering=username')
      .then((r) => {
        users = r.results.filter((u) => u.is_active !== false)
        devOwner = session.data?.user?.id ?? users[0]?.id ?? null
      })
      .catch(() => {})
  })

  async function loadScanners() {
    try {
      scanners = await listScanners()
    } catch {
      toastService.showError(t('scan.could_not_load_scanners'))
    }
  }

  async function loadDevices() {
    try {
      const r = await listDevices()
      devices = r.results
      smtp = r.smtp
    } catch {
      toastService.showError(t('scanners.could_not_load_devices'))
    }
  }

  async function search() {
    searching = true
    searchError = ''
    try {
      const r = await discover()
      found = r.results
      if (r.error) searchError = t('scanners.search_failed')
    } catch {
      searchError = t('scanners.search_failed')
    } finally {
      searching = false
    }
  }

  async function add(url: string, name?: string) {
    adding = url
    addError = ''
    try {
      const s = await addScanner(url, name)
      toastService.show({ content: t('scanners.scanner_added', { PH: s.name }), delay: 5000, level: 'success' })
      manualUrl = ''
      manualName = ''
      await loadScanners()
      if (found) found = found.map((f) => (f.base_url === s.base_url ? { ...f, known: true } : f))
    } catch (e) {
      addError = fieldMessage(e)
    } finally {
      adding = null
    }
  }

  async function saveName(s: Scanner) {
    const name = editName.trim()
    if (!name) return
    try {
      await renameScanner(s.id, name)
      editing = null
      await loadScanners()
    } catch (e) {
      toastService.showError(fieldMessage(e))
    }
  }

  async function removeScanner(s: Scanner) {
    try {
      await deleteScanner(s.id)
      await loadScanners()
    } catch (e) {
      toastService.showError(fieldMessage(e))
    }
  }

  async function create() {
    if (devOwner === null || !devName.trim()) return
    creating = true
    devError = ''
    try {
      const d = await createDevice(devName.trim(), devOwner, devIps.trim())
      newToken = { name: d.name, token: d.token ?? '' }
      copied = false
      devName = ''
      devIps = ''
      await loadDevices()
    } catch (e) {
      devError = fieldMessage(e)
    } finally {
      creating = false
    }
  }

  async function revoke(d: Device) {
    try {
      await revokeDevice(d.id)
      await loadDevices()
    } catch (e) {
      toastService.showError(fieldMessage(e))
    }
  }

  async function remove(d: Device) {
    try {
      await deleteDevice(d.id)
      await loadDevices()
    } catch (e) {
      toastService.showError(fieldMessage(e))
    }
  }

  function copy(text: string) {
    navigator.clipboard?.writeText(text).then(() => (copied = true))
  }

  const fmt = (s: string | null) => (s ? new Date(s).toLocaleString() : '–')
</script>

<pngx-page-header>
  <PageHeader title={t('scanners.title')} subTitle={t('scanners.subtitle')}>
    <a class="btn btn-sm btn-outline-secondary me-2" href={router.href('tools/scan')}>
      <pngx-bs-icon name="printer" class="me-1">{@html icon('printer')}</pngx-bs-icon>{t('scan.title')}
    </a>
    <a class="btn btn-sm btn-outline-secondary" href={DOCS} target="_blank" rel="noopener noreferrer">
      <pngx-bs-icon name="question-circle" class="me-1">{@html icon('question-circle')}</pngx-bs-icon>{t('common.documentation')}
    </a>
  </PageHeader>
</pngx-page-header>

{#if !isSuperuser}
  <div class="alert alert-warning">{t('scanners.superuser_only')}</div>
{:else}
  <!-- eSCL-Scanner -->
  <h5 class="mt-2">{t('scanners.scanners_heading')}</h5>
  <p class="text-muted small">{t('scanners.scanners_help')}</p>
  {#if scanners.length}
    <ul class="list-group mb-3">
      {#each scanners as s (s.id)}
        <li class="list-group-item d-flex flex-wrap align-items-center gap-2">
          {#if editing === s.id}
            <input class="form-control form-control-sm w-auto flex-grow-1" maxlength="128" bind:value={editName} aria-label={t('scanners.name')} />
            <button class="btn btn-sm btn-primary" type="button" onclick={() => saveName(s)}>{t('scanners.save')}</button>
            <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => (editing = null)}>{t('scan.cancel')}</button>
          {:else}
            <div class="me-auto">
              <div class="fw-semibold">{s.name}</div>
              <div class="small text-muted">{s.model ?? ''} <code class="ms-1">{s.base_url}</code></div>
            </div>
            <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => { editing = s.id; editName = s.name }}>{t('scanners.rename')}</button>
            <pngx-confirm-button><ConfirmButton label={t('scanners.remove')} buttonClasses="btn-sm btn-outline-danger" onconfirm={() => removeScanner(s)} /></pngx-confirm-button>
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <div class="alert alert-light border small">{t('scanners.no_scanners_yet')}</div>
  {/if}

  <div class="card mb-4">
    <div class="card-body">
      <div class="d-flex flex-wrap align-items-center gap-2 mb-2">
        <button class="btn btn-sm btn-outline-primary" type="button" onclick={search} disabled={searching}>
          {#if searching}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{:else}<pngx-bs-icon name="search" class="me-1">{@html icon('search')}</pngx-bs-icon>{/if}{t('scanners.search_network')}
        </button>
        <span class="small text-muted">{t('scanners.search_hint')}</span>
      </div>
      {#if searchError}<div class="alert alert-warning small">{searchError}</div>{/if}
      {#if found}
        {#if !found.length}
          <div class="small text-muted mb-2">{t('scanners.nothing_found')}</div>
        {:else}
          <ul class="list-group mb-3">
            {#each found as f (f.base_url)}
              <li class="list-group-item d-flex flex-wrap align-items-center gap-2">
                <div class="me-auto">
                  <div>{f.name}{#if f.tls}<span class="badge text-bg-light border ms-2">HTTPS</span>{/if}</div>
                  <div class="small text-muted"><code>{f.base_url}</code></div>
                </div>
                {#if f.known}
                  <span class="badge text-bg-secondary">{t('scanners.already_added')}</span>
                {:else}
                  <button class="btn btn-sm btn-primary" type="button" disabled={adding !== null} onclick={() => add(f.base_url, f.name)}>
                    {#if adding === f.base_url}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{/if}{t('scanners.add')}
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}

      <form class="row g-2 align-items-end" onsubmit={(e) => { e.preventDefault(); if (manualUrl.trim()) add(manualUrl.trim(), manualName.trim() || undefined) }}>
        <div class="col-sm-5">
          <label class="form-label small" for="scanner-url">{t('scanners.address')}</label>
          <input id="scanner-url" class="form-control form-control-sm" placeholder="192.168.1.20" bind:value={manualUrl} />
        </div>
        <div class="col-sm-4">
          <label class="form-label small" for="scanner-name">{t('scanners.name_optional')}</label>
          <input id="scanner-name" class="form-control form-control-sm" maxlength="128" bind:value={manualName} />
        </div>
        <div class="col-sm-3">
          <button class="btn btn-sm btn-primary w-100" type="submit" disabled={adding !== null || !manualUrl.trim()}>
            {#if adding === manualUrl.trim()}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{/if}{t('scanners.add_by_address')}
          </button>
        </div>
      </form>
      {#if addError}<div class="alert alert-danger small mt-2 mb-0">{addError}</div>{/if}
    </div>
  </div>

  <!-- Devices for SMTP reception -->
  <h5>{t('scanners.devices_heading')}</h5>
  <p class="text-muted small">{t('scanners.devices_help')}</p>
  {#if smtp}
    {#if smtp.error}
      <div class="alert alert-danger small">{t('scanners.smtp_error', { PH: smtp.error })}</div>
    {:else if !smtp.enabled}
      <div class="alert alert-info small">
        {t('scanners.smtp_off')}
        <a class="ms-1" href={router.href('settings/system')}>{t('scanners.open_system_settings')}</a>
      </div>
    {:else}
      <div class="alert alert-light border small">
        <div>{t('scanners.smtp_on', { PH: smtpHost(smtp, host) })}</div>
        {#if !smtp.tls}<div class="text-warning-emphasis mt-1">{t('scanners.smtp_no_tls')}</div>{/if}
      </div>
    {/if}
  {/if}

  {#if newToken}
    <div class="alert alert-success" role="status">
      <div class="fw-semibold mb-1">{t('scanners.token_created', { PH: newToken.name })}</div>
      <div class="small mb-2">{t('scanners.token_once')}</div>
      <div class="input-group input-group-sm mb-2">
        <input class="form-control font-monospace" readonly value={newToken.token} aria-label={t('scanners.token')} />
        <button class="btn btn-outline-secondary" type="button" onclick={() => copy(newToken?.token ?? '')}>{copied ? t('scanners.copied') : t('scanners.copy')}</button>
      </div>
      <div class="small">{t('scanners.token_usage', { PH: `${newToken.token}@${host || 'paperrust'}` })}</div>
      <button class="btn btn-sm btn-outline-secondary mt-2" type="button" onclick={() => (newToken = null)}>{t('scanners.done')}</button>
    </div>
  {/if}

  {#if devices.length}
    <div class="table-responsive mb-3">
      <table class="table table-sm align-middle">
        <thead>
          <tr>
            <th>{t('scanners.name')}</th>
            <th>{t('scanners.owner')}</th>
            <th>{t('scanners.token')}</th>
            <th>{t('scanners.last_used')}</th>
            <th class="text-end">{t('scanners.documents')}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each devices as d (d.id)}
            <tr class:text-muted={!!d.revoked_at}>
              <td>{d.name}{#if d.allowed_ips}<div class="small text-muted"><code>{d.allowed_ips}</code></div>{/if}</td>
              <td>{d.owner_username ?? d.owner_id}</td>
              <td><code>{d.token_hint}</code>{#if d.revoked_at}<span class="badge text-bg-secondary ms-2">{t('scanners.revoked')}</span>{/if}</td>
              <td class="small">{fmt(d.last_used)}{#if d.last_ip}<div class="text-muted">{d.last_ip}</div>{/if}</td>
              <td class="text-end">{d.documents_received}</td>
              <td class="text-end">
                {#if d.revoked_at}
                  <pngx-confirm-button><ConfirmButton label={t('scanners.delete')} buttonClasses="btn-sm btn-outline-danger" onconfirm={() => remove(d)} /></pngx-confirm-button>
                {:else}
                  <pngx-confirm-button><ConfirmButton label={t('scanners.revoke')} buttonClasses="btn-sm btn-outline-danger" onconfirm={() => revoke(d)} /></pngx-confirm-button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <form class="card" onsubmit={(e) => { e.preventDefault(); create() }}>
    <div class="card-body row g-2 align-items-end">
      <div class="col-md-4">
        <label class="form-label small" for="dev-name">{t('scanners.device_name')}</label>
        <input id="dev-name" class="form-control form-control-sm" maxlength="128" placeholder="HP OfficeJet Pro 9010" bind:value={devName} />
      </div>
      <div class="col-md-3">
        <label class="form-label small" for="dev-owner">{t('scanners.owner')}</label>
        <select id="dev-owner" class="form-select form-select-sm" bind:value={devOwner}>
          {#each users as u (u.id)}
            <option value={u.id}>{u.username}</option>
          {/each}
        </select>
      </div>
      <div class="col-md-3">
        <label class="form-label small" for="dev-ips">{t('scanners.allowed_ips')}</label>
        <input id="dev-ips" class="form-control form-control-sm" placeholder="192.168.1.20" bind:value={devIps} />
      </div>
      <div class="col-md-2">
        <button class="btn btn-sm btn-primary w-100" type="submit" disabled={creating || !devName.trim() || devOwner === null}>
          {#if creating}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{/if}{t('scanners.create_token')}
        </button>
      </div>
      {#if devError}<div class="col-12"><div class="alert alert-danger small mb-0">{devError}</div></div>{/if}
    </div>
  </form>
{/if}
