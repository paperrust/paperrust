<!-- Scan (PaperRust extension, no Angular counterpart). Route tools/scan, host <pngx-scan> in App.svelte.
     Choose scanner, source/duplex/resolution/color, then progress and result. API: /api/x-scan/… (crates/pl-scan).
     The result goes into processing like an upload and belongs to the logged-in user. -->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { session } from '../../../stores/settings.svelte'
  import { router } from '../../../router.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import {
    cancelJob, capabilities, choices, COLOR_KEYS, defaultResolution, effectiveSource, ERROR_KEYS, errorCode, getJob, isFinished,
    listScanners, SOURCE_KEYS, startScan,
    type Capabilities, type ColorId, type ScanJob, type Scanner, type SourceId,
  } from './scan-api'

  const DOCS = '/rs/docs/scanner.md'

  let scanners = $state<Scanner[]>([])
  let loading = $state(true)
  let loadError = $state('')
  let scannerId = $state<number | null>(null)
  let caps = $state<Capabilities | null>(null)
  let capsLoading = $state(false)
  let capsError = $state('')

  let source = $state<SourceId>('adf')
  let duplex = $state(false)
  let resolution = $state<number | undefined>(undefined)
  let color = $state<ColorId>('color')
  let title = $state('')

  let job = $state<ScanJob | null>(null)
  let startError = $state('')
  let starting = $state(false)
  let timer: ReturnType<typeof setTimeout> | undefined

  const choice = $derived(choices(caps, effectiveSource(source, duplex)))
  const running = $derived(!!job && !isFinished(job.state))
  const isSuperuser = $derived(!!session.data?.user?.is_superuser)

  const errorText = (code: string) => t(ERROR_KEYS[code] ?? 'scan.error_protocol')

  onMount(async () => {
    try {
      scanners = await listScanners()
      if (scanners.length) await select(scanners[0].id)
    } catch {
      loadError = t('scan.could_not_load_scanners')
    } finally {
      loading = false
    }
  })
  onDestroy(() => clearTimeout(timer))

  async function select(id: number) {
    scannerId = id
    caps = null
    capsError = ''
    capsLoading = true
    try {
      caps = await capabilities(id)
      const c = choices(caps, 'adf')
      source = c.sources[0] ?? 'flatbed'
      duplex = false
      applyDefaults()
    } catch (e) {
      capsError = errorText(errorCode(e))
    } finally {
      capsLoading = false
    }
  }

  /** Adapt values to the chosen source (replace unsupported ones). */
  function applyDefaults() {
    const c = choices(caps, effectiveSource(source, duplex))
    if (resolution === undefined || !c.resolutions.includes(resolution)) resolution = defaultResolution(c.resolutions)
    if (!c.colors.includes(color)) color = c.colors.includes('color') ? 'color' : (c.colors[0] ?? 'color')
  }

  async function scan() {
    if (scannerId === null) return
    startError = ''
    starting = true
    try {
      job = await startScan({ scanner: scannerId, source: effectiveSource(source, duplex), resolution, color_mode: color, title: title.trim() || undefined })
      poll()
    } catch (e) {
      startError = errorText(errorCode(e))
    } finally {
      starting = false
    }
  }

  function poll() {
    clearTimeout(timer)
    timer = setTimeout(async () => {
      if (!job) return
      try {
        job = await getJob(job.id)
      } catch {
        // brief network errors: keep trying
      }
      if (job && !isFinished(job.state)) poll()
    }, 1000)
  }

  async function cancel() {
    if (!job) return
    try {
      job = (await cancelJob(job.id)).job
    } catch {}
  }

  function again() {
    job = null
    title = ''
  }
</script>

