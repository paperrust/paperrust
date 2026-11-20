<!-- Port of dashboard/widgets/statistics-widget/statistics-widget.component.html (GET /api/statistics/).
     Host <pngx-statistics-widget> in the parent markup. Popover per file type (ngbPopover) follows. -->
<script lang="ts">
  import '../../../../styles/components/statistics-widget.css'
  import { onDestroy, onMount } from 'svelte'
  import { t, locale } from '../../../../lib/i18n'
  import { api } from '../../../../api/client'
  import { session } from '../../../../stores/settings.svelte'
  import { router } from '../../../../router.svelte'
  import { list } from '../../../../stores/listView.svelte'
  import { consumerStatus } from '../../../../stores/uploads.svelte'
  import { FILTER_HAS_TAGS_ANY, FILTER_MIME_TYPE } from '../../../../lib/filter-rule-type'
  import type { FilterRule } from '../../../../lib/filter-rules'
  import WidgetFrame from '../widget-frame/WidgetFrame.svelte'
  import { fileTypeExtension, fileTypeName, groupFileTypes, type DocumentFileType, type Statistics } from './mime-names'

  let loading = $state(false)
  let statistics = $state<Statistics>({})
  let destroyed = false

  async function reload() {
    if (loading) return
    loading = true
    try {
      const s = await api.get<Statistics>('/api/statistics/')
      if (destroyed) return
      statistics = groupFileTypes(s, t('common.other'))
    } catch {
      // Angular: error without display (loading indicator stays)
      return
    }
    loading = false
  }

  onMount(() => {
    reload()
    const off = consumerStatus.onDocumentConsumptionFinished(() => reload())
    return off
  })
  onDestroy(() => (destroyed = true))

  const nf = (opts: Intl.NumberFormatOptions = {}) => new Intl.NumberFormat(locale(), { maximumFractionDigits: 3, ...opts })
  const num = (n: number | undefined) => (n == null ? '' : nf().format(n))
  const getFileTypePercent = (f: DocumentFileType) => (f.mime_type_count / (statistics?.documents_total ?? 0)) * 100
  const getItemOpacity = (i: number) => 1 - i / (statistics?.document_file_type_counts?.length ?? 1)
  const fileTypes = $derived(statistics?.document_file_type_counts ?? [])

  function quickFilter(rules: FilterRule[]) {
    list.setFilterRules(rules)
    router.navigate('documents')
  }
  function goToInbox() {
    quickFilter([{ rule_type: FILTER_HAS_TAGS_ANY, value: (statistics.inbox_tags ?? []).map((id) => id.toString()).join(',') }])
  }
  function filterByFileType(f: DocumentFileType) {
    if (f.is_other) return
    quickFilter([{ rule_type: FILTER_MIME_TYPE, value: f.mime_type }])
  }
  const go = (p: string) => () => router.navigate(p)
</script>

