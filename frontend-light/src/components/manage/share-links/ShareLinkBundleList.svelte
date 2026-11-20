<!-- Port of manage/share-links/share-link-bundle-list (table of the share bundles: GET /api/share_link_bundles/,
     silent reload every 5 s, error details as popover, copy link only when ready, "Retry" on error
     → POST …/rebuild/, delete with confirmation). Loading state reported to the parent component via onloading. -->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import type { ShareLinkBundle } from '../../../api/v1'
  import { formatDate } from '../../../lib/dates'
  import { Popover } from '../../../lib/popover.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { EXTRA_KEYS, setSetting, setting, storeSettings } from '../../../stores/settingsSave.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { fileSize } from '../../document-detail/document-detail'
  import Pagination from '../../common/pagination/Pagination.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import { copyText, fileVersionLabel, isExpired, replaceBundle, slugShareUrl, statusLabel } from '../../common/share-link-bundle-dialog/share-link-bundle'
  import { nextSort, pageAfterDelete, sortClass } from './share-links'

  let { onloading }: { onloading?: (loading: boolean) => void } = $props()

  let loading = $state(true)
  let bundles = $state<ShareLinkBundle[]>([])
  let error = $state<string | null>(null)
  let copiedSlug = $state<string | null>(null)
  let total = $state(0)
  let page = $state(1)
  let sortField = $state('created')
  let sortReverse = $state(true)
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const pageSize = $derived<number>(setting(EXTRA_KEYS.OBJECT_LIST_SIZES)?.share_link_bundles || 25)
  $effect(() => onloading?.(loading))

  const pops = new Map<number, Popover>()
  const pop = (id: number) => {
    let p = pops.get(id)
    if (!p) pops.set(id, (p = new Popover({ triggers: 'mouseenter:mouseleave' })))
    return p
  }

  async function setPageSize(size: number) {
    setSetting(EXTRA_KEYS.OBJECT_LIST_SIZES, { ...setting(EXTRA_KEYS.OBJECT_LIST_SIZES), share_link_bundles: size })
    try {
      await storeSettings()
      page = 1
      triggerRefresh(false)
    } catch (e) {
      toastService.showError(t('manage.attributes.error_saving_settings'), e)
    }
  }

  // refresh$ with switchMap: only the response of the most recent request counts
  let seq = 0
  async function triggerRefresh(silent: boolean) {
    const my = ++seq
    if (!silent) loading = true
    error = null
    try {
      const results = await api.listShareLinkBundles(page, pageSize, sortField, sortReverse)
      if (my !== seq) return
      bundles = results.results
      total = results.count
      copiedSlug = null
    } catch (e) {
      if (my !== seq) return
      error = t('manage.share_links.bundles_load_failed')
      toastService.showError(t('manage.share_links.bundles_fetch_error'), e)
    }
    loading = false
  }
  let timer: ReturnType<typeof setInterval> | undefined
  onMount(() => {
    void triggerRefresh(false)
    timer = setInterval(() => void triggerRefresh(true), 5000)
  })
  onDestroy(() => clearInterval(timer))

  function setPage(p: number) {
    page = p
    void triggerRefresh(false)
  }
  function onSort(column: string) {
    ;({ sortField, sortReverse } = nextSort(sortField, sortReverse, column))
    page = 1
    void triggerRefresh(false)
  }
  async function copy(bundle: ShareLinkBundle) {
    if (bundle.status !== 'ready') return
    if (await copyText(slugShareUrl(bundle.slug))) {
      copiedSlug = bundle.slug
      setTimeout(() => (copiedSlug = null), 3000)
    }
  }
  async function deleteBundle(bundle: ShareLinkBundle) {
    error = null
    loading = true
    try {
      await api.deleteShareLinkBundle(bundle.id)
      page = pageAfterDelete(bundles.length, page)
      toastService.showSuccess(t('manage.share_links.bundle_deleted'))
      void triggerRefresh(false)
    } catch (e) {
      loading = false
      toastService.showError(t('manage.share_links.bundle_delete_failed'), e)
    }
  }
  async function retry(bundle: ShareLinkBundle) {
    error = null
    try {
      const updated = await api.rebuildShareLinkBundle(bundle.id)
      toastService.showInfo(t('manage.share_links.bundle_rebuild_requested'))
      bundles = replaceBundle(bundles, updated)
    } catch (e) {
      toastService.showError(t('manage.share_links.bundle_rebuild_failed'), e)
    }
  }
</script>

