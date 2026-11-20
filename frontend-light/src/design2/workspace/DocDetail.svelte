<!-- Right pane (concept A): preview + metadata with tabs Details/Notes/History/Content.
     Wide: preview and metadata side by side; narrow: preview as its own tab. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { Document } from '../../api/v1'
  import { APP_BASE } from '../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { ws } from '../stores/workspace.svelte'
  import { ui } from '../stores/ui.svelte'
  import { onCommand } from '../lib/bus'
  import Icon from '../components/Icon.svelte'
  import { SYMBOL } from '../lib/icons'
  import SideSheet from '../components/SideSheet.svelte'
  import Preview from './Preview.svelte'
  import MetaForm from './MetaForm.svelte'
  import Notes from './Notes.svelte'
  import History from './History.svelte'
  import RunWorkflowSheet from './RunWorkflowSheet.svelte'
  import AskDocument from '../assistant/AskDocument.svelte'
  import type { ManualWorkflow } from '../../api/v1'
  import { loadManualWorkflows } from '../../lib/manual-workflows'
  import { data } from '../../stores/data.svelte'
  import { counts } from '../stores/counts.svelte'
  import Menu from '../components/Menu.svelte'
  import * as pdf from './pdf-actions'

  let { id }: { id: number } = $props()
  let doc = $state<Document | null>(null)
  // "Done": remove inbox tags (did not exist in the PaperRust design; feedback 2026-11-17)
  const inboxTagIds = $derived(doc ? (doc.tags ?? []).filter((tid) => data.tags.get(tid)?.is_inbox_tag) : [])
  async function markDone() {
    if (!doc || !inboxTagIds.length) return
    const tags = (doc.tags ?? []).filter((tid) => !inboxTagIds.includes(tid))
    try {
      const d = doc
      await api.request('PATCH', `/api/documents/${d.id}/`, { tags })
      doc = { ...d, tags }
      if (ws.ctx.kind === 'inbox') { ws.removeLocal([d.id]); ws.open(null) } else ws.patchLocal({ id: d.id, tags })
      counts.refresh()
      toastService.showSuccess(t('m.workspace.removed_from_inbox'))
    } catch (e) {
      toastService.showError(t('m.workspace.remove_from_inbox_failed'), e)
    }
  }
  let error = $state('')
  let width = $state(1000)
  let tab = $state<'preview' | 'details' | 'notes' | 'history' | 'content'>('details')
  let confirmDelete = $state(false)
  const wide = $derived(width >= 760 && !ui.mobile)
  const notesOn = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED) && session.can('view', 'note'))
  const historyOn = $derived(session.get(SETTINGS_KEYS.AUDITLOG_ENABLED) && session.can('view', 'auditlog'))

  // Manual workflow run: only owner with change permission (like "Reprocess")
  let manualWorkflows = $state<ManualWorkflow[]>([])
  let runSheet = $state(false)
  $effect(() => {
    void loadManualWorkflows(session.can('view', 'workflow')).then((w) => (manualWorkflows = w))
  })
  const canRunWorkflow = $derived(!!doc && session.can('change', 'document') && doc.user_can_change !== false && session.ownsObject(doc))
  async function runWorkflow(workflowId: number, force: boolean) {
    const d = doc
    if (!d) return
    try {
      await api.runWorkflow([d.id], workflowId, !force)
      runSheet = false
      toastService.showInfo(t('common.workflow_run_in_background', { PH: d.title }))
    } catch (e) {
      toastService.showError(t('common.error_executing_operation'), e)
    }
  }

  // Actions menu: PDF tools and further actions like in the classic UI (pdf-actions.ts)
  let actionsOpen = $state(false)
  const canEditOwn = $derived(!!doc && session.can('change', 'document') && doc.user_can_change !== false && session.ownsObject(doc))
  const emailOn = $derived(!!session.get(SETTINGS_KEYS.EMAIL_ENABLED))
  function act(fn: () => unknown) {
    actionsOpen = false
    void fn()
  }
  function afterPdfEdit(deletedOriginal: boolean) {
    if (deletedOriginal) {
      ws.removeLocal([id])
      ws.open(null, true)
    } else reloadKey++
  }

  let reloadKey = $state(0)
  $effect(() => {
    const my = id
    void reloadKey
    doc = null
    error = ''
    api
      .getDocument(my)
      .then((d) => my === id && (doc = d))
      .catch((e) => my === id && (error = e?.status === 404 ? t('m.workspace.document_not_found_no') : String(e?.message ?? e)))
  })
  $effect(() => {
    if (wide && tab === 'preview') tab = 'details'
  })
  $effect(() => {
    if (ui.mobile && tab === 'details' && !doc) tab = 'preview'
  })

  const tabs = $derived(
    [
      !wide && { k: 'preview', l: t('common.preview') },
      { k: 'details', l: t('common.details') },
      notesOn && { k: 'notes', l: t('common.notes') + (doc?.notes?.length ? ` (${doc.notes.length})` : '') },
      historyOn && { k: 'history', l: t('common.history') },
      { k: 'content', l: t('common.content') },
    ].filter(Boolean) as { k: typeof tab; l: string }[],
  )

  async function del() {
    confirmDelete = false
    try {
      await api.deleteDocument(id)
      toastService.showSuccess(t('m.workspace.document_moved_trash'))
      const idx = ws.docs.findIndex((d) => d.id === id)
      ws.removeLocal([id])
      const next = ws.docs[Math.min(idx, ws.docs.length - 1)]
      ws.open(next?.id ?? null, true)
    } catch (e) {
      toastService.showError(t('common.error_deleting_document'), e)
    }
  }

  $effect(() =>
    onCommand((c) => {
      if (c === 'edit-title') {
        tab = 'details'
        setTimeout(() => (document.getElementById('d2-field-title') as HTMLInputElement | null)?.select(), 0)
        return true
      }
      if (c === 'escape' && ui.mobile) {
        ws.open(null)
        return true
      }
      return false
    }),
  )
