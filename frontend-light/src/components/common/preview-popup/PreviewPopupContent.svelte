<!-- Port of the <ng-template #previewContent> from common/preview-popup/preview-popup.component.html
     (content of .preview-popup-container) and init()/onError() from preview-popup.component.ts.
     Loaded by PreviewPopup.svelte only on the first hover. If the document is missing (only ID known),
     title/MIME type/archive file are loaded; if that fails, it shows the preview as <object>. -->
<script lang="ts">
  import '../../../styles/components/preview-popup.css'
  import { onDestroy, onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { api } from '../../../api/client'
  import { withSearchHash } from '../../../stores/searchQuery'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import PdfViewer from '../pdf-viewer/PdfViewer.svelte'
  import { PdfRenderMode } from '../pdf-viewer/pdfjs'
  import { isPdf, previewKind, type PreviewDoc } from './preview-popup'

  let { id, document: initial = undefined, ondocument }: { id: number; document?: PreviewDoc; ondocument?: (d: PreviewDoc) => void } = $props()

  // svelte-ignore state_referenced_locally
  let doc = $state<PreviewDoc | undefined>(initial?.mime_type !== undefined ? initial : undefined)
  let error = $state(false)
  let requiresPassword = $state(false)
  let previewText = $state<string | null>(null)
  let destroyed = false
  onDestroy(() => (destroyed = true))

  const previewUrl = $derived(api.previewUrl(id))
  const kind = $derived(doc ? previewKind(doc, !!session.get(SETTINGS_KEYS.USE_NATIVE_PDF_VIEWER)) : undefined)

  onMount(() => {
    if (doc) return
    api.getDocument(id, null, 'id,title,mime_type,archived_file_name').then(
      (d) => {
        if (destroyed) return
        doc = d
        ondocument?.(d)
      },
      () => {
        if (!destroyed) doc = { id }
      }
    )
  })

  // init(): load text documents via HTTP
  let textRequested = false
  $effect(() => {
    if (kind !== 'text' || textRequested) return
    textRequested = true
    fetch(previewUrl, { credentials: 'same-origin' })
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error(String(r.status)))))
      .then(
        (txt) => !destroyed && (previewText = txt),
        () => !destroyed && (error = true)
      )
  })

  // onError
  function onError(e: unknown) {
    if ((e as { name?: string } | null)?.name == 'PasswordException') requiresPassword = true
    else error = true
  }

  const LOCK_ICON =
    '<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="2em" fill="currentColor" class="bi bi-file-earmark-lock" viewBox="0 0 16 16"><path d="M10 7v1.076c.54.166 1 .597 1 1.224v2.4c0 .816-.781 1.3-1.5 1.3h-3c-.719 0-1.5-.484-1.5-1.3V9.3c0-.627.46-1.058 1-1.224V7a2 2 0 1 1 4 0M7 7v1h2V7a1 1 0 0 0-2 0M6 9.3v2.4c0 .042.02.107.105.175A.64.64 0 0 0 6.5 12h3a.64.64 0 0 0 .395-.125c.085-.068.105-.133.105-.175V9.3c0-.042-.02-.107-.105-.175A.64.64 0 0 0 9.5 9h-3a.64.64 0 0 0-.395.125C6.02 9.193 6 9.258 6 9.3"/><path d="M14 14V4.5L9.5 0H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2M9.5 3A1.5 1.5 0 0 0 11 4.5h2V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1h5.5z"/></svg>'
</script>

{#if error}
  <div class="w-100 h-100 position-relative">
    <p class="fst-italic position-absolute top-50 start-50 translate-middle">{t('app.preview.load_error')}</p>
  </div>
{:else if kind === 'text'}
  {#if previewText !== null}
    <div class="bg-light p-3 overflow-auto whitespace-preserve" {...{ width: '100%' }}>{previewText}</div>
  {/if}
{:else if kind === 'object'}
  <object data={withSearchHash(previewUrl)} width="100%" class="bg-light" class:p-2={!isPdf(doc)} title=""></object>
{:else if kind === 'pdfjs'}
  {#if requiresPassword}
    <div class="w-100 h-100 position-relative">
      <pngx-bs-icon width="2em" height="2em" class="position-absolute top-50 start-50 translate-middle">{@html LOCK_ICON}</pngx-bs-icon>
    </div>
  {:else}
    <pngx-pdf-viewer><PdfViewer src={previewUrl} renderMode={PdfRenderMode.All} onloaderror={onError} /></pngx-pdf-viewer>
  {/if}
{/if}
