<!-- ASN label generator (PaperRust extension, no Angular counterpart). Route tools/asn, host <pngx-asn-labels>
     in App.svelte. API: /rs/api/v2/asn/… (crates/pl-api-v2/src/asn.rs). Preview = SVG from the server (same draw list
     as the PDF), above it a grid to click already used labels. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { modals } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { session } from '../../../stores/settings.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import {
    buildJob, checkRange, content, createRange, CUSTOM, deleteRange, emptyCustom, fitsRange, getConfig, getPresets, getPreview, getPrinted,
    getRanges, groupPresets, labelsPdf, putConfig, releasePrinted, saveBlob, separatorPdf, skipUpTo, slots, suggestRange, toggleSkip,
    updateRange, v2Error,
    type AsnConfig, type AsnRange, type CodeType, type Collisions, type Form, type Preview, type PrintedRange, type RangeInput, type Sheet,
  } from './asn'

  const DOCS = '/rs/docs/asn-labels.md'

  let presets = $state<Sheet[]>([])
  let config = $state<AsnConfig | null>(null)
  let history = $state<PrintedRange[]>([])
  let loadError = $state('')

  let form = $state<Form>({
    preset: 'avery-l4731',
    custom: emptyCustom(),
    code: 'qr',
    showText: true,
    digits: 5,
    start: null,
    mode: 'sheets',
    sheets: 1,
    count: 40,
    skip: [],
    offsetX: 0,
    offsetY: 0,
    outline: false,
    range: null,
  })
  let page = $state(0)
  let preview = $state<Preview | null>(null)
  let previewError = $state('')
  let previewFields = $state<Record<string, string[]>>({})
  let loadingPreview = $state(false)
  let busy = $state(false)
  let conflict = $state<{ collisions: Collisions; record: boolean } | null>(null)

  let sep = $state({ paper: 'a4', code: 'qr' as CodeType, copies: 1, lang: locale().startsWith('de') ? 'de' : 'en' })

  const groups = $derived(groupPresets(presets))
  const selected = $derived(form.preset === CUSTOM ? form.custom : presets.find((p) => p.id === form.preset))
  const sheet = $derived(preview?.sheet ?? selected)
  const overlay = $derived(sheet ? slots(sheet, form.offsetX || 0, form.offsetY || 0) : [])
  const usedOnPage = $derived(new Set(page === 0 ? form.skip : []))
  const perSheet = $derived(sheet ? sheet.cols * sheet.rows : 0)
  const prefix = $derived(config?.barcode.prefix ?? 'ASN')
  const canRelease = (r: PrintedRange) => session.data?.user?.is_superuser || r.user_id === session.data?.user?.id

  // Number ranges (only with admin switch "Multiple number ranges")
  let ranges = $state<AsnRange[]>([])
  let rangeEdit = $state<{ id: number | null; form: RangeInput; errors: Record<string, string>; serverError: string } | null>(null)
  let savingCfg = $state(false)
  let fieldName = $state('')
  const rangesOn = $derived(!!config?.ranges_enabled)
  const canManage = $derived(!!config?.can_manage_ranges)
  const curRange = $derived(rangesOn ? (ranges.find((r) => r.id === form.range) ?? null) : null)
  // Suggestion: in the number range the next free number in it, otherwise as before
  const suggested = $derived(rangesOn && curRange ? curRange.next_asn : (config?.next_asn.suggested ?? null))
  const rangeFit = $derived(curRange && preview ? fitsRange(curRange, preview.first_asn, preview.count) : 'ok')

  onMount(async () => {
    try {
      const [p, c] = await Promise.all([getPresets(), getConfig()])
      presets = p.items
      config = c
      form.preset = p.default
      form.digits = c.default_digits
      form.start = c.next_asn.suggested
      fieldName = c.location_field_name
      if (c.ranges_enabled) await loadRanges(true)
      if (paperOf(p.items.find((x) => x.id === p.default)) === 'letter') sep.paper = 'letter'
    } catch (e) {
      loadError = v2Error(e).message
    }
    reloadHistory()
  })

  function paperOf(s?: Sheet) {
    return s?.paper === 'letter' ? 'letter' : 'a4'
  }

  async function loadRanges(selectFirst = false) {
    try {
      ranges = (await getRanges()).items
    } catch {
      ranges = []
    }
    if (form.range != null && !ranges.some((r) => r.id === form.range)) form.range = null
    if (selectFirst && form.range == null) {
      const r = ranges.find((x) => x.next_asn !== null) ?? ranges[0]
      if (r) {
        form.range = r.id
        if (r.next_asn !== null) form.start = r.next_asn
      }
    }
  }

  function pickRange() {
    const r = ranges.find((x) => x.id === form.range)
    if (r && r.next_asn !== null) form.start = r.next_asn
    form.skip = []
    page = 0
  }

  async function setRangesEnabled(on: boolean) {
    savingCfg = true
    try {
      config = await putConfig({ ranges_enabled: on })
      if (on) await loadRanges(true)
      else {
        form.range = null
        if (config) form.start = config.next_asn.suggested
      }
    } catch (e) {
      toastService.showError(t('tools.asn_range_save_failed', { PH: v2Error(e).message }))
    } finally {
      savingCfg = false
    }
  }

  async function saveFieldName() {
    savingCfg = true
    try {
      config = await putConfig({ location_field_name: fieldName })
      if (config) fieldName = config.location_field_name
    } catch (e) {
      toastService.showError(t('tools.asn_range_save_failed', { PH: v2Error(e).message }))
    } finally {
      savingCfg = false
    }
  }

  function editRange(r: AsnRange | null) {
    const f: RangeInput = r
      ? { name: r.name, code: r.code, location: r.location, first_asn: r.first_asn, last_asn: r.last_asn, note: r.note }
      : suggestRange(ranges)
    rangeEdit = { id: r?.id ?? null, form: f, errors: {}, serverError: '' }
  }

  async function saveRange() {
    if (!rangeEdit) return
    const ed = rangeEdit
    ed.errors = checkRange(ed.form, ranges, ed.id)
    ed.serverError = ''
    if (Object.keys(ed.errors).length) return
    busy = true
    try {
      const body = { ...ed.form, first_asn: Math.round(Number(ed.form.first_asn)), last_asn: Math.round(Number(ed.form.last_asn)) }
      const saved = ed.id == null ? await createRange(body) : await updateRange(ed.id, body)
      rangeEdit = null
      await loadRanges()
      if (form.range == null || form.range === saved.id) {
        form.range = saved.id
        pickRange()
      }
    } catch (e) {
      const err = v2Error(e)
      ed.serverError = Object.values(err.fields).flat().join(' ') || err.message
    } finally {
      busy = false
    }
  }

  function removeRange(r: AsnRange) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('tools.asn_range_delete'),
        messageBold: t('tools.asn_range_delete_confirm', { PH: r.name }),
        message: t('tools.asn_range_delete_info'),
        btnClass: 'btn-danger',
        btnCaption: t('common.delete'),
        onconfirm: async () => {
          try {
            await deleteRange(r.id)
            if (rangeEdit?.id === r.id) rangeEdit = null
            await loadRanges()
          } catch (e) {
            toastService.showError(t('tools.asn_range_save_failed', { PH: v2Error(e).message }))
          }
          ref.close()
        },
      },
      { backdrop: 'static' },
    )
  }

  function rangeErr(code: string | undefined) {
    switch (code) {
      case 'required':
        return t('tools.asn_range_err_required')
      case 'code_format':
        return t('tools.asn_range_err_code_format')
      case 'code_taken':
        return t('tools.asn_range_err_code_taken')
      case 'number':
        return t('tools.asn_range_err_number')
      case 'order':
        return t('tools.asn_range_err_order')
      case 'overlap':
        return t('tools.asn_range_err_overlap')
      default:
        return ''
    }
  }

  async function reloadHistory() {
    try {
      history = (await getPrinted()).items
    } catch {}
  }

  // Live preview (debounced); discard outdated responses
  let seq = 0
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    if (!presets.length) return
    const job = buildJob(form)
    const p = page
    clearTimeout(timer)
    const my = ++seq
    timer = setTimeout(async () => {
      loadingPreview = true
      try {
        const r = await getPreview(job, p)
        if (my !== seq) return
        preview = r
        previewError = ''
        previewFields = {}
      } catch (e) {
        if (my !== seq) return
        const err = v2Error(e)
        // Page no longer exists (fewer labels) → back to page 1
        if (err.fields.page && p > 0) {
          page = 0
          return
        }
        previewError = err.message
        previewFields = err.fields
      } finally {
        if (my === seq) loadingPreview = false
      }
    }, 300)
    return () => clearTimeout(timer)
  })

  function onPreset() {
    form.skip = []
    page = 0
    const s = presets.find((p) => p.id === form.preset)
    if (s) sep.paper = paperOf(s)
  }

  function clickSlot(index: number, e: MouseEvent) {
    if (page !== 0) return
    form.skip = e.shiftKey ? skipUpTo(index) : toggleSkip(form.skip, index, perSheet)
  }

  async function download(testPrint: boolean, allowCollisions = false) {
    busy = true
    try {
      const job = buildJob(form, testPrint ? { outline: true } : {})
      const r = await labelsPdf(job, { record: !testPrint, allowCollisions })
      saveBlob(r.blob, testPrint ? r.filename.replace(/\.pdf$/, '-test.pdf') : r.filename)
      conflict = null
      if (!testPrint) {
        toastService.show({ content: t('tools.labels_reserved_print_100', { PH: preview?.first_content ?? '', PH_1: preview?.last_content ?? '' }), delay: 8000, level: 'success' })
        await reloadHistory()
        // next free number for the next sheet
        try {
          config = await getConfig()
          if (rangesOn) {
            await loadRanges()
            if (curRange?.next_asn != null) form.start = curRange.next_asn
          } else if (config) form.start = config.next_asn.suggested
          form.skip = []
          page = 0
        } catch {}
      }
    } catch (e) {
      const err = v2Error(e)
      if (err.code === 'asn_collision' && err.collisions) conflict = { collisions: err.collisions, record: !testPrint }
      else toastService.showError(t('tools.could_not_create_pdf', { PH: err.message }))
    } finally {
      busy = false
    }
  }

  async function downloadSeparator() {
    busy = true
    try {
      const r = await separatorPdf({ ...sep, copies: Math.min(50, Math.max(1, Math.round(sep.copies || 1))) })
      saveBlob(r.blob, r.filename)
    } catch (e) {
      toastService.showError(t('tools.could_not_create_pdf', { PH: v2Error(e).message }))
    } finally {
      busy = false
    }
  }

  function release(r: PrintedRange) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('tools.release_number_range'),
        messageBold: t('tools.release_2', { PH: content(r.prefix, r.digits, r.first_asn), PH_1: content(r.prefix, r.digits, r.last_asn) }),
        message: t('tools.only_labels_never_used'),
        btnClass: 'btn-danger',
        btnCaption: t('tools.release'),
        onconfirm: async () => {
          try {
            await releasePrinted(r.id)
            await reloadHistory()
            config = await getConfig()
            if (rangesOn) await loadRanges()
          } catch (e) {
            toastService.showError(t('tools.could_not_release_range', { PH: v2Error(e).message }))
          }
          ref.close()
        },
      },
      { backdrop: 'static' },
    )
  }

  function warningText(code: string, fallback: string) {
    switch (code) {
      case 'module_too_small':
        return t('tools.code_very_small_label')
      case 'bars_too_short':
        return t('tools.bars_very_short_use')
      case 'text_too_small':
        return t('tools.text_very_small_label')
      case 'prefix_differs':
        return t('tools.prefix_differs_configured_asn')
      default:
        return fallback
    }
  }

  function hintText(code: string, fallback: string) {
    switch (code) {
      case 'asn_barcode_disabled':
        return t('tools.asn_barcodes_currently_not')
      case 'barcode_split_disabled':
        return t('tools.scanned_batches_currently_not')
      case 'prefix_unprintable':
        return t('tools.configured_asn_prefix_cannot')
      default:
        return fallback
    }
  }

  const fmtDate = (s: string) => new Date(s).toLocaleString(locale())
  const codeName = (c: string) => (c === 'qr' ? 'QR' : c === 'data_matrix' ? 'DataMatrix' : c === 'code128' ? 'Code 128' : c)
  const fieldError = (f: string) => previewFields[f]?.join(' ')
