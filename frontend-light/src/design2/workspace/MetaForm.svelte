<!-- Edit metadata directly with autosave (docs/DESIGN2.md §4.3): every change is saved debounced
     (changed fields only) – via API v2 with If-Match if UUID/rev of the document are determined, otherwise v1 PATCH.
     Status visible; errors with "Retry", conflict (412) with "Reload". -->
<script lang="ts">
  import { onDestroy } from 'svelte'
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { Document } from '../../api/v1'
  import { data } from '../../stores/data.svelte'
  import { session } from '../../stores/settings.svelte'
  import { Autosaver, type SaveState } from '../lib/autosave'
  import { ws } from '../stores/workspace.svelte'
  import { Conflict, idMaps, patchV2, resolveDoc, toV2Patch, type IdMaps, type V2Ref } from '../api/v2docs'
  import ObjectPicker from '../components/ObjectPicker.svelte'
  import { openCreateObject } from './pdf-actions'
  import Icon from '../components/Icon.svelte'
  import Visibility from '../components/Visibility.svelte'
  import { canChangeVisibility, type VisibilityValue } from '../lib/visibility'

  let { doc, onsaved, onreload }: { doc: Document; onsaved: (d: Document) => void; onreload?: () => void } = $props()

  // v2 with If-Match (fallback v1): determine UUID/rev once per document
  let v2ref: V2Ref | null = null
  let v2maps: IdMaps | null = null
  let via = $state<'v1' | 'v2' | ''>('')
  let conflict = $state(false)

  let saveState = $state<SaveState>('idle')
  let errorText = $state('')
  const readonly = $derived(doc.user_can_change === false || !session.can('change', 'document'))
  // local values (immediately visible, independent of the server state while saving)
  let title = $state('')
  let created = $state('')
  let asn = $state('')
  let cfs = $state<{ field: number; value: unknown }[]>([])
  let docId = 0
  $effect.pre(() => {
    if (doc.id !== docId) {
      docId = doc.id
      title = doc.title
      created = (doc.created ?? '').slice(0, 10)
      asn = doc.archive_serial_number != null ? String(doc.archive_serial_number) : ''
      cfs = (doc.custom_fields ?? []).map((c) => ({ ...c }))
      v2ref = null
      const my = doc.id
      Promise.all([resolveDoc(doc), idMaps()]).then(([r, m]) => {
        if (my !== docId) return
        v2ref = r
        v2maps = m
      })
    }
  })

  async function save(patch: Record<string, unknown>): Promise<Document> {
    const vp = v2ref && v2maps ? toV2Patch(patch, v2maps) : null
    if (v2ref && vp) {
      v2ref = { ...v2ref, rev: await patchV2(v2ref, vp) }
      via = 'v2'
      return api.getDocument(docId)
    }
    via = 'v1'
    const d = await api.patchDocument({ id: docId, ...patch })
    // v1 has increased the rev → determine again for later v2 writes
    if (v2ref) v2ref = await resolveDoc(d)
    return d
  }

  const saver = new Autosaver<Document>({
    save,
    onState: (s, e: any) => {
      saveState = s
      conflict = s === 'error' && e instanceof Conflict
      errorText = s !== 'error' ? '' : conflict ? t('m.workspace.document_changed_elsewhere_in') : describe(e)
    },
    onSaved: (d) => {
      if (d?.id === docId) onsaved(d)
      ws.patchLocal(d)
    },
  })
  function describe(e: any): string {
    const b = e?.body
    if (b && typeof b === 'object') return Object.entries(b).map(([k, v]) => `${k}: ${Array.isArray(v) ? v.join(' ') : typeof v === 'object' ? JSON.stringify(v) : v}`).join('; ')
    return e?.message ?? String(e)
  }
  // Document switch/close: send open changes immediately
  let prevId = 0
  $effect(() => {
    const id = doc.id
    if (prevId && prevId !== id) saver.flush()
    prevId = id
  })
  onDestroy(() => {
    saver.flush()
  })
  export const flush = () => saver.flush()

  const opts = (m: Map<number, { id: number; name: string; color?: string }>) => [...m.values()].map((o) => ({ id: o.id, name: o.name, color: o.color })).sort((a, b) => a.name.localeCompare(b.name))
  const one = (v: number | null) => (v === null || v === undefined ? [] : [v])

  function setCf(i: number, value: unknown, immediate = false) {
    cfs = cfs.map((c, j) => (j === i ? { ...c, value } : c))
    saver.change('custom_fields', cfs.map((c) => ({ field: c.field, value: c.value })), immediate)
  }
  function addCf(id: number | null) {
    if (id === null || cfs.some((c) => c.field === id)) return
    cfs = [...cfs, { field: id, value: null }]
    saver.change('custom_fields', cfs.map((c) => ({ field: c.field, value: c.value })), true)
  }
  function removeCf(i: number) {
    cfs = cfs.filter((_, j) => j !== i)
    saver.change('custom_fields', cfs.map((c) => ({ field: c.field, value: c.value })), true)
  }
  const cfDef = (id: number) => data.customFields.find((f) => f.id === id)
  // "Visible to" (DESIGN2-ENTWIRREN §3): owner + set_permissions, only superuser/owner/without owner (upstream)
  const visEditable = $derived(canChangeVisibility(doc.owner, session.data?.user))
  function setVisibility(v: VisibilityValue) {
    onsaved({ ...doc, owner: v.owner, permissions: v.set_permissions })
    saver.change('owner', v.owner, true)
    saver.change('set_permissions', v.set_permissions, true)
  }
  const status = $derived(
    saveState === 'saving' || saveState === 'pending' ? t('m.workspace.saving') : saveState === 'saved' ? t('m.workspace.saved') : saveState === 'error' ? t('m.workspace.not_saved') : '',
  )
