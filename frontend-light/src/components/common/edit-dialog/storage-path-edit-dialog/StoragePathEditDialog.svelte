<!-- Port of common/edit-dialog/storage-path-edit-dialog/storage-path-edit-dialog.component.html
     (accordion "Preview" like ngbAccordion: content only on expanding; document search like ng-select with typeahead). -->
<script lang="ts">
  import '../../../../styles/components/storage-path-edit-dialog.css'
  import '../../../../styles/parts/select.css'
  import { t, locale } from '../../../../lib/i18n'
  import { api } from '../../../../api/client'
  import { queryString, type Paged } from '../../../../api/v1'
  import { testStoragePath } from '../../../../api/objects'
  import { formatDate } from '../../../../lib/dates'
  import { data } from '../../../../stores/data.svelte'
  import { DEFAULT_MATCHING_ALGORITHM, matchingAlgorithms, patternRequired, type MatchingModel } from '../../../../lib/matching'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputTextarea from '../../input/textarea/InputTextarea.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import InputCheck from '../../input/check/InputCheck.svelte'
  import PermissionsForm from '../../input/permissions/permissions-form/PermissionsForm.svelte'
  import { PARTS, PRESETS, buildTemplate, examplePath, parseTemplate, type PartId } from './path-builder'

  type Doc = { id: number; title: string; created?: string }
  let props: EditDialogProps<MatchingModel & { path?: string }> = $props()
  const dlg = new EditDialogState<MatchingModel>('storage_paths', props, {
    name: '',
    path: '',
    matching_algorithm: DEFAULT_MATCHING_ALGORITHM,
    match: '',
    is_insensitive: true,
    permissions_form: null,
  })
  const users = $derived([...data.users.values()])
  const title = $derived(dlg.mode === 'create' ? t('dialog.storage_path.create_new_storage_path') : t('dialog.storage_path.edit_storage_path'))

  let previewOpen = $state(false)
  let testDocument: Doc | null = null
  let testResult = $state<string | null | undefined>(undefined)
  let testFailed = $state(false)
  let testLoading = $state(false)
  let loading = $state(false)
  let term = $state('')
  let found = $state<Doc[]>([])
  let selectOpen = $state(false)
  let selected = $state<Doc | null>(null)
  let timer: ReturnType<typeof setTimeout> | undefined

  // Builder (simple) ↔ template (advanced): simple as long as the template only contains known placeholders
  const parsed = $derived(parseTemplate(dlg.form.path ?? ''))
  let advanced = $state(false)
  $effect(() => { if (parsed === null) advanced = true })
  const parts = $derived(parsed ?? [])
  const PART_IDS = Object.keys(PARTS) as PartId[]
  function setParts(next: PartId[]) { dlg.form.path = buildTemplate(next) }
  function addPart(id: PartId) {
    const cur = parts.slice()
    // insert before the file name, file name parts replace the previous file name
    if (id === 'title' || id === 'asn') { if (cur.length) cur[cur.length - 1] = id; else cur.push(id) }
    else cur.splice(Math.max(cur.length - 1, 0), 0, id)
    setParts(cur)
  }
  function removePart(i: number) { const cur = parts.slice(); cur.splice(i, 1); setParts(cur) }

  async function testPath(document: Doc | null) {
    if (!document) {
      testResult = null
      return
    }
    testDocument = document
    testLoading = true
    try {
      const result = await testStoragePath(dlg.form.path, document.id)
      if (result?.length) {
        testResult = result
        testFailed = false
      } else {
        testResult = null
        testFailed = true
      }
    } catch {
      testResult = null
      testFailed = true
    }
    testLoading = false
  }
  // Path changed → test again (valueChanges, only with a selected document)
  let lastPath = dlg.form.path
  $effect(() => {
    const p = dlg.form.path
    if (p !== lastPath) {
      lastPath = p
      if (p && testDocument) testPath(testDocument)
    }
  })
  function search() {
    clearTimeout(timer)
    selectOpen = true
    if (term.length < 2) return
    loading = true
    timer = setTimeout(() => {
      api
        .get<Paged<Doc>>(`/api/documents/${queryString({ page: 1, ordering: '-created', truncate_content: true, title__icontains: term })}`)
        .then((r) => (found = r.results))
        .catch(() => (found = []))
        .finally(() => (loading = false))
    }, 0)
  }
  function choose(d: Doc | null) {
    selected = d
    term = ''
    selectOpen = false
    testPath(d)
  }
</script>

<svelte:window onclick={() => (selectOpen = false)} />

