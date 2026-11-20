<!-- Port of common/pdf-viewer/pdf-viewer.component.{html,ts}: same pdf.js version and options
     (PDFViewer/PDFSinglePageViewer, textLayerMode 1, AnnotationMode.ENABLE, removePageBorders, zoom via
     currentScaleValue + Faktor). Host <pngx-pdf-viewer> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/pdf-viewer.css'
  import { onDestroy, onMount } from 'svelte'
  import { loadPdfjs, pdfjsAssetUrl, PdfRenderMode, PdfZoomLevel, PdfZoomScale, type PdfjsLib, type PdfjsViewer } from './pdfjs'

  let {
    src,
    sourceRevision = 0,
    password = undefined,
    page = undefined,
    rotation = undefined,
    renderMode = PdfRenderMode.All,
    selectable = true,
    zoom = PdfZoomLevel.One,
    zoomScale = PdfZoomScale.PageWidth,
    onpagechange,
    onafterloadcomplete,
    onrendered,
    onloaderror,
  }: {
    src: string | undefined
    sourceRevision?: number
    password?: string
    page?: number
    rotation?: number
    renderMode?: PdfRenderMode
    selectable?: boolean
    zoom?: PdfZoomLevel | string
    zoomScale?: PdfZoomScale | string
    onpagechange?: (p: number) => void
    onafterloadcomplete?: (pdf: { numPages: number }) => void
    onrendered?: () => void
    onloaderror?: (e: unknown) => void
  } = $props()

  let container: HTMLDivElement
  let viewerEl: HTMLDivElement
  let lib: PdfjsLib
  let V: PdfjsViewer
  let eventBus: any
  let linkService: any
  let findController: any
  let pdfViewer: any
  let pdf: any
  let loadingTask: any
  let resizeObserver: ResizeObserver | undefined
  let hasLoaded = false
  let lastViewerPage: number | undefined
  let ready = false
  let destroyed = false

  const onPageRendered = () => onrendered?.()
  const onPagesInit = () => applyViewerState()
  const onPageChanging = (evt: { pageNumber: number }) => {
    lastViewerPage = evt.pageNumber
    onpagechange?.(evt.pageNumber)
  }

  onMount(async () => {
    ;({ lib, viewer: V } = await loadPdfjs())
    if (destroyed) return
    eventBus = new V.EventBus()
    linkService = new V.PDFLinkService({ eventBus, externalLinkTarget: V.LinkTarget.BLANK, externalLinkRel: 'noopener noreferrer nofollow' })
    findController = new V.PDFFindController({ eventBus, linkService, updateMatchesCountOnProgress: false })
    ready = true
    setupResizeObserver()
    initViewer()
    loadDocument()
  })

  onDestroy(() => {
    destroyed = true
    eventBus?.off('pagerendered', onPageRendered)
    eventBus?.off('pagesinit', onPagesInit)
    eventBus?.off('pagechanging', onPageChanging)
    resizeObserver?.disconnect()
    loadingTask?.destroy()
    pdfViewer?.cleanup()
    pdfViewer = undefined
  })

  // ngOnChanges: source/revision/password → reload; page/zoom → apply
  let prevSrc: string | undefined
  let prevRev = -1
  let prevPw: string | undefined
  $effect(() => {
    const s = src
    const r = sourceRevision
    const p = password
    if (!ready) return
    if (s !== prevSrc || r !== prevRev || p !== prevPw) {
      const first = prevRev === -1
      prevSrc = s
      prevRev = r
      prevPw = p
      if (first) return
      resetViewerState()
      if (s) loadDocument()
    }
  })
  let prevZoom: string | undefined
  let prevScale: string | undefined
  let prevPage: number | undefined
  $effect(() => {
    const z = zoom
    const zs = zoomScale
    const pg = page
    const rot = rotation
    if (!ready) return
    const scaleChanged = z !== prevZoom || zs !== prevScale
    if (zs !== prevScale) setupResizeObserver()
    if (scaleChanged || pg !== prevPage) applyViewerState(scaleChanged || rot !== undefined)
    prevZoom = z
    prevScale = zs
    prevPage = pg
  })

  function resetViewerState() {
    hasLoaded = false
    lastViewerPage = undefined
    loadingTask?.destroy()
    loadingTask = undefined
    pdf = undefined
    linkService.setDocument(null)
    if (pdfViewer) {
      pdfViewer.setDocument(null)
      pdfViewer.currentPageNumber = 1
    }
  }

  async function loadDocument() {
    if (hasLoaded || !src) return
    hasLoaded = true
    loadingTask?.destroy()
    loadingTask = lib.getDocument({
      url: src,
      password,
      withCredentials: true,
      wasmUrl: pdfjsAssetUrl('wasm'),
      iccUrl: pdfjsAssetUrl('iccs'),
      cMapUrl: pdfjsAssetUrl('cmaps'),
    } as any)
    try {
      const doc = await loadingTask.promise
      if (destroyed) return
      pdf = doc
      linkService.setDocument(doc)
      findController.onIsPageVisible = () => true
      pdfViewer?.setDocument(doc)
      applyViewerState()
      onafterloadcomplete?.(doc)
    } catch (err) {
      onloaderror?.(err)
    }
  }

  function setupResizeObserver() {
    resizeObserver?.disconnect()
    resizeObserver = new ResizeObserver(() => applyScale())
    resizeObserver.observe(container)
  }

  function initViewer() {
    viewerEl.innerHTML = ''
    pdfViewer?.cleanup()
    const options = {
      container,
      viewer: viewerEl,
      eventBus,
      linkService,
      findController,
      textLayerMode: selectable === false ? 0 : 1,
      annotationMode: lib.AnnotationMode.ENABLE,
      enableSelectionRendering: false,
      removePageBorders: true,
    }
    pdfViewer = renderMode === PdfRenderMode.Single ? new V.PDFSinglePageViewer(options as any) : new V.PDFViewer(options as any)
    linkService.setViewer(pdfViewer)
    eventBus.off('pagerendered', onPageRendered)
    eventBus.off('pagesinit', onPagesInit)
    eventBus.off('pagechanging', onPageChanging)
    eventBus.on('pagerendered', onPageRendered)
    eventBus.on('pagesinit', onPagesInit)
    eventBus.on('pagechanging', onPageChanging)
    if (pdf) {
      pdfViewer.setDocument(pdf)
      applyViewerState()
    }
  }

  function applyViewerState(applyScaleToo = true) {
    if (!pdfViewer) return
    const hasPages = pdfViewer.pagesCount > 0
    if (typeof rotation === 'number' && hasPages) pdfViewer.pagesRotation = rotation
    if (typeof page === 'number' && hasPages && page !== lastViewerPage) {
      const next = Math.min(Math.max(Math.trunc(page), 1), pdfViewer.pagesCount)
      if (next !== pdfViewer.currentPageNumber) pdfViewer.currentPageNumber = next
    }
    if (page === lastViewerPage) lastViewerPage = undefined
    if (hasPages && applyScaleToo) applyScale()
  }

  function applyScale() {
    if (!pdfViewer || pdfViewer.pagesCount === 0) return
    const factor = Number(zoom) || 1
    pdfViewer.currentScaleValue = zoomScale
    if (factor !== 1) pdfViewer.currentScale = pdfViewer.currentScale * factor
  }
</script>

<div bind:this={container} class="pngx-pdf-viewer-container">
  <div bind:this={viewerEl} class="pdfViewer"></div>
</div>
