<!-- PaperRust-specific: redact (pl-pdftools, docs/contract/pl-pdftools.md §4). Draw pages with pdf.js,
     drag rectangles with the mouse or search for search terms and confirm hits. The server really removes the
     content underneath (text, images, vector graphics) and creates a new version. -->
<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte'
  import { SvelteSet } from 'svelte/reactivity'
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { loadPdfjs, pdfjsAssetUrl } from '../../common/pdf-viewer/pdfjs'
  import { collectAreas, cssBox, errorText, normRect, redact, redactSearch, toPdf, usableRect, type Matrix, type PdfRect, type RedactArea, type RedactHit } from './pdf-tools-api'

  let { modal, documentId, src, onqueued }: { modal: ModalRef<any>; documentId: number; src: string; onqueued?: () => void } = $props()

  /** Width at which pages are drawn (CSS pixels). */
  const TARGET_WIDTH = 760

  let pages = $state<{ num: number; width: number; height: number }[]>([])
  let loading = $state(true)
  let loadFailed = $state(false)
  let drawn = $state<RedactArea[]>([])
  let hits = $state<RedactHit[]>([])
  const confirmed = new SvelteSet<number>()
  let termsText = $state('')
  let searching = $state(false)
  let searched = $state(false)
  let truncated = $state(false)
  let fromArchive = $state(false)
  let removeMetadata = $state(false)
  let busy = $state(false)
  let drag = $state<{ page: number; x0: number; y0: number; x1: number; y1: number } | null>(null)

  // pdf.js objects stay outside the reactive state
  let task: any
  let pdf: any
  const viewports: any[] = []
  const canvases: HTMLCanvasElement[] = []
  let destroyed = false

  onMount(async () => {
    try {
      const { lib } = await loadPdfjs()
      task = lib.getDocument({ url: src, withCredentials: true, wasmUrl: pdfjsAssetUrl('wasm'), iccUrl: pdfjsAssetUrl('iccs'), cMapUrl: pdfjsAssetUrl('cmaps') } as any)
      pdf = await task.promise
      const list: { num: number; width: number; height: number }[] = []
      const objs: any[] = []
      for (let i = 1; i <= pdf.numPages; i++) {
        const p = await pdf.getPage(i)
        const base = p.getViewport({ scale: 1 })
        const vp = p.getViewport({ scale: Math.min(2, TARGET_WIDTH / base.width) })
        viewports[i] = vp
        objs[i] = p
        list.push({ num: i, width: vp.width, height: vp.height })
      }
      if (destroyed) return
      pages = list
      loading = false
      await tick()
      const dpr = window.devicePixelRatio || 1
      for (const pg of list) {
        if (destroyed) return
        const c = canvases[pg.num]
        const ctx = c?.getContext('2d')
        if (!c || !ctx) continue
        c.width = Math.floor(pg.width * dpr)
        c.height = Math.floor(pg.height * dpr)
        await objs[pg.num].render({ canvasContext: ctx, viewport: viewports[pg.num], transform: dpr !== 1 ? [dpr, 0, 0, dpr, 0, 0] : undefined }).promise
      }
    } catch (e) {
      loading = false
      loadFailed = true
      toastService.showError(t('pdftools.error_loading_pdf'), e)
    }
  })

  onDestroy(() => {
    destroyed = true
    task?.destroy()
  })

  /** PDF rectangle → CSS box on the drawn page (own matrix calculation, pdf.js 6 no longer has convertToViewportRectangle) */
  function box(page: number, r: PdfRect) {
    const vp = viewports[page]
    if (!vp) return ''
    const b = cssBox(vp.transform as Matrix, r)
    return `left:${b.left}px;top:${b.top}px;width:${b.width}px;height:${b.height}px`
  }

  function localPoint(e: PointerEvent) {
    const el = e.currentTarget as HTMLElement
    const r = el.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  function onDown(e: PointerEvent, page: number) {
    if (e.button !== 0 || busy) return
    const p = localPoint(e)
    ;(e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId)
    drag = { page, x0: p.x, y0: p.y, x1: p.x, y1: p.y }
  }

  function onMove(e: PointerEvent) {
    if (!drag) return
    const p = localPoint(e)
    drag = { ...drag, x1: p.x, y1: p.y }
  }

  function onUp() {
    if (!drag) return
    const vp = viewports[drag.page]
    if (vp) {
      const m = vp.transform as Matrix
      const [x0, y0] = toPdf(m, drag.x0, drag.y0)
      const [x1, y1] = toPdf(m, drag.x1, drag.y1)
      const rect = normRect([x0, y0, x1, y1])
      if (usableRect(rect)) drawn = [...drawn, { page: drag.page, rect }]
    }
    drag = null
  }

  function removeDrawn(i: number) {
    drawn = drawn.filter((_, j) => j !== i)
  }

  async function search() {
    const terms = termsText
      .split(/[\n;]/)
      .map((s) => s.trim())
      .filter(Boolean)
    if (!terms.length) return
    searching = true
    try {
      const r = await redactSearch(documentId, terms)
      hits = r.hits
      truncated = r.truncated
      fromArchive = r.source === 'archive'
      confirmed.clear()
      r.hits.forEach((_, i) => confirmed.add(i))
      searched = true
    } catch (e) {
      toastService.showError(errorText(e) ?? t('pdftools.error_searching'), e)
    } finally {
      searching = false
    }
  }

  function toggleHit(i: number) {
    if (confirmed.has(i)) confirmed.delete(i)
    else confirmed.add(i)
  }

  let areas = $derived(collectAreas(drawn, hits, confirmed))

  async function apply() {
    if (!areas.length && !removeMetadata) return
    busy = true
    try {
      await redact(documentId, areas, removeMetadata)
      toastService.showInfo(t('pdftools.redaction_queued'))
      onqueued?.()
      modal.close()
    } catch (e) {
      toastService.showError(errorText(e) ?? t('pdftools.error_redacting'), e)
      busy = false
    }
  }
</script>

<pngx-redact-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('pdftools.redact_document')}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
    <p class="small text-muted mb-2">{t('pdftools.redact_help')}</p>
    <div class="row g-3">
        <div class="col-lg-8">
            {#if loading}
                <div class="d-flex align-items-center gap-2 p-3"><div class="spinner-border spinner-border-sm" role="status"></div>{t('pdftools.loading_pages')}</div>
            {:else if loadFailed}
                <div class="alert alert-danger">{t('pdftools.error_loading_pdf')}</div>
            {/if}
            <div class="redact-pages" style={"max-height: 70vh; overflow: auto;"}>
                {#each pages as pg (pg.num)}
                    <div class="text-muted small mt-2">{t('pdftools.page_n', { PH: pg.num })}</div>
                    <div class="position-relative border shadow-sm mb-2" style="width:{pg.width}px;height:{pg.height}px;">
                        <canvas bind:this={canvases[pg.num]} style="width:{pg.width}px;height:{pg.height}px;display:block;"></canvas>
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div class="position-absolute top-0 start-0 w-100 h-100" style={"cursor: crosshair; touch-action: none;"}
                            onpointerdown={(e) => onDown(e, pg.num)} onpointermove={onMove} onpointerup={onUp} onpointercancel={() => (drag = null)}>
                            {#each hits as h, i (i)}
                                {#if h.page === pg.num}
                                    {#each h.rects as r}
                                        <div class="position-absolute" style="{box(pg.num, r)};{confirmed.has(i) ? 'background: rgba(0,0,0,.6); outline: 2px solid #fd7e14;' : 'outline: 2px dashed #fd7e14;'}"></div>
                                    {/each}
                                {/if}
                            {/each}
                            {#each drawn as a, i (i)}
                                {#if a.page === pg.num}
                                    <div class="position-absolute" style="{box(pg.num, a.rect)}; background: rgba(0,0,0,.6); outline: 2px solid #dc3545;">
                                        <button type="button" class="btn btn-sm btn-danger position-absolute top-0 end-0 py-0 px-1 lh-1" title={t('pdftools.remove_area')} aria-label={t('pdftools.remove_area')}
                                            onpointerdown={(e) => e.stopPropagation()} onclick={() => removeDrawn(i)}>×</button>
                                    </div>
                                {/if}
                            {/each}
                            {#if drag && drag.page === pg.num}
                                <div class="position-absolute" style="left:{Math.min(drag.x0, drag.x1)}px;top:{Math.min(drag.y0, drag.y1)}px;width:{Math.abs(drag.x1 - drag.x0)}px;height:{Math.abs(drag.y1 - drag.y0)}px;outline:2px dashed #dc3545;background:rgba(220,53,69,.15);"></div>
                            {/if}
                        </div>
                    </div>
                {/each}
            </div>
        </div>
        <div class="col-lg-4">
            <label for="redactTerms" class="form-label">{t('pdftools.search_terms')}</label>
            <textarea id="redactTerms" class="form-control" rows="3" bind:value={termsText} placeholder={t('pdftools.search_terms_hint')}></textarea>
            <button type="button" class="btn btn-sm btn-outline-primary mt-2" onclick={search} disabled={searching || busy || !termsText.trim()}>
                {#if searching}<span class="spinner-border spinner-border-sm me-1" role="status"></span>{/if}{t('pdftools.search')}
            </button>
            {#if searched}
                <div class="mt-3 small">
                    {#if !hits.length}
                        <div class="text-muted">{t('pdftools.no_hits')}</div>
                    {:else}
                        <div class="mb-1">{t('pdftools.hits_confirm', { PH: hits.length })}</div>
                        {#if fromArchive}<div class="text-muted fst-italic mb-1">{t('pdftools.hits_from_ocr')}</div>{/if}
                        {#if truncated}<div class="text-warning mb-1">{t('pdftools.hits_truncated')}</div>{/if}
                        <ul class="list-unstyled mb-0" style={"max-height: 40vh; overflow: auto;"}>
                            {#each hits as h, i (i)}
                                <li class="form-check">
                                    <input class="form-check-input" type="checkbox" id="hit{i}" checked={confirmed.has(i)} onchange={() => toggleHit(i)}>
                                    <label class="form-check-label" for="hit{i}"><span class="badge text-bg-secondary me-1">{t('pdftools.page_n', { PH: h.page })}</span>{h.context}</label>
                                </li>
                            {/each}
                        </ul>
                    {/if}
                </div>
            {/if}
            <hr>
            <div class="form-check">
                <input class="form-check-input" type="checkbox" id="redactMetadata" bind:checked={removeMetadata}>
                <label class="form-check-label" for="redactMetadata">{t('pdftools.remove_metadata')}</label>
            </div>
            <div class="alert alert-warning small mt-3 mb-0">{t('pdftools.redact_warning')}</div>
        </div>
    </div>
</div>
<div class="modal-footer">
    <span class="me-auto small text-muted">{t('pdftools.areas_selected', { PH: areas.length })}</span>
    <button type="button" class="btn btn-outline-secondary" onclick={() => modal.close()} disabled={busy}>{t('common.cancel')}</button>
    <button type="button" class="btn btn-danger" onclick={apply} disabled={busy || (!areas.length && !removeMetadata)}>
        {#if busy}<span class="spinner-border spinner-border-sm me-1" role="status"></span>{/if}{t('pdftools.redact_permanently')}
    </button>
</div></pngx-redact-dialog>