<pngx-page-header>
  <PageHeader title={t('scan.title')} subTitle={t('scan.subtitle')}>
    {#if isSuperuser}
      <a class="btn btn-sm btn-outline-secondary me-2" href={router.href('tools/scanners')}>
        <pngx-bs-icon name="gear" class="me-1">{@html icon('gear')}</pngx-bs-icon>{t('scan.manage_scanners')}
      </a>
    {/if}
    <a class="btn btn-sm btn-outline-secondary" href={DOCS} target="_blank" rel="noopener noreferrer">
      <pngx-bs-icon name="question-circle" class="me-1">{@html icon('question-circle')}</pngx-bs-icon>{t('common.documentation')}
    </a>
  </PageHeader>
</pngx-page-header>

<div class="row">
  <div class="col-lg-7 col-xl-6">
    {#if loading}
      <div class="d-flex align-items-center gap-2 text-muted"><span class="spinner-border spinner-border-sm" aria-hidden="true"></span>{t('scan.loading')}</div>
    {:else if loadError}
      <div class="alert alert-danger">{loadError}</div>
    {:else if !scanners.length}
      <div class="alert alert-info">
        <p class="mb-2">{t('scan.no_scanners')}</p>
        {#if isSuperuser}
          <a class="btn btn-sm btn-primary" href={router.href('tools/scanners')}>{t('scan.add_scanner')}</a>
        {:else}
          <span class="small">{t('scan.ask_admin')}</span>
        {/if}
      </div>
    {:else if job}
      <!-- Progress / result -->
      <div class="card">
        <div class="card-body">
          {#if job.state === 'scanning'}
            <div class="d-flex align-items-center gap-2 mb-3">
              <span class="spinner-border spinner-border-sm" aria-hidden="true"></span>
              <span>{job.pages ? t('scan.scanning_pages', { PH: String(job.pages) }) : t('scan.scanning')}</span>
            </div>
            <button class="btn btn-outline-danger btn-sm" type="button" onclick={cancel}>{t('scan.cancel')}</button>
          {:else if job.state === 'processing'}
            <div class="d-flex align-items-center gap-2">
              <span class="spinner-border spinner-border-sm" aria-hidden="true"></span>
              <span>{t('scan.building_pdf', { PH: String(job.pages) })}</span>
            </div>
          {:else if job.state === 'done'}
            <div class="alert alert-success mb-3" role="status">
              <pngx-bs-icon name="check-circle" class="me-1">{@html icon('check-circle')}</pngx-bs-icon>{t('scan.done', { PH: String(job.pages) })}
            </div>
            <button class="btn btn-primary btn-sm me-2" type="button" onclick={again}>{t('scan.scan_more')}</button>
            {#if session.can('view', 'paperlesstask')}
              <a class="btn btn-outline-secondary btn-sm" href={router.href('tasks')}>{t('common.tasks')}</a>
            {/if}
          {:else}
            <div class="alert {job.state === 'cancelled' ? 'alert-secondary' : 'alert-danger'} mb-3" role="alert">
              {errorText(job.error?.code ?? 'protocol')}
            </div>
            <button class="btn btn-primary btn-sm" type="button" onclick={again}>{t('scan.try_again')}</button>
          {/if}
        </div>
      </div>
    {:else}
      <form class="card" onsubmit={(e) => { e.preventDefault(); scan() }}>
        <div class="card-body">
          <div class="mb-3">
            <label class="form-label" for="scan-scanner">{t('scan.scanner')}</label>
            <select id="scan-scanner" class="form-select" value={scannerId} onchange={(e) => select(Number((e.currentTarget as HTMLSelectElement).value))}>
              {#each scanners as s (s.id)}
                <option value={s.id}>{s.name}</option>
              {/each}
            </select>
          </div>

          {#if capsLoading}
            <div class="d-flex align-items-center gap-2 text-muted mb-3"><span class="spinner-border spinner-border-sm" aria-hidden="true"></span>{t('scan.asking_scanner')}</div>
          {:else if capsError}
            <div class="alert alert-warning d-flex align-items-center justify-content-between">
              <span>{capsError}</span>
              <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => scannerId !== null && select(scannerId)}>{t('scan.retry')}</button>
            </div>
          {:else if caps}
            <fieldset class="mb-3">
              <legend class="form-label fs-6">{t('scan.source')}</legend>
              {#each choice.sources as s (s)}
                <div class="form-check form-check-inline">
                  <input class="form-check-input" type="radio" id="scan-src-{s}" name="scan-source" value={s} checked={source === s}
                    onchange={() => { source = s; if (s === 'flatbed') duplex = false; applyDefaults() }} />
                  <label class="form-check-label" for="scan-src-{s}">{t(SOURCE_KEYS[s])}</label>
                </div>
              {/each}
              {#if choice.duplex && source === 'adf'}
                <div class="form-check form-switch mt-2">
                  <input class="form-check-input" type="checkbox" role="switch" id="scan-duplex" bind:checked={duplex} onchange={applyDefaults} />
                  <label class="form-check-label" for="scan-duplex">{t('scan.duplex')}</label>
                </div>
              {/if}
            </fieldset>

            <div class="row g-3 mb-3">
              <div class="col-sm-6">
                <label class="form-label" for="scan-res">{t('scan.resolution')}</label>
                <select id="scan-res" class="form-select" bind:value={resolution}>
                  {#each choice.resolutions as r (r)}
                    <option value={r}>{r} dpi</option>
                  {/each}
                </select>
              </div>
              <div class="col-sm-6">
                <label class="form-label" for="scan-color">{t('scan.color_mode')}</label>
                <select id="scan-color" class="form-select" bind:value={color}>
                  {#each choice.colors as c (c)}
                    <option value={c}>{t(COLOR_KEYS[c])}</option>
                  {/each}
                </select>
              </div>
            </div>

            <div class="mb-3">
              <label class="form-label" for="scan-title">{t('scan.title_optional')}</label>
              <input id="scan-title" class="form-control" maxlength="128" bind:value={title} />
            </div>

            {#if caps.adf_state === 'ScannerAdfEmpty' && source === 'adf'}
              <div class="alert alert-warning small">{t('scan.adf_looks_empty')}</div>
            {/if}
            {#if startError}
              <div class="alert alert-danger">{startError}</div>
            {/if}

            <button class="btn btn-primary" type="submit" disabled={starting || running}>
              {#if starting}<span class="spinner-border spinner-border-sm me-1" aria-hidden="true"></span>{:else}<pngx-bs-icon name="printer" class="me-1">{@html icon('printer')}</pngx-bs-icon>{/if}{t('scan.scan')}
            </button>
          {/if}
        </div>
      </form>
    {/if}
  </div>
</div>