</script>

<pngx-page-header>
  <PageHeader title={t('common.asn_labels')} subTitle={t('tools.print_barcode_labels_archive')}>
    <a class="btn btn-sm btn-outline-secondary" href={DOCS} target="_blank" rel="noopener noreferrer">
      <pngx-bs-icon name="question-circle" class="me-1">{@html icon('question-circle')}</pngx-bs-icon>{t('common.documentation')}
    </a>
  </PageHeader>
</pngx-page-header>

{#if loadError}
  <div class="alert alert-danger">{loadError}</div>
{/if}

<div class="card mb-3">
  <div class="card-body">
    <h5 class="card-title d-flex align-items-center">
      <pngx-bs-icon name="upc-scan" class="me-2">{@html icon('upc-scan')}</pngx-bs-icon>{t('tools.how_works')}
    </h5>
    <ol class="mb-2">
      <li>{t('tools.print_sheet_labels_below')}</li>
      <li>{t('tools.stick_one_label_first')}</li>
      <li>{t('tools.put_whole_stack_in')}</li>
      <li>{t('tools.paperrust_splits_batch_every')}</li>
    </ol>
    <p class="small text-muted mb-0">
      {t('tools.write_number_folder_box')}
      {#if config}
        {t('tools.configured_prefix', { PH: config.barcode.prefix })} ·
        {t('tools.separator_barcode', { PH: config.barcode.separator_string })}
      {/if}
    </p>
    {#if config}
      {@const splitOn = !config.hints?.some((h: any) => h.code === 'barcode_split_disabled')}
      {@const asnOn = !config.hints?.some((h: any) => h.code === 'asn_barcode_disabled')}
      <ul class="list-unstyled small mt-3 mb-0 asn-status">
        <li><span class="badge {splitOn ? 'text-bg-success' : 'text-bg-secondary'} me-2">{splitOn ? t('tools.status_on') : t('tools.status_off')}</span>{t('tools.status_split')}</li>
        <li class="mt-1"><span class="badge {asnOn ? 'text-bg-success' : 'text-bg-secondary'} me-2">{asnOn ? t('tools.status_on') : t('tools.status_off')}</span>{t('tools.status_asn')}</li>
      </ul>
      {#if config.split_on_asn}
        <div class="alert alert-success small py-2 mt-3 mb-0">
          <pngx-bs-icon name="check-circle" class="me-1">{@html icon('check-circle')}</pngx-bs-icon>{t('tools.ready_barcodes_asn_barcodes')}
        </div>
      {/if}
      {#each config.hints as h (h.code)}
        <div class="alert alert-warning small py-2 mt-3 mb-0">
          <pngx-bs-icon name="exclamation-triangle" class="me-1">{@html icon('exclamation-triangle')}</pngx-bs-icon>{hintText(h.code, h.message)}
        </div>
      {/each}
    {/if}
  </div>
</div>

<div class="row g-3">
  <div class="col-lg-4">
    <div class="card mb-3">
      <div class="card-header">{t('tools.labels')}</div>
      <div class="card-body">
        {#if rangesOn}
          <div class="mb-3">
            <label class="form-label" for="asn-range">{t('tools.asn_range')}</label>
            <select id="asn-range" class="form-select form-select-sm" class:is-invalid={!!fieldError('range')} bind:value={form.range} onchange={pickRange}>
              <option value={null}>{t('tools.asn_range_choose')}</option>
              {#each ranges as r (r.id)}
                <option value={r.id}>{r.code} – {r.name} ({r.first_asn}–{r.last_asn})</option>
              {/each}
            </select>
            {#if curRange}
              <div class="form-text">
                {#if curRange.location}{t('tools.asn_range_location', { PH: curRange.location })} · {/if}
                {#if curRange.next_asn === null}
                  <span class="text-danger">{t('tools.asn_range_exhausted')}</span>
                {:else}
                  {t('tools.asn_range_remaining', { PH: curRange.remaining })}
                {/if}
              </div>
            {:else if !ranges.length}
              <div class="form-text">{t('tools.asn_range_none_yet')}</div>
            {:else}
              <div class="form-text">{t('tools.asn_range_pick_first')}</div>
            {/if}
            {#if fieldError('range')}<div class="invalid-feedback d-block">{fieldError('range')}</div>{/if}
          </div>
        {/if}
        <div class="mb-3">
          <label class="form-label" for="asn-preset">{t('tools.label_sheet')}</label>
          <select id="asn-preset" class="form-select form-select-sm" bind:value={form.preset} onchange={onPreset}>
            {#each groups as [vendor, items] (vendor)}
              <optgroup label={vendor}>
                {#each items as p (p.id)}
                  <option value={p.id}>{p.name}{p.verified ? '' : ' *'}</option>
                {/each}
              </optgroup>
            {/each}
            <option value={CUSTOM}>{t('tools.custom_layout')}</option>
          </select>
          {#if selected && form.preset !== CUSTOM}
            <div class="form-text">
              {#if !selected.verified}<span class="text-warning">* {t('tools.dimensions_not_verified_check')}</span>{/if}
              {selected.cols} × {selected.rows} = {selected.cols * selected.rows} · {selected.label_width} × {selected.label_height} mm · {selected.paper === 'letter' ? 'Letter' : 'A4'}
            </div>
          {/if}
          {#if fieldError('preset')}<div class="invalid-feedback d-block">{fieldError('preset')}</div>{/if}
        </div>

        {#if form.preset === CUSTOM}
          <fieldset class="border rounded p-2 mb-3">
            <legend class="float-none w-auto px-1 fs-6 mb-0">{t('tools.custom_layout_mm')}</legend>
            <div class="row g-2">
              <div class="col-6"><label class="form-label small" for="c-pw">{t('tools.page_width')}</label><input id="c-pw" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.page_width}></div>
              <div class="col-6"><label class="form-label small" for="c-ph">{t('tools.page_height')}</label><input id="c-ph" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.page_height}></div>
              <div class="col-6"><label class="form-label small" for="c-lw">{t('tools.label_width')}</label><input id="c-lw" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.label_width}></div>
              <div class="col-6"><label class="form-label small" for="c-lh">{t('tools.label_height')}</label><input id="c-lh" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.label_height}></div>
              <div class="col-6"><label class="form-label small" for="c-cols">{t('tools.columns')}</label><input id="c-cols" type="number" min="1" max="60" class="form-control form-control-sm" bind:value={form.custom.cols}></div>
              <div class="col-6"><label class="form-label small" for="c-rows">{t('tools.rows')}</label><input id="c-rows" type="number" min="1" max="60" class="form-control form-control-sm" bind:value={form.custom.rows}></div>
              <div class="col-6"><label class="form-label small" for="c-ml">{t('tools.left_margin')}</label><input id="c-ml" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.margin_left}></div>
              <div class="col-6"><label class="form-label small" for="c-mt">{t('tools.top_margin')}</label><input id="c-mt" type="number" step="0.1" class="form-control form-control-sm" bind:value={form.custom.margin_top}></div>
              <div class="col-6"><label class="form-label small" for="c-gx">{t('tools.horizontal_gap')}</label><input id="c-gx" type="number" step="0.1" min="0" class="form-control form-control-sm" bind:value={form.custom.gap_x}></div>
              <div class="col-6"><label class="form-label small" for="c-gy">{t('tools.vertical_gap')}</label><input id="c-gy" type="number" step="0.1" min="0" class="form-control form-control-sm" bind:value={form.custom.gap_y}></div>
            </div>
            {#each Object.entries(previewFields).filter(([k]) => k.startsWith('sheet.')) as [, msgs]}
              <div class="invalid-feedback d-block">{msgs.join(' ')}</div>
            {/each}
          </fieldset>
        {/if}

        <div class="mb-3">
          <span class="form-label d-block">{t('tools.code_type')}</span>
          <div class="btn-group btn-group-sm w-100" role="group">
            {#each [['qr', 'QR'], ['data_matrix', 'DataMatrix'], ['code128', 'Code 128']] as [v, label] (v)}
              <input type="radio" class="btn-check" name="asn-code" id={'asn-code-' + v} value={v} bind:group={form.code}>
              <label class="btn btn-outline-primary" for={'asn-code-' + v}>{label}</label>
            {/each}
          </div>
          <div class="form-text">{t('tools.qr_most_robust_choice')}</div>
        </div>

        <div class="form-check form-switch mb-3">
          <input class="form-check-input" type="checkbox" id="asn-text" bind:checked={form.showText}>
          <label class="form-check-label" for="asn-text">{t('tools.print_number_text')}</label>
        </div>

        <div class="row g-2 mb-3">
          <div class="col-7">
            <label class="form-label" for="asn-start">{t('tools.first_asn')}</label>
            <input id="asn-start" type="number" min="0" class="form-control form-control-sm" class:is-invalid={!!fieldError('start')} bind:value={form.start}>
            {#if config && suggested !== null}
              <div class="form-text">
                {t('tools.next_free', { PH: suggested })}
                {#if form.start !== suggested}
                  <button type="button" class="btn btn-link btn-sm p-0 align-baseline" onclick={() => (form.start = suggested)}>{t('tools.use')}</button>
                {/if}
              </div>
            {/if}
            {#if fieldError('start')}<div class="invalid-feedback d-block">{fieldError('start')}</div>{/if}
          </div>
          <div class="col-5">
            <label class="form-label" for="asn-digits">{t('tools.digits')}</label>
            <input id="asn-digits" type="number" min="1" max="10" class="form-control form-control-sm" class:is-invalid={!!fieldError('digits')} bind:value={form.digits}>
            <div class="form-text text-truncate">{content(prefix, Math.max(1, Math.min(10, form.digits || 1)), form.start ?? 1)}</div>
          </div>
        </div>

        <div class="mb-3">
          <div class="btn-group btn-group-sm mb-2" role="group">
            <input type="radio" class="btn-check" name="asn-mode" id="asn-mode-sheets" value="sheets" bind:group={form.mode}>
            <label class="btn btn-outline-secondary" for="asn-mode-sheets">{t('tools.sheets')}</label>
            <input type="radio" class="btn-check" name="asn-mode" id="asn-mode-count" value="count" bind:group={form.mode}>
            <label class="btn btn-outline-secondary" for="asn-mode-count">{t('tools.number_labels')}</label>
          </div>
          {#if form.mode === 'sheets'}
            <input type="number" min="1" max={config?.max_sheets ?? 200} class="form-control form-control-sm" aria-label={t('tools.sheets')} class:is-invalid={!!fieldError('sheets')} bind:value={form.sheets}>
          {:else}
            <input type="number" min="1" max={config?.max_labels ?? 20000} class="form-control form-control-sm" aria-label={t('tools.number_labels')} class:is-invalid={!!fieldError('count')} bind:value={form.count}>
          {/if}
          {#if fieldError('sheets') || fieldError('count')}<div class="invalid-feedback d-block">{fieldError('sheets') ?? fieldError('count')}</div>{/if}
        </div>

        <div class="mb-3">
          <span class="form-label d-block">{t('tools.partly_used_sheet')}</span>
          <div class="small text-muted">
            {t('tools.click_labels_in_preview', { PH: form.skip.length })}
          </div>
          {#if form.skip.length}
            <button type="button" class="btn btn-link btn-sm p-0" onclick={() => (form.skip = [])}>{t('tools.clear_marks')}</button>
          {/if}
          {#if fieldError('skip')}<div class="invalid-feedback d-block">{fieldError('skip')}</div>{/if}
        </div>

        <details class="mb-1">
          <summary class="mb-2">{t('tools.printer_calibration')}</summary>
          <div class="row g-2">
            <div class="col-6">
              <label class="form-label small" for="asn-dx">{t('tools.shift_right_mm')}</label>
              <input id="asn-dx" type="number" step="0.1" min="-20" max="20" class="form-control form-control-sm" class:is-invalid={!!fieldError('offset_x')} bind:value={form.offsetX}>
            </div>
            <div class="col-6">
              <label class="form-label small" for="asn-dy">{t('tools.shift_down_mm')}</label>
              <input id="asn-dy" type="number" step="0.1" min="-20" max="20" class="form-control form-control-sm" class:is-invalid={!!fieldError('offset_y')} bind:value={form.offsetY}>
            </div>
          </div>
          <div class="form-check mt-2">
            <input class="form-check-input" type="checkbox" id="asn-outline" bind:checked={form.outline}>
            <label class="form-check-label" for="asn-outline">{t('tools.print_label_outlines')}</label>
          </div>
          <div class="form-text">{t('tools.make_test_print_plain')}</div>
        </details>
      </div>
      <div class="card-footer d-flex flex-wrap gap-2">
        <button type="button" class="btn btn-sm btn-primary" disabled={busy || !preview || !!previewError || (rangesOn && !curRange)} onclick={() => download(false)}>
          <pngx-bs-icon name="download" class="me-1">{@html icon('download')}</pngx-bs-icon>{t('tools.download_pdf')}
        </button>
        <button type="button" class="btn btn-sm btn-outline-secondary" disabled={busy || !preview || !!previewError} onclick={() => download(true)} title={t('tools.outlines_numbers_not_reserved')}>
          <pngx-bs-icon name="printer" class="me-1">{@html icon('printer')}</pngx-bs-icon>{t('tools.test_print')}
        </button>
      </div>
    </div>

    <div class="card mb-3">
      <div class="card-header">{t('tools.separator_sheet')}</div>
      <div class="card-body">
        <p class="small text-muted">
          {t('tools.alternative_without_labels_put')}
        </p>
        <div class="row g-2">
          <div class="col-6">
            <label class="form-label small" for="sep-paper">{t('tools.paper')}</label>
            <select id="sep-paper" class="form-select form-select-sm" bind:value={sep.paper}><option value="a4">A4</option><option value="letter">Letter</option></select>
          </div>
          <div class="col-6">
            <label class="form-label small" for="sep-code">{t('tools.code_type')}</label>
            <select id="sep-code" class="form-select form-select-sm" bind:value={sep.code}><option value="qr">QR</option><option value="data_matrix">DataMatrix</option><option value="code128">Code 128</option></select>
          </div>
          <div class="col-6">
            <label class="form-label small" for="sep-copies">{t('tools.copies')}</label>
            <input id="sep-copies" type="number" min="1" max="50" class="form-control form-control-sm" bind:value={sep.copies}>
          </div>
          <div class="col-6">
            <label class="form-label small" for="sep-lang">{t('common.language')}</label>
            <select id="sep-lang" class="form-select form-select-sm" bind:value={sep.lang}><option value="de">Deutsch</option><option value="en">English</option></select>
          </div>
        </div>
      </div>
      <div class="card-footer">
        <button type="button" class="btn btn-sm btn-outline-primary" disabled={busy} onclick={downloadSeparator}>
          <pngx-bs-icon name="download" class="me-1">{@html icon('download')}</pngx-bs-icon>{t('tools.download_separator_sheet')}
        </button>
      </div>
    </div>

    {#if canManage}
      <details class="card mb-3" open={rangesOn}>
        <summary class="card-header small">{t('tools.asn_advanced')}</summary>
        <div class="card-body">
          <div class="form-check form-switch">
            <input class="form-check-input" type="checkbox" id="asn-ranges-on" checked={rangesOn} disabled={savingCfg} onchange={(e) => setRangesEnabled(e.currentTarget.checked)}>
            <label class="form-check-label" for="asn-ranges-on">{t('tools.asn_ranges_switch')}</label>
          </div>
          <div class="form-text">{t('tools.asn_ranges_switch_help')}</div>
          {#if rangesOn}
            <label class="form-label small mt-3" for="asn-field-name">{t('tools.asn_location_field')}</label>
            <div class="input-group input-group-sm">
              <input id="asn-field-name" type="text" maxlength="128" class="form-control" bind:value={fieldName}>
              <button type="button" class="btn btn-outline-secondary" disabled={savingCfg || !fieldName.trim() || fieldName === config?.location_field_name} onclick={saveFieldName}>{t('common.save')}</button>
            </div>
          {/if}
        </div>
      </details>
    {/if}
  </div>

  <div class="col-lg-8">
    <div class="card mb-3">
      <div class="card-header d-flex align-items-center flex-wrap gap-2">
        <span class="me-auto">{t('common.preview')}</span>
        {#if loadingPreview}<div class="spinner-border spinner-border-sm text-secondary" role="status"><span class="visually-hidden">{t('common.loading')}</span></div>{/if}
        {#if preview && preview.pages > 1}
          <div class="btn-group btn-group-sm">
            <button type="button" class="btn btn-outline-secondary" disabled={page === 0} onclick={() => page--} aria-label={t('common.previous')}>
              <pngx-bs-icon name="arrow-left">{@html icon('arrow-left')}</pngx-bs-icon>
            </button>
            <span class="btn btn-outline-secondary disabled">{t('tools.sheet_2', { PH: page + 1, PH_1: preview.pages })}</span>
            <button type="button" class="btn btn-outline-secondary" disabled={page >= preview.pages - 1} onclick={() => page++} aria-label={t('common.next')}>
              <pngx-bs-icon name="arrow-right">{@html icon('arrow-right')}</pngx-bs-icon>
            </button>
          </div>
        {/if}
      </div>
      <div class="card-body">
        {#if previewError}
          <div class="alert alert-danger small">{previewError}</div>
        {/if}
        {#if preview}
          <p class="small mb-2">
            <strong>{preview.first_content}</strong> – <strong>{preview.last_content}</strong> ·
            {t('tools.labels_sheet_s', { PH: preview.count, PH_1: preview.pages })}
          </p>
          {#if curRange && rangeFit !== 'ok'}
            <div class="alert alert-danger small py-2">{t('tools.asn_range_outside', { PH: curRange.first_asn, PH_1: curRange.last_asn })}</div>
          {/if}
          {#each preview.warnings as w (w.code)}
            <div class="alert alert-warning small py-2">{warningText(w.code, w.message)}</div>
          {/each}
          {#if preview.collisions.documents_total > 0 || preview.collisions.printed.length > 0}
            <div class="alert alert-warning small py-2">
              {#if preview.collisions.documents_total > 0}
                <div>{t('tools.number_s_in_range', { PH: preview.collisions.documents_total, PH_1: preview.collisions.documents.slice(0, 12).join(', ') + (preview.collisions.documents_total > 12 ? ', …' : '') })}</div>
              {/if}
              {#each preview.collisions.printed as r (r.id)}
                <div>{t('tools.overlaps_labels_printed', { PH: fmtDate(r.created), PH_1: r.first_asn, PH_2: r.last_asn })}</div>
              {/each}
              {#if config && suggested !== null}
                <button type="button" class="btn btn-link btn-sm p-0" onclick={() => (form.start = suggested)}>{t('tools.start_next_free_number', { PH: suggested })}</button>
              {/if}
            </div>
          {/if}
        {/if}
        {#if conflict}
          <div class="alert alert-danger small">
            <p class="mb-2">{t('tools.some_numbers_already_used', { PH: conflict.collisions.documents_total, PH_1: conflict.collisions.printed.length })}</p>
            <button type="button" class="btn btn-sm btn-danger me-2" disabled={busy} onclick={() => conflict && download(!conflict.record, true)}>{t('tools.print_anyway')}</button>
            <button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => (conflict = null)}>{t('common.cancel')}</button>
          </div>
        {/if}
        {#if sheet}
          <div class="asn-sheet border shadow-sm mx-auto" class:opacity-50={loadingPreview && !preview} style:aspect-ratio="{sheet.page_width} / {sheet.page_height}">
            {#if preview}{@html preview.svg}{/if}
            <svg class="asn-overlay" viewBox="0 0 {sheet.page_width} {sheet.page_height}" preserveAspectRatio="none" role="group" aria-label={t('tools.label_positions')}>
              <defs>
                <pattern id="asn-used" width="2" height="2" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                  <rect width="2" height="2" fill="#e9ecef" /><line x1="0" y1="0" x2="0" y2="2" stroke="#adb5bd" stroke-width="0.8" />
                </pattern>
              </defs>
              {#each overlay as s (s.index)}
                <rect
                  class="asn-slot"
                  class:used={usedOnPage.has(s.index)}
                  class:clickable={page === 0}
                  x={s.x} y={s.y} width={s.w} height={s.h} rx={sheet.corner_radius}
                  role="button"
                  tabindex={page === 0 ? 0 : -1}
                  aria-pressed={usedOnPage.has(s.index)}
                  aria-label={t('tools.label', { PH: s.index + 1 })}
                  onclick={(e) => clickSlot(s.index, e)}
                  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); if (page === 0) form.skip = toggleSkip(form.skip, s.index, perSheet) } }}
                ><title>{usedOnPage.has(s.index) ? t('tools.already_used_click_use') : t('tools.click_mark_already_used')}</title></rect>
              {/each}
            </svg>
          </div>
        {/if}
        <p class="small text-muted mt-2 mb-0">
          {t('tools.print_100_actual_size')}
        </p>
      </div>
    </div>

    {#if rangesOn}
      <div class="card mb-3">
        <div class="card-header d-flex align-items-center">
          <span class="me-auto">{t('tools.asn_ranges')}</span>
          {#if canManage}
            <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => editRange(null)}>
              <pngx-bs-icon name="plus" class="me-1">{@html icon('plus')}</pngx-bs-icon>{t('tools.asn_range_add')}
            </button>
          {/if}
        </div>
        <div class="table-responsive">
          <table class="table table-sm table-striped align-middle mb-0">
            <thead>
              <tr>
                <th class="fw-normal">{t('tools.asn_range_code')}</th>
                <th class="fw-normal">{t('common.name')}</th>
                <th class="fw-normal">{t('tools.asn_range_numbers')}</th>
                <th class="fw-normal d-none d-md-table-cell">{t('tools.asn_range_location_label')}</th>
                <th class="fw-normal">{t('tools.asn_range_used')}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {#each ranges as r (r.id)}
                <tr class:table-active={r.id === form.range}>
                  <td class="text-nowrap"><strong>{r.code}</strong></td>
                  <td>{r.name}{#if r.note}<div class="small text-muted">{r.note}</div>{/if}</td>
                  <td class="text-nowrap">{r.first_asn} – {r.last_asn}</td>
                  <td class="d-none d-md-table-cell small">{r.location}</td>
                  <td class="text-nowrap">{r.used} / <span class:text-danger={r.next_asn === null}>{r.remaining}</span></td>
                  <td class="text-end text-nowrap">
                    {#if canManage}
                      <button type="button" class="btn btn-sm btn-link p-0 me-2" onclick={() => editRange(r)} title={t('common.edit')} aria-label={t('common.edit')}>
                        <pngx-bs-icon name="pencil">{@html icon('pencil')}</pngx-bs-icon>
                      </button>
                      <button type="button" class="btn btn-sm btn-link text-danger p-0" onclick={() => removeRange(r)} title={t('tools.asn_range_delete')} aria-label={t('tools.asn_range_delete')}>
                        <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
                      </button>
                    {/if}
                  </td>
                </tr>
              {:else}
                <tr><td colspan="6" class="text-muted small">{t('tools.asn_range_none_yet')}</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if rangeEdit}
          <form class="card-body border-top" onsubmit={(e) => { e.preventDefault(); saveRange() }}>
            <h6>{rangeEdit.id == null ? t('tools.asn_range_add') : t('tools.asn_range_edit')}</h6>
            <div class="row g-2">
              <div class="col-sm-8">
                <label class="form-label small" for="rg-name">{t('common.name')}</label>
                <input id="rg-name" type="text" maxlength="64" class="form-control form-control-sm" class:is-invalid={!!rangeEdit.errors.name} bind:value={rangeEdit.form.name}>
                {#if rangeEdit.errors.name}<div class="invalid-feedback">{rangeErr(rangeEdit.errors.name)}</div>{/if}
              </div>
              <div class="col-sm-4">
                <label class="form-label small" for="rg-code">{t('tools.asn_range_code')}</label>
                <input id="rg-code" type="text" maxlength="8" class="form-control form-control-sm" class:is-invalid={!!rangeEdit.errors.code} bind:value={rangeEdit.form.code}>
                {#if rangeEdit.errors.code}<div class="invalid-feedback">{rangeErr(rangeEdit.errors.code)}</div>{:else}<div class="form-text">{t('tools.asn_range_code_help')}</div>{/if}
              </div>
              <div class="col-12">
                <label class="form-label small" for="rg-loc">{t('tools.asn_range_location_label')}</label>
                <input id="rg-loc" type="text" maxlength="128" class="form-control form-control-sm" placeholder={t('tools.asn_range_location_placeholder')} bind:value={rangeEdit.form.location}>
              </div>
              <div class="col-6">
                <label class="form-label small" for="rg-first">{t('tools.asn_range_first')}</label>
                <input id="rg-first" type="number" min="0" class="form-control form-control-sm" class:is-invalid={!!rangeEdit.errors.first_asn} bind:value={rangeEdit.form.first_asn}>
                {#if rangeEdit.errors.first_asn}<div class="invalid-feedback">{rangeErr(rangeEdit.errors.first_asn)}</div>{/if}
              </div>
              <div class="col-6">
                <label class="form-label small" for="rg-last">{t('tools.asn_range_last')}</label>
                <input id="rg-last" type="number" min="0" class="form-control form-control-sm" class:is-invalid={!!rangeEdit.errors.last_asn} bind:value={rangeEdit.form.last_asn}>
                {#if rangeEdit.errors.last_asn}<div class="invalid-feedback">{rangeErr(rangeEdit.errors.last_asn)}</div>{/if}
              </div>
              <div class="col-12">
                <label class="form-label small" for="rg-note">{t('tools.asn_range_note')}</label>
                <input id="rg-note" type="text" maxlength="2000" class="form-control form-control-sm" bind:value={rangeEdit.form.note}>
              </div>
            </div>
            {#if rangeEdit.serverError}<div class="alert alert-danger small py-2 mt-2 mb-0">{rangeEdit.serverError}</div>{/if}
            <div class="d-flex gap-2 mt-3">
              <button type="submit" class="btn btn-sm btn-primary" disabled={busy}>{t('common.save')}</button>
              <button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => (rangeEdit = null)}>{t('common.cancel')}</button>
            </div>
          </form>
        {/if}
        <div class="card-footer small text-muted">{t('tools.asn_range_location_hint', { PH: config?.location_field_name ?? '' })}</div>
      </div>
    {/if}

    <div class="card mb-3">
      <div class="card-header">{t('tools.printed_ranges')}</div>
      <div class="table-responsive">
        <table class="table table-sm table-striped align-middle mb-0">
          <thead>
            <tr>
              <th class="fw-normal">{t('common.range')}</th>
              <th class="fw-normal">{t('tools.labels')}</th>
              <th class="fw-normal d-none d-md-table-cell">{t('tools.sheet')}</th>
              <th class="fw-normal d-none d-md-table-cell">{t('tools.code')}</th>
              <th class="fw-normal d-none d-sm-table-cell">{t('common.user')}</th>
              <th class="fw-normal">{t('common.date')}</th>
              <th class="fw-normal">{t('tools.used')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {#each history as r (r.id)}
              <tr>
                <td class="text-nowrap">{content(r.prefix, r.digits, r.first_asn)} – {content(r.prefix, r.digits, r.last_asn)}</td>
                <td>{r.label_count}</td>
                <td class="d-none d-md-table-cell small">{presets.find((p) => p.id === r.sheet)?.name.split(' – ')[0] ?? r.sheet}</td>
                <td class="d-none d-md-table-cell">{codeName(r.code_type)}</td>
                <td class="d-none d-sm-table-cell">{r.username ?? ''}</td>
                <td class="small text-nowrap">{fmtDate(r.created)}</td>
                <td>{r.used} / {r.label_count}</td>
                <td class="text-end">
                  {#if canRelease(r)}
                    <button type="button" class="btn btn-sm btn-link text-danger p-0" onclick={() => release(r)} title={t('tools.release_number_range')} aria-label={t('tools.release_number_range')}>
                      <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
                    </button>
                  {/if}
                </td>
              </tr>
            {:else}
              <tr><td colspan="8" class="text-muted small">{t('tools.no_labels_printed_yet')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
  </div>
</div>

<style>
  .asn-sheet {
    position: relative;
    max-width: 640px;
    background: #fff;
  }
  /* Lay the server SVG (width/height in mm) and click grid exactly on top of each other on the area */
  .asn-sheet :global(svg) {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .asn-slot {
    fill: transparent;
    stroke: none;
    outline: none;
  }
  .asn-slot.clickable {
    cursor: pointer;
  }
  .asn-slot.clickable:hover,
  .asn-slot.clickable:focus-visible {
    fill: rgba(23, 84, 31, 0.12);
    stroke: var(--bs-primary, #17541f);
    stroke-width: 0.4;
  }
  .asn-slot.used {
    fill: url(#asn-used);
    fill-opacity: 0.9;
  }
</style>
