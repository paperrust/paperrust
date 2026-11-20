<!-- Preview: PDF with pdf.js (component of the classic Light UI, loaded lazily), image, text, otherwise thumbnail. -->
<script lang="ts">
  import type { Component } from 'svelte'
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { Document } from '../../api/v1'
  import { renderType } from '../api/docs'
  import Icon from '../components/Icon.svelte'

  let { doc }: { doc: Document } = $props()
  const kind = $derived(renderType(doc.mime_type, !!doc.archived_file_name))
  let Pdf = $state<Component<any> | null>(null)
  let mod: any = $state(null)
  let zoom = $state<string>('page-width')
  let level = $state<string>('1')
  let pages = $state(0)
  let page = $state(1)
  let text = $state<string | null>(null)
  let failed = $state(false)

  $effect(() => {
    if (kind === 'pdf' && !Pdf)
      Promise.all([import('../../components/common/pdf-viewer/PdfViewer.svelte'), import('../../components/common/pdf-viewer/pdfjs')]).then(([c, m]) => {
        mod = m
        Pdf = c.default
      })
  })
  $effect(() => {
    void doc.id
    page = 1
    pages = 0
    failed = false
    text = null
    if (kind === 'text')
      fetch(api.previewUrl(doc.id), { credentials: 'same-origin' })
        .then((r) => r.text())
        .then((s) => (text = s))
        .catch(() => (failed = true))
  })
  const zoomIn = () => (level = String(Math.min(3, +level + 0.25)))
  const zoomOut = () => (level = String(Math.max(0.25, +level - 0.25)))
</script>

<div class="pv" aria-label={t('common.preview')}>
  {#if kind === 'pdf'}
    {#if Pdf && mod && !failed}
      <pngx-pdf-viewer sh-pdf-viewer="">
        <Pdf src={api.previewUrl(doc.id)} renderMode={mod.PdfRenderMode.All} zoomScale={zoom} zoom={level} {page} onpagechange={(p: number) => (page = p)} onafterloadcomplete={(p: { numPages: number }) => (pages = p.numPages)} onloaderror={() => (failed = true)} />
      </pngx-pdf-viewer>
      <div class="ctl" role="toolbar" aria-label={t('common.preview')}>
        {#if pages}<span class="pg">{t('m.workspace.page', { PH: page, PH_1: pages })}</span>{/if}
        <button class="d2-btn ghost sm icon" aria-label={t('common.zoom_out')} onclick={zoomOut}><Icon name="zoom-out" /></button>
        <button class="d2-btn ghost sm icon" aria-label={t('common.zoom_in')} onclick={zoomIn}><Icon name="zoom-in" /></button>
        <button class="d2-btn ghost sm" aria-pressed={zoom === 'page-fit'} onclick={() => { zoom = zoom === 'page-fit' ? 'page-width' : 'page-fit'; level = '1' }}>{zoom === 'page-fit' ? t('common.fit_width') : t('common.fit_page')}</button>
      </div>
    {:else if failed}
      <div class="d2-empty"><img class="th" src={api.thumbUrl(doc.id)} alt="" /><p>{t('m.workspace.preview_could_not_loaded')}</p></div>
    {:else}
      <div class="d2-empty" aria-busy="true">{t('common.loading')}</div>
    {/if}
  {:else if kind === 'image' || kind === 'tiff'}
    <div class="img d2-scroll"><img src={api.previewUrl(doc.id)} alt={doc.title} /></div>
  {:else if kind === 'text'}
    <pre class="txt d2-scroll">{text ?? ''}</pre>
  {:else}
    <div class="d2-empty"><img class="th" src={api.thumbUrl(doc.id)} alt="" /><p>{t('m.workspace.no_preview_available_file')}</p></div>
  {/if}
</div>

<style>
  .pv {
    position: relative;
    height: 100%;
    min-height: 0;
    background: var(--d2-surface-2);
  }
  .ctl {
    position: absolute;
    left: 50%;
    bottom: var(--d2-s3);
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 3px 6px;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r-pill);
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-1);
    white-space: nowrap;
  }
  .pg {
    padding: 0 var(--d2-s2);
    font-size: var(--d2-fs-sm);
    color: var(--d2-text-2);
  }
  .img {
    height: 100%;
    padding: var(--d2-s4);
    text-align: center;
  }
  .img img {
    max-width: 100%;
    box-shadow: var(--d2-shadow-1);
  }
  .txt {
    height: 100%;
    margin: 0;
    padding: var(--d2-s4);
    white-space: pre-wrap;
    font-family: var(--d2-mono);
    font-size: var(--d2-fs-sm);
  }
  .th {
    max-width: 200px;
    border: 1px solid var(--d2-border);
  }
  :global(.d2) .pv :global(.pngx-pdf-viewer-container) {
    padding: var(--d2-s3) 0 56px;
  }
</style>
