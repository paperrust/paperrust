<!-- Bulk actions for the selection (v1 bulk_edit / bulk_download). Delete with confirmation in the side sheet.
     "Run workflow": active workflows with trigger "Manual" (bulk_edit run_workflow, runs in the background). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { data } from '../../stores/data.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { router } from '../../router.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { ws } from '../stores/workspace.svelte'
  import { bulkDownload, bulkEdit, type BulkMethod } from '../api/docs'
  import ObjectPicker from '../components/ObjectPicker.svelte'
  import Icon from '../components/Icon.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import RunWorkflowSheet from './RunWorkflowSheet.svelte'
  import { api } from '../../api/client'
  import type { ManualWorkflow } from '../../api/v1'
  import { canRunManualWorkflow, loadManualWorkflows } from '../../lib/manual-workflows'
  import Visibility from '../components/Visibility.svelte'
  import Menu from '../components/Menu.svelte'
  import { SYMBOL } from '../lib/icons'
  import * as pdf from './pdf-actions'
  import { bulkParameters, canBulkChangeVisibility, type VisibilityMode, type VisibilityValue } from '../lib/visibility'
  import { folders } from '../stores/folders.svelte'
  import { folderOptions } from '../api/folders'

  let busy = $state(false)
  let confirmDelete = $state(false)
  const n = $derived(ws.selected.length)
  const opts = (m: Map<number, { id: number; name: string; color?: string }>) => [...m.values()].map((o) => ({ id: o.id, name: o.name, color: o.color })).sort((a, b) => a.name.localeCompare(b.name))
  const canEdit = $derived(session.can('change', 'document'))
  let manualWorkflows = $state<ManualWorkflow[]>([])
  let runSheet = $state(false)
  $effect(() => {
    void loadManualWorkflows(session.can('view', 'workflow')).then((w) => (manualWorkflows = w))
  })
  // only owner of all selected documents (idea from paperless-ngx dev #14762); the server checks the same
  const canRunWorkflow = $derived(
    canRunManualWorkflow({
      canChange: canEdit,
      selectedIds: ws.selected,
      docs: ws.docs,
      owns: (d) => session.ownsObject(d),
      isSuperuser: !!session.data?.user?.is_superuser,
      workflowCount: manualWorkflows.length,
    }),
  )
  async function runWorkflow(workflowId: number, force: boolean) {
    try {
      await api.runWorkflow([...ws.selected], workflowId, !force)
      runSheet = false
      toastService.showInfo(t('m.workspace.workflow_run_document_s', { PH: n }))
    } catch (e) {
      toastService.showError(t('common.error_executing_bulk_operation'), e)
    }
  }

  // "Visible to" (DESIGN2-ENTWIRREN §3) via bulk_edit set_permissions; server checks ownership of all documents
  let visSheet = $state(false)
  let visDraft = $state<VisibilityValue | null>(null)
  let visMode = $state<VisibilityMode | ''>('')
  let visMerge = $state(false)
  const canVisibility = $derived(
    canEdit &&
      canBulkChangeVisibility(
        ws.docs.filter((d) => ws.selected.includes(d.id)).map((d) => d.owner),
        session.data?.user,
      ),
  )
  function openVis() {
    visDraft = null
    visMode = ''
    visMerge = false
    visSheet = true
  }
  function applyVis() {
    const v = visDraft
    if (!v) return
    visSheet = false
    run('set_permissions', bulkParameters(v, visMode === 'custom' && visMerge), t('m.visibility.bulk_done', { PH: n }))
  }

  async function run(method: BulkMethod, parameters: Record<string, unknown>, done: string) {
    busy = true
    try {
      await bulkEdit([...ws.selected], method, parameters)
      toastService.showSuccess(done)
      if (method === 'delete') {
        ws.removeLocal([...ws.selected])
        if (ws.ctx.activeId && !ws.docs.some((d) => d.id === ws.ctx.activeId)) ws.open(null, true)
      } else await ws.reload()
    } catch (e) {
      toastService.showError(t('common.error_executing_bulk_operation'), e)
    } finally {
      busy = false
    }
  }
  // Folder view: "Move to folder…" (target needs change permission, the server checks the documents)
  const showFolders = $derived(canEdit && folders.enabled)
  $effect(() => {
    if (showFolders) folders.ensure()
  })
  const folderOpts = $derived(folderOptions(folders.items, (f) => f.user_can_change))
  // shared with drag & drop: clears the selection afterwards, moved documents leave the open folder (stage-1 bug)
  async function moveTo(folder: number | null) {
    busy = true
    try {
      await folders.moveDocs([...ws.selected], folder)
    } finally {
      busy = false
    }
  }
  // PDF actions of the selection like in the classic bulk edit (pdf-actions.ts)
  let pdfOpen = $state(false)
  const selectedDocs = $derived(ws.docs.filter((d) => ws.selected.includes(d.id)))
  const owner = $derived(canEdit && selectedDocs.length === n && selectedDocs.every((d) => session.ownsObject(d)))
  const convertible = $derived(selectedDocs.length > 0 && selectedDocs.every((d) => pdf.canConvert(d)))
  function pdfAct(fn: () => unknown) {
    pdfOpen = false
    void fn()
  }
  const reload = () => void ws.reload()
  // Send (share bundle, email) and download variants like in the classic bulk edit
  let sendOpen = $state(false)
  let dlOpen = $state(false)
  const emailOn = $derived(!!session.get(SETTINGS_KEYS.EMAIL_ENABLED))
  const fieldOpts = $derived(data.customFields.filter((f) => f.id !== null).map((f) => ({ id: f.id as number, name: f.name })).sort((a, b) => a.name.localeCompare(b.name)))
  async function downloadAs(content: 'archive' | 'originals' | 'both') {
    dlOpen = false
    busy = true
    try {
      await bulkDownload([...ws.selected], content)
    } catch (e) {
      toastService.showError(t('m.workspace.error_downloading_documents'), e)
    } finally {
      busy = false
    }
  }
  function afterMerge(deletedOriginals: boolean) {
    if (deletedOriginals) ws.clearSelection()
    void ws.reload()
  }

  async function download() {
    busy = true
    try {
      await bulkDownload([...ws.selected])
    } catch (e) {
      toastService.showError(t('m.workspace.error_downloading_documents'), e)
    } finally {
      busy = false
    }
  }