</script>

<form class="meta" onsubmit={(e) => (e.preventDefault(), saver.flush())} aria-describedby="d2-save-state" id="d2-meta">
  <div class="status" id="d2-save-state" role="status" aria-live="polite" data-state={saveState} data-via={via}>
    {#if saveState === 'saved'}<Icon name="check-circle" />{:else if saveState === 'error'}<Icon name="exclamation-triangle" />{:else if saveState === 'saving' || saveState === 'pending'}<span class="spin" aria-hidden="true"></span>{/if}
    <span>{status}</span>
    {#if saveState === 'error'}
      <span class="errtxt">{errorText}</span>
      {#if conflict && onreload}
        <button type="button" class="d2-btn sm" id="d2-save-reload" onclick={() => { saver.reset(); onreload?.() }}>{t('common.reload')}</button>
      {:else}
        <button type="button" class="d2-btn sm" onclick={() => saver.retry()}>{t('common.retry')}</button>
      {/if}
    {/if}
    {#if readonly}<span class="d2-muted"><Icon name="shield-check" /> {t('m.workspace.read_only')}</span>{/if}
  </div>

  <label class="f">
    <span class="d2-label">{t('common.title')}</span>
    <input class="d2-input" id="d2-field-title" value={title} disabled={readonly} required
      oninput={(e) => { title = e.currentTarget.value; if (title.trim()) saver.change('title', title) }}
      onblur={() => saver.flush()} />
  </label>
  <div class="two">
    <label class="f">
      <span class="d2-label">{t('common.date_created')}</span>
      <input class="d2-input" type="date" id="d2-field-created" value={created} disabled={readonly}
        onchange={(e) => { created = e.currentTarget.value; if (created) saver.change('created', created, true) }} />
    </label>
    <label class="f">
      <span class="d2-label">{t('common.archive_serial_number')}</span>
      <input class="d2-input" type="number" min="0" id="d2-field-asn" value={asn} disabled={readonly}
        oninput={(e) => { asn = e.currentTarget.value; saver.change('archive_serial_number', asn === '' ? null : +asn) }}
        onblur={() => saver.flush()} />
    </label>
  </div>
  <div class="f">
    <span class="d2-label" id="d2-l-corr">{t('common.correspondent')}</span>
    <ObjectPicker label={t('common.correspondent')} id="d2-field-correspondent" options={opts(data.correspondents)} selected={one(doc.correspondent)} placeholder={t('common.not_assigned')} disabled={readonly}
      oncreate={readonly || !session.can('add', 'correspondent') ? undefined : (n) => openCreateObject('correspondent', n, (v) => { onsaved({ ...doc, correspondent: v }); saver.change('correspondent', v, true) })}
      onchange={(n) => { const v = (n[0] as number | undefined) ?? null; onsaved({ ...doc, correspondent: v }); saver.change('correspondent', v, true) }} />
  </div>
  <div class="f">
    <span class="d2-label">{t('common.document_type')}</span>
    <ObjectPicker label={t('common.document_type')} id="d2-field-document_type" options={opts(data.documentTypes)} selected={one(doc.document_type)} placeholder={t('common.not_assigned')} disabled={readonly}
      oncreate={readonly || !session.can('add', 'documenttype') ? undefined : (n) => openCreateObject('document_type', n, (v) => { onsaved({ ...doc, document_type: v }); saver.change('document_type', v, true) })}
      onchange={(n) => { const v = (n[0] as number | undefined) ?? null; onsaved({ ...doc, document_type: v }); saver.change('document_type', v, true) }} />
  </div>
  <div class="f">
    <span class="d2-label">{t('common.storage_path')}</span>
    <ObjectPicker label={t('common.storage_path')} id="d2-field-storage_path" options={opts(data.storagePaths)} selected={one(doc.storage_path)} placeholder={t('common.default')} disabled={readonly}
      oncreate={readonly || !session.can('add', 'storagepath') ? undefined : (n) => openCreateObject('storage_path', n, (v) => { onsaved({ ...doc, storage_path: v }); saver.change('storage_path', v, true) })}
      onchange={(n) => { const v = (n[0] as number | undefined) ?? null; onsaved({ ...doc, storage_path: v }); saver.change('storage_path', v, true) }} />
  </div>
  <div class="f">
    <span class="d2-label">{t('common.tags')}</span>
    <ObjectPicker label={t('common.tags')} id="d2-field-tags" multi options={opts(data.tags)} selected={doc.tags} placeholder={t('m.workspace.no_tags')} disabled={readonly}
      oncreate={readonly || !session.can('add', 'tag') ? undefined : (n) => openCreateObject('tag', n, (v) => { const tags = [...doc.tags.filter((x) => x !== v), v]; onsaved({ ...doc, tags }); saver.change('tags', tags, true) })}
      onchange={(n) => { const v = n.filter((x): x is number => x !== null); onsaved({ ...doc, tags: v }); saver.change('tags', v, true) }} />
    {#if doc.tags.length}
      <div class="tagrow">
        {#each doc.tags as id (id)}
          {@const tag = data.tags.get(id)}
          <span class="d2-chip" style:background={tag?.color} style:color={tag?.text_color}>{tag?.name ?? '#' + id}
            {#if !readonly}<button type="button" class="x" aria-label={t('common.remove', { PH: tag?.name ?? id })} onclick={() => { const v = doc.tags.filter((x) => x !== id); onsaved({ ...doc, tags: v }); saver.change('tags', v, true) }}><Icon name="x-lg" /></button>{/if}
          </span>
        {/each}
      </div>
    {/if}
  </div>

  <div class="f">
    <span class="d2-label" id="d2-l-visibility">{t('m.visibility.label')}</span>
    <Visibility id="d2-field-visibility" owner={doc.owner} permissions={doc.permissions} disabled={readonly || !visEditable} onchange={setVisibility} />
    {#if !readonly && !visEditable}<span class="d2-muted vis-ro">{t('m.visibility.owner_only')}</span>{/if}
  </div>

  {#if data.customFields.length}
    <h3 class="sec">{t('common.custom_fields')}</h3>
    {#each cfs as c, i (c.field)}
      {@const def = cfDef(c.field)}
      <div class="f cf">
        <span class="d2-label">{def?.name ?? '#' + c.field}</span>
        <div class="cfrow">
          {#if def?.data_type === 'boolean'}
            <label class="d2-check"><input type="checkbox" checked={!!c.value} disabled={readonly} onchange={(e) => setCf(i, e.currentTarget.checked, true)} />{c.value ? t('common.yes') : t('common.no')}</label>
          {:else if def?.data_type === 'date'}
            <input class="d2-input" type="date" value={(c.value as string) ?? ''} disabled={readonly} aria-label={def.name} onchange={(e) => setCf(i, e.currentTarget.value || null, true)} />
          {:else if def?.data_type === 'integer' || def?.data_type === 'float'}
            <input class="d2-input" type="number" step={def.data_type === 'float' ? 'any' : '1'} value={(c.value as number) ?? ''} disabled={readonly} aria-label={def.name} oninput={(e) => setCf(i, e.currentTarget.value === '' ? null : +e.currentTarget.value)} onblur={() => saver.flush()} />
          {:else if def?.data_type === 'select'}
            <select class="d2-select" value={(c.value as string) ?? ''} disabled={readonly} aria-label={def.name} onchange={(e) => setCf(i, e.currentTarget.value || null, true)}>
              <option value="">–</option>
              {#each def.extra_data?.select_options ?? [] as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
            </select>
          {:else if def?.data_type === 'documentlink'}
            <span class="d2-muted">{Array.isArray(c.value) ? (c.value as number[]).map((x) => '#' + x).join(', ') : '–'}</span>
          {:else if def?.data_type === 'longtext'}
            <textarea class="d2-textarea" value={(c.value as string) ?? ''} disabled={readonly} aria-label={def.name} oninput={(e) => setCf(i, e.currentTarget.value)} onblur={() => saver.flush()}></textarea>
          {:else}
            <input class="d2-input" type={def?.data_type === 'url' ? 'url' : 'text'} value={(c.value as string) ?? ''} disabled={readonly} aria-label={def?.name ?? ''} placeholder={def?.data_type === 'monetary' ? 'EUR0.00' : ''} oninput={(e) => setCf(i, e.currentTarget.value)} onblur={() => saver.flush()} />
          {/if}
          {#if !readonly}<button type="button" class="d2-btn ghost sm icon" aria-label={t('common.remove', { PH: def?.name ?? '' })} onclick={() => removeCf(i)}><Icon name="x-lg" /></button>{/if}
        </div>
      </div>
    {/each}
    {#if !readonly}
      <ObjectPicker label={t('common.add_field')} variant="chip" icon="plus-lg" options={data.customFields.filter((f) => !cfs.some((c) => c.field === f.id)).map((f) => ({ id: f.id, name: f.name }))} selected={[]} onchange={(_, id) => addCf(id)} />
    {/if}
  {/if}
</form>

<style>
  .meta {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
    padding: var(--d2-s4);
  }
  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-height: 22px;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-sm);
  }
  .status[data-state='saved'] {
    color: var(--d2-ok);
  }
  .status[data-state='error'] {
    color: var(--d2-danger);
  }
  .errtxt {
    flex-basis: 100%;
    font-size: var(--d2-fs-xs);
  }
  .spin {
    width: 12px;
    height: 12px;
    border: 2px solid var(--d2-border-strong);
    border-top-color: var(--d2-accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .f {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--d2-s3);
  }
  .tagrow {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }
  .x {
    display: inline-flex;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font-size: 9px;
    cursor: pointer;
  }
  .sec {
    margin-top: var(--d2-s2);
    font-size: var(--d2-fs-md);
  }
  .vis-ro {
    font-size: var(--d2-fs-xs);
  }
  .cfrow {
    display: flex;
    align-items: center;
    gap: 4px;
  }
</style>