<pngx-widget-frame><WidgetFrame title={t('dashboard.statistics')} {loading}>
    <div class="list-group border-light placeholder-glow">
      {#if loading}
        <div class="list-group-item d-flex">
          <div class="placeholder w-50"></div>
          <span class="placeholder badge rounded-pill ms-auto pr-w-25px">&nbsp;</span>
        </div>
        <div class="list-group-item d-flex">
          <div class="placeholder w-25"></div>
          <span class="placeholder badge rounded-pill ms-auto pr-w-25px">&nbsp;</span>
        </div>
        <div class="list-group-item d-flex">
          <div class="placeholder w-25"></div>
          <span class="placeholder badge rounded-pill ms-auto pr-w-25px">&nbsp;</span>
        </div>
        <div class="list-group-item d-flex">
          <div class="placeholder w-25"></div>
          <span class="placeholder badge rounded-pill ms-auto pr-w-25px">&nbsp;</span>
        </div>
        <div class="list-group-item filetypes">
          <div class="placeholder w-100 d-block mb-2"></div>
          <div class="placeholder w-100 d-block mb-2"></div>
          <div class="placeholder w-100 d-block"></div>
        </div>
      {:else}
        {#if statistics?.documents_inbox !== null}
          <!-- svelte-ignore a11y_invalid_attribute -->
          <a class="list-group-item list-group-item-action d-flex justify-content-between align-items-center" title={t('common.go_inbox')} href="javascript:void(0)" onclick={goToInbox}>
            {t('dashboard.documents_in_inbox')}:
            <span class="badge rounded-pill" class:bg-primary={(statistics?.documents_inbox ?? 0) > 0} class:bg-muted={statistics?.documents_inbox === 0}>{statistics?.documents_inbox ?? ''}</span>
          </a>
        {/if}
        <a class="list-group-item list-group-item-action d-flex justify-content-between align-items-center" title={t('common.go_documents')} href={router.href('documents')}>
          {t('dashboard.total_documents')}:
          <span class="badge bg-primary rounded-pill">{statistics?.documents_total ?? ''}</span>
        </a>
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="list-group-item d-flex justify-content-between align-items-center" onclick={go('documents')}>
          {t('dashboard.total_characters')}:
          <span class="badge bg-secondary text-light rounded-pill">{num(statistics?.character_count)}</span>
        </div>
        {#if statistics?.current_asn}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="list-group-item d-flex justify-content-between align-items-center" onclick={go('documents')}>
            {t('dashboard.current_asn')}:
            <span class="badge bg-secondary text-light rounded-pill">{statistics?.current_asn}</span>
          </div>
        {/if}
      {/if}
      {#if fileTypes.length > 1}
        <div class="list-group-item filetypes">
          <div class="d-flex justify-content-between align-items-center my-2">
            <div class="progress flex-grow-1">
              {#each fileTypes as filetype, i (filetype)}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <div
                  class="progress-bar bg-primary"
                  role="progressbar"
                  onclick={() => filterByFileType(filetype)}
                  aria-label={fileTypeName(filetype.mime_type)}
                  class:me-1px={i !== fileTypes.length - 1}
                  style:width={getFileTypePercent(filetype) + '%'}
                  style:opacity={getItemOpacity(i)}
                  aria-valuenow={getFileTypePercent(filetype)}
                  aria-valuemin="0"
                  aria-valuemax="100">
                </div>
              {/each}
            </div>
          </div>
          <div class="d-flex flex-wrap align-items-start">
            {#each fileTypes as filetype, i (filetype)}
              <div class="d-flex">
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <div class="text-nowrap me-2" class:cursor-pointer={!filetype.is_other} onclick={() => filterByFileType(filetype)}>
                  <span class="badge rounded-pill bg-primary d-inline-block p-0 me-1" style:opacity={getItemOpacity(i)}></span>
                  <small class="text-nowrap"><span class="fw-bold">{fileTypeExtension(filetype.mime_type)}</span>&nbsp;<span class="text-muted">({nf({ maximumFractionDigits: 1 }).format(getFileTypePercent(filetype))}%)</span></small>
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>

    <div class="list-group border-light mt-3">
      {#if session.can('view', 'tag')}
        {#if loading}
          <div class="placeholder-glow list-group-item">
            <span class="placeholder w-100"></span>
          </div>
        {/if}
        {#if (statistics?.tag_count ?? 0) > 0}
          <a class="list-group-item d-flex justify-content-between align-items-center" href={router.href('tags')}>
            {t('common.tags')}:
            <span class="badge bg-secondary text-light rounded-pill">{num(statistics?.tag_count)}</span>
          </a>
        {/if}
      {/if}
      {#if session.can('view', 'correspondent')}
        {#if loading}
          <div class="placeholder-glow list-group-item">
            <span class="placeholder w-100"></span>
          </div>
        {/if}
        {#if (statistics?.correspondent_count ?? 0) > 0}
          <a class="list-group-item d-flex justify-content-between align-items-center" href={router.href('correspondents')}>
            {t('common.correspondents')}:
            <span class="badge bg-secondary text-light rounded-pill">{num(statistics?.correspondent_count)}</span>
          </a>
        {/if}
      {/if}
      {#if session.can('view', 'documenttype')}
        {#if loading}
          <div class="placeholder-glow list-group-item">
            <span class="placeholder w-100"></span>
          </div>
        {/if}
        {#if (statistics?.document_type_count ?? 0) > 0}
          <a class="list-group-item d-flex justify-content-between align-items-center" href={router.href('documenttypes')}>
            {t('common.document_types')}:
            <span class="badge bg-secondary text-light rounded-pill">{num(statistics?.document_type_count)}</span>
          </a>
        {/if}
      {/if}
      {#if session.can('view', 'storagepath')}
        {#if loading}
          <div class="placeholder-glow list-group-item">
            <span class="placeholder w-100"></span>
          </div>
        {/if}
        {#if (statistics?.storage_path_count ?? 0) > 0}
          <a class="list-group-item d-flex justify-content-between align-items-center" href={router.href('storagepaths')}>
            {t('common.storage_paths')}:
            <span class="badge bg-secondary text-light rounded-pill">{num(statistics?.storage_path_count)}</span>
          </a>
        {/if}
      {/if}
    </div>
</WidgetFrame></pngx-widget-frame>