{#snippet loadingTemplate()}
  <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
  <div class="visually-hidden">{t('common.loading')}</div>
{/snippet}

<pngx-storage-path-edit-dialog><form onsubmit={(e) => { e.preventDefault(); dlg.save() }} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    {#if dlg.object?.id}
      <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {dlg.object.id}</span>
    {/if}
    <button type="button" disabled={!dlg.closeEnabled} class="btn-close" aria-label="Close" onclick={() => dlg.cancel()}>
    </button>
  </div>
  <div class="modal-body">

    <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
    {#if !advanced}
      <div class="mb-3 sp-builder">
        <label class="form-label" for="sp-presets">{t('dialog.storage_path.folder_structure')}</label>
        <div class="d-flex flex-wrap gap-2 mb-2" id="sp-presets">
          {#each PRESETS as pr (pr.id)}
            <button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => setParts(pr.parts)}>{pr.parts.map((x) => t(`dialog.storage_path.part_${x}`)).join(' / ')}</button>
          {/each}
        </div>
        <div class="sp-chain d-flex flex-wrap align-items-center gap-1 mb-2" aria-live="polite">
          {#each parts as x, i (i + x)}
            {#if i > 0}<span class="text-muted">/</span>{/if}
            <span class="badge rounded-pill text-bg-light border sp-part">{t(`dialog.storage_path.part_${x}`)}<button type="button" class="btn-close btn-close-sm ms-1" aria-label={t('common.delete')} onclick={() => removePart(i)}></button></span>
          {:else}
            <span class="text-muted small">{t('dialog.storage_path.choose_parts')}</span>
          {/each}
        </div>
        <div class="d-flex flex-wrap gap-1 mb-2">
          <span class="small text-muted me-1">{t('dialog.storage_path.add_level')}</span>
          {#each PART_IDS as id (id)}
            <button type="button" class="btn btn-sm btn-light border" onclick={() => addPart(id)}>+ {t(`dialog.storage_path.part_${id}`)}</button>
          {/each}
        </div>
        {#if parts.length}<div class="small"><span class="text-muted">{t('dialog.storage_path.example')}</span> <code>{examplePath(parts)}</code></div>{/if}
        {#if dlg.error?.path}<div class="invalid-feedback d-block">{dlg.error.path}</div>{/if}
        <button type="button" class="btn btn-link btn-sm px-0 mt-1" onclick={() => (advanced = true)}>{t('dialog.storage_path.edit_template')}</button>
      </div>
    {:else}
      <pngx-input-textarea><InputTextarea title={t('common.path')} bind:value={dlg.form.path} error={dlg.error?.path} hint={t("dialog.storage_path.see_target_blank_href")} monospace={true} /></pngx-input-textarea>
      {#if parsed !== null}<button type="button" class="btn btn-link btn-sm px-0 mb-2" onclick={() => (advanced = false)}>{t('dialog.storage_path.use_builder')}</button>{/if}
    {/if}

    <div class="accordion">
      <div class="accordion-item">
        <h2 class="accordion-header" class:collapsed={!previewOpen} role="heading">
          <button class="accordion-button" class:collapsed={!previewOpen} type="button" id="ngb-accordion-item-sp-toggle" aria-controls="ngb-accordion-item-sp-collapse" aria-expanded={previewOpen} onclick={() => (previewOpen = !previewOpen)}>{t('common.preview')}</button>
        </h2>
        <div class="accordion-collapse collapse" class:show={previewOpen} role="region" id="ngb-accordion-item-sp-collapse" aria-labelledby="ngb-accordion-item-sp-toggle">
          <div class="accordion-body">
            {#if previewOpen}
              <div class="card mb-2">
                <div class="card-body p-2">
                  {#if testLoading}
                    {@render loadingTemplate()}
                  {:else if testResult}
                    <code>{testResult}</code>
                  {:else if testFailed}
                    <div class="text-danger">{t('dialog.storage_path.path_test_failed')}</div>
                  {:else}
                    <div class="text-muted small">{t('dialog.storage_path.no_document_selected')}</div>
                  {/if}
                </div>
              </div>
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <ng-select name="testDocument" class="ng-select ng-select-single ng-select-searchable ng-select-clearable" class:ng-select-opened={selectOpen} class:ng-select-bottom={selectOpen} class:ng-has-value={!!selected} onclick={(e) => e.stopPropagation()}><div class="ng-select-container" class:ng-has-value={!!selected}><div class="ng-value-container">{#if !selected && !term}<div class="ng-placeholder">{t('dialog.storage_path.search_document')}</div>{/if}{#if selected && !term}<div class="ng-value"><span class="ng-value-label">{selected.title}</span></div>{/if}<div class="ng-input"><input aria-autocomplete="list" role="combobox" type="text" autocorrect="off" autocapitalize="off" autocomplete="off" aria-expanded={selectOpen} bind:value={term} oninput={search}></div></div>{#if loading}{@render loadingTemplate()}{/if}{#if selected}<span class="ng-clear-wrapper" title="Clear all" role="button" tabindex="0" onclick={() => choose(null)} onkeydown={(e) => e.key === 'Enter' && choose(null)}><span aria-hidden="true" class="ng-clear">×</span></span>{/if}<span class="ng-arrow-wrapper"><span class="ng-arrow"></span></span></div>{#if selectOpen}<div role="listbox" class="ng-dropdown-panel ng-select-bottom"><div class="ng-dropdown-panel-items scroll-host"><div></div><div>{#each term.length >= 2 ? found : [] as document (document.id)}<div role="option" tabindex="-1" aria-selected="false" class="ng-option" onmousedown={(e) => { e.preventDefault(); choose(document) }}><div>{document.title} <small class="text-muted">({formatDate(document.created, 'shortDate', locale())})</small></div></div>{:else}<div class="ng-option ng-option-disabled">{term.length >= 2 ? t('common.no_documents_found') : 'Type to search'}</div>{/each}</div></div></div>{/if}</ng-select>
            {/if}
          </div>
        </div>
      </div>
    </div>

    <hr/>

    <pngx-input-select><InputSelect title={t('common.matching_algorithm')} items={matchingAlgorithms()} bind:value={dlg.form.matching_algorithm} /></pngx-input-select>
    {#if patternRequired(dlg.form.matching_algorithm)}
      <pngx-input-text><InputText title={t('common.matching_pattern')} bind:value={dlg.form.match} error={dlg.error?.match} /></pngx-input-text>
      <pngx-input-check><InputCheck title={t('common.case_insensitive')} bind:value={dlg.form.is_insensitive} /></pngx-input-check>
    {/if}

    {#if dlg.userIsOwner}
      <div>
        <pngx-permissions-form accordion="true"><PermissionsForm {users} accordion={true} bind:value={dlg.form.permissions_form} /></pngx-permissions-form>
      </div>
    {/if}

  </div>
  <div class="modal-footer">
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-storage-path-edit-dialog>
