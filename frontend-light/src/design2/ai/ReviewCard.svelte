<!-- One document of the AI review list: thumbnail, per field previous → new with confidence ("certain / probable / uncertain"),
     per field accept / change / discard, collectively "Accept certain", "Accept all", "Discard all" and
     undo. Permissions are checked by the server (change_document, object visibility, add_<model> for new objects). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { api } from '../../api/client'
  import { data } from '../../stores/data.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import { applyReview, rejectReview, undoReview, type Override, type Review, type ReviewChange } from './api'
  import { alreadyThere, applyBody, canUndo, certainty, CERTAINTY_KEY, FIELD_KEY, STATUS_KEY, TRIGGER_KEY, isOpen, newValue, oldValue, openIndexes, reviewIsOpen, type DocNow, type Names } from './review'

  let { review, doc, canEdit, onchange }: { review: Review; doc: DocNow | null; canEdit: boolean; onchange: (r: Review) => void } = $props()

  let busy = $state(false)
  let editing = $state<number | null>(null)
  let draft = $state<string>('')
  const edits = $state<Record<number, Override>>({})

  const names: Names = {
    tag: (id) => data.tags.get(id)?.name ?? null,
    correspondent: (id) => data.correspondents.get(id)?.name ?? null,
    documentType: (id) => data.documentTypes.get(id)?.name ?? null,
    storagePath: (id) => data.storagePaths.get(id)?.name ?? null,
    field: (id) => data.customFields.find((f) => f.id === id)?.name ?? null,
  }
  const fieldLabel = (c: ReviewChange) => (c.field === 'custom_fields' && c.existing_id !== null ? (names.field(c.existing_id) ?? t(FIELD_KEY.custom_fields)) : t(FIELD_KEY[c.field]))
  const open = $derived(openIndexes(review))
  const sureOpen = $derived(open.filter((i) => certainty(review.changes[i].confidence) === 'sure'))
  const title = $derived(doc?.title || `#${review.document}`)

  function objectOptions(c: ReviewChange): { id: number; name: string }[] {
    const m = c.field === 'tags' ? data.tags : c.field === 'correspondent' ? data.correspondents : c.field === 'document_type' ? data.documentTypes : c.field === 'storage_path' ? data.storagePaths : null
    if (!m) return []
    return [...m.values()].map((o: any) => ({ id: o.id as number, name: o.name as string })).sort((a, b) => a.name.localeCompare(b.name))
  }
  const isObject = (c: ReviewChange) => ['tags', 'correspondent', 'document_type', 'storage_path'].includes(c.field)

  function startEdit(i: number) {
    const c = review.changes[i]
    editing = i
    const e = edits[i]
    draft = isObject(c) ? String(e?.existing_id ?? c.existing_id ?? '') : String(e?.value ?? c.value ?? '')
  }
  function keepEdit(i: number) {
    const c = review.changes[i]
    if (isObject(c)) {
      const id = Number(draft)
      if (id) edits[i] = { existing_id: id }
    } else if (draft.trim()) edits[i] = { value: draft.trim() }
    editing = null
  }
  function shown(i: number) {
    const c = review.changes[i]
    const e = edits[i]
    if (!e) return newValue(c, names)
    if (e.existing_id !== undefined) return { text: newValue({ ...c, existing_id: e.existing_id, new_name: null }, names).text, isNew: false }
    return { text: String(e.value ?? ''), isNew: false }
  }

  async function act(fn: () => Promise<Review>, ok: string) {
    busy = true
    try {
      const r = await fn()
      for (const k of Object.keys(edits)) delete edits[Number(k)]
      editing = null
      onchange(r)
      toastService.show({ content: ok, delay: 4000, level: 'success' })
    } catch (e: any) {
      const body = e?.body
      const msgText = body?.detail ?? body?.overrides ?? body?.status ?? null
      toastService.showError(t('m.ai.could_not_save'), msgText ? { message: JSON.stringify(msgText) } : e)
    } finally {
      busy = false
    }
  }
  const take = (idx: number[]) => act(() => applyReview(review.id, applyBody(idx, edits)), t('m.ai.applied'))
  const dismiss = (idx: number[]) => act(() => applyReview(review.id, applyBody([], {}, idx)), t('m.ai.dismissed'))
  const rejectAll = () => act(() => rejectReview(review.id), t('m.ai.dismissed'))
  const undo = () => act(() => undoReview(review.id), t('m.ai.undone'))
