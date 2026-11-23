<!-- Port of document-notes/document-notes.component.html (host <pngx-document-notes> in the parent markup).
     Notes as Markdown (upstream 3.5.1, PR #14626): format toggle plain/markdown (default plain) with preview. -->
<script lang="ts">
  import '../../styles/components/document-notes.css'
  import { t } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { formatDate } from '../../lib/dates'
  import { api } from '../../api/client'
  import type { DocumentNote, NoteFormat } from '../../api/v1'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { noteDisplayName, noteFormat, renderNoteMarkdown } from './notes'
  let {
    documentId,
    notes = [],
    addDisabled = false,
    dateLocale,
    onupdated,
  }: { documentId: number; notes?: DocumentNote[]; addDisabled?: boolean; dateLocale: string; onupdated?: (n: DocumentNote[]) => void } = $props()
  let newNote = $state('')
  let newNoteError = $state(false)
  let format = $state<NoteFormat>('plain')
  let preview = $state(false)
  let networkActive = $state(false)
  const dateFormat = $derived(session.get(SETTINGS_KEYS.DATE_FORMAT))

  function addNote() {
    const note = newNote.toString().trim()
    if (note.length == 0) {
      newNoteError = true
      return
    }
    newNoteError = false
    networkActive = true
    api.addNote(documentId, note, format).then(
      (r) => {
        newNote = ''
        preview = false
        networkActive = false
        onupdated?.(r)
      },
      (e) => {
        networkActive = false
        console.error('Error saving note', e)
      },
    )
  }
  function deleteNote(id: number) {
    api.deleteNote(documentId, id).then(
      (r) => {
        networkActive = false
        onupdated?.(r)
      },
      (e) => {
        networkActive = false
        console.error('Error deleting note', e)
      },
    )
  }
  function setFormat(f: NoteFormat) {
    format = f
    if (f === 'plain') preview = false
  }
  function keydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') addNote()
  }
</script>

{#if notes}
  <div>
    {#if session.can('add', 'note')}
    <form class="needs-validation mt-3" novalidate>
      <div class="form-group">
        <textarea class="form-control form-control-sm" class:is-invalid={newNoteError} class:d-none={preview} aria-invalid={newNoteError} rows="3" bind:value={newNote} placeholder={format === 'markdown' ? t('detail.enter_note_markdown') : t('detail.enter_note')} onkeydown={keydown} required></textarea>
        {#if preview}
          <section class="form-control form-control-sm note-preview" aria-label={t('detail.note_preview')}>
            {#if newNote.trim()}
              <div class="note-markdown text-break text-wrap">{@html renderNoteMarkdown(newNote)}</div>
            {:else}
              <span class="text-muted">{t('detail.nothing_to_preview')}</span>
            {/if}
          </section>
        {/if}
        <div class="invalid-feedback">
                {t('detail.please_enter_note')}
            </div>
      </div>
      <div class="form-group mt-2 d-flex flex-wrap gap-2 justify-content-end align-items-center">
        {#if networkActive}
          <div class="spinner-border spinner-border-sm fw-normal me-auto" role="status"></div>
        {/if}
        <fieldset class="btn-group btn-group-sm" aria-label={t('detail.note_format')}>
          <input type="radio" class="btn-check" id="noteFormatPlain-{documentId}" name="noteFormat-{documentId}" checked={format === 'plain'} onchange={() => setFormat('plain')}>
          <label class="btn btn-outline-secondary" for="noteFormatPlain-{documentId}">{t('detail.note_format_plain')}</label>
          <input type="radio" class="btn-check" id="noteFormatMarkdown-{documentId}" name="noteFormat-{documentId}" checked={format === 'markdown'} onchange={() => setFormat('markdown')}>
          <label class="btn btn-outline-secondary" for="noteFormatMarkdown-{documentId}">{t('detail.note_format_markdown')}</label>
        </fieldset>
        {#if format === 'markdown'}
          <input type="checkbox" class="btn-check" id="notePreview-{documentId}" autocomplete="off" bind:checked={preview}>
          <label class="btn btn-outline-secondary btn-sm" for="notePreview-{documentId}">{t('common.preview')}</label>
        {/if}
        <button type="button" class="btn btn-primary btn-sm" disabled={networkActive || addDisabled} onclick={addNote}>{t('common.add_note')}</button>
      </div>
    </form>
    {/if}
    <hr>
      {#each notes as note (note.id)}
        <div class="card border mb-3">
          <div class="card-body text-dark">
            {#if noteFormat(note) === 'markdown'}
              <div class="card-text note-markdown text-break text-wrap">{@html renderNoteMarkdown(note.note)}</div>
            {:else}
              <p class="card-text">{note.note}</p>
            {/if}
          </div>
          <div class="d-flex card-footer small bg-light text-primary justify-content-between align-items-center">
            <span>{noteDisplayName(note, data.users)} - {formatDate(note.created, dateFormat, dateLocale)}</span>
            {#if session.can('delete', 'note')}
            <button type="button" class="btn btn-link btn-sm p-0 fade" title={t('common.delete_note')} onclick={() => deleteNote(note.id)}>
              <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
              <span class="visually-hidden">{t('common.delete_note')}</span>
            </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
