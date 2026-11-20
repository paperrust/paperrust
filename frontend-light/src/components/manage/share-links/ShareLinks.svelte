<!-- Port of manage/share-links/share-links.component (page header + tabs "Document links"/"Bundles" depending on
     permission view sharelink / view sharelinkbundle; tab in the query parameter `type`, like router.navigate([], {queryParams})). -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { router } from '../../../router.svelte'
  import { session } from '../../../stores/settings.svelte'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ShareLinkList from './ShareLinkList.svelte'
  import ShareLinkBundleList from './ShareLinkBundleList.svelte'
  import { initialTab, TAB_BUNDLES, TAB_LINKS, type ShareLinksTab } from './share-links'

  const canViewDocumentLinks = $derived(session.can('view', 'sharelink'))
  const canViewBundles = $derived(session.can('view', 'sharelinkbundle'))
  // svelte-ignore state_referenced_locally
  let activeNavID = $state<ShareLinksTab>(initialTab(new URLSearchParams(router.query).get('type'), canViewDocumentLinks, canViewBundles))
  let linksLoading = $state(true)
  let bundlesLoading = $state(true)
  const loading = $derived(activeNavID === TAB_LINKS ? linksLoading : bundlesLoading)

  function selectTab(e: Event, tab: ShareLinksTab) {
    e.preventDefault()
    activeNavID = tab
    // queryParamsHandling: 'merge'
    const q = new URLSearchParams(router.query)
    q.set('type', tab)
    history.replaceState(null, '', `${location.pathname}?${q}`)
  }
</script>

<PageHeader
  title={t('common.share_links')}
  info={t('manage.share_links.info')}
  {loading}
/>

<ul class="nav-tabs nav" role="tablist">
  {#if canViewDocumentLinks}
    <li class="nav-item" role="presentation">
      <button type="button" class="nav-link" class:active={activeNavID === TAB_LINKS} role="tab" id="ngb-nav-links" aria-selected={activeNavID === TAB_LINKS} onclick={(e) => selectTab(e, TAB_LINKS)}>{t('manage.share_links.tab_document_links')}</button>
    </li>
  {/if}
  {#if canViewBundles}
    <li class="nav-item" role="presentation">
      <button type="button" class="nav-link" class:active={activeNavID === TAB_BUNDLES} role="tab" id="ngb-nav-bundles" aria-selected={activeNavID === TAB_BUNDLES} onclick={(e) => selectTab(e, TAB_BUNDLES)}>{t('manage.share_links.tab_bundles')}</button>
    </li>
  {/if}
</ul>

<div class="bg-body">
  {#if activeNavID === TAB_LINKS && canViewDocumentLinks}
    <div class="tab-pane active" role="tabpanel" aria-labelledby="ngb-nav-links">
      <ShareLinkList onloading={(l) => (linksLoading = l)} />
    </div>
  {:else if activeNavID === TAB_BUNDLES && canViewBundles}
    <div class="tab-pane active" role="tabpanel" aria-labelledby="ngb-nav-bundles">
      <ShareLinkBundleList onloading={(l) => (bundlesLoading = l)} />
    </div>
  {/if}
</div>
