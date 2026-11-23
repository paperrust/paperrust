<!-- Notes of a document (v1 /api/documents/{id}/notes/); plain or Markdown (upstream 3.5.1, PR #14626). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { Document, DocumentNote, NoteFormat } from '../../api/v1'
  import { noteFormat, renderNoteMarkdown } from '../../components/document-notes/notes'
  import { data } from '../../stores/data.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'

  let { doc, onchange }: { doc: Document; onchange: (notes: DocumentNote[]) => void } = $props()
  let text = $state('')
  let busy = $state(false)
  let format = $state<NoteFormat>('plain')
  let preview = $state(false)
  const notes = $derived((doc.notes ?? []) as DocumentNote[])
  const canAdd = $derived(session.can('add', 'note'))
  const canDelete = $derived(session.can('delete', 'note'))
  const fmt = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })
  const who = (n: DocumentNote) => (typeof n.user === 'object' && n.user ? [n.user.first_name, n.user.last_name].filter(Boolean).join(' ') || n.user.username : data.username((n.user as number) ?? null))

  async function add() {
    if (!text.trim()) return
    busy = true
    try {
      onchange(await api.addNote(doc.id, text.trim(), format))
      text = ''
      preview = false
    } catch (e) {
      toastService.showError(t('m.workspace.error_saving_note'), e)
    } finally {
      busy = false
    }
  }
  function setFormat(f: NoteFormat) {
    format = f
    if (f === 'plain') preview = false
  }
  async function del(n: DocumentNote) {
    try {
      onchange(await api.deleteNote(doc.id, n.id))
    } catch (e) {
      toastService.showError(t('m.workspace.error_deleting_note'), e)
    }
  }
</script>

<div class="notes">
  {#if canAdd}
    <form onsubmit={(e) => (e.preventDefault(), add())}>
      <label class="d2-label" for="d2-note-new">{t('m.workspace.new_note')}</label>
      <textarea id="d2-note-new" class="d2-textarea" class:hidden={preview} bind:value={text} rows="3" placeholder={format === 'markdown' ? t('detail.enter_note_markdown') : undefined} onkeydown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && add()}></textarea>
      {#if preview}
        <section class="preview md" aria-label={t('detail.note_preview')}>
          {#if text.trim()}{@html renderNoteMarkdown(text)}{:else}<span class="d2-muted">{t('detail.nothing_to_preview')}</span>{/if}
        </section>
      {/if}
      <div class="act">
        <div class="seg" role="group" aria-label={t('detail.note_format')}>
          <button type="button" id="d2-note-plain" aria-pressed={format === 'plain'} onclick={() => setFormat('plain')}>{t('detail.note_format_plain')}</button>
          <button type="button" id="d2-note-markdown" aria-pressed={format === 'markdown'} onclick={() => setFormat('markdown')}>{t('detail.note_format_markdown')}</button>
        </div>
        {#if format === 'markdown'}
          <button type="button" class="d2-btn ghost sm" id="d2-note-preview" aria-pressed={preview} onclick={() => (preview = !preview)}>{preview ? t('common.edit') : t('common.preview')}</button>
        {/if}
        <span class="d2-muted hint">Ctrl+Enter</span><button class="d2-btn primary sm" disabled={busy || !text.trim()} id="d2-note-add">{t('common.add_note')}</button></div>
    </form>
  {/if}
  <ul>
    {#each [...notes].reverse() as n (n.id)}
      <li>
        <div class="hd"><strong>{who(n)}</strong><span class="d2-muted">{fmt.format(new Date(n.created))}</span>
          {#if canDelete}<button class="d2-btn ghost sm icon" aria-label={t('common.delete_note')} onclick={() => del(n)}><Icon name="trash" /></button>{/if}
        </div>
        {#if noteFormat(n) === 'markdown'}
          <div class="md">{@html renderNoteMarkdown(n.note)}</div>
        {:else}
          <p>{n.note}</p>
        {/if}
      </li>
    {:else}
      <li class="d2-muted none">{t('m.workspace.no_notes_yet')}</li>
    {/each}
  </ul>
</div>

<style>
  .notes {
    padding: var(--d2-s4);
    display: flex;
    flex-direction: column;
    gap: var(--d2-s4);
  }
  .act {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--d2-s2);
    margin-top: 6px;
  }
  .hint {
    font-size: var(--d2-fs-xs);
    margin-left: auto;
  }
  .act {
    flex-wrap: wrap;
  }
  .hidden {
    display: none;
  }
  .seg {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    border-radius: var(--d2-r1);
    background: var(--d2-surface-2);
  }
  .seg button {
    min-height: 24px;
    padding: 0 var(--d2-s2);
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--d2-text-2);
    font: inherit;
    font-size: var(--d2-fs-xs);
    cursor: pointer;
  }
  .seg button[aria-pressed='true'] {
    background: var(--d2-surface);
    color: var(--d2-text);
    font-weight: 600;
  }
  .preview {
    min-height: 4.5em;
    padding: var(--d2-s2) var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
  }
  /* rendered Markdown (renderer output, styled via :global because it comes from {@html}) */
  .md {
    overflow-wrap: anywhere;
  }
  .md :global(:is(h1, h2, h3, h4, h5, h6)) {
    font-size: 1em;
    font-weight: 600;
    margin: 0 0 4px;
  }
  .md :global(:is(p, ul, ol, blockquote, pre, table)) {
    margin: 0 0 6px;
  }
  .md > :global(:last-child) {
    margin-bottom: 0;
  }
  .md :global(pre) {
    padding: var(--d2-s2);
    overflow-x: auto;
    background: var(--d2-surface-2);
    border-radius: var(--d2-r1);
  }
  .md :global(blockquote) {
    padding-left: var(--d2-s3);
    border-left: 3px solid var(--d2-border);
    color: var(--d2-text-2);
  }
  .md :global(table) {
    display: block;
    overflow-x: auto;
    border-collapse: collapse;
  }
  .md :global(:is(th, td)) {
    padding: 2px 6px;
    border: 1px solid var(--d2-border);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
  }
  li {
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
  }
  .hd {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    margin-bottom: 4px;
    font-size: var(--d2-fs-sm);
  }
  .hd .d2-muted {
    flex: 1;
  }
  p {
    white-space: pre-wrap;
  }
  .none {
    border: 0;
    text-align: center;
  }
</style>
