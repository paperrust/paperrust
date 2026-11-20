<!-- Port of file-drop/file-drop.component.html/.ts: drop files anywhere → upload.
     Host <pngx-file-drop> in the parent markup (App.svelte); content (ng-content [content]) as children. -->
<script lang="ts">
  import '../../styles/components/file-drop.css'
  import type { Snippet } from 'svelte'
  import { t } from '../../lib/i18n'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { dropzone } from './dropzone.svelte'

  let { children }: { children: Snippet } = $props()

  let fileIsOver = $state(false)
  let hidden = $state(true)
  let fileLeaveTimeoutID: ReturnType<typeof setTimeout> | undefined

  const dragDropEnabled = () => dropzone.enabled && session.can('add', 'document')

  function onDragOver(event: DragEvent) {
    if (!dragDropEnabled() || !event.dataTransfer?.types?.includes('Files')) return
    event.preventDefault()
    event.stopImmediatePropagation()
    dropzone.active = true
    // allows the transition
    setTimeout(() => (fileIsOver = true), 1)
    hidden = false
    clearTimeout(fileLeaveTimeoutID)
  }

  function onDragLeave(event: DragEvent | null, immediate = false) {
    if (!dragDropEnabled()) return
    event?.preventDefault()
    event?.stopImmediatePropagation()
    dropzone.active = false
    fileLeaveTimeoutID = setTimeout(
      () => {
        fileIsOver = false
        // wait for the transition
        setTimeout(() => (hidden = true), 150)
      },
      immediate ? 0 : 500,
    )
  }

  function traverseFileTree(entry: FileSystemEntry): Promise<File[]> {
    if (entry.isFile) return new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject)).then((f) => [f])
    if (entry.isDirectory) {
      return new Promise<File[]>((resolve, reject) => {
        const reader = (entry as FileSystemDirectoryEntry).createReader()
        const all: FileSystemEntry[] = []
        const readEntries = () => {
          reader.readEntries((batch) => {
            if (batch.length === 0) {
              Promise.all(all.map((c) => traverseFileTree(c)))
                .then((r) => resolve(r.flat()))
                .catch(reject)
            } else {
              all.push(...batch)
              readEntries()
            }
          }, reject)
        }
        readEntries()
      })
    }
    return Promise.resolve([])
  }

  function onDrop(event: DragEvent) {
    if (!dragDropEnabled()) return
    event.preventDefault()
    event.stopImmediatePropagation()
    const files: File[] = []
    const entries: FileSystemEntry[] = []
    if (event.dataTransfer?.items && event.dataTransfer.items.length) {
      for (const item of Array.from(event.dataTransfer.items)) {
        if (item.webkitGetAsEntry) {
          const entry = item.webkitGetAsEntry()
          if (entry) entries.push(entry)
        } else if (item.kind === 'file') {
          const file = item.getAsFile()
          if (file) files.push(file)
        }
      }
    } else if (event.dataTransfer?.files) {
      files.push(...Array.from(event.dataTransfer.files))
    }
    if (entries.length) {
      Promise.all(entries.map((e) => traverseFileTree(e)))
        .then((r) => {
          files.push(...r.flat())
          consumerStatus.uploadFiles(files)
        })
        .catch((e) => toastService.showError(t('frame.failed_read_dropped_items', { PH: e?.message })))
    } else if (files.length) consumerStatus.uploadFiles(files)
    onDragLeave(event, true)
  }
</script>

<svelte:document
  ondragover={onDragOver}
  ondragleave={(e) => onDragLeave(e)}
  ondrop={onDrop}
  onvisibilitychange={() => {
    if (document.hidden && fileIsOver) onDragLeave(null)
  }} />
<svelte:window onblur={() => onDragLeave(null)} />

<div class:pe-none={fileIsOver}>
    {@render children()}
</div>

<div class="global-dropzone-overlay position-fixed top-0 start-0 bottom-0 end-0 text-center pe-none" class:active={fileIsOver && !hidden}>
    <h2 class="pe-none position-absolute top-50 start-50 translate-middle">{t('frame.drop_files_begin_upload')}</h2>
</div>
