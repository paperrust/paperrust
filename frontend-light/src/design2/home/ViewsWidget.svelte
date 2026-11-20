<!-- "Saved views" component: views with "On the home page" (like the classic dashboard), per view
     the first documents in the sort order of the view. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { session, rawValue } from '../../stores/settings.svelte'
  import { queryParamsFromFilterRules } from '../../lib/query-params'
  import Widget from './Widget.svelte'
  import DocRows from './DocRows.svelte'
  import Icon from '../components/Icon.svelte'
  import { homeViews, VIEWS_SORT_KEY, VIEWS_VISIBLE_KEY } from './widgets'

  const views = $derived(homeViews(data.savedViews, rawValue(session.settings, VIEWS_VISIBLE_KEY), rawValue(session.settings, VIEWS_SORT_KEY)))
  const query = (v: (typeof views)[number]) => ({
    ...queryParamsFromFilterRules(v.filter_rules as any),
    ...(v.sort_field ? { ordering: (v.sort_reverse ? '-' : '') + v.sort_field } : {}),
  })
</script>

<Widget id="views" title={t('common.saved_views')} icon="bookmark" href={router.href('savedviews')} linkLabel={t('m.home.manage_views')}>
  {#if data.savedViewsLoading}
    <p class="d2-muted">{t('common.loading')}</p>
  {:else if views.length === 0}
    <p class="d2-muted">{t('m.home.no_views_on_home')}</p>
  {:else}
    <div class="views">
      {#each views as v (v.id)}
        <div class="view">
          <h3><a href={router.href('view/' + v.id)}><Icon name="bookmark" />{v.name}</a></h3>
          <DocRows query={query(v)} limit={5} empty={t('common.no_documents_found')} />
        </div>
      {/each}
    </div>
  {/if}
</Widget>

<style>
  .views {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s4);
  }
  h3 {
    margin: 0 0 var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
  h3 a {
    display: inline-flex;
    align-items: center;
    gap: var(--d2-s2);
    color: var(--d2-text);
    text-decoration: none;
  }
  h3 a:hover {
    color: var(--d2-accent-text);
    text-decoration: underline;
  }
</style>
