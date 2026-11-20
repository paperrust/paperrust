<!-- Port of common/pdf-editor/pdf-editor.component.{html,ts,scss} (dialog "PDF editor", size xl). Pages as
     thumbnails, selection, rotating, deleting, reordering by dragging, split "Split here", mode "New documents" /
     "Add document version", "Copy metadata", "Delete original". Confirm calls
     onconfirm(setEnabled, { operations, deleteOriginal, updateDocument, includeMetadata }); cancel = dismiss.
     Deviations: pdf.js loads the file once and draws each page into a <canvas> when it becomes visible (Angular: one
     own pngx-pdf-viewer per page); dragging via HTML5 drag & drop instead of CDK (cdkDropListOrientation="mixed");
     an already entered PDF password is passed along. -->
<script module lang="ts">
  export type PdfEditorResult = {
    operations: { page: number; rotate: number; doc: number }[]
    deleteOriginal: boolean
    updateDocument: boolean
    includeMetadata: boolean
  }
</script>

<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { registerIcons, ricon } from '../../../lib/route-icons'
  import iScissors from 'bootstrap-icons/icons/scissors.svg?raw'
  registerIcons({ 'scissors': iScissors })
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { api } from '../../../api/client'
  import { rawValue, session } from '../../../stores/settings.svelte'
  import { loadPdfjs, pdfjsAssetUrl } from '../pdf-viewer/pdfjs'
  import { PdfEditorEditMode, type PageOperation, initialPages, rotateBy, moveItem, getOperations, editModeFrom } from './pdf-editor'

  let {
    modal,
    title = t('common.pdf_editor'),
    btnClass = 'btn-primary',
    btnCaption = t('common.proceed'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    documentID,
    versionID = undefined,
    password = undefined,
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    documentID: number
    versionID?: number
    password?: string
    onconfirm?: (setEnabled: (b: boolean) => void, result: PdfEditorResult) => void | Promise<void>
  } = $props()

  let buttonsEnabled = $state(true)
  let pages = $state<PageOperation[]>([])
  let editMode = $state<PdfEditorEditMode>(editModeFrom(rawValue(session.settings, 'general-settings:document-editing:default-edit-mode')))
  let deleteOriginal = $state(false)
  let includeMetadata = $state(true)
  let loadError = $state<string | null>(null)

  const hasSelection = $derived(pages.some((p) => p.selected))
  const hasSplit = $derived(pages.some((p) => p.splitAfter))

  // ---- pdf.js: load file once (pdfLoaded) ------------------------------------------------
  let pdfDoc: any = null
  let loadingTask: any = null
  let destroyed = false
  const pdfSrc = () => api.previewUrl(documentID, false, versionID ?? null)

  onMount(async () => {
    try {
      const { lib } = await loadPdfjs()
      if (destroyed) return
      loadingTask = lib.getDocument({
        url: pdfSrc(),
        password,
        withCredentials: true,
        wasmUrl: pdfjsAssetUrl('wasm'),
        iccUrl: pdfjsAssetUrl('iccs'),
        cMapUrl: pdfjsAssetUrl('cmaps'),
      } as any)
      const doc = await loadingTask.promise
      if (destroyed) return
      pdfDoc = doc
      pages = initialPages(doc.numPages)
    } catch (e: any) {
      if (!destroyed) loadError = String(e?.message ?? e)
    }
  })
  onDestroy(() => {
    destroyed = true
    loadingTask?.destroy()
  })

  /** Preview of a page: draw only when it becomes visible (@defer (on viewport)), redraw on rotation */
  function thumb(node: HTMLCanvasElement, params: { pageNo: number; rotate: number; onrendered: () => void }) {
    let current = params
    let visible = false
    let drawn: string | null = null
    let task: any = null
    let seq = 0
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        visible = true
        io.disconnect()
        draw()
      }
    })
    io.observe(node.parentElement ?? node)
    async function draw() {
      if (!visible || !pdfDoc) return
      const key = `${current.pageNo}:${current.rotate}`
      if (key === drawn) return
      const my = ++seq
      task?.cancel()
      try {
        const page = await pdfDoc.getPage(current.pageNo)
        if (my !== seq || destroyed) return
        const rotation = ((page.rotate ?? 0) + current.rotate) % 360
        const base = page.getViewport({ scale: 1, rotation })
        const box = node.parentElement
        const w = box?.clientWidth || 180
        const h = box?.clientHeight || 240
        const dpr = window.devicePixelRatio || 1
        const scale = Math.min(w / base.width, h / base.height)
        const vp = page.getViewport({ scale: scale * dpr, rotation })
        node.width = Math.floor(vp.width)
        node.height = Math.floor(vp.height)
        node.style.width = `${Math.floor(vp.width / dpr)}px`
        node.style.height = `${Math.floor(vp.height / dpr)}px`
        task = page.render({ canvas: node, viewport: vp })
        await task.promise
        if (my !== seq) return
        drawn = key
        current.onrendered()
      } catch {
        // aborted (new rotation) or page not readable: placeholder stays
      }
    }
    return {
      update(p: typeof params) {
        current = p
        draw()
      },
      destroy() {
        io.disconnect()
        seq++
        task?.cancel()
      },
    }
  }

  // ---- Page operations ---------------------------------------------------------------------
  const toggleSelection = (i: number) => (pages[i].selected = !pages[i].selected)
  const rotate = (i: number, counterclockwise = false) => (pages[i].rotate = rotateBy(pages[i].rotate, counterclockwise ? -90 : 90))
  function rotateSelected(dir: number) {
    for (const p of pages) if (p.selected) p.rotate = rotateBy(p.rotate, dir)
  }
  const remove = (i: number) => pages.splice(i, 1)
  function toggleSplit(i: number) {
    pages[i].splitAfter = !pages[i].splitAfter
    // force create mode
    if (pages[i].splitAfter) editMode = PdfEditorEditMode.Create
  }
  const selectAll = () => pages.forEach((p) => (p.selected = true))
  const deselectAll = () => pages.forEach((p) => (p.selected = false))
  const deleteSelected = () => (pages = pages.filter((p) => !p.selected))

  // ---- Reordering (cdkDropList → HTML5 drag & drop) -------------------------------------------
  let dragIndex = $state<number | null>(null)
  let overIndex = $state<number | null>(null)
  function onDragStart(e: DragEvent, i: number) {
    dragIndex = i
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move'
      e.dataTransfer.setData('text/plain', String(i))
    }
  }
  function onDragOver(e: DragEvent, i: number) {
    if (dragIndex === null) return
    e.preventDefault()
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'
    overIndex = i
  }
  function onDrop(e: DragEvent, i: number) {
    if (dragIndex === null) return
    e.preventDefault()
    pages = moveItem(pages, dragIndex, i)
    dragIndex = overIndex = null
  }
  const onDragEnd = () => (dragIndex = overIndex = null)

  async function confirm() {
    const result: PdfEditorResult = {
      operations: getOperations(pages),
      deleteOriginal,
      updateDocument: editMode == PdfEditorEditMode.Update,
      includeMetadata,
    }
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), result)
    else modal.close('confirm')
  }
