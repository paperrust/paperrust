<!-- Port of dashboard/widgets/saved-view-widget/saved-view-widget.component.html (table, small/large
     cards depending on the setting of the saved view). Host <pngx-saved-view-widget> in the parent markup. -->
<script lang="ts">
  import '../../../../styles/components/saved-view-widget.css'
  import { onDestroy, onMount } from 'svelte'
  import { t, locale } from '../../../../lib/i18n'
  import { formatDate } from '../../../../lib/dates'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { api } from '../../../../api/client'
  import { queryString, type Document, type Paged } from '../../../../api/v1'
  import { session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import { data } from '../../../../stores/data.svelte'
  import { router } from '../../../../router.svelte'
  import { list, DEFAULT_DISPLAY_FIELDS, DOCUMENT_SORT_FIELDS } from '../../../../stores/listView.svelte'
  import { consumerStatus } from '../../../../stores/uploads.svelte'
  import { queryParamsFromFilterRules } from '../../../../lib/query-params'
  import type { FilterRule } from '../../../../lib/filter-rules'
  import {
    FILTER_CORRESPONDENT,
    FILTER_DOCUMENT_TYPE,
    FILTER_FULLTEXT_MORELIKE,
    FILTER_HAS_TAGS_ALL,
    FILTER_OWNER_ANY,
    FILTER_STORAGE_PATH,
  } from '../../../../lib/filter-rule-type'
  import WidgetFrame from '../widget-frame/WidgetFrame.svelte'
  import Tag from '../../../common/tag/Tag.svelte'
  import PreviewPopup from '../../../common/preview-popup/PreviewPopup.svelte'
  import CustomFieldDisplay from '../../../common/custom-field-display/CustomFieldDisplay.svelte'
  import DocumentCardSmall from '../../../document-list/document-card-small/DocumentCardSmall.svelte'
  import DocumentCardLarge from '../../../document-list/document-card-large/DocumentCardLarge.svelte'
  import { DEFAULT_DASHBOARD_DISPLAY_FIELDS, activateViewWithDocuments, allDisplayFieldIds, dashboardPageSize, type DashboardView } from '../../dashboard'

  let { savedView }: { savedView: DashboardView } = $props()

  let documents = $state<Document[]>([])
  let loading = $state(true)
  let show = $state(false)
  let count = $state<number | null>(null)
  let error = $state<string | null>(null)
  let seq = 0

  // svelte-ignore state_referenced_locally
  const placeholderRows = Array.from({ length: dashboardPageSize(savedView) }, (_, i) => i)
  const displayMode = $derived(savedView.display_mode ?? 'table')
  const notesEnabled = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED))
  /** display_fields of the view (otherwise default), filtered by SettingsService.allDisplayFields */
  const displayFields = $derived.by(() => {
    const fields = savedView.display_fields?.length ? savedView.display_fields : DEFAULT_DASHBOARD_DISPLAY_FIELDS
    const allowed = allDisplayFieldIds(notesEnabled)
    return fields.filter((f) => allowed.includes(f))
  })
  const dateFormat = $derived(session.get(SETTINGS_KEYS.DATE_FORMAT))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const customDate = (v: string) => formatDate(v, dateFormat, dateLocale)
  const docTitle = (title: string) => title || t('common.no_title')

  async function reload() {
    const my = ++seq
    loading = documents.length == 0
    error = null
    show = true
    try {
      const r = await api.get<Paged<Document>>(
        `/api/documents/${queryString({
          page: 1,
          page_size: dashboardPageSize(savedView),
          ordering: savedView.sort_field ? (savedView.sort_reverse ? '-' : '') + savedView.sort_field : undefined,
          truncate_content: true,
          ...queryParamsFromFilterRules(savedView.filter_rules as FilterRule[]),
        })}`,
      )
      if (my !== seq) return
      documents = r.results
      count = r.count
      // SavedViewService.setDocumentCount (counter in the sidebar)
      data.viewCounts = { ...data.viewCounts, [savedView.id]: r.count }
      loading = false
      show = true
    } catch (e: any) {
      if (my !== seq) return
      documents = []
      count = null
      const body = e?.body
      error =
        body && typeof body === 'object' && Object.keys(body).length > 0
          ? Object.keys(body)
              .map((fieldName) => {
                const base = fieldName.split('__')[0]
                const fieldError = body[fieldName] as string[]
                return `${t(DOCUMENT_SORT_FIELDS.find((f) => f.field?.split('__')[0] == base)?.name ?? base)}: ${fieldError[0]}`
              })
              .join(', ')
          : String(body ?? e)
      loading = false
      show = true
    }
  }

  onMount(() => {
    reload()
    return consumerStatus.onDocumentConsumptionFinished(() => reload())
  })
  onDestroy(() => seq++)

  const here = $derived(router.href(router.path) + router.query)
  function showAll(e: Event) {
    e.preventDefault()
    const visible: number[] = session.get(SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS) ?? []
    if (visible.includes(savedView.id)) router.navigate(`view/${savedView.id}`)
    else router.navigate(`documents?view=${savedView.id}`)
  }
  function quickFilter(rules: FilterRule[]) {
    list.quickFilter(rules) // leaves an active view, sets the rules, opens documents
  }
  const stopped = (fn: () => void) => (e: Event) => {
    e.preventDefault()
    e.stopImmediatePropagation()
    fn()
  }
  const clickTag = (id: number) => quickFilter([{ rule_type: FILTER_HAS_TAGS_ALL, value: id.toString() }])
  const clickCorrespondent = (id: number) => quickFilter([{ rule_type: FILTER_CORRESPONDENT, value: id.toString() }])
  const clickDocType = (id: number) => quickFilter([{ rule_type: FILTER_DOCUMENT_TYPE, value: id.toString() }])
  const clickStoragePath = (id: number) => quickFilter([{ rule_type: FILTER_STORAGE_PATH, value: id.toString() }])
  const clickMoreLike = (id: number) => quickFilter([{ rule_type: FILTER_FULLTEXT_MORELIKE, value: id.toString() }])
  const clickOwner = (id: number) => quickFilter([{ rule_type: FILTER_OWNER_ANY, value: id.toString() }])
  /** activateViewForNavigation (upstream #14699): give the widget's documents to the list (back/forward in the detail view) */
  function activateViewForNavigation() {
    activateViewWithDocuments(list, savedView, documents, count ?? 0, dashboardPageSize(savedView))
  }
  const openDocumentDetail = (d: Document) => {
    activateViewForNavigation()
    router.navigate(`documents/${d.id}`)
  }

  function getColumnTitle(field: string): string | undefined {
    if (field.startsWith('custom_field_')) {
      const id = field.split('_')[2]
      return data.customFields.find((f) => f.id === parseInt(id))?.name
    }
    const name = DEFAULT_DISPLAY_FIELDS.find((f) => f.id === field)?.name
    return name ? t(name) : undefined
  }