</script>

<article class="card" aria-labelledby="d2-air-{review.id}">
  <a class="thumb" href={router.href('documents/' + review.document)} tabindex="-1" aria-hidden="true">
    <img src={api.thumbUrl(review.document)} alt="" loading="lazy" />
  </a>
  <div class="main">
    <header class="hd">
      <h2 id="d2-air-{review.id}"><a href={router.href('documents/' + review.document)}>{title}</a></h2>
      <span class="d2-muted small">{t(TRIGGER_KEY[review.trigger])} · {new Date(review.created).toLocaleString()}</span>
      {#if !reviewIsOpen(review)}<span class="d2-badge">{t(STATUS_KEY[review.status])}</span>{/if}
    </header>

    <table class="rows">
      <thead class="d2-sr">
        <tr><th>{t('m.ai.field')}</th><th>{t('m.ai.before')}</th><th>{t('m.ai.suggestion')}</th><th>{t('m.ai.certainty')}</th><th>{t('m.ai.actions')}</th></tr>
      </thead>
      <tbody>
        {#each review.changes as c, i (i)}
          {@const nv = shown(i)}
          {@const ov = oldValue(c, doc, names)}
          {@const cert = certainty(edits[i] ? 1 : c.confidence)}
          <tr class:done={!isOpen(c)}>
            <th scope="row" class="fld">{fieldLabel(c)}</th>
            <td class="old">
              {#if c.field === 'tags'}<span class="d2-muted">{t('m.ai.add_tag')}</span>{:else if ov}<span class="was">{ov}</span>{:else}<span class="d2-muted">{t('m.ai.empty')}</span>{/if}
            </td>
            <td class="new">
              {#if editing === i}
                {#if isObject(c)}
                  <select class="d2-select" bind:value={draft} aria-label={fieldLabel(c)}>
                    {#each objectOptions(c) as o (o.id)}<option value={String(o.id)}>{o.name}</option>{/each}
                  </select>
                {:else}
                  <input class="d2-input" bind:value={draft} aria-label={fieldLabel(c)} type={c.field === 'created' ? 'date' : 'text'} onkeydown={(e) => e.key === 'Enter' && keepEdit(i)} />
                {/if}
                <button class="d2-btn sm" onclick={() => keepEdit(i)}>{t('m.ai.keep')}</button>
              {:else}
                <span class="nv">{nv.text}</span>
                {#if nv.isNew}<span class="d2-badge accent">{t('m.ai.new_object')}</span>{/if}
                {#if edits[i]}<span class="d2-badge">{t('m.ai.changed')}</span>{/if}
                {#if isOpen(c) && alreadyThere(c, doc)}<span class="d2-muted small">{t('m.ai.already_set')}</span>{/if}
              {/if}
            </td>
            <td class="cert">
              {#if c.applied}<span class="st ok"><Icon name="check-lg" />{t('m.ai.status_applied')}</span>
              {:else if c.dismissed}<span class="st">{t('m.ai.status_dismissed')}</span>
              {:else}<span class="chip {cert}" title={c.why}>{t(CERTAINTY_KEY[cert])}</span>{/if}
            </td>
            <td class="act">
              {#if isOpen(c) && reviewIsOpen(review) && canEdit}
                <button class="d2-btn ghost sm icon" disabled={busy} onclick={() => take([i])} title={t('m.ai.apply')} aria-label="{t('m.ai.apply')}: {fieldLabel(c)}"><Icon name="check-lg" /></button>
                <button class="d2-btn ghost sm icon" disabled={busy} onclick={() => startEdit(i)} title={t('m.ai.change')} aria-label="{t('m.ai.change')}: {fieldLabel(c)}"><Icon name="pencil" /></button>
                <button class="d2-btn ghost sm icon" disabled={busy} onclick={() => dismiss([i])} title={t('m.ai.dismiss')} aria-label="{t('m.ai.dismiss')}: {fieldLabel(c)}"><Icon name="x-lg" /></button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>

    {#if canEdit}
      <footer class="ft">
        {#if reviewIsOpen(review) && open.length}
          {#if sureOpen.length && sureOpen.length < open.length}
            <button class="d2-btn primary sm" disabled={busy} onclick={() => take(sureOpen)}><Icon name="check2" />{t('m.ai.apply_sure', { PH: sureOpen.length })}</button>
          {/if}
          <button class="d2-btn sm" class:primary={!sureOpen.length || sureOpen.length === open.length} disabled={busy} onclick={() => take(open)}><Icon name="check-lg" />{t('m.ai.apply_all')}</button>
          <button class="d2-btn ghost sm" disabled={busy} onclick={rejectAll}><Icon name="x-lg" />{t('m.ai.dismiss_all')}</button>
        {/if}
        {#if canUndo(review)}
          <button class="d2-btn ghost sm" disabled={busy} onclick={undo}><Icon name="arrow-counterclockwise" />{t('m.ai.undo')}</button>
        {/if}
      </footer>
    {/if}
  </div>
</article>

<style>
  .card {
    display: grid;
    grid-template-columns: 96px minmax(0, 1fr);
    gap: var(--d2-s3);
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  .thumb img {
    width: 96px;
    height: 128px;
    object-fit: cover;
    object-position: top;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
    background: var(--d2-surface-2);
  }
  .main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
  }
  .hd {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--d2-s2);
  }
  h2 {
    font-size: var(--d2-fs-lg);
  }
  h2 a {
    color: var(--d2-text);
    text-decoration: none;
  }
  h2 a:hover {
    text-decoration: underline;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
  .rows {
    width: 100%;
    border-collapse: collapse;
  }
  .rows td,
  .rows th {
    padding: 4px var(--d2-s2) 4px 0;
    border-top: 1px solid var(--d2-border);
    text-align: left;
    vertical-align: middle;
  }
  .fld {
    width: 22%;
    color: var(--d2-text-2);
    font-weight: 500;
  }
  .old {
    width: 26%;
  }
  .was {
    color: var(--d2-text-2);
    text-decoration: line-through;
    text-decoration-color: var(--d2-text-3);
  }
  .new {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--d2-s2);
  }
  .nv {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .new .d2-input,
  .new .d2-select {
    max-width: 260px;
  }
  .cert {
    width: 1%;
    white-space: nowrap;
  }
  .act {
    width: 1%;
    white-space: nowrap;
  }
  .chip {
    display: inline-block;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: var(--d2-fs-xs);
    font-weight: 600;
  }
  .chip.sure {
    background: var(--d2-ok-soft);
    color: var(--d2-ok);
  }
  .chip.likely {
    background: var(--d2-info-soft);
    color: var(--d2-info);
  }
  .chip.unsure {
    background: var(--d2-warn-soft);
    color: var(--d2-warn);
  }
  .st {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--d2-fs-xs);
    color: var(--d2-text-3);
  }
  .st.ok {
    color: var(--d2-ok);
  }
  tr.done .nv {
    font-weight: 400;
    color: var(--d2-text-2);
  }
  .ft {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s2);
  }
  @media (max-width: 640px) {
    .card {
      grid-template-columns: minmax(0, 1fr);
    }
    .thumb {
      display: none;
    }
    .old {
      display: none;
    }
  }
</style>