</script>

<pngx-pdf-editor><div class="modal-header">
  <h4 class="modal-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}></button>
</div>
<div class="modal-body pdf-editor-body">
  <div class="btn-toolbar mb-2">
    <div class="btn-group me-3">
      <button class="btn btn-sm btn-secondary" onclick={selectAll} title={t('dialog.pdf_editor.select_all_pages')}>
        <pngx-bs-icon name="check-all">{@html icon('check-all')}</pngx-bs-icon>
      </button>
      <button class="btn btn-sm btn-secondary" onclick={deselectAll} disabled={!hasSelection} title={t('dialog.pdf_editor.deselect_all_pages')}>
        <pngx-bs-icon name="x">{@html icon('x')}</pngx-bs-icon>
      </button>
    </div>
    <div class="btn-group">
      <button class="btn btn-sm btn-secondary" onclick={() => rotateSelected(-90)} disabled={!hasSelection} title={t('dialog.pdf_editor.rotate_selected_left')}>
        <pngx-bs-icon name="arrow-counterclockwise">{@html icon('arrow-counterclockwise')}</pngx-bs-icon>
      </button>
      <button class="btn btn-sm btn-secondary" onclick={() => rotateSelected(90)} disabled={!hasSelection} title={t('dialog.pdf_editor.rotate_selected_right')}>
        <pngx-bs-icon name="arrow-clockwise">{@html icon('arrow-clockwise')}</pngx-bs-icon>
      </button>
      <button class="btn btn-sm btn-danger" onclick={deleteSelected} disabled={!hasSelection} title={t('dialog.pdf_editor.delete_selected_pages')}>
        <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
      </button>
    </div>
  </div>
  {#if loadError}
    <div class="alert alert-danger small">{loadError}</div>
  {/if}
  <div class="d-flex flex-wrap row-cols-2 row-cols-md-5" role="list">
    {#each pages as p, i (p.page)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div
        class="page-item rounded p-2"
        class:selected={p.selected}
        class:drag-over={overIndex === i && dragIndex !== null && dragIndex !== i}
        class:dragging={dragIndex === i}
        role="listitem"
        draggable="true"
        onclick={() => toggleSelection(i)}
        ondragstart={(e) => onDragStart(e, i)}
        ondragover={(e) => onDragOver(e, i)}
        ondrop={(e) => onDrop(e, i)}
        ondragend={onDragEnd}
      >
        <div class="btn-toolbar hover-actions z-10">
          <div class="btn-group me-2">
            <button class="btn btn-sm btn-dark" onclick={(e) => { e.stopPropagation(); rotate(i, true) }} title={t('dialog.pdf_editor.rotate_page_left')}>
              <pngx-bs-icon name="arrow-counterclockwise">{@html icon('arrow-counterclockwise')}</pngx-bs-icon>
            </button>
            <button class="btn btn-sm btn-dark" onclick={(e) => { e.stopPropagation(); rotate(i) }} title={t('dialog.pdf_editor.rotate_page_right')}>
              <pngx-bs-icon name="arrow-clockwise">{@html icon('arrow-clockwise')}</pngx-bs-icon>
            </button>
          </div>
          <div class="btn-group">
            <button class="btn btn-sm btn-dark text-danger" onclick={(e) => { e.stopPropagation(); remove(i) }} title={t('dialog.pdf_editor.delete_page')}>
              <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
            </button>
            <button class="btn btn-sm btn-dark" onclick={(e) => { e.stopPropagation(); toggleSplit(i) }} title={t('dialog.pdf_editor.toggle_split')}>
              <pngx-bs-icon name="scissors">{@html ricon('scissors')}</pngx-bs-icon>
            </button>
          </div>
        </div>
        <div class="border-end border-bottom bg-light py-1 px-2 document-check z-10">
          <div class="form-check">
            <input type="checkbox" class="form-check-input" id="page{i}" checked={p.selected} onclick={(e) => { e.stopPropagation(); toggleSelection(i) }}>
            <label class="form-check-label" for="page{i}"></label>
          </div>
        </div>
        <div class="pdf-viewer-container w-100" class:selected={p.selected}>
          {#if !p.loaded}
            <div class="placeholder-glow w-100 h-100 z-10 position-absolute top-0 start-0">
              <span class="placeholder w-100 h-100"></span>
            </div>
          {/if}
          <canvas class="fade" class:show={p.loaded} use:thumb={{ pageNo: p.page, rotate: p.rotate, onrendered: () => (p.loaded = true) }}></canvas>
        </div>
        {#if p.splitAfter}
          <div class="split-after rounded position-absolute top-0 end-0 bg-dark text-uppercase text-center h-100 px-1 small fw-bold">&mdash; <span>{t('dialog.pdf_editor.split_here')}</span> &mdash;</div>
        {/if}
      </div>
    {/each}
  </div>
</div>
<div class="modal-footer">
  <div class="d-flex flex-column flex-md-row w-100 gap-3 align-items-center">
    <div class="btn-group" role="group">
      <input type="radio" class="btn-check" bind:group={editMode} value={PdfEditorEditMode.Create} id="editModeCreate" name="editmode">
      <label for="editModeCreate" class="btn btn-outline-primary btn-sm">
        <pngx-bs-icon name="plus">{@html icon('plus')}</pngx-bs-icon>
        <span class="form-check-label ms-1">{t('admin.settings.create_new_document_s')}</span>
      </label>
      <input type="radio" class="btn-check" bind:group={editMode} value={PdfEditorEditMode.Update} id="editModeUpdate" name="editmode" disabled={hasSplit}>
      <label for="editModeUpdate" class="btn btn-outline-primary btn-sm">
        <pngx-bs-icon name="pencil">{@html icon('pencil')}</pngx-bs-icon>
        <span class="form-check-label ms-2">{t('admin.settings.add_document_version')}</span>
      </label>
    </div>
    {#if editMode === PdfEditorEditMode.Create}
      <div class="form-group d-flex">
        <div class="form-check">
          <input class="form-check-input" type="checkbox" id="copyMeta" bind:checked={includeMetadata}>
          <label class="form-check-label" for="copyMeta">{t('dialog.unprotect.copy_metadata')}</label>
        </div>
        <div class="form-check ms-3">
          <input class="form-check-input" type="checkbox" id="deleteOriginal" bind:checked={deleteOriginal}>
          <label class="form-check-label" for="deleteOriginal">{t('dialog.unprotect.delete_original')}</label>
        </div>
      </div>
    {/if}
    <div class="form-group ms-md-auto">
      <button type="button" class="btn me-2 {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>{cancelBtnCaption}</button>
      <button type="button" class="btn {btnClass}" onclick={confirm} disabled={pages.length === 0 || !buttonsEnabled}>{btnCaption}</button>
    </div>
  </div>
</div></pngx-pdf-editor>

<style>
  /* pdf-editor.component.scss (no adopted Angular CSS available, hence here) */
  .pdf-editor-body {
    max-height: calc(100vh - 14rem);
    overflow-y: auto;
  }
  .page-item {
    position: relative;
    cursor: pointer;
    border: 1px solid transparent;
    background-origin: border-box;
  }
  .page-item.selected {
    background-color: var(--pr-primary-darken-5);
  }
  .page-item.dragging {
    opacity: 0.5;
  }
  .page-item.drag-over {
    border-color: var(--bs-primary);
  }
  .pdf-viewer-container {
    position: relative;
    background-color: gray;
    height: 240px;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .pdf-viewer-container canvas {
    max-width: 100%;
    max-height: 100%;
  }
  .hover-actions {
    position: absolute;
    top: 0;
    right: 0;
    display: none;
  }
  .page-item:hover .hover-actions {
    display: block;
  }
  .document-check {
    display: none;
    position: absolute;
    top: 0;
    left: 0;
    padding: 0.5rem;
    border-top-left-radius: 0.25rem;
    border-bottom-right-radius: 0.25rem;
    pointer-events: none;
  }
  .document-check .form-check {
    padding: 0;
    min-height: 0;
    margin-bottom: 0;
  }
  .document-check .form-check-input {
    margin-left: 0;
  }
  .page-item:hover .document-check,
  .selected .document-check {
    display: block;
  }
  .z-10 {
    z-index: 10;
  }
  .split-after {
    writing-mode: vertical-rl;
  }
</style>
