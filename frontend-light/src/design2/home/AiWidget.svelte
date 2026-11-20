<!-- "AI suggestions" component (only with AI): number of open suggestions and way to the review list. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import Icon from '../components/Icon.svelte'
  import Widget from './Widget.svelte'
  import { aiStatus } from '../ai/status.svelte'

  onMount(() => {
    if (!aiStatus.data) aiStatus.refresh()
  })
  const n = $derived(aiStatus.pending ?? 0)
</script>

<Widget id="ai" title={t('m.ai.review_title')} icon="stars" href={router.href('automation/review')} linkLabel={t('common.show_all')}>
  <div class="aiw">
    {#if n > 0}
      <p class="big">{n}</p>
      <p>{t('m.ai.widget_open', { PH: n })}</p>
      <a class="d2-btn primary sm" href={router.href('automation/review')}><Icon name="check2" />{t('m.ai.widget_check')}</a>
    {:else}
      <p class="d2-muted"><Icon name="check-circle" /> {t('m.ai.nothing_open')}</p>
    {/if}
    {#if aiStatus.data?.auto_mode === 'off'}<p class="d2-muted small">{t('m.ai.auto_off_hint')}</p>{/if}
  </div>
</Widget>

<style>
  .aiw {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--d2-s2);
    padding: var(--d2-s3);
  }
  .big {
    font-size: var(--d2-fs-2xl);
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
</style>