<pngx-share-link-bundle-list><div class="border border-top-0 rounded-bottom p-3">
  {#if !loading && error}
    <div class="alert alert-danger mb-0" role="alert">
      {error}
    </div>
  {/if}
  {#if !loading && !error}
    {#if bundles.length === 0}
      <p class="mb-0 text-muted fst-italic">{t('manage.share_links.no_bundles')}</p>
    {/if}
    {#if bundles.length > 0}
      <div class="table-responsive">
        <table class="table table-sm align-middle mb-0 bg-body">
          <thead>
            <tr>
              <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'created')}" pngxSortable="created" onclick={() => onSort('created')}>{t('common.created')}</th>
              <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'status')}" pngxSortable="status" onclick={() => onSort('status')}>{t('common.status')}</th>
              <th scope="col">{t('dialog.share_bundle.size')}</th>
              <th scope="col" class="fw-normal {sortClass(sortField, sortReverse, 'expiration')}" pngxSortable="expiration" onclick={() => onSort('expiration')}>{t('ui.expires')}</th>
              <th scope="col">{t('common.documents_2')}</th>
              <th scope="col">{t('dialog.share_bundle.file_version')}</th>
              <th scope="col" class="text-end">{t('common.actions')}</th>
            </tr>
          </thead>
          <tbody>
            {#each bundles as bundle (bundle.id)}
              <tr>
                <td>
                  <div>{formatDate(bundle.created, 'short', dateLocale)}</div>
                  {#if bundle.built_at}
                    <div class="small text-muted">
                      {t('manage.share_links.built')} {formatDate(bundle.built_at, 'short', dateLocale)}
                    </div>
                  {/if}
                </td>
                <td>
                  <div class="d-flex align-items-center gap-2">
                    {#if bundle.status === 'failed' && bundle.last_error}
                      {@const p = pop(bundle.id)}
                      <button
                        type="button"
                        class="btn btn-link p-0 text-danger"
                        aria-label={t('manage.share_links.view_error_details')}
                        use:p.triggerEl
                      >
                        <span class="badge text-bg-warning text-uppercase me-2">{statusLabel(bundle.status)}</span>
                        <pngx-bs-icon name="exclamation-triangle-fill" class="text-warning">{@html icon('exclamation-triangle-fill')}</pngx-bs-icon>
                      </button>
                      {#if p.open}
                        <ngb-popover-window role="tooltip" class="popover popover-sm shadow" id={p.id} style={"position: absolute; min-width: 300px; max-width: 400px;"} use:p.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
                          {#if bundle.last_error.timestamp}
                            <div class="text-muted small mb-1">
                              {formatDate(bundle.last_error.timestamp, 'short', dateLocale)}
                            </div>
                          {/if}
                          <h6>{bundle.last_error.exception_type || t('manage.share_links.unknown_error')}</h6>
                          {#if bundle.last_error.message}
                            <pre class="text-muted small"><code>{bundle.last_error.message}</code></pre>
                          {/if}
                        </div></ngb-popover-window>
                      {/if}
                    {/if}
                    {#if bundle.status === 'processing' || bundle.status === 'pending'}
                      <span class="spinner-border spinner-border-sm" role="status"></span>
                    {/if}
                    {#if bundle.status !== 'failed'}
                      <span class="badge text-bg-secondary text-uppercase">{statusLabel(bundle.status)}</span>
                    {/if}
                  </div>
                </td>
                <td>
                  {#if bundle.size_bytes !== undefined && bundle.size_bytes !== null}
                    {fileSize(bundle.size_bytes)}
                  {:else}
                    <span class="text-muted">&mdash;</span>
                  {/if}
                </td>
                <td>
                  {#if bundle.expiration}
                    {formatDate(bundle.expiration, 'short', dateLocale)}
                    {#if isExpired(bundle.expiration)}
                      <span class="badge text-bg-danger ms-2">{t('manage.share_links.expired')}</span>
                    {/if}
                  {:else}
                    <span>{t('ui.never')}</span>
                  {/if}
                </td>
                <td>{bundle.document_count}</td>
                <td>{fileVersionLabel(bundle.file_version)}</td>
                <td class="text-end">
                  <div class="d-inline-block position-relative">
                    <span
                      class="badge bg-primary small fade position-absolute top-50 end-100 translate-middle-y me-2 pe-none z-3 text-nowrap"
                      class:show={copiedSlug === bundle.slug}
                    >{t('ui.copied')}</span>
                    <div class="btn-group btn-group-sm">
                      <button
                        type="button"
                        class="btn btn-outline-primary"
                        disabled={bundle.status !== 'ready'}
                        onclick={() => copy(bundle)}
                        title={t('manage.share_links.copy_share_link')}
                      >
                        {#if copiedSlug === bundle.slug}
                          <pngx-bs-icon name="clipboard-check">{@html icon('clipboard-check')}</pngx-bs-icon>
                        {:else}
                          <pngx-bs-icon name="clipboard">{@html icon('clipboard')}</pngx-bs-icon>
                        {/if}
                        <span class="visually-hidden">{t('manage.share_links.copy_share_link')}</span>
                      </button>
                      {#if bundle.status === 'failed'}
                        <button
                          type="button"
                          class="btn btn-outline-warning"
                          disabled={loading}
                          onclick={() => retry(bundle)}
                        >
                          <pngx-bs-icon name="arrow-clockwise">{@html icon('arrow-clockwise')}</pngx-bs-icon>
                          <span class="visually-hidden">{t('common.retry')}</span>
                        </button>
                      {/if}
                      <pngx-confirm-button><ConfirmButton
                        buttonClasses="btn btn-sm btn-outline-danger"
                        disabled={loading}
                        onconfirm={() => deleteBundle(bundle)}
                        iconName="trash"
                      /></pngx-confirm-button>
                    </div>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <div class="d-flex flex-wrap justify-content-end align-items-center gap-3 mt-3 ms-auto">
        <div class="d-flex flex-wrap justify-content-end align-items-center gap-3">
          <div class="d-flex align-items-center">
            <label class="small text-muted me-2" for="shareLinkBundlePageSize">{t('manage.attributes.show_label')}</label>
            <select id="shareLinkBundlePageSize" class="form-select form-select-sm w-auto" value={pageSize} onchange={(e) => setPageSize(+e.currentTarget.value)}>
              <option value={25}>25</option>
              <option value={50}>50</option>
              <option value={100}>100</option>
            </select>
            <span class="small text-muted ms-2 d-none d-md-inline">{t('manage.attributes.per_page')}</span>
          </div>
          {#if total > pageSize}
            <ngb-pagination class="mb-0" size="sm" aria-label={t('manage.share_links.bundles_pagination')} role="navigation"><Pagination {pageSize} collectionSize={total} {page} maxSize={5} onpage={setPage} /></ngb-pagination>
          {/if}
        </div>
      </div>
    {/if}
  {/if}
</div></pngx-share-link-bundle-list>