</script>

<div class="bulk" role="toolbar" aria-label={t('m.workspace.bulk_actions')} id="d2-bulkbar">
  <strong>{t('m.workspace.selected', { PH: n })}</strong>
  {#if ws.total && n < ws.total}
    <button class="d2-btn ghost sm" onclick={() => ws.selectAll()}>{t('m.workspace.select_all', { PH: ws.total })}</button>
  {/if}
  <button class="d2-btn ghost sm" onclick={() => ws.clearSelection()}>{t('common.clear_selection')}</button>
  <span class="grow"></span>
  {#if canEdit}
    <ObjectPicker variant="chip" label={t('m.workspace.add_tag')} icon="tag" options={opts(data.tags)} selected={[]} disabled={busy} id="d2-bulk-tag"
      onchange={(_, id) => id !== null && run('add_tag', { tag: id }, t('m.workspace.tag_added_documents', { PH: n }))} />
    <ObjectPicker variant="chip" label={t('common.remove_tag')} icon="tag" options={opts(data.tags)} selected={[]} disabled={busy}
      onchange={(_, id) => id !== null && run('remove_tag', { tag: id }, t('m.workspace.tag_removed_documents', { PH: n }))} />
    <ObjectPicker variant="chip" label={t('common.correspondent')} icon="person" allowNone options={opts(data.correspondents)} selected={[]} disabled={busy}
      onchange={(_, id) => run('set_correspondent', { correspondent: id }, t('m.workspace.correspondent_set_documents', { PH: n }))} />
    <ObjectPicker variant="chip" label={t('common.document_type')} icon="file-earmark-text" allowNone options={opts(data.documentTypes)} selected={[]} disabled={busy}
      onchange={(_, id) => run('set_document_type', { document_type: id }, t('m.workspace.document_type_set_documents', { PH: n }))} />
    <ObjectPicker variant="chip" label={t('common.storage_path')} icon="folder" allowNone options={opts(data.storagePaths)} selected={[]} disabled={busy} id="d2-bulk-path"
      onchange={(_, id) => run('set_storage_path', { storage_path: id }, t('m.workspace.storage_path_set_documents', { PH: n }))} />
    {#if showFolders}
      <ObjectPicker variant="chip" label={t('m.folders.move_to')} icon="folder2" allowNone noneLabel={t('m.folders.unsorted')} options={folderOpts} selected={[]} disabled={busy} id="d2-bulk-folder"
        onchange={(_, id) => void moveTo(id)} />
    {/if}
    {#if fieldOpts.length}
      <ObjectPicker variant="chip" label={t('common.custom_fields')} icon={SYMBOL.customField} options={fieldOpts} selected={[]} disabled={busy} id="d2-bulk-field"
        onchange={(_, id) => id !== null && void pdf.openBulkCustomField([...ws.selected], id, data.customFields, reload)} />
    {/if}
  {/if}
  {#if canVisibility}
    <button class="d2-btn sm" onclick={openVis} disabled={busy} id="d2-bulk-visibility"><Icon name="eye" />{t('m.visibility.label')}</button>
  {/if}
  {#if canRunWorkflow}
    <button class="d2-btn sm" onclick={() => (runSheet = true)} disabled={busy} id="d2-bulk-run-workflow"><Icon name="diagram-3" />{t('common.run_workflow_2')}</button>
  {/if}
  {#if canEdit}
    <span class="menuwrap">
      <button class="d2-btn sm" onclick={() => (pdfOpen = !pdfOpen)} disabled={busy} aria-haspopup="menu" aria-expanded={pdfOpen} id="d2-bulk-pdf"><Icon name="file-earmark-pdf" />{t('m.workspace.pdf_actions')}</button>
      {#if pdfOpen}
        {@const ids = [...ws.selected]}
        <Menu label={t('m.workspace.pdf_actions')} onclose={() => (pdfOpen = false)} width="16rem">
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-rotate" disabled={!owner} onclick={() => pdfAct(() => pdf.openRotate(ids, reload))}><Icon name="arrow-repeat" />{t('list.rotate')}</button>
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-merge" disabled={n < 2 || !session.can('add', 'document')} onclick={() => pdfAct(() => pdf.openMerge(ids, afterMerge))}><Icon name="union" />{t('list.merge')}</button>
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-merge-versions" disabled={n < 2 || !owner || !session.can('delete', 'document')} onclick={() => pdfAct(() => pdf.openMergeAsVersions(ids, afterMerge.bind(null, true)))}><Icon name="layers" />{t('list.merge_versions')}</button>
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-reprocess" disabled={!owner} onclick={() => pdfAct(() => pdf.openBulkReprocess(ids, reload))}><Icon name="arrow-counterclockwise" />{t('common.reprocess')}</button>
          {#if convertible}
            <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-convert" disabled={!owner} onclick={() => pdfAct(() => pdf.openConvertPdf(selectedDocs, reload))}><Icon name="file-earmark-pdf" />{t('convert.menu')}</button>
          {/if}
        </Menu>
      {/if}
    </span>
  {/if}
  <span class="menuwrap">
    <button class="d2-btn sm" onclick={() => (sendOpen = !sendOpen)} disabled={busy} aria-haspopup="menu" aria-expanded={sendOpen} id="d2-bulk-send"><Icon name="envelope" />{t('common.send')}</button>
    {#if sendOpen}
      <Menu label={t('common.send')} onclose={() => (sendOpen = false)} width="17rem">
        {#if session.can('add', 'sharelink')}
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-bundle" onclick={() => { sendOpen = false; void pdf.openShareBundle(selectedDocs, () => router.navigate('share-links?type=bundles')) }}><Icon name="link-45deg" />{t('list.create_share_link_bundle')}</button>
          <button type="button" role="menuitem" class="d2-menu-item" onclick={() => { sendOpen = false; router.navigate('share-links?type=bundles') }}><Icon name="link-45deg" />{t('list.manage_share_link_bundles')}</button>
        {/if}
        {#if emailOn}
          <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-email" onclick={() => { sendOpen = false; void pdf.openEmail([...ws.selected], selectedDocs.every((d) => !!d.archived_file_name)) }}><Icon name="envelope" />{t('common.email')}</button>
        {/if}
      </Menu>
    {/if}
  </span>
  <span class="menuwrap">
    <button class="d2-btn sm" onclick={download} disabled={busy}><Icon name="download" />{t('common.download')}</button>
    <button class="d2-btn sm icon" onclick={() => (dlOpen = !dlOpen)} disabled={busy} aria-haspopup="menu" aria-expanded={dlOpen} aria-label={t('common.download_options')} title={t('common.download_options')} id="d2-bulk-dl-options"><Icon name="chevron-down" /></button>
    {#if dlOpen}
      <Menu label={t('common.download_options')} onclose={() => (dlOpen = false)} width="15rem">
        <button type="button" role="menuitem" class="d2-menu-item" onclick={() => downloadAs('archive')}>{t('list.archived_files')}</button>
        <button type="button" role="menuitem" class="d2-menu-item" id="d2-bulk-dl-originals" onclick={() => downloadAs('originals')}>{t('list.original_files')}</button>
        <button type="button" role="menuitem" class="d2-menu-item" onclick={() => downloadAs('both')}>{t('list.archived_files')} + {t('list.original_files')}</button>
      </Menu>
    {/if}
  </span>
  {#if session.can('delete', 'document')}
    <button class="d2-btn sm danger" onclick={() => (confirmDelete = true)} disabled={busy} id="d2-bulk-delete"><Icon name="trash" />{t('common.delete')}</button>
  {/if}
</div>

{#if runSheet}
  <RunWorkflowSheet message={t('common.run_workflow_selected_document', { PH: n })} workflows={manualWorkflows} onclose={() => (runSheet = false)} onrun={runWorkflow} />
{/if}

{#if visSheet}
  <SideSheet title={t('m.visibility.bulk_title', { PH: n })} onclose={() => (visSheet = false)} width="460px">
    <div class="vis">
      <span class="d2-label">{t('m.visibility.label')}</span>
      <Visibility id="d2-bulk-vis" deferred ondraft={(v, m) => ((visDraft = v), (visMode = m))} />
      {#if (visMode === 'team' || visMode === 'private') && visDraft?.owner === session.data?.user?.id}
        <p class="d2-muted note">{t('m.visibility.bulk_owner_hint')}</p>
      {/if}
      {#if visMode === 'custom'}
        <label class="d2-check"><input type="checkbox" bind:checked={visMerge} id="d2-bulk-vis-merge" />{t('m.visibility.bulk_merge')}</label>
      {/if}
    </div>
    {#snippet footer()}
      <button class="d2-btn" onclick={() => (visSheet = false)}>{t('common.cancel')}</button>
      <button class="d2-btn primary" id="d2-bulk-vis-apply" disabled={!visDraft} onclick={applyVis}>{t('m.visibility.apply')}</button>
    {/snippet}
  </SideSheet>
{/if}

{#if confirmDelete}
  <SideSheet title={t('m.workspace.delete_documents')} onclose={() => (confirmDelete = false)} width="420px">
    <p>{t('common.move_selected_document_s', { PH: n })}</p>
    {#snippet footer()}
      <button class="d2-btn" onclick={() => (confirmDelete = false)}>{t('common.cancel')}</button>
      <button class="d2-btn primary" id="d2-bulk-delete-confirm" onclick={() => { confirmDelete = false; run('delete', {}, t('m.workspace.document_s_moved_trash', { PH: n })) }}>{t('common.delete')}</button>
    {/snippet}
  </SideSheet>
{/if}

<style>
  .menuwrap {
    position: relative;
    display: inline-flex;
  }
  .bulk {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: var(--d2-s2) var(--d2-s3);
    border-bottom: 1px solid var(--d2-border);
    background: var(--d2-accent-soft);
  }
  strong {
    font-size: var(--d2-fs-sm);
  }
  .vis {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
  }
  .note {
    margin: 0;
    font-size: var(--d2-fs-sm);
  }
  .grow {
    flex: 1;
  }
</style>