</script>

<section class="detail" bind:clientWidth={width} aria-label={t('m.workspace.document')} id="d2-detail">
  <header class="d2-pane-head">
    {#if ui.mobile}<button class="d2-btn ghost icon" aria-label={t('m.workspace.back_list')} onclick={() => ws.open(null)}><Icon name="arrow-left" /></button>{/if}
    <h2 title={doc?.title}>{doc?.title ?? (error ? t('common.error') : t('common.loading'))}</h2>
    {#if doc}
      <span class="grow"></span>
      {#if manualWorkflows.length > 0}
        <button class="d2-btn ghost sm icon" aria-label={t('common.run_workflow_2')} title={t('common.run_workflow_2')} onclick={() => (runSheet = true)} disabled={!canRunWorkflow} id="d2-doc-run-workflow"><Icon name={SYMBOL.workflow} /></button>
      {/if}
      {#if inboxTagIds.length && doc.user_can_change !== false}
        <button class="d2-btn sm" onclick={markDone} id="d2-doc-done" title={t('m.workspace.remove_from_inbox_hint')}><Icon name="check-circle" />{t('m.workspace.done_inbox')}</button>
      {/if}
      <span class="menuwrap">
        <button class="d2-btn ghost sm icon" aria-label={t('common.actions')} title={t('common.actions')} aria-haspopup="menu" aria-expanded={actionsOpen} onclick={() => (actionsOpen = !actionsOpen)} id="d2-doc-actions"><Icon name="three-dots" /></button>
        {#if actionsOpen}
          {@const d = doc}
          <Menu label={t('common.actions')} onclose={() => (actionsOpen = false)} width="15rem">
            {#if pdf.isPdf(d)}
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-pdf-editor" disabled={!canEditOwn} onclick={() => act(() => pdf.openPdfEditor(d, afterPdfEdit))}><Icon name="file-earmark-break" />{t('common.pdf_editor')}</button>
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-redact" disabled={!canEditOwn} onclick={() => act(() => pdf.openRedact(d))}><Icon name="eraser" />{t('pdftools.redact_menu')}</button>
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-shrink" disabled={!canEditOwn} onclick={() => act(() => pdf.openShrink(d))}><Icon name="arrows-angle-contract" />{t('pdftools.shrink_menu')}</button>
            {/if}
            {#if pdf.canConvert(d)}
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-convert" disabled={!canEditOwn} onclick={() => act(() => pdf.openConvertPdf([d], () => reloadKey++))}><Icon name="file-earmark-pdf" />{t('convert.menu')}</button>
            {/if}
            <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-reprocess" disabled={!canEditOwn} onclick={() => act(() => pdf.openReprocess(d))}><Icon name="arrow-counterclockwise" />{t('common.reprocess')}</button>
            <div class="d2-menu-sep" role="separator"></div>
            <a role="menuitem" class="d2-menu-item" id="d2-act-download-original" href={api.downloadUrlFor(d.id, true)} onclick={() => (actionsOpen = false)}><Icon name="download" />{t('m.workspace.download_original')}</a>
            {#if !ui.mobile}
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-print" onclick={() => act(() => pdf.printDocument(d))}><Icon name="printer" />{t('detail.print')}</button>
            {/if}
            {#if emailOn}
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-email" onclick={() => act(() => pdf.openEmail([d.id], !!d.archived_file_name))}><Icon name="envelope" />{t('common.email')}</button>
            {/if}
            {#if session.can('add', 'sharelink')}
              <button type="button" role="menuitem" class="d2-menu-item" id="d2-act-share" onclick={() => act(() => pdf.openShareLinks(d))}><Icon name="link-45deg" />{t('common.share_links')}</button>
            {/if}
          </Menu>
        {/if}
      </span>
      <a class="d2-btn ghost sm icon" href={api.downloadUrl(id)} aria-label={t('common.download')} title={t('common.download')}><Icon name="download" /></a>
      <a class="d2-btn ghost sm icon" href={`${APP_BASE}/documents/${id}/details`} aria-label={t('common.open_in_classic_interface')} title={t('common.open_in_classic_interface')}><Icon name="box-arrow-up-right" /></a>
      {#if session.can('delete', 'document') && doc.user_can_change !== false}
        <button class="d2-btn ghost sm icon danger" aria-label={t('common.delete')} title={t('common.delete')} onclick={() => (confirmDelete = true)} id="d2-doc-delete"><Icon name="trash" /></button>
      {/if}
      {#if !ui.mobile}<button class="d2-btn ghost sm icon" aria-label={t('common.close')} title={t('common.close')} onclick={() => ws.open(null)}><Icon name="x-lg" /></button>{/if}
    {/if}
  </header>
  {#if error}
    <div class="d2-empty"><Icon name="exclamation-triangle" size="2rem" /><p>{error}</p></div>
  {:else if doc}
    <div class="body" class:wide>
      {#if wide}
        <div class="pvcol"><Preview {doc} /></div>
      {/if}
      <div class="metacol">
        <div class="tabs" role="tablist" aria-label={t('m.workspace.document')}>
          {#each tabs as x (x.k)}
            <button role="tab" id="d2-tab-{x.k}" aria-selected={tab === x.k} aria-controls="d2-tabpanel" tabindex={tab === x.k ? 0 : -1} onclick={() => (tab = x.k)}
              onkeydown={(e) => {
                const i = tabs.findIndex((y) => y.k === tab)
                if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                  const n = tabs[(i + (e.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length]
                  tab = n.k
                  document.getElementById('d2-tab-' + n.k)?.focus()
                  e.preventDefault()
                }
              }}>{x.l}</button>
          {/each}
        </div>
        <div class="panel" class:d2-scroll={tab !== 'preview'} role="tabpanel" id="d2-tabpanel" aria-labelledby="d2-tab-{tab}">
          {#if tab === 'preview'}
            <Preview {doc} />
          {:else if tab === 'details'}
            {#if doc}<AskDocument {doc} />{/if}
            {#key reloadKey}<MetaForm {doc} onsaved={(d) => (doc = { ...doc!, ...d })} onreload={() => reloadKey++} />{/key}
            <dl class="facts">
              <dt>{t('common.added')}</dt><dd>{new Date(doc.added).toLocaleString()}</dd>
              {#if doc.modified}<dt>{t('common.modified')}</dt><dd>{new Date(doc.modified).toLocaleString()}</dd>{/if}
              {#if doc.original_file_name}<dt>{t('common.original_filename')}</dt><dd class="br">{doc.original_file_name}</dd>{/if}
              {#if doc.page_count}<dt>{t('common.pages')}</dt><dd>{doc.page_count}</dd>{/if}
              {#if doc.mime_type}<dt>{t('m.workspace.mime_type')}</dt><dd>{doc.mime_type}</dd>{/if}
            </dl>
          {:else if tab === 'notes'}
            <Notes {doc} onchange={(n) => { doc = { ...doc!, notes: n }; ws.patchLocal({ id: doc.id, notes: n }) }} />
          {:else if tab === 'history'}
            <History {id} />
          {:else}
            <pre class="content">{doc.content || t('m.workspace.no_text_content')}</pre>
          {/if}
        </div>
      </div>
    </div>
  {:else}
    <div class="d2-empty" aria-busy="true">{t('common.loading')}</div>
  {/if}
</section>

{#if runSheet && doc}
  <RunWorkflowSheet message={t('common.run_workflow', { PH: doc.title })} workflows={manualWorkflows} forceDefault singleDocument onclose={() => (runSheet = false)} onrun={runWorkflow} />
{/if}

{#if confirmDelete}
  <SideSheet title={t('m.workspace.delete_document')} onclose={() => (confirmDelete = false)} width="420px">
    <p>{t('m.workspace.move_trash', { PH: doc?.title ?? '' })}</p>
    {#snippet footer()}
      <button class="d2-btn" onclick={() => (confirmDelete = false)}>{t('common.cancel')}</button>
      <button class="d2-btn primary" onclick={del} id="d2-doc-delete-confirm">{t('common.delete')}</button>
    {/snippet}
  </SideSheet>
{/if}

<style>
  .menuwrap {
    position: relative;
    display: inline-flex;
  }
  .detail {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--d2-surface);
  }
  .grow {
    flex: 1;
  }
  .d2-pane-head h2 {
    flex: 0 1 auto;
    min-width: 0;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .body.wide {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(300px, var(--d2-meta-w));
  }
  .pvcol {
    min-height: 0;
    border-right: 1px solid var(--d2-border);
  }
  .metacol {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }
  .tabs {
    display: flex;
    gap: 2px;
    flex: none;
    padding: 0 var(--d2-s3);
    border-bottom: 1px solid var(--d2-border);
    overflow-x: auto;
  }
  [role='tab'] {
    position: relative;
    height: 40px;
    padding: 0 var(--d2-s3);
    border: 0;
    background: none;
    color: var(--d2-text-2);
    font-weight: 500;
    white-space: nowrap;
    cursor: pointer;
  }
  [role='tab'][aria-selected='true'] {
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  [role='tab'][aria-selected='true']::after {
    content: '';
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: -1px;
    height: 2px;
    border-radius: 2px;
    background: var(--d2-accent);
  }
  .panel {
    flex: 1;
    min-height: 0;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px var(--d2-s3);
    margin: 0 var(--d2-s4) var(--d2-s4);
    padding-top: var(--d2-s3);
    border-top: 1px solid var(--d2-border);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
  }
  dd {
    margin: 0;
    color: var(--d2-text);
  }
  .br {
    word-break: break-all;
  }
  .content {
    margin: 0;
    padding: var(--d2-s4);
    white-space: pre-wrap;
    font-family: var(--d2-font);
    font-size: var(--d2-fs-sm);
  }
</style>