</script>

{#if session.can('view', 'document')}
<pngx-widget-frame><WidgetFrame title={savedView.name} titleIcon={savedView.icon || 'funnel'} loading={false} draggable={savedView}>
  {#snippet titleBadge()}
    {#if count !== null}
      <span class="badge bg-info text-dark ms-2">{count}</span>
    {/if}
  {/snippet}
  {#snippet headerButtons()}
    {#if documents.length}
      <a class="btn-link text-decoration-none" href={here} onclick={showAll}>{t('common.show_all')}</a>
    {/if}
  {/snippet}

  <div class="wrapper fade" class:show>
  {#if error}
    <div class="alert alert-danger mb-0" role="alert">{t('common.error_while_loading_documents')}: {error}</div>
  {:else if displayMode === 'table'}
    <table class="table table-hover mb-0 mt-n2 align-middle">
      <thead>
        <tr>
          {#each displayFields as field, i (field)}
            <th
              scope="col"
              class="text-truncate"
              class:d-none={i > 1}
              class:d-md-table-cell={i > 1}
              class:w-25={field === 'created' || field === 'added'}
              title={getColumnTitle(field)}>
              {getColumnTitle(field)}
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#if loading && !documents.length}
          {#each placeholderRows as row (row)}
            <tr>
              {#each displayFields as field, j (field)}
                <td class="py-2 py-md-3 position-relative" class:d-none={j > 1} class:d-md-table-cell={j > 1}>
                  <div class="placeholder-glow text-start">
                    <span class="placeholder bg-secondary w-50" style:opacity={1 - (row * 1) / placeholderRows.length}></span>
                  </div>
                </td>
              {/each}
            </tr>
          {/each}
        {/if}
        {#each documents as doc (doc.id)}
          <tr>
            {#each displayFields as field, j (field)}
              <td class="py-2 py-md-3 position-relative" class:d-none={j > 1} class:d-md-table-cell={j > 1}>
                  {#if field === 'added'}
                      <a href={router.href(`documents/${doc.id}`)} onclick={activateViewForNavigation} class="btn-link text-dark text-decoration-none py-2 py-md-3" title={t('common.open_document')}>{customDate(doc.added)}</a>
                  {:else if field === 'created'}
                      <a href={router.href(`documents/${doc.id}`)} onclick={activateViewForNavigation} class="btn-link text-dark text-decoration-none py-2 py-md-3" title={t('common.open_document')}>{customDate(doc.created)}</a>
                  {:else if field === 'title'}
                      <a href={router.href(`documents/${doc.id}`)} onclick={activateViewForNavigation} title={t('common.open_document')} class="btn-link text-dark text-decoration-none py-2 py-md-3">{docTitle(doc.title)}</a>
                  {:else if field === 'correspondent'}
                      {#if doc.correspondent}
                        <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <a class="btn-link text-dark text-decoration-none" type="button" onclick={stopped(() => clickCorrespondent(doc.correspondent!))} title={t('common.filter_correspondent')}>{data.correspondents.get(doc.correspondent)?.name ?? ''}</a>
                      {/if}
                  {:else if field === 'tag'}
                      {#each doc.tags as tagID (tagID)}
                        <pngx-tag class="ms-1 fs-6"><Tag tag={data.tags.get(tagID)} clickable={true} linkTitle={t('common.filter_tag')} onclick={stopped(() => clickTag(tagID))} /></pngx-tag>
                      {/each}
                  {:else if field === 'documenttype'}
                      {#if doc.document_type}
                        <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <a class="btn-link text-dark text-decoration-none" type="button" onclick={stopped(() => clickDocType(doc.document_type!))} title={t('common.filter_document_type')}>{data.documentTypes.get(doc.document_type)?.name ?? ''}</a>
                      {/if}
                  {:else if field === 'storagepath'}
                      {#if doc.storage_path}
                        <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <a class="btn-link text-dark text-decoration-none" type="button" onclick={stopped(() => clickStoragePath(doc.storage_path!))} title={t('common.filter_storage_path')}>{data.storagePaths.get(doc.storage_path)?.name ?? ''}</a>
                      {/if}
                  {:else if field === 'owner'}
                      {#if doc.owner}
                        <!-- svelte-ignore a11y_missing_attribute, a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <a class="btn-link text-dark text-decoration-none" type="button" onclick={stopped(() => clickOwner(doc.owner!))} title={t('dashboard.filter_owner')}>{data.username(doc.owner)}</a>
                      {/if}
                  {:else if field === 'asn'}
                      {doc.archive_serial_number ?? ''}
                  {:else if field === 'pagecount'}
                      {doc.page_count ?? ''}
                  {:else if field === 'shared'}
                      {#if doc.is_shared_by_requester}{t('common.yes')}{:else}{t('common.no')}{/if}
                  {:else if field === 'note'}
                      {#if doc.notes.length}
                        <a href={router.href(`documents/${doc.id}/notes`)} onclick={activateViewForNavigation} class="btn btn-sm p-0">
                          <span class="badge rounded-pill bg-light border text-primary">
                            <pngx-bs-icon width="1.2em" height="1.2em" class="ms-1 me-1" name="chat-left-text">{@html icon('chat-left-text', '1.2em', '1.2em')}</pngx-bs-icon>
                          {doc.notes.length}</span>
                        </a>
                      {/if}
                  {/if}
                  {#if field.startsWith('custom_field_')}
                    <pngx-custom-field-display><CustomFieldDisplay document={doc} fieldId={+field.slice(13)} /></pngx-custom-field-display>
                  {/if}
                  {#if j === displayFields.length - 1}
                    <div class="btn-group position-absolute top-50 end-0 translate-middle-y">
                      <pngx-preview-popup><PreviewPopup id={doc.id} linkClasses="btn px-4 btn-dark border-dark-subtle">
                        <pngx-bs-icon width="0.8em" height="0.8em" name="eye">{@html icon('eye', '0.8em', '0.8em')}</pngx-bs-icon>
                      </PreviewPopup></pngx-preview-popup>
                      <a href={api.downloadUrl(doc.id)} class="btn px-4 btn-dark border-dark-subtle" title={t('common.download')} onclick={(e) => e.stopPropagation()}>
                        <pngx-bs-icon width="0.8em" height="0.8em" name="download">{@html icon('download', '0.8em', '0.8em')}</pngx-bs-icon>
                      </a>
                    </div>
                  {/if}
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  {:else if displayMode === 'smallCards'}
    <div class="row row-cols-paperless-cards my-n2">
      {#each documents as d (d.id)}
        <pngx-document-card-small class="p-0 opacity-100" onclick={activateViewForNavigation} role="none"><DocumentCardSmall
          document={d}
          displayFields={displayFields}
          {dateFormat}
          {dateLocale}
          ontoggle={() => {}}
          onopen={() => openDocumentDetail(d)}
          onclicktag={clickTag}
          onclickcorrespondent={clickCorrespondent}
          onclickstoragepath={clickStoragePath}
          onclickdocumenttype={clickDocType} />
        </pngx-document-card-small>
      {/each}
    </div>
  {:else if displayMode === 'largeCards'}
    <div class="row my-n2">
      {#each documents as d (d.id)}
        <pngx-document-card-large class="opacity-100" onclick={activateViewForNavigation} role="none"><DocumentCardLarge
          onopen={() => openDocumentDetail(d)}
          document={d}
          displayFields={displayFields}
          {dateLocale}
          ontoggle={() => {}}
          onclicktag={clickTag}
          onclickcorrespondent={clickCorrespondent}
          onclickstoragepath={clickStoragePath}
          onclickdocumenttype={clickDocType}
          onclickmorelike={() => clickMoreLike(d.id)} />
        </pngx-document-card-large>
      {/each}
    </div>
  {:else}
    <p class="text-center text-muted mb-0 fst-italic">{t('dashboard.no_documents')}</p>
  {/if}

  </div>
</WidgetFrame></pngx-widget-frame>
{/if}
