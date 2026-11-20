<!-- Port of document-notes/document-notes.component.html (Host <pngx-document-notes> in the parent markup). -->
<script lang="ts">
  import '../../styles/components/document-notes.css'
  import { t } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { formatDate } from '../../lib/dates'
  import { api } from '../../api/client'
  import type { DocumentNote } from '../../api/v1'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { noteDisplayName } from './notes'
  let {
    documentId,
    notes = [],
    addDisabled = false,
    dateLocale,
    onupdated,
  }: { documentId: number; notes?: DocumentNote[]; addDisabled?: boolean; dateLocale: string; onupdated?: (n: DocumentNote[]) => void } = $props()
  let newNote = $state('')
  let newNoteError = $state(false)
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
    api.addNote(documentId, note).then(
      (r) => {
        newNote = ''
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
  function keydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') addNote()
  }
</script>

{#if notes}
  <div>
    {#if session.can('add', 'note')}
    <form class="needs-validation mt-3" novalidate>
      <div class="form-group">
        <textarea class="form-control form-control-sm" class:is-invalid={newNoteError} rows="3" bind:value={newNote} placeholder={t('detail.enter_note')} onkeydown={keydown} required></textarea>
        <div class="invalid-feedback">
                {t('detail.please_enter_note')}
            </div>
      </div>
      <div class="form-group mt-2 d-flex justify-content-end align-items-center">
        {#if networkActive}
          <div class="spinner-border spinner-border-sm fw-normal me-auto" role="status"></div>
        {/if}
        <button type="button" class="btn btn-primary btn-sm" disabled={networkActive || addDisabled} onclick={addNote}>{t('common.add_note')}</button>
      </div>
    </form>
    {/if}
    <hr>
      {#each notes as note (note.id)}
        <div class="card border mb-3">
          <div class="card-body text-dark">
            <p class="card-text">{note.note}</p>
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
