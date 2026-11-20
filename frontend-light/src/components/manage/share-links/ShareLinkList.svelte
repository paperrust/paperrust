<!-- Port of manage/share-links/share-link-list (table of the document share links: GET /api/share_links/
     with page/sorting, copy link, copy ID, delete with confirmation). Loading state reported to the parent component via onloading. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import type { ShareLink } from '../../../api/v1'
  import { formatDate } from '../../../lib/dates'
  import { router } from '../../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { EXTRA_KEYS, setSetting, setting, storeSettings } from '../../../stores/settingsSave.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import Pagination from '../../common/pagination/Pagination.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import { copyText, docTitle, fileVersionLabel, isExpired, slugShareUrl } from '../../common/share-link-bundle-dialog/share-link-bundle'
  import { nextSort, pageAfterDelete, sortClass } from './share-links'

  let { onloading }: { onloading?: (loading: boolean) => void } = $props()

  let loading = $state(true)
  let links = $state<ShareLink[]>([])
  let total = $state(0)
  let page = $state(1)
  let sortField = $state('created')
  let sortReverse = $state(true)
  let copiedID = $state<number | null>(null)
  let copiedDocumentID = $state<number | null>(null)
  let error = $state<string | null>(null)
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const pageSize = $derived<number>(setting(EXTRA_KEYS.OBJECT_LIST_SIZES)?.share_links || 25)
  $effect(() => onloading?.(loading))

  async function setPageSize(size: number) {
    setSetting(EXTRA_KEYS.OBJECT_LIST_SIZES, { ...setting(EXTRA_KEYS.OBJECT_LIST_SIZES), share_links: size })
    try {
      await storeSettings()
      page = 1
      void reload()
    } catch (e) {
      toastService.showError(t('manage.attributes.error_saving_settings'), e)
    }
  }

  async function reload() {
    loading = true
    error = null
    try {
      const r = await api.listShareLinks(page, pageSize, sortField, sortReverse)
      links = r.results
      total = r.count
      loading = false
    } catch (e) {
      loading = false
      error = t('manage.share_links.links_load_failed')
      toastService.showError(t('manage.share_links.links_fetch_error'), e)
    }
  }
  onMount(() => void reload())

  function setPage(p: number) {
    page = p
    void reload()
  }
  function onSort(column: string) {
    ;({ sortField, sortReverse } = nextSort(sortField, sortReverse, column))
    page = 1
    void reload()
  }
  async function copy(link: ShareLink) {
    if (await copyText(slugShareUrl(link.slug))) {
      copiedID = link.id
      setTimeout(() => (copiedID = null), 3000)
    }
  }
  async function deleteLink(link: ShareLink) {
    try {
      await api.deleteShareLink(link.id)
      page = pageAfterDelete(links.length, page)
      toastService.showSuccess(t('manage.share_links.link_deleted'))
      void reload()
    } catch (e) {
      toastService.showError(t('manage.share_links.link_delete_failed'), e)
    }
  }
  async function copyDocumentID(documentID: number) {
    if (await copyText(documentID.toString())) {
      copiedDocumentID = documentID
      setTimeout(() => (copiedDocumentID = null), 3000)
    }
  }
</script>

<pngx-share-link-list><div class="border border-top-0 rounded-bottom p-3">
  {#if !loading && error}
    <div class="alert alert-danger mb-0" role="alert">{error}</div>
  {/if}
  {#if !loading && !error && links.length === 0}
    <p class="mb-0 text-muted fst-italic">
      {t('manage.share_links.no_links')}
    </p>
  {/if}
  {#if !loading && !error && links.length > 0}
    <div class="table-responsive">
      <table class="table table-sm align-middle mb-0 bg-body">
        <thead>
          <tr>
            <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'document__title')}" pngxSortable="document__title" onclick={() => onSort('document__title')}>{t('m.workspace.document')}</th>
            <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'created')}" pngxSortable="created" onclick={() => onSort('created')}>{t('common.created')}</th>
            <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'expiration')}" pngxSortable="expiration" onclick={() => onSort('expiration')}>{t('ui.expires')}</th>
            <th scope="col">{t('dialog.share_bundle.file_version')}</th>
            <th scope="col" class="text-end">{t('common.actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#each links as link (link.id)}
            <tr>
              <td>
                <a href={router.href(`documents/${link.document}`)}>{docTitle(link.document_title)}</a>
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                <span class="badge bg-primary text-primary-text-contrast ms-3 small fs-normal cursor-pointer" onclick={() => copyDocumentID(link.document)}>
                  {#if copiedDocumentID === link.document}
                    <pngx-bs-icon width="1em" height="1em" name="clipboard-check" class="me-1">{@html icon('clipboard-check', '1em', '1em')}</pngx-bs-icon>{t('ui.copied')}
                  {:else}
                    ID: {link.document}
                  {/if}
                </span>
              </td>
              <td>{formatDate(link.created, 'short', dateLocale)}</td>
              <td>
                {#if link.expiration}
                  {formatDate(link.expiration, 'short', dateLocale)}
                  {#if isExpired(link.expiration)}
                    <span class="badge text-bg-danger ms-2">{t('manage.share_links.expired')}</span>
                  {/if}
                {:else}
                  <span>{t('ui.never')}</span>
                {/if}
              </td>
              <td>{fileVersionLabel(link.file_version)}</td>
              <td class="text-end">
                <div class="d-inline-block position-relative">
                  <span
                    class="badge bg-primary small fade position-absolute top-50 end-100 translate-middle-y me-2 pe-none z-3 text-nowrap"
                    class:show={copiedID === link.id}
                  >{t('ui.copied')}</span>
                  <div class="btn-group btn-group-sm">
                    <button
                      type="button"
                      class="btn btn-outline-primary"
                      onclick={() => copy(link)}
                      title={t('manage.share_links.copy_share_link')}
                    >
                      {#if copiedID === link.id}
                        <pngx-bs-icon name="clipboard-check">{@html icon('clipboard-check')}</pngx-bs-icon>
                      {:else}
                        <pngx-bs-icon name="clipboard">{@html icon('clipboard')}</pngx-bs-icon>
                      {/if}
                      <span class="visually-hidden">{t('manage.share_links.copy_share_link')}</span>
                    </button>
                    {#if session.can('delete', 'sharelink')}
                      <pngx-confirm-button><ConfirmButton
                        buttonClasses="btn btn-sm btn-outline-danger"
                        onconfirm={() => deleteLink(link)}
                        iconName="trash"
                        label=""
                      /></pngx-confirm-button>
                    {/if}
                  </div>
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div class="d-flex flex-wrap justify-content-end align-items-center gap-3 mt-3 ms-auto">
      <div class="d-flex align-items-center">
        <label class="small text-muted me-2" for="shareLinkPageSize">{t('manage.attributes.show_label')}</label>
        <select id="shareLinkPageSize" class="form-select form-select-sm w-auto" value={pageSize} onchange={(e) => setPageSize(+e.currentTarget.value)}>
          <option value={25}>25</option>
          <option value={50}>50</option>
          <option value={100}>100</option>
        </select>
        <span class="small text-muted ms-2 d-none d-md-inline">{t('manage.attributes.per_page')}</span>
      </div>
      {#if total > pageSize}
        <ngb-pagination class="mb-0" size="sm" aria-label={t('manage.share_links.links_pagination')} role="navigation"><Pagination {pageSize} collectionSize={total} {page} maxSize={5} onpage={setPage} /></ngb-pagination>
      {/if}
    </div>
  {/if}
</div></pngx-share-link-list>
