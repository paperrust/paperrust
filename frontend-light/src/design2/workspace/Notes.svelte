<!-- Notes of a document (v1 /api/documents/{id}/notes/). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { Document, DocumentNote } from '../../api/v1'
  import { data } from '../../stores/data.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'

  let { doc, onchange }: { doc: Document; onchange: (notes: DocumentNote[]) => void } = $props()
  let text = $state('')
  let busy = $state(false)
  const notes = $derived((doc.notes ?? []) as DocumentNote[])
  const canAdd = $derived(session.can('add', 'note'))
  const canDelete = $derived(session.can('delete', 'note'))
  const fmt = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })
  const who = (n: DocumentNote) => (typeof n.user === 'object' && n.user ? [n.user.first_name, n.user.last_name].filter(Boolean).join(' ') || n.user.username : data.username((n.user as number) ?? null))

  async function add() {
    if (!text.trim()) return
    busy = true
    try {
      onchange(await api.addNote(doc.id, text.trim()))
      text = ''
    } catch (e) {
      toastService.showError(t('m.workspace.error_saving_note'), e)
    } finally {
      busy = false
    }
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
      <textarea id="d2-note-new" class="d2-textarea" bind:value={text} rows="3" onkeydown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && add()}></textarea>
      <div class="act"><span class="d2-muted hint">Ctrl+Enter</span><button class="d2-btn primary sm" disabled={busy || !text.trim()} id="d2-note-add">{t('common.add_note')}</button></div>
    </form>
  {/if}
  <ul>
    {#each [...notes].reverse() as n (n.id)}
      <li>
        <div class="hd"><strong>{who(n)}</strong><span class="d2-muted">{fmt.format(new Date(n.created))}</span>
          {#if canDelete}<button class="d2-btn ghost sm icon" aria-label={t('common.delete_note')} onclick={() => del(n)}><Icon name="trash" /></button>{/if}
        </div>
        <p>{n.note}</p>
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
