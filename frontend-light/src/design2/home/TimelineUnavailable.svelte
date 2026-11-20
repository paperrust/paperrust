<!-- Hint when the server does not offer /rs/api/v2/timeline (deadlines, year bar); demo only on request. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { mockEnabled } from '../api/assist'
  const demo = mockEnabled()
  function tryDemo() {
    try {
      localStorage.setItem('paperrust.design2.mock', '1')
    } catch {}
    location.reload()
  }
</script>

<div class="un">
  <p><strong>{t('common.timeline_service_not_available')}</strong></p>
  <p class="d2-muted">{t('m.timeline.server_not_provide_rs')}</p>
  {#if !demo}<button class="d2-btn sm" onclick={tryDemo}>{t('m.timeline.show_demo')}</button>{/if}
</div>

<style>
  .un p {
    margin: 0 0 var(--d2-s2);
  }
</style>
